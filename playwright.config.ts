import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/ui",
  outputDir: "./test-results",
  reporter: [["list"], ["html", { open: "never", outputFolder: "playwright-report" }]],
  timeout: 60_000,
  workers: 1,
  use: {
    baseURL: "http://localhost:1420",
    viewport: { width: 1440, height: 900 },
    browserName: "chromium",
  },
  webServer: {
    command: "pnpm exec vite",
    url: "http://localhost:1420",
    reuseExistingServer: true,
    timeout: 60_000,
  },
});
