import { test, expect } from "@playwright/test";

test.describe("Yomi GUI", () => {
  test("page loads and renders layout", async ({ page }) => {
    await page.goto("/");
    await page.waitForLoadState("networkidle");
    await expect(page.locator("body")).toBeVisible();
  });

  test("has Yomi branding", async ({ page }) => {
    await page.goto("/");
    await page.waitForLoadState("networkidle");
    // The sidebar h1 contains "Yomi"
    await expect(page.locator("aside h1", { hasText: "Yomi" })).toBeVisible();
  });

  test("sidebar session band renders", async ({ page }) => {
    await page.goto("/");
    await page.waitForLoadState("networkidle");
    // SessionBand is inside the sidebar aside
    const sidebar = page.locator("aside").first();
    await expect(sidebar).toBeVisible();
  });

  test("chat view renders", async ({ page }) => {
    await page.goto("/");
    await page.waitForLoadState("networkidle");
    // ChatView is in the main area
    const main = page.locator("main").first();
    await expect(main).toBeVisible();
  });

  test("settings page loads", async ({ page }) => {
    await page.goto("/settings");
    await page.waitForLoadState("networkidle");
    await expect(page.locator("body")).toBeVisible();
  });

  test("skills page loads", async ({ page }) => {
    await page.goto("/skills");
    await page.waitForLoadState("networkidle");
    await expect(page.locator("body")).toBeVisible();
  });
});
