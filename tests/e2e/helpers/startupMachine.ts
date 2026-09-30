/**
 * A throwaway machine for the Startup / Performance / Browser Plugins end-to-end tests: a temp
 * HOME and system root, fake `systemctl`, `crontab` and `pkexec` executables (the ONLY thing on
 * the server's PATH) and a scripted process list (`CLEARSWEEP_FAKE_PROCESSES`).
 *
 * State of the fakes lives in plain files under `state/`:
 *   user-units.txt / system-units.txt   `name.service enabled|disabled` per line
 *   crontab                             the user's crontab (absent = `no crontab for user`)
 *   log.txt                             every command a fake was asked to run
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
  systemctl: `${HEADER}
scope=system
if [ "$1" = "--user" ]; then scope=user; shift; fi
file="$S/$scope-units.txt"
case "$1" in
  list-unit-files)
    if [ "$scope" = "user" ]; then
      awk '{print $1, $2, "enabled"}' "$file" 2>/dev/null
    else
      awk '{print $1, $2, "enabled"}' "$file" 2>/dev/null
    fi
    exit 0 ;;
  enable|disable)
    verb="$1"; unit="$2"
    state=enabled; [ "$verb" = "disable" ] && state=disabled
    if ! grep -q "^$unit " "$file"; then echo "Failed to $verb unit: Unit file $unit does not exist." >&2; exit 1; fi
    sed "s/^$unit .*/$unit $state/" "$file" > "$file.new" && mv "$file.new" "$file"
    exit 0 ;;
esac
exit 1
`,
  crontab: `${HEADER}
if [ "$1" = "-l" ]; then
  if [ -f "$S/crontab" ]; then cat "$S/crontab"; exit 0; fi
  echo "no crontab for user" >&2; exit 1
fi
cp "$1" "$S/crontab"
exit 0
`,
  // Only used when the tests do not run as root: behave like an authorised pkexec.
  pkexec: `${HEADER}exec "$@"\n`,
};

export interface StartupMachine {
  server: ServerInfo;
  home: string;
  /** `CLEARSWEEP_ROOT` (the fake `/`). */
  sysRoot: string;
  dataDir: string;
  state: string;
  configDir: string;
  autostartDir: string;
  /** Path below the fake system root, e.g. `sys('/etc/xdg/autostart')`. */
  sys: (p: string) => string;
  readState: (file: string) => string;
  writeState: (file: string, content: string) => void;
  log: () => string[];
  stop: () => Promise<void>;
}

export interface MachineOptions {
  /** Value of CLEARSWEEP_FAKE_PROCESSES: `name|exe|rssMB|cpu`, comma separated. */
  processes?: string;
}

interface Prepared {
  machine: StartupMachine;
  tmp: string;
  dirs: Record<string, string>;
  fakeBin: string;
  state: string;
}

function prepare(): Prepared {
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
  for (const [name, body] of Object.entries(SCRIPTS)) {
    const p = path.join(fakeBin, name);
    writeFileSync(p, body);
    chmodSync(p, 0o755);
  }
  writeFileSync(path.join(state, 'log.txt'), '');
  writeFileSync(path.join(state, 'user-units.txt'), '');
  writeFileSync(path.join(state, 'system-units.txt'), '');

  const sys = (p: string) => path.join(dirs.CLEARSWEEP_ROOT, p);
  const machine: StartupMachine = {
    server: { url: '', origin: '', token: '', dir: tmp },
    home,
    sysRoot: dirs.CLEARSWEEP_ROOT,
    dataDir: dirs.CLEARSWEEP_DATA_DIR,
    state,
    configDir: dirs.XDG_CONFIG_HOME,
    autostartDir: path.join(dirs.XDG_CONFIG_HOME, 'autostart'),
    sys,
    readState: (f) => readFileSync(path.join(state, f), 'utf8'),
    writeState: (f, c) => writeFileSync(path.join(state, f), c),
    log: () =>
      readFileSync(path.join(state, 'log.txt'), 'utf8')
        .split('\n')
        .filter(Boolean),
    stop: async () => undefined,
  };
  return { machine, tmp, dirs, fakeBin, state };
}

async function launch(
  tmp: string,
  dirs: Record<string, string>,
  fakeBin: string,
  state: string,
  opts: MachineOptions,
  machine: StartupMachine,
): Promise<void> {
  const child: ChildProcess = spawn(bin, ['ui', '--no-open', '--no-exit-on-idle', '--port', '0', '--print-url'], {
    env: {
      ...process.env,
      ...dirs,
      PATH: fakeBin,
      FAKE_STATE: state,
      CLEARSWEEP_FAKE_PROCESSES: opts.processes ?? '',
      CLEARSWEEP_TEST_IGNORE_CTIME: '1',
    },
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
}

/**
 * Create the machine, let `setup` put files in place (starting the server afterwards keeps the
 * first list deterministic), then start the server.
 */
export async function bootMachine(
  setup: (m: StartupMachine) => void,
  opts: MachineOptions = {},
): Promise<StartupMachine> {
  const { machine, tmp, dirs, fakeBin, state } = prepare();
  try {
    setup(machine);
    await launch(tmp, dirs, fakeBin, state, opts, machine);
  } catch (e) {
    rmSync(tmp, { recursive: true, force: true });
    throw e;
  }
  return machine;
}

export function exists(p: string): boolean {
  return existsSync(p);
}
