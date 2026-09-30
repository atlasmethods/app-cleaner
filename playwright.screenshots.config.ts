import { defineConfig } from '@playwright/test';

// `pnpm screenshots`: regenerates docs/screenshots/*.png (needs `pnpm build` and
// `cargo build -p sweep-cli --features testutil`, like the e2e suite).
export default defineConfig({
  testDir: './scripts',
  testMatch: 'screenshots.ts',
  globalSetup: './tests/e2e/global-setup.ts',
  timeout: 300_000,
  workers: 1,
  reporter: 'list',
  use: {
    browserName: 'chromium',
    launchOptions: { args: ['--no-sandbox'] },
    viewport: { width: 380, height: 640 },
  },
});
