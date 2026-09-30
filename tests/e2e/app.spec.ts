import { expect, noHorizontalScroll, test } from './fixtures';
import { TABS, TOOL_TILES as TILES } from './helpers/routes';

test('token is stripped from the URL and the app keeps working after reload', async ({ app }) => {
  expect(app.url()).not.toContain('t=');
  await app.reload();
  await expect(app.getByTestId('tabbar')).toBeVisible();
  await app.getByTestId('tab-tools').click();
  await app.getByTestId('tile-sysinfo').click();
  // Authenticated API call succeeds using the token remembered in sessionStorage.
  await expect(app.getByTestId('sysinfo-cpu')).toBeVisible();
});

test('browser mode sends heartbeats', async ({ page, server }) => {
  const hb = page.waitForRequest((r) => r.url().endsWith('/api/heartbeat'));
  await page.goto(server.url);
  await hb;
});

test('every bottom tab loads its page', async ({ app }) => {
  for (const id of TABS) {
    await app.getByTestId(`tab-${id}`).click();
    await expect(app.getByTestId(`page-${id}`)).toBeVisible();
    await expect(app.getByTestId(`tab-${id}`)).toHaveAttribute('aria-current', 'page');
    const m = await noHorizontalScroll(app);
    expect(m.scrollWidth).toBeLessThanOrEqual(m.innerWidth);
  }
});

test('tools tab shows a tile for every tool', async ({ app }) => {
  await app.getByTestId('tab-tools').click();
  for (const t of TILES) await expect(app.getByTestId(`tile-${t.id}`)).toBeVisible();
  await expect(app.locator('[data-testid^="tile-"]')).toHaveCount(TILES.length);
});

for (const t of TILES) {
  test(`tile ${t.id} opens its page`, async ({ app }) => {
    await app.getByTestId('tab-tools').click();
    await app.getByTestId(`tile-${t.id}`).click();
    await expect(app.getByTestId(`page-${t.id}`)).toBeVisible();
    await expect(app.getByTestId('appbar-title')).toHaveText(t.title);
    // stays on the Tools tab, back button returns to the grid
    await expect(app.getByTestId('tab-tools')).toHaveAttribute('aria-current', 'page');
    const m = await noHorizontalScroll(app);
    expect(m.scrollWidth).toBeLessThanOrEqual(m.innerWidth);
    await app.getByTestId('appbar-back').click();
    await expect(app.getByTestId('page-tools')).toBeVisible();
  });
}

test('the Home tab is the health check, not a placeholder', async ({ app }) => {
  await app.getByTestId('tab-home').click();
  await expect(app.getByTestId('btn-scan')).toBeVisible();
  await expect(app.getByTestId('page-home')).not.toContainText('Coming soon');
});

test('System Info shows CPU, memory and disks', async ({ app }) => {
  await app.getByTestId('tab-tools').click();
  await app.getByTestId('tile-sysinfo').click();
  await expect(app.getByTestId('page-sysinfo')).toBeVisible();
  const cpu = app.getByTestId('sysinfo-cpu');
  await expect(cpu).toBeVisible();
  await expect(cpu).toContainText(/\d+ logical cores/);
  await expect(app.getByTestId('sysinfo-cpu-brand')).not.toBeEmpty();
  const mem = app.getByTestId('sysinfo-memory');
  await expect(mem).toBeVisible();
  await expect(mem).toContainText(/of .+ used/);
  await expect(app.getByTestId('sysinfo-disks')).toBeVisible();
  await expect(app.getByTestId('sysinfo-os')).toContainText('Uptime');
  await expect(app.getByRole('progressbar').first()).toBeVisible();
  await expect(app.getByTestId('error-banner')).toHaveCount(0);
  const m = await noHorizontalScroll(app);
  expect(m.scrollWidth).toBeLessThanOrEqual(m.innerWidth);
});

test('API rejects requests without the token', async ({ server }) => {
  const res = await fetch(`${server.origin}/api/call`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ callId: 'x', method: 'sysinfo.get', params: {} }),
  });
  expect(res.status).toBe(401);
  const ok = await fetch(`${server.origin}/api/call`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'X-Sweep-Token': server.token },
    body: JSON.stringify({ callId: 'y', method: 'api.methods', params: {} }),
  });
  expect(ok.status).toBe(200);
  expect(ok.headers.get('content-type')).toContain('application/x-ndjson');
});
