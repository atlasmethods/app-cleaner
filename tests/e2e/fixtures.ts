import { test as base, expect, type Page } from '@playwright/test';
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '../..');
const bin = path.join(root, 'target', 'debug', process.platform === 'win32' ? 'clearsweep.exe' : 'clearsweep');

export interface ServerInfo {
  /** Full launch URL including ?t=<token>. */
  url: string;
  origin: string;
  token: string;
  /** Root of the throwaway machine the server runs against (home/, root/, data/). */
  dir: string;
}

/**
 * (Re)build the fake machine: wipes home/, root/ and data/ below `dir` and fills them with
 * browser profiles, caches, cookies and temp files. Needs `cargo build -p sweep-cli --features testutil`.
 */
export function resetSandbox(dir: string): void {
  const r = spawnSync(bin, ['dev-fixture', dir], { encoding: 'utf8' });
  if (r.status !== 0) {
    throw new Error(`dev-fixture failed (${r.status}): ${r.stderr}\nBuild the CLI with: cargo build -p sweep-cli --features testutil`);
  }
}

/** Call an API method straight over HTTP (NDJSON) and return the result value. */
export async function callApi<T = unknown>(server: ServerInfo, method: string, params: unknown = {}): Promise<T> {
  const res = await fetch(`${server.origin}/api/call`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Sweep-Token': server.token },
    body: JSON.stringify({ callId: `t-${Math.random()}`, method, params }),
  });
  const lines = (await res.text()).split('\n').filter(Boolean).map((l) => JSON.parse(l) as { type: string; value?: unknown; error?: { message: string } });
  const last = lines[lines.length - 1];
  if (!last || last.type === 'error') throw new Error(`${method} failed: ${last?.error?.message ?? 'no result'}`);
  return last.value as T;
}

export interface StartedServer {
  info: ServerInfo;
  stop: () => Promise<void>;
  /** The server process (for tests that watch it exit). */
  child: ChildProcess;
}

/** Start a sandboxed `clearsweep ui`; `args` replaces the default `--no-exit-on-idle`. */
export async function startServer(args: string[] = ['--no-exit-on-idle']): Promise<StartedServer> {
  // Everything the app could touch lives in a throwaway directory.
  const tmp = mkdtempSync(path.join(os.tmpdir(), 'clearsweep-e2e-'));
  const empty = path.join(tmp, 'empty-path');
  const dirs = {
    HOME: path.join(tmp, 'home'),
    XDG_CONFIG_HOME: path.join(tmp, 'home', '.config'),
    XDG_CACHE_HOME: path.join(tmp, 'home', '.cache'),
    XDG_DATA_HOME: path.join(tmp, 'home', '.local', 'share'),
    CLEARSWEEP_ROOT: path.join(tmp, 'root'),
    CLEARSWEEP_DATA_DIR: path.join(tmp, 'data'),
    TMPDIR: path.join(tmp, 'root', 'tmp'),
  };
  for (const d of Object.values(dirs)) mkdirSync(d, { recursive: true });
  mkdirSync(empty, { recursive: true });
  resetSandbox(tmp);
  // The server sees no external programs (clipboard, DNS tools, ...) and a fixed process
  // list, so a clean can never touch the developer's real desktop session.
  const sandboxEnv = { ...dirs, PATH: empty, CLEARSWEEP_FAKE_PROCESSES: '', CLEARSWEEP_TEST_IGNORE_CTIME: '1' };

  const child: ChildProcess = spawn(
    bin,
    ['ui', '--no-open', ...args, '--port', '0', '--print-url'],
    { env: { ...process.env, ...sandboxEnv }, stdio: ['ignore', 'pipe', 'pipe'] },
  );

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
  const info: ServerInfo = { url, origin: u.origin, token: u.searchParams.get('t') ?? '', dir: tmp };
  const stop = async () => {
    if (child.exitCode === null && child.signalCode === null) {
      const exited = new Promise<void>((r) => child.once('exit', () => r()));
      child.kill('SIGTERM');
      const t = setTimeout(() => child.kill('SIGKILL'), 3000);
      await exited;
      clearTimeout(t);
    }
    rmSync(tmp, { recursive: true, force: true });
  };
  return { info, stop, child };
}

/** Where things live inside the fake machine (Linux layout). */
export interface Sandbox {
  dir: string;
  home: string;
  chromeCache: (profile?: string) => string;
  chromeData: (profile?: string) => string;
}

interface TestOptions {
  /** HTTP statuses (and the console line the browser logs for them) that a test expects. */
  allowHttpStatuses: number[];
}

interface TestFixtures {
  /** The fake machine, freshly rebuilt for this test. */
  sandbox: Sandbox;
  /** Page already opened on the app via the tokenised URL, with the tab bar rendered. */
  app: Page;
  /** Fails the test if the page logged console errors or threw. */
  consoleErrors: string[];
}

interface WorkerFixtures {
  server: ServerInfo;
}

export const test = base.extend<TestFixtures & TestOptions, WorkerFixtures>({
  allowHttpStatuses: [[], { option: true }],

  server: [
    async ({}, use) => {
      const { info, stop } = await startServer();
      try {
        await use(info);
      } finally {
        await stop();
      }
    },
    { scope: 'worker' },
  ],

  consoleErrors: [
    async ({ page, allowHttpStatuses }, use) => {
      const errors: string[] = [];
      // 0 stands for "the connection itself failed" (net::ERR_*), used when a test stops the server.
      const allowed = (text: string) =>
        allowHttpStatuses.some((s) => text.includes(`status of ${s}`)) ||
        (allowHttpStatuses.includes(0) && text.includes('net::ERR_'));
      page.on('console', (m) => {
        if (m.type() === 'error' && !allowed(m.text())) errors.push(`console.error: ${m.text()}`);
      });
      page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
      page.on('response', (r) => {
        if (r.status() >= 400 && !allowHttpStatuses.includes(r.status())) errors.push(`http ${r.status()}: ${r.url()}`);
      });
      await use(errors);
      expect(errors, 'no console errors / page errors').toEqual([]);
    },
    { auto: true },
  ],

  sandbox: async ({ server }, use) => {
    resetSandbox(server.dir);
    const home = path.join(server.dir, 'home');
    await use({
      dir: server.dir,
      home,
      chromeCache: (p = 'Default') => path.join(home, '.cache', 'google-chrome', p),
      chromeData: (p = 'Default') => path.join(home, '.config', 'google-chrome', p),
    });
  },

  app: async ({ page, server, sandbox }, use) => {
    void sandbox; // every test starts on a freshly built machine and default settings
    await page.goto(server.url);
    // The tab bar (compact) or the navigation rail (wide) both carry the tab test ids.
    await expect(page.getByTestId('tab-home')).toBeVisible();
    await use(page);
  },
});

export { expect };

/** True when the document does not scroll horizontally. */
export async function noHorizontalScroll(page: Page): Promise<{ scrollWidth: number; innerWidth: number }> {
  return page.evaluate(() => ({
    scrollWidth: document.documentElement.scrollWidth,
    innerWidth: window.innerWidth,
  }));
}
