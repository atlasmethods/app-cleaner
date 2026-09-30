/**
 * Opening the app in a browser: token handling, the friendly page without a token, deep links,
 * unknown routes, history navigation, idle exit, and rejection of cross-origin requests.
 */
import { test as plain, expect as plainExpect } from '@playwright/test';
import http from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, startServer, test } from './fixtures';

test.describe('without a valid token', () => {
  test.use({ allowHttpStatuses: [401] });

  test('a tab opened without the link gets a friendly page, not raw JSON', async ({ page, server }) => {
    await page.goto(server.origin + '/');
    await expect(page.getByTestId('page-auth-required')).toBeVisible();
    await expect(page.getByRole('heading', { name: /Open ClearSweep from its link/ })).toBeVisible();
    await expect(page.getByText('clearsweep ui', { exact: false }).first()).toBeVisible();
    await expect(page.getByTestId('tabbar')).toHaveCount(0);
    await expect(page.locator('body')).not.toContainText('unauthorized');
  });

  test('a stale token from an earlier launch shows the same page', async ({ page, server }) => {
    await page.goto(`${server.origin}/?t=${'0'.repeat(64)}`);
    await expect(page.getByTestId('page-auth-required')).toBeVisible();
    expect(page.url()).not.toContain('t=');
  });

  test('a second tab of the same browser (own sessionStorage) needs the link too', async ({ context, server }) => {
    const first = await context.newPage();
    await first.goto(server.url);
    await expect(first.getByTestId('tab-home')).toBeVisible();
    const second = await context.newPage();
    await second.goto(server.origin + '/');
    await expect(second.getByTestId('page-auth-required')).toBeVisible();
    // The first tab is unaffected.
    await first.getByTestId('tab-tools').click();
    await expect(first.getByTestId('page-tools')).toBeVisible();
  });
});

test('the token is removed from the address bar and survives reloads', async ({ page, server }) => {
  await page.goto(server.url);
  await expect(page.getByTestId('tab-home')).toBeVisible();
  expect(page.url()).not.toContain('t=');
  expect(await page.evaluate(() => window.history.length)).toBeGreaterThan(0);
  expect(await page.evaluate(() => window.sessionStorage.getItem('clearsweep.token'))).toBe(server.token);
  await page.reload();
  await expect(page.getByTestId('tab-home')).toBeVisible();
  await page.goto(`${page.url().split('#')[0]}#/tools/sysinfo`);
  await expect(page.getByTestId('sysinfo-cpu')).toBeVisible();
});

test('deep links work on first load and on reload', async ({ page, server }) => {
  await page.goto(`${server.url}#/tools/wiper`);
  await expect(page.getByTestId('page-wiper')).toBeVisible();
  expect(page.url()).toContain('#/tools/wiper');
  expect(page.url()).not.toContain('t=');
  await page.reload();
  await expect(page.getByTestId('page-wiper')).toBeVisible();
  await expect(page.getByTestId('appbar-title')).toHaveText('Drive Wiper');
  await page.goto(`${page.url().split('#')[0]}#/settings/schedules`);
  await page.reload();
  await expect(page.getByTestId('page-schedules')).toBeVisible();
});

