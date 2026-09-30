/**
 * A throwaway machine for the Smart Cleaning / Scheduled Cleaning / Startup & Background tests: a
 * temp HOME, fake `systemctl` (the user manager "works"), `crontab` and `xdg-open` executables that
 * are the ONLY things on the server's PATH, and a fixed process list. Nothing can reach the real
 * user's systemd, crontab or desktop.
 *
 * Fakes keep state in plain files under `state/`:
 *   log.txt   every command a fake was asked to run (`systemctl --user enable --now ...`)
 *   crontab   the user's crontab (absent = `no crontab for user`)
 */
import { spawn, type ChildProcess } from 'node:child_process';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import type { ServerInfo } from '../fixtures';

const root = path.resolve(import.meta.dirname, '../../..');
const bin = path.join(root, 'target', 'debug', process.platform === 'win32' ? 'clearsweep.exe' : 'clearsweep');

const HEADER = '#!/bin/sh\nPATH=/usr/bin:/bin\nS="$FAKE_STATE"\necho "$(basename "$0") $*" >> "$S/log.txt"\n';

const SCRIPTS: Record<string, string> = {
  systemctl: `${HEADER}exit 0\n`,
  crontab: `${HEADER}
if [ "$1" = "-l" ]; then
  if [ -f "$S/crontab" ]; then cat "$S/crontab"; exit 0; fi
  echo "no crontab for user" >&2; exit 1
fi
cp "$1" "$S/crontab"
exit 0
`,
  'xdg-open': `${HEADER}exit 0\n`,
};

export type Tool = keyof typeof SCRIPTS;

export interface ScheduleMachine {
  server: ServerInfo;
  home: string;
  dataDir: string;
  state: string;
  /** `~/.config/systemd/user` */
  unitDir: string;
  /** `~/.config/autostart` */
  autostartDir: string;
  /** A 300 KiB Chrome cache file that `chrome.cache` cleans. */
  chromeCacheFile: string;
  log: () => string[];
  readState: (file: string) => string;
  stop: () => Promise<void>;
}

export interface Options {
  /** Fake executables on PATH (default: systemctl, crontab and xdg-open). */
  tools?: Tool[];
  /** Runs before the server starts. */
  setup?: (m: ScheduleMachine) => void;
}

export async function bootScheduleMachine(opts: Options = {}): Promise<ScheduleMachine> {
  const tmp = mkdtempSync(path.join(os.tmpdir(), 'clearsweep-e2e-'));
  const fakeBin = path.join(tmp, 'fake-bin');
  const state = path.join(tmp, 'state');
  const home = path.join(tmp, 'home');
  const dirs = {
    HOME: home,
    XDG_CONFIG_HOME: path.join(home, '.config'),
    XDG_CACHE_HOME: path.join(home, '.cache'),
    XDG_DATA_HOME: path.join(home, '.local', 'share'),
    CLEARSWEEP_ROOT: path.join(tmp, 'root'),
    CLEARSWEEP_DATA_DIR: path.join(tmp, 'data'),
    TMPDIR: path.join(tmp, 'root', 'tmp'),
  };
  for (const d of [...Object.values(dirs), fakeBin, state]) mkdirSync(d, { recursive: true });
  for (const name of opts.tools ?? (Object.keys(SCRIPTS) as Tool[])) {
    const p = path.join(fakeBin, name);
    writeFileSync(p, SCRIPTS[name]!);
    chmodSync(p, 0o755);
  }
  writeFileSync(path.join(state, 'log.txt'), '');

  const chromeCacheFile = path.join(home, '.cache', 'google-chrome', 'Default', 'Cache', 'Cache_Data', 'data_0');
  mkdirSync(path.dirname(chromeCacheFile), { recursive: true });
  writeFileSync(chromeCacheFile, Buffer.alloc(300 * 1024, 1));

  const machine: ScheduleMachine = {
    server: { url: '', origin: '', token: '', dir: tmp },
    home,
    dataDir: dirs.CLEARSWEEP_DATA_DIR,
    state,
    unitDir: path.join(dirs.XDG_CONFIG_HOME, 'systemd', 'user'),
    autostartDir: path.join(dirs.XDG_CONFIG_HOME, 'autostart'),
    chromeCacheFile,
    log: () =>
      readFileSync(path.join(state, 'log.txt'), 'utf8')
        .split('\n')
        .filter(Boolean),
    readState: (f) => readFileSync(path.join(state, f), 'utf8'),
    stop: async () => undefined,
  };

  try {
    opts.setup?.(machine);
    const child: ChildProcess = spawn(bin, ['ui', '--no-open', '--no-exit-on-idle', '--port', '0', '--print-url'], {
      env: { ...process.env, ...dirs, PATH: fakeBin, FAKE_STATE: state, CLEARSWEEP_FAKE_PROCESSES: '', CLEARSWEEP_TEST_IGNORE_CTIME: '1' },
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stderr = '';
    child.stderr?.on('data', (d: Buffer) => (stderr += d.toString()));
    const url = await new Promise<string>((resolve, reject) => {
      let out = '';
      const timer = setTimeout(() => reject(new Error(`clearsweep did not print a URL in time. stderr: ${stderr}`)), 15_000);
      child.stdout?.on('data', (d: Buffer) => {
        out += d.toString();
        const m = /ClearSweep running at (http:\/\/\S+)/.exec(out);
        if (m?.[1]) {
          clearTimeout(timer);
          resolve(m[1]);
        }
      });
      child.on('exit', (code) => {
        clearTimeout(timer);
        reject(new Error(`clearsweep exited early (code ${code}). stderr: ${stderr}`));
      });
    });
    const u = new URL(url);
    machine.server = { url, origin: u.origin, token: u.searchParams.get('t') ?? '', dir: tmp };
    machine.stop = async () => {
      if (child.exitCode === null) {
        const exited = new Promise<void>((r) => child.once('exit', () => r()));
        child.kill('SIGTERM');
        const t = setTimeout(() => child.kill('SIGKILL'), 3000);
        await exited;
        clearTimeout(t);
      }
      rmSync(tmp, { recursive: true, force: true });
    };
  } catch (e) {
    rmSync(tmp, { recursive: true, force: true });
    throw e;
  }
  return machine;
}

export function exists(p: string): boolean {
  return existsSync(p);
}
