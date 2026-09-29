import { test as base, expect, type Page } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
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
}

async function startServer(): Promise<{ info: ServerInfo; stop: () => Promise<void> }> {
  // Everything the app could touch lives in a throwaway directory.
  const tmp = mkdtempSync(path.join(os.tmpdir(), 'clearsweep-e2e-'));
  const dirs = {
    HOME: path.join(tmp, 'home'),
    XDG_CONFIG_HOME: path.join(tmp, 'home', '.config'),
    XDG_CACHE_HOME: path.join(tmp, 'home', '.cache'),
    XDG_DATA_HOME: path.join(tmp, 'home', '.local', 'share'),
    CLEARSWEEP_ROOT: path.join(tmp, 'root'),
    CLEARSWEEP_DATA_DIR: path.join(tmp, 'data'),
  };
  for (const d of Object.values(dirs)) mkdirSync(d, { recursive: true });

  const child: ChildProcess = spawn(
    bin,
    ['ui', '--no-open', '--no-exit-on-idle', '--port', '0', '--print-url'],
    { env: { ...process.env, ...dirs }, stdio: ['ignore', 'pipe', 'pipe'] },
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
  const info: ServerInfo = { url, origin: u.origin, token: u.searchParams.get('t') ?? '' };
  const stop = async () => {
    if (child.exitCode === null) {
      const exited = new Promise<void>((r) => child.once('exit', () => r()));
      child.kill('SIGTERM');
      const t = setTimeout(() => child.kill('SIGKILL'), 3000);
      await exited;
      clearTimeout(t);
    }
    rmSync(tmp, { recursive: true, force: true });
  };
  return { info, stop };
}

interface TestFixtures {
  /** Page already opened on the app via the tokenised URL, with the tab bar rendered. */
  app: Page;
  /** Fails the test if the page logged console errors or threw. */
  consoleErrors: string[];
}

interface WorkerFixtures {
  server: ServerInfo;
}

export const test = base.extend<TestFixtures, WorkerFixtures>({
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
    async ({ page }, use) => {
      const errors: string[] = [];
      page.on('console', (m) => {
        if (m.type() === 'error') errors.push(`console.error: ${m.text()}`);
      });
      page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));
      page.on('response', (r) => {
        if (r.status() >= 400) errors.push(`http ${r.status()}: ${r.url()}`);
      });
      await use(errors);
      expect(errors, 'no console errors / page errors').toEqual([]);
    },
    { auto: true },
  ],

  app: async ({ page, server }, use) => {
    await page.goto(server.url);
    await expect(page.getByTestId('tabbar')).toBeVisible();
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