test('an unknown route shows a not-found page with a way home', async ({ app }) => {
  await app.evaluate(() => {
    location.hash = '#/tools/does-not-exist';
  });
  await expect(app.getByTestId('page-not-found')).toBeVisible();
  await expect(app.getByTestId('page-not-found')).toContainText('/tools/does-not-exist');
  await expect(app.getByTestId('appbar-title')).toHaveText('Page not found');
  await app.getByTestId('not-found-home').click();
  await expect(app.getByTestId('page-home')).toBeVisible();
  expect(app.url()).toMatch(/#\/$/);
});

test('the browser back button walks back through the pages', async ({ app }) => {
  await app.getByTestId('tab-tools').click();
  await app.getByTestId('tile-sysinfo').click();
  await expect(app.getByTestId('page-sysinfo')).toBeVisible();
  await app.goBack();
  await expect(app.getByTestId('page-tools')).toBeVisible();
  await app.goBack();
  await expect(app.getByTestId('page-home')).toBeVisible();
  await app.goForward();
  await expect(app.getByTestId('page-tools')).toBeVisible();
  // The app bar back button goes to the parent, not through history.
  await app.getByTestId('tile-cookies').click();
  await app.getByTestId('appbar-back').click();
  await expect(app.getByTestId('page-tools')).toBeVisible();
});

test.describe('server stopped', () => {
  test.use({ allowHttpStatuses: [0] });
  test('when the server goes away the error banner says so and offers a retry', async ({ page }) => {
  const { info, stop } = await startServer();
  try {
    await page.goto(info.url);
    await expect(page.getByTestId('tab-home')).toBeVisible();
    await page.getByTestId('tab-tools').click();
    await page.getByTestId('tile-sysinfo').click();
    await expect(page.getByTestId('sysinfo-cpu')).toBeVisible();
    await stop();
    await page.getByTestId('tab-settings').click();
    const banner = page.getByTestId('error-banner').first();
    await expect(banner).toContainText('Backend unreachable');
    await expect(banner).toContainText('not reachable');
    await expect(banner.getByTestId('error-retry')).toBeVisible();
  } finally {
    await stop();
  }
});
});

// ---------------------------------------------------------------- separate servers

plain.describe('idle exit and cross-origin', () => {
  plain('the server exits after the last tab closes (short --idle-timeout)', async ({ browser }) => {
    plain.setTimeout(60_000);
    const { info, child, stop } = await startServer(['--idle-timeout', '8']);
    try {
      const page = await (await browser.newContext()).newPage();
      await page.goto(info.url);
      await plainExpect(page.getByTestId('tab-home')).toBeVisible();
      // Heartbeats keep it alive well past one timeout while the tab is open.
      await page.waitForTimeout(3000);
      plainExpect(child.exitCode).toBeNull();
      const exited = new Promise<number | null>((r) => child.once('exit', (code) => r(code)));
      await page.close();
      const code = await Promise.race([exited, new Promise<'timeout'>((r) => setTimeout(() => r('timeout'), 30_000))]);
      plainExpect(code).not.toBe('timeout');
    } finally {
      await stop();
    }
  });

  plain('requests from another origin are rejected', async ({ browser }) => {
    const { info, stop } = await startServer();
    // A tiny "evil" page on another port that tries to use the API with the stolen token.
    const evil = http.createServer((_q, res) => {
      res.setHeader('content-type', 'text/html');
      res.end('<!doctype html><title>evil</title><p>hi</p>');
    });
    await new Promise<void>((r) => evil.listen(0, '127.0.0.1', r));
    const evilOrigin = `http://127.0.0.1:${(evil.address() as AddressInfo).port}`;
    try {
      const page = await (await browser.newContext()).newPage();
      await page.goto(evilOrigin);
      const result = await page.evaluate(
        async ({ target, token }) => {
          const out: Record<string, string> = {};
          try {
            const r = await fetch(`${target}/api/call`, {
              method: 'POST',
              headers: { 'Content-Type': 'application/json', 'X-Sweep-Token': token },
              body: JSON.stringify({ callId: 'x', method: 'settings.get', params: {} }),
            });
            out.withToken = `status ${r.status}`;
          } catch (e) {
            out.withToken = `blocked: ${(e as Error).name}`;
          }
          try {
            const r = await fetch(`${target}/api/call`, {
              method: 'POST',
              mode: 'no-cors',
              headers: { 'Content-Type': 'text/plain' },
              body: JSON.stringify({ callId: 'x', method: 'settings.get', params: {} }),
            });
            out.noToken = `type ${r.type}`; // opaque: the page cannot read it either way
          } catch (e) {
            out.noToken = `blocked: ${(e as Error).name}`;
          }
          return out;
        },
        { target: info.origin, token: info.token },
      );
      // With a custom header the browser sends a preflight, which the server refuses (no CORS grant).
      plainExpect(result.withToken).toMatch(/^blocked: TypeError$/);

      // Even a request that reaches the server with the right token is refused when its Origin is foreign.
      const status = await new Promise<number>((resolve, reject) => {
        const req = http.request(
          `${info.origin}/api/call`,
          {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', 'X-Sweep-Token': info.token, Origin: evilOrigin },
          },
          (res) => {
            res.resume();
            resolve(res.statusCode ?? 0);
          },
        );
        req.on('error', reject);
        req.end(JSON.stringify({ callId: 'y', method: 'settings.get', params: {} }));
      });
      plainExpect(status).toBe(403);

      // DNS rebinding: a foreign Host header is refused before anything else.
      const hostStatus = await new Promise<number>((resolve, reject) => {
        const req = http.request(info.origin + '/', { headers: { Host: 'evil.example:1234' } }, (res) => {
          res.resume();
          resolve(res.statusCode ?? 0);
        });
        req.on('error', reject);
        req.end();
      });
      plainExpect(hostStatus).toBe(403);
    } finally {
      await new Promise((r) => evil.close(r));
      await stop();
    }
  });
});
