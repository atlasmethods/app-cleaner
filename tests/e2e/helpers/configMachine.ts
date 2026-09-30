/**
 * A throwaway Linux "machine" for the Config Issues and System Restore end-to-end tests: a
 * sandboxed HOME / root / data dir, and (optionally) fake `timeshift`, `snapper` and `pkexec`
 * executables that are the ONLY programs on the server's PATH. Same idea as fakeSystem.ts, with
 * the restore tools instead of the package managers.
 *
 * The fakes keep their state in plain files under `state/`:
 *   timeshift.db   name|tags|description   (one snapshot per line)
 *   snapper.db     number|date|description
 *   log.txt        every command a fake was asked to run
 */
import { spawn, type ChildProcess } from 'node:child_process';
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import type { ServerInfo } from '../fixtures';

const root = path.resolve(import.meta.dirname, '../../..');
const bin = path.join(root, 'target', 'debug', process.platform === 'win32' ? 'clearsweep.exe' : 'clearsweep');

const HEADER = '#!/bin/sh\nPATH=/usr/bin:/bin\nS="$FAKE_STATE"\necho "$(basename "$0") $*" >> "$S/log.txt"\n';

const SCRIPTS: Record<string, string> = {
  timeshift: `${HEADER}
case "$1" in
  --list)
    echo "Device : /dev/sda1"
    echo "Mode   : RSYNC"
    echo
    echo "Num     Name                 Tags  Description"
    echo "------------------------------------------------------------------------------"
    n=0
    while IFS='|' read -r name tags desc; do
      [ -z "$name" ] && continue
      printf '%s    >  %s  %s     %s\\n' "$n" "$name" "$tags" "$desc"
      n=$((n+1))
    done < "$S/timeshift.db"
    exit 0 ;;
  --create)
    # timeshift --create --comments TEXT --tags O --scripted
    cnt=$(wc -l < "$S/timeshift.db")
    name=$(printf '2024-06-%02d_12-00-00' $((cnt+10)))
    echo "$name|O|$3" >> "$S/timeshift.db"
    echo "Snapshot saved"
    exit 0 ;;
  --delete)
    # timeshift --delete --snapshot NAME --yes --scripted
    grep -v "^$3|" "$S/timeshift.db" > "$S/ts.new"; mv "$S/ts.new" "$S/timeshift.db"
    echo "Snapshot deleted"
    exit 0 ;;
esac
exit 1
`,
  snapper: `${HEADER}
# global options first: [--iso] [-c CONFIG] COMMAND ...
while [ "$1" = "--iso" ] || [ "$1" = "-c" ]; do
  if [ "$1" = "-c" ]; then shift; fi
  shift
done
case "$1" in
  list-configs)
    echo "Config | Subvolume"
    echo "-------+----------"
    echo "root   | /"
    exit 0 ;;
  list)
    echo "# | Type   | Pre # | Date                | User | Cleanup | Description | Userdata"
    echo "--+--------+-------+---------------------+------+---------+-------------+---------"
    echo "0 | single |       |                     | root |         | current     |"
    while IFS='|' read -r num date desc; do
      [ -z "$num" ] && continue
      echo "$num | single |       | $date | root |         | $desc |"
    done < "$S/snapper.db"
    exit 0 ;;
  create)
    # create --type single --description TEXT
    cnt=$(wc -l < "$S/snapper.db")
    echo "$((cnt+10))|2024-06-01 12:00:00|$5" >> "$S/snapper.db"
    exit 0 ;;
  delete)
    grep -v "^$2|" "$S/snapper.db" > "$S/sn.new"; mv "$S/sn.new" "$S/snapper.db"
    exit 0 ;;
esac
exit 1
`,
  // Only used when the tests do not run as root: behave like an authorised pkexec.
  pkexec: `${HEADER}exec "$@"\n`,
};

export const TIMESHIFT_ROWS = [
  '2024-01-01_10-00-01|O|Before the big update',
  '2024-02-01_03-00-00|D|Daily snapshot',
  '2024-03-01_03-00-00|D|Newest timeshift',
];

export const SNAPPER_ROWS = ['1|2024-01-05 09:00:00|first snapper', '2|2024-02-05 09:00:00|second snapper'];

export interface ConfigMachine {
  server: ServerInfo;
  home: string;
  /** Redirected system root (`/usr/share/applications` lives at `<root>/usr/share/applications`). */
  root: string;
  configDir: string;
  dataDir: string;
  state: string;
  readState: (file: string) => string;
  writeState: (file: string, content: string) => void;
  /** Lines the fake executables logged, in order. */
  log: () => string[];
  stop: () => Promise<void>;
}

export async function startConfigMachine(opts: { tools: boolean }): Promise<ConfigMachine> {
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
  if (opts.tools) {
    for (const [name, body] of Object.entries(SCRIPTS)) {
      const p = path.join(fakeBin, name);
      writeFileSync(p, body);
      chmodSync(p, 0o755);
    }
  }
  writeFileSync(path.join(state, 'timeshift.db'), TIMESHIFT_ROWS.join('\n') + '\n');
  writeFileSync(path.join(state, 'snapper.db'), SNAPPER_ROWS.join('\n') + '\n');
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
    home,
    root: dirs.CLEARSWEEP_ROOT,
    configDir: dirs.XDG_CONFIG_HOME,
    dataDir: dirs.CLEARSWEEP_DATA_DIR,
    state,
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
