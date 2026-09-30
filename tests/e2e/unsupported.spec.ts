import type { Page } from '@playwright/test';
import { expect, noHorizontalScroll, test } from './fixtures';

// The sandbox server's PATH is empty: no fwupdmgr, ubuntu-drivers, apt, ... A machine without
// those tools is normal, so the pages must explain it neutrally instead of showing a red error.
// Runs at 380 and 320 px (the two Playwright projects).
test.skip(process.platform !== 'linux', 'the server reports the Linux tool names');

async function fits(page: Page): Promise<void> {
  const m = await noHorizontalScroll(page);
  expect(m.scrollWidth, 'no horizontal scroll').toBeLessThanOrEqual(m.innerWidth);
}

async function openTool(page: Page, id: string, pageId: string): Promise<void> {
  await page.getByTestId('tab-tools').click();
  await page.getByTestId(`tile-${id}`).click();
  await expect(page.getByTestId(pageId)).toBeVisible();
}

test.describe('Unsupported is not an error', () => {
  test('Driver Updater without fwupd / ubuntu-drivers shows a friendly empty state', async ({ app }) => {
    await openTool(app, 'drivers', 'page-drivers');
    await app.getByTestId('btn-scan').click();
    const empty = app.getByTestId('drivers-unsupported');
    await expect(empty).toBeVisible();
    await expect(empty).toContainText("Driver updates aren't available on this system");
    await expect(app.getByTestId('drivers-unsupported-detail')).toContainText('fwupd');
    const hints = app.getByTestId('drivers-unsupported-hints');
    await expect(hints).toContainText('sudo apt install fwupd');
    await expect(hints).toContainText('ubuntu-drivers-common');
    await expect(app.getByTestId('error-banner')).toHaveCount(0);
    await expect(app.getByRole('alert')).toHaveCount(0);
    await expect(app.getByTestId('btn-update-selected')).toHaveCount(0);
    await fits(app);
    // the hints stay inside the viewport
    const box = await hints.boundingBox();
    const vw = app.viewportSize()?.width ?? 0;
    expect(box && box.x >= 0 && box.x + box.width <= vw).toBe(true);
  });

  test('Software Updater without a package manager shows the same neutral state', async ({ app }) => {
    await openTool(app, 'updater', 'page-updater');
    const empty = app.getByTestId('updater-unsupported');
    await expect(empty).toBeVisible();
    await expect(empty).toContainText("Software updates aren't available on this system");
    await expect(app.getByTestId('error-banner')).toHaveCount(0);
    await expect(app.getByRole('alert')).toHaveCount(0);
    await fits(app);
  });
});
