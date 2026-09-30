import { expect, test, type Page } from "@playwright/test";
import { calls, clearCalls, openHome, shot } from "./helpers";

// Mock rule (src/testing/mockBackend.ts): project 1 "ceremony" = folder 1 = ids 1..ceil(n/2), project 2 "reception" = the rest.
// 201 photos -> ceremony 101 (ids 1-101), reception 100 (ids 102-201).
const P = "projects-";

async function openProject(page: Page, id: number) {
  await page.getByTestId(`project-open-${id}`).click();
  await expect(page.getByTestId("top-bar")).toBeVisible();
  await expect(page.getByTestId("grid-toolbar")).toBeVisible();
}

test.describe("projects home page", () => {
  test("lists cards with counts and step; sort and search work", async ({ page }) => {
    await openHome(page);
    await expect(page.getByTestId("home-grid").locator("article")).toHaveCount(2);
    await expect(page.getByTestId("project-name-1")).toHaveText("ceremony");
    await expect(page.getByTestId("project-path-1")).toHaveText("/shoot/ceremony");
    await expect(page.getByTestId("project-photos-1")).toHaveText("101");
    await expect(page.getByTestId("project-photos-2")).toHaveText("100");
    await expect(page.getByTestId("project-step-1")).toHaveText("Cull");
    await expect(page.getByTestId("project-opened-2")).toContainText("Never opened");
    await expect(page.getByTestId("project-cover-1")).toBeVisible();

    // Default sort: last opened first (ceremony has been opened, reception never).
    await expect(page.getByTestId("home-grid").locator("article").first()).toHaveAttribute("data-project-name", "ceremony");
    await page.getByTestId("home-sort").selectOption("created");
    await expect(page.getByTestId("home-grid").locator("article").first()).toHaveAttribute("data-project-name", "reception");
    await page.getByTestId("home-sort").selectOption("name");
    await expect(page.getByTestId("home-grid").locator("article").first()).toHaveAttribute("data-project-name", "ceremony");

    await page.getByTestId("home-search").fill("recep");
    await expect(page.getByTestId("home-grid").locator("article")).toHaveCount(1);
    await expect(page.getByTestId("project-card-2")).toBeVisible();
    await page.getByTestId("home-search").fill("/shoot/cere"); // path matches too
    await expect(page.getByTestId("project-card-1")).toBeVisible();
    await page.getByTestId("home-search").fill("zzz");
    await expect(page.getByTestId("home-no-match")).toBeVisible();
    await page.getByTestId("home-search-clear").click();
    await expect(page.getByTestId("home-grid").locator("article")).toHaveCount(2);
  });

  test("screenshots at 1280x800 and 1728x1117", async ({ page }) => {
    await page.setViewportSize({ width: 1280, height: 800 });
    await openHome(page);
    await expect(page.getByTestId("project-cover-2")).toBeVisible();
    await shot(page, `${P}home-1280`);
    await page.setViewportSize({ width: 1728, height: 1117 });
    await shot(page, `${P}home-1728`);
    await page.setViewportSize({ width: 1280, height: 800 });
    await page.getByTestId("project-open-1").click();
    await expect(page.getByTestId("grid-toolbar")).toBeVisible();
    await shot(page, `${P}inside-1280`);
  });

  test("empty state, then New project creates and opens", async ({ page }) => {
    await page.addInitScript(() => (window.__mockPickDir = "/shoot/Smith Wedding"));
    await openHome(page, 0, "&projects=0");
    await expect(page.getByTestId("home-empty")).toContainText("Create your first project");
    await shot(page, `${P}empty`);
    await page.getByTestId("empty-new-project").click();
    const dlg = page.getByTestId("new-project-dialog");
    await expect(dlg).toBeVisible();
    await expect(page.getByTestId("new-project-create")).toBeDisabled();
    await page.getByTestId("new-project-pick").click();
    await expect(page.getByTestId("new-project-path")).toHaveText("/shoot/Smith Wedding");
    await expect(page.getByTestId("new-project-name")).toHaveValue("Smith Wedding"); // defaults to the folder name
    await page.getByTestId("new-project-shoot").selectOption("portrait");
    await shot(page, `${P}new-dialog`);
    await clearCalls(page);
    await page.getByTestId("new-project-create").click();
    await expect(page.getByTestId("top-bar")).toBeVisible();
    await expect(page.getByTestId("project-name")).toHaveText("Smith Wedding");
    const [create] = await calls(page, "create_project");
    expect(create.args).toMatchObject({ path: "/shoot/Smith Wedding", name: null, shootType: "portrait" });
    expect((await calls(page, "open_project")).length).toBe(1);
    await expect(page.getByTestId("shoot-select")).toHaveValue("portrait");
    await expect(page.getByTestId("grid-empty-catalog")).toBeVisible(); // a new project has no photos yet
  });

  test("an existing folder opens its project and says so", async ({ page }) => {
    await page.addInitScript(() => (window.__mockPickDir = "/shoot/ceremony/sub"));
    await openHome(page);
    await page.getByTestId("new-project").click();
    await page.getByTestId("new-project-pick").click();
    await page.getByTestId("new-project-create").click();
    await expect(page.getByTestId("project-name")).toHaveText("ceremony");
  });

  test("switching projects shows only each project's photos; reload returns home", async ({ page }) => {
    await openHome(page);
    await clearCalls(page);
    await openProject(page, 1);
    await expect(page.getByTestId("project-name")).toHaveText("ceremony");
    await expect(page.getByTestId("grid-toolbar")).toContainText("101 photos");
    await expect(page.getByTestId("cell-1")).toBeVisible();
    await expect(page.getByTestId("cell-102")).toHaveCount(0);
    const q1 = (await calls(page, "list_image_ids")).at(-1)!.args.query as { projectId: number };
    expect(q1.projectId).toBe(1);
    expect((await calls(page, "get_filter_counts")).at(-1)!.args).toMatchObject({ folderId: null, projectId: 1 });
    await expect(page.getByTestId("filter-bar")).toContainText("Unflagged 86"); // 8 + 7 + 86 = 101 photos of this project

    // Folder dropdown only lists this project's folder.
    const folders = page.getByTestId("folder-select").locator("option");
    await expect(folders).toHaveCount(2); // "All folders" + ceremony
    await expect(folders.nth(1)).toHaveText(/ceremony/);

    // Switch via the TopBar.
    await clearCalls(page);
    await page.getByTestId("project-switcher").click();
    await expect(page.getByTestId("switcher-project-1")).toBeVisible();
    await shot(page, `${P}switcher`);
    await page.getByTestId("switcher-project-2").click();
    await expect(page.getByTestId("project-name")).toHaveText("reception");
    await expect(page.getByTestId("grid-toolbar")).toContainText("100 photos");
    await expect(page.getByTestId("cell-102")).toBeVisible();
    await expect(page.getByTestId("cell-1")).toHaveCount(0);
    expect(((await calls(page, "list_image_ids")).at(-1)!.args.query as { projectId: number }).projectId).toBe(2);
    expect((await calls(page, "get_filter_counts")).at(-1)!.args).toMatchObject({ projectId: 2 });
    expect((await calls(page, "open_project")).at(-1)!.args).toEqual({ projectId: 2 });

    // Scenes and burst lookups carry the project too.
    await clearCalls(page);
    await page.getByTestId("analyze-menu").click();
    await page.getByTestId("scenes-detect").click();
    await expect(page.getByTestId("scene-strip")).toBeVisible();
    expect((await calls(page, "detect_scenes"))[0].args).toMatchObject({ folderId: null, projectId: 2 });
    await expect.poll(async () => (await calls(page, "list_scenes")).at(-1)?.args).toMatchObject({ projectId: 2 });
    // Re-analyze all is scoped to the project.
    await clearCalls(page);
    await page.getByTestId("analyze-menu").click();
    await page.getByTestId("reanalyze-all").click();
    expect((await calls(page, "analyze_images"))[0].args.scope).toEqual({ kind: "project", projectId: 2 });
    // Clearing filters keeps the scope.
    await clearCalls(page);
    await page.getByTestId("shoot-select").selectOption("sports");
    expect((await calls(page, "set_project_shoot_type"))[0].args).toEqual({ projectId: 2, shootType: "sports" });

    // Back to the home page: counts on the cards are still right.
    await page.getByTestId("project-switcher").click();
    await page.getByTestId("switcher-home").click();
    await expect(page.getByTestId("home-page")).toBeVisible();
    await expect(page.getByTestId("project-photos-1")).toHaveText("101");
    await expect(page.getByTestId("project-photos-2")).toHaveText("100");
    await expect(page.getByTestId("home-grid").locator("article").first()).toHaveAttribute("data-project-name", "reception"); // opened last

    // Relaunch returns to the home page, never a project.
    await openProject(page, 1);
    await page.reload();
    await expect(page.getByTestId("home-page")).toBeVisible();
    await expect(page.getByTestId("top-bar")).toHaveCount(0);
  });

  test("the logo button returns to the home page", async ({ page }) => {
    await openHome(page);
    await openProject(page, 2);
    await page.getByTestId("home-button").click();
    await expect(page.getByTestId("home-page")).toBeVisible();
  });

  test("rename and choose cover", async ({ page }) => {
    await openHome(page);
    await page.getByTestId("project-menu-1").click();
    await page.getByTestId("project-menu-rename-1").click();
    await page.getByTestId("rename-input").fill("  Ceremony (final)  ");
    await page.getByTestId("rename-save").click();
    await expect(page.getByTestId("project-name-1")).toHaveText("Ceremony (final)");
    expect((await calls(page, "rename_project")).at(-1)!.args).toEqual({ projectId: 1, name: "Ceremony (final)" });

    await page.getByTestId("project-menu-1").click();
    await expect(page.getByTestId("project-menu-autocover-1")).toHaveCount(0);
    await page.getByTestId("project-menu-cover-1").click();
    await expect(page.getByTestId("cover-dialog")).toBeVisible();
    await page.getByTestId("cover-option-7").click();
    await expect(page.getByTestId("project-cover-1")).toHaveAttribute("src", /thumb\/7\.jpg/);
    await page.getByTestId("project-menu-1").click();
    await page.getByTestId("project-menu-autocover-1").click();
    await expect.poll(async () => (await calls(page, "set_project_cover")).at(-1)?.args.imageId).toBe(null);
  });

  test("set the cover from inside a project", async ({ page }) => {
    await openHome(page);
    await openProject(page, 1);
    await page.getByTestId("cell-5").click();
    await page.getByTestId("project-switcher").click();
    await page.getByTestId("switcher-set-cover").click();
    expect((await calls(page, "set_project_cover")).at(-1)!.args).toEqual({ projectId: 1, imageId: 5 });
  });

  test("remove asks first, says files are never deleted, and leaves the rest working", async ({ page }) => {
    await openHome(page);
    await page.getByTestId("project-menu-2").click();
    await page.getByTestId("project-menu-remove-2").click();
    await expect(page.getByTestId("remove-copy")).toContainText("never deleted");
    await shot(page, `${P}remove-dialog`);
    await page.getByTestId("remove-cancel").click();
    await expect(page.getByTestId("project-card-2")).toBeVisible();
    expect((await calls(page, "remove_project")).length).toBe(0);

    await page.getByTestId("project-menu-2").click();
    await page.getByTestId("project-menu-remove-2").click();
    await page.getByTestId("remove-confirm").click();
    await expect(page.getByTestId("project-card-2")).toHaveCount(0);
    await expect(page.getByTestId("toasts")).toContainText("Files on disk were not touched");
    expect((await calls(page, "remove_project"))[0].args).toEqual({ projectId: 2 });
    await expect(page.getByTestId("home-grid").locator("article")).toHaveCount(1);

    // The surviving project still opens with its own photos and the switcher lists only it.
    await openProject(page, 1);
    await expect(page.getByTestId("grid-toolbar")).toContainText("101 photos");
    await page.getByTestId("project-switcher").click();
    await expect(page.getByTestId("switcher-project-2")).toHaveCount(0);
    await expect(page.getByTestId("error")).toHaveCount(0);

    // Removing the last project leaves the empty state.
    await page.keyboard.press("Escape");
    await page.getByTestId("home-button").click();
    await page.getByTestId("project-menu-1").click();
    await page.getByTestId("project-menu-remove-1").click();
    await page.getByTestId("remove-confirm").click();
    await expect(page.getByTestId("home-empty")).toBeVisible();
  });

  test("a moved folder offers Locate folder and relinks it", async ({ page }) => {
    await page.addInitScript(() => (window.__mockPickDir = "/missing/old-drive/Shoot"));
    await openHome(page);
    await page.getByTestId("new-project").click();
    await page.getByTestId("new-project-pick").click();
    await page.getByTestId("new-project-create").click();
    await expect(page.getByTestId("top-bar")).toBeVisible();
    await page.getByTestId("home-button").click();
    await expect(page.getByTestId("project-missing-3")).toContainText("Folder not found");
    await shot(page, `${P}missing-folder`);
    await page.evaluate(() => (window.__mockPickDir = "/Volumes/new/Shoot"));
    await page.getByTestId("project-locate-3").click();
    await expect(page.getByTestId("project-missing-3")).toHaveCount(0);
    expect((await calls(page, "relocate_folder")).at(-1)!.args).toMatchObject({ newPath: "/Volumes/new/Shoot" });
    await expect(page.getByTestId("project-path-3")).toHaveText("/Volumes/new/Shoot");
  });

  test("reveal in Finder uses the project folder", async ({ page }) => {
    await openHome(page);
    await page.getByTestId("project-menu-1").click();
    await page.getByTestId("project-menu-reveal-1").click();
    expect((await calls(page, "reveal_in_finder")).at(-1)!.args).toEqual({ path: "/shoot/ceremony" });
  });
});
