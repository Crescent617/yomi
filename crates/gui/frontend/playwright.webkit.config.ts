import { defineConfig, devices } from "@playwright/test";

// On-demand WebKit runs: the Tauri app embeds WKWebView (WebKit engine), so
// scroll/RAF/ResizeObserver timing differences vs Chromium matter. Usage:
//   npx playwright test -c playwright.webkit.config.ts [spec]
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  retries: 0,
  reporter: "list",
  use: {
    baseURL: "http://localhost:1420",
  },
  webServer: {
    command: "npm run dev",
    url: "http://localhost:1420",
    reuseExistingServer: true,
  },
  projects: [
    {
      name: "webkit",
      use: { ...devices["Desktop Safari"] },
    },
  ],
});
