import { defineConfig } from '@playwright/test';

// Assumes `pnpm build` and `cargo build -p sweep-cli` have already run (see scripts/check.sh).
// Chromium comes from PLAYWRIGHT_BROWSERS_PATH (pre-installed); never run `playwright install` here.
export default defineConfig({
  testDir: './tests/e2e',
  globalSetup: './tests/e2e/global-setup.ts',
  timeout: 30_000,
  expect: { timeout: 7_000 },
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: 0,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : 'list',
  use: {
    browserName: 'chromium',
    launchOptions: { args: ['--no-sandbox'] },
    trace: 'retain-on-failure',
  },
  projects: [
    { name: 'w380', use: { viewport: { width: 380, height: 640 } } },
    { name: 'w320', use: { viewport: { width: 320, height: 640 } } },
  ],
});
