import { defineConfig } from "@playwright/test";

// UI_PORT lets parallel worktrees run their own dev server instead of reusing another checkout's on 1420.
const port = Number(process.env.UI_PORT ?? 1420);

export default defineConfig({
  testDir: "./tests/ui",
  outputDir: "./test-results",
  reporter: [["list"], ["html", { open: "never", outputFolder: "playwright-report" }]],
  timeout: 60_000,
  workers: 1,
  use: {
    baseURL: `http://localhost:${port}`,
    viewport: { width: 1440, height: 900 },
    browserName: "chromium",
  },
  webServer: {
    command: `pnpm exec vite --port ${port}`,
    url: `http://localhost:${port}`,
    reuseExistingServer: !process.env.UI_PORT,
    timeout: 60_000,
  },
});
