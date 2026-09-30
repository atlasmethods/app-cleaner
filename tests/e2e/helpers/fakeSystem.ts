/**
 * A throwaway "machine" for the uninstall / software updater / driver updater end-to-end tests:
 * fake `dpkg-query`, `apt`, `apt-get`, `fwupdmgr` and `pkexec` executables in a temp bin dir that
 * is the ONLY directory on the server's PATH, plus the usual sandboxed HOME / data dirs.
 *
 * The fakes keep their state in plain files under `state/` so tests can inspect and change it:
 *   dpkg.txt        installed packages (tab separated, dpkg-query -f format)
 *   upgradable.txt  lines of `apt list --upgradable`
 *   fwupd.json      pending firmware update (absent = `fwupdmgr` exits 2, nothing to do)
 *   log.txt         every command a fake was asked to run
 */
import { spawn, type ChildProcess } from 'node:child_process';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import type { ServerInfo } from '../fixtures';

const root = path.resolve(import.meta.dirname, '../../..');
const bin = path.join(root, 'target', 'debug', process.platform === 'win32' ? 'clearsweep.exe' : 'clearsweep');

export const DPKG_ROWS = [
  'bash\t5.2.21\t1844\tUbuntu Developers <dev@example.com>\trequired\tyes\tinstalled',
  'clearsweep-demo\t1.0.0\t120\tDemo Maker <demo@example.com>\toptional\t\tinstalled',
  'htop\t3.3.0\t400\tSomeone <s@example.com>\toptional\t\tinstalled',
];

export const UPGRADABLE = [
  'Listing... Done',
  'firefox/noble-updates,noble-security 126.0+build2 amd64 [upgradable from: 125.0.3]',
  'vim/noble-updates 2:9.1.0016-1ubuntu7.2 amd64 [upgradable from: 2:9.1.0016-1ubuntu7]',
];

export const FWUPD_JSON = JSON.stringify({
  Devices: [
    {
      Name: 'System Firmware',
      DeviceId: '3fc0a2fa8d0ff8ec5b4d0d4dbbc91ba6a3d1d1d1',
      Flags: ['updatable', 'needs-reboot'],
      Vendor: 'Dell Inc.',
      Version: '1.20.0',
      Releases: [{ Summary: 'Firmware for the XPS 13', Version: '1.22.0', Vendor: 'Dell', Flags: ['is-upgrade'] }],
    },
  ],
});

const HEADER = '#!/bin/sh\nPATH=/usr/bin:/bin\nS="$FAKE_STATE"\necho "$(basename "$0") $*" >> "$S/log.txt"\n';

const SCRIPTS: Record<string, string> = {
  'dpkg-query': `${HEADER}cat "$S/dpkg.txt"\n`,
  apt: `${HEADER}
if [ "$1" = "list" ]; then cat "$S/upgradable.txt"; exit 0; fi
exit 1
`,
  'apt-get': `${HEADER}
case "$1" in
  -s)
    # apt-get -s remove PKG: removing htop drags htop-plugins along.
    echo "Reading package lists..."
    echo "Remv $3 [1.0]"
    if [ "$3" = "htop" ]; then echo "Remv htop-plugins [1.0]"; fi
    exit 0 ;;
  remove)
    pkg="$3"
    grep -v "^$pkg$(printf '\t')" "$S/dpkg.txt" > "$S/dpkg.new"; mv "$S/dpkg.new" "$S/dpkg.txt"
    echo "Removing $pkg ..."
    exit 0 ;;
  install)
    shift 3
    for p in "$@"; do grep -v "^$p/" "$S/upgradable.txt" > "$S/up.new"; mv "$S/up.new" "$S/upgradable.txt"; done
    echo "Setting up $* ..."
    exit 0 ;;
  update) echo "Reading package lists... Done"; exit 0 ;;
esac
exit 1
`,
  fwupdmgr: `${HEADER}
case "$1" in
  get-updates)
    if [ -f "$S/fwupd.json" ]; then cat "$S/fwupd.json"; exit 0; fi
    echo "No updatable devices"; exit 2 ;;
  update)
    rm -f "$S/fwupd.json"
    echo "Successfully installed firmware"
    echo "An update requires a reboot to complete."
    exit 0 ;;
esac
exit 1
`,
  // Only used when the tests do not run as root: behave like an authorised pkexec.
  pkexec: `${HEADER}exec "$@"\n`,
};

export interface FakeMachine {
  server: ServerInfo;
  /** `state/` directory of the fakes. */
  state: string;
  home: string;
  configDir: string;
  readState: (file: string) => string;
  writeState: (file: string, content: string) => void;
  /** Lines the fake executables logged, in order. */
  log: () => string[];
  stop: () => Promise<void>;
}

export async function startFakeMachine(): Promise<FakeMachine> {
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
  writeFileSync(path.join(state, 'dpkg.txt'), DPKG_ROWS.join('\n') + '\n');
  writeFileSync(path.join(state, 'upgradable.txt'), UPGRADABLE.join('\n') + '\n');
  writeFileSync(path.join(state, 'log.txt'), '');

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
  const server: ServerInfo = { url, origin: u.origin, token: u.searchParams.get('t') ?? '', dir: tmp };
  return {
    server,
    state,
    home,
    configDir: dirs.XDG_CONFIG_HOME,
    readState: (f) => readFileSync(path.join(state, f), 'utf8'),
    writeState: (f, c) => writeFileSync(path.join(state, f), c),
    log: () =>
      readFileSync(path.join(state, 'log.txt'), 'utf8')
        .split('\n')
        .filter(Boolean),
    stop: async () => {
      if (child.exitCode === null) {
        const exited = new Promise<void>((r) => child.once('exit', () => r()));
        child.kill('SIGTERM');
        const t = setTimeout(() => child.kill('SIGKILL'), 3000);
        await exited;
        clearTimeout(t);
      }
      rmSync(tmp, { recursive: true, force: true });
    },
  };
}
