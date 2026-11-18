// Drives the web UI in headless Chromium against the dev bridge and saves
// screenshots of every main screen. Used to review layout on desktop and
// phone-sized viewports.
//
//   cargo run -p sift --example bridge &
//   pnpm dev &
//   node scripts/shots.mjs /path/to/photos [outDir]
//
// BRIDGE and DEV name the two servers when they are not on their usual ports.
import { chromium } from "playwright-core";
import { copyFileSync, mkdirSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

const folder = process.argv[2];
const out = process.argv[3] ?? "/tmp/sift-shots";
if (!folder) throw new Error("usage: node scripts/shots.mjs <photo folder> [outDir]");
mkdirSync(out, { recursive: true });

const only = process.env.ONLY?.split(",");
const lookFiles = (process.env.LOOKS ?? "").split(",").filter(Boolean);
const bridge = process.env.BRIDGE ?? "http://127.0.0.1:4777";
const baseFor = (dir) => `${process.env.DEV ?? "http://localhost:1420"}/?bridge=${bridge}&pick=${encodeURIComponent(dir)}${lookFiles.map((l) => `&look=${encodeURIComponent(l)}`).join("")}`;
const base = baseFor(folder);
const reset = () => fetch(`${bridge}/invoke/dev_reset`, { method: "POST", body: "{}" });
const browser = await chromium.launch({ channel: process.env.CHANNEL ?? "chrome" });

async function session(name, { width, height, mobile = false, scheme = "dark", lang = "zh-CN", platform = "macos", query = "", pick }, steps) {
  if (only && !only.includes(name)) return;
  // A closing page still saves its progress; let that land before wiping it.
  await new Promise((r) => setTimeout(r, 500));
  await reset();
  const context = await browser.newContext({
    viewport: { width, height },
    deviceScaleFactor: 2,
    colorScheme: scheme,
    locale: lang,
    hasTouch: mobile,
    isMobile: mobile,
  });
  const page = await context.newPage();
  page.on("pageerror", (e) => console.log(`[${name}] page error: ${e.message}`));
  page.on("response", (r) => r.status() >= 400 && console.log(`[${name}] ${r.status()} ${decodeURIComponent(r.url()).slice(0, 160)}`));
  await page.addInitScript(() => localStorage.setItem("sift.settings", JSON.stringify({ onboarded: true })));
  await page.goto(`${pick ? baseFor(pick) : base}&platform=${platform}${mobile ? "&mobile" : ""}${query}`);
  let n = 0;
  const shot = async (label) => {
    await page.waitForTimeout(350);
    await page.screenshot({ path: `${out}/${name}-${String(++n).padStart(2, "0")}-${label}.png` });
  };
  try {
    await steps(page, shot);
  } catch (e) {
    console.log(`[${name}] FAILED: ${e.message.split("\n")[0]}`);
    await shot("failure");
  }
  await context.close();
}

const openLibrary = async (page) => {
  await page.locator(".welcome button.primary").click();
  await page.locator(".cell img").first().waitFor();
  await page.waitForLoadState("networkidle");
};

const desktop = { width: 1320, height: 860 };
const phone = { width: 390, height: 844, mobile: true, platform: "ios" };

await session("desktop", desktop, async (page, shot) => {
  await shot("welcome");
  await openLibrary(page);
  await page.locator(".cell").nth(1).click();
  await page.keyboard.press("3");
  await page.keyboard.press("P");
  await page.locator(".cell").nth(3).click();
  await page.keyboard.press("X");
  await page.locator(".cell").nth(0).click();
  await shot("library");

  await page.keyboard.press("Enter");
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  await shot("viewer");
  await page.keyboard.press("I");
  await shot("viewer-no-sidebar");
  await page.keyboard.press("I");
  await page.keyboard.press("Z");
  await shot("viewer-100");
  await page.keyboard.press("Z");
  await page.keyboard.press("G");
  await page.keyboard.press("G");
  await page.keyboard.press("G");
  await page.keyboard.press("R");
  await shot("viewer-spiral-crop");
  await page.keyboard.press("R");
  await page.keyboard.press("G");
  await page.keyboard.press("G");
  await page.keyboard.press("G");
  await page.keyboard.press("ArrowRight");
  await page.locator(".tier.full.ready").waitFor({ timeout: 30000 });
  await shot("viewer-pair");
  await page.keyboard.press("N");
  await page.waitForTimeout(300);
  await page.locator(".tier.full.ready").waitFor({ timeout: 30000 });
  await shot("viewer-original");
  await page.keyboard.press("N");
  await page.keyboard.press("ArrowLeft");
  await page.keyboard.press("C");
  await page.locator(".pane").nth(1).locator(".tier.full.ready").waitFor({ timeout: 20000 });
  await shot("compare");
  await page.keyboard.press("C");
  await page.keyboard.press("Escape");

  await page.locator('[aria-label="导出"]').click();
  await page.locator('.sheet [role="switch"]').first().click();
  await shot("export-copy");
  await page.getByRole("tab", { name: "加参数边框" }).click();
  await page.locator(".frame-preview img").waitFor({ timeout: 20000 });
  await shot("export-frame");
  await page.keyboard.press("Escape");
  await page.locator('[aria-label="传输"]').click();
  await page.locator(".qr svg").waitFor();
  await shot("transfer");
  await page.keyboard.press("Escape");
  await page.locator('[aria-label="更多"]').click();
  await shot("more-menu");
  await page.getByRole("menuitem", { name: "设置" }).click();
  await shot("settings");
  await page.keyboard.press("Escape");
  await page.locator('[aria-label="更多"]').click();
  await page.getByRole("menuitem", { name: "帮助" }).click();
  await shot("help");
});

await session("eyes", desktop, async (page, shot) => {
  await openLibrary(page);
  await page.locator(".cell", { has: page.locator('img[src*="portrait"]') }).dblclick();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  await page.keyboard.press("E");
  await page.waitForTimeout(1500);
  await shot("eye-zoom");
});

// Eye close-up on animals: run on a folder of pet and bird photos.
await session("animals", desktop, async (page, shot) => {
  await openLibrary(page);
  const count = await page.locator(".cell").count();
  await page.locator(".cell").first().dblclick();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  await page.keyboard.press("E");
  for (let i = 0; i < count; i++) {
    await page.waitForTimeout(1800);
    const toast = (await page.locator(".toast").count()) ? await page.locator(".toast").textContent() : "zoomed to the eyes";
    console.log(`[animals] photo ${i + 1}: ${toast}`);
    await shot(`eyes-${i + 1}`);
    await page.keyboard.press("ArrowRight");
  }
});

// Import looks, apply one per photo, and export with them.
await session("looks", desktop, async (page, shot) => {
  if (!lookFiles.length) return console.log("[looks] skipped: set LOOKS=a.cube,b.NP3");
  await openLibrary(page);
  await page.locator(".cell", { has: page.locator(".cell-raw") }).first().dblclick();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  const pickLook = async (name) => {
    await page.locator('.dock [aria-label="滤镜"]').click();
    await page.getByRole("menuitem", { name }).click();
    await page.waitForTimeout(400);
    await page.locator(".tier.full.ready").waitFor({ timeout: 60000 });
  };
  await page.locator('.dock [aria-label="滤镜"]').click();
  await page.getByRole("menuitem", { name: "导入滤镜" }).click();
  await page.waitForTimeout(800);
  await page.locator('.dock [aria-label="滤镜"]').click();
  await shot("menu");
  await page.keyboard.press("Escape");
  for (const name of ["TealOrange", "Mono_Contrast", "Faded_Warm", "原片"]) {
    await pickLook(name);
    await shot(name);
  }
  await pickLook("TealOrange");
  await page.keyboard.press("P");
  await page.keyboard.press("Escape");
  await page.locator('[aria-label="导出"]').click();
  await shot("export");
});

// Nikon's engine (start the bridge with SIFT_NIKON_ROOT set): NP3 looks on a NEF come from it.
await session("sdk", desktop, async (page, shot) => {
  const info = await (await fetch(`${bridge}/invoke/app_info`, { method: "POST", body: "{}" })).json();
  if (!info.nikon_sdk || !lookFiles.length) return console.log("[sdk] skipped: needs a bridge with Nikon's engine and LOOKS=<NP3 files>");
  await openLibrary(page);
  await page.locator('[aria-label="更多"]').click();
  await page.getByRole("menuitem", { name: "设置" }).click();
  await page.getByText("已内置", { exact: true }).scrollIntoViewIfNeeded();
  await shot("settings");
  await page.keyboard.press("Escape");
  await page.locator(".cell", { has: page.locator(".cell-raw") }).first().dblclick();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  await page.locator('.dock [aria-label="滤镜"]').click();
  await page.getByRole("menuitem", { name: "导入滤镜" }).click();
  await page.waitForTimeout(1200);
  await page.locator('.dock [aria-label="滤镜"]').click();
  await shot("menu");
  const labels = await page.getByRole("menuitem").allTextContents();
  console.log(`[sdk] menu: ${labels.filter((l) => l.includes("·")).join(" | ")}`);
  await page.keyboard.press("Escape");
  for (const name of (process.env.SDK_LOOKS ?? "").split(",").filter(Boolean)) {
    const t0 = Date.now();
    await page.locator('.dock [aria-label="滤镜"]').click();
    await page.getByRole("menuitem", { name }).click();
    // The pill names the look once it is on screen; until then it says the look is being applied.
    await page.locator(".status-pill.quiet").waitFor({ timeout: 60000 });
    console.log(`[sdk] ${name}: shown after ${((Date.now() - t0) / 1000).toFixed(1)}s, label "${await page.locator(".status-pill").textContent()}"`);
    await page.locator(".histogram path").first().waitFor({ timeout: 20000 });
    await shot(name.replace(/\W+/g, "_"));
  }
  // The look list in Settings says who renders each look with the engine there.
  await page.keyboard.press("Escape");
  await page.locator('[aria-label="更多"]').click();
  await page.getByRole("menuitem", { name: "设置" }).click();
  await page.getByText("管理滤镜").scrollIntoViewIfNeeded();
  await shot("settings-looks");
});

// Look tools: clipping warning, keeping the in-camera recipe, the look wall and batch apply.
await session("tools", desktop, async (page, shot) => {
  if (!lookFiles.length) return console.log("[tools] skipped: set LOOKS=<look files>");
  await openLibrary(page);
  await page.locator(".cell", { has: page.locator(".cell-raw") }).first().dblclick();
  await page.locator(".tier.full.ready").waitFor({ timeout: 30000 });
  await page.locator('.dock [aria-label="滤镜"]').click();
  await page.getByRole("menuitem", { name: "导入滤镜" }).click();
  await page.waitForTimeout(1200);
  await page.keyboard.press("H");
  await page.locator(".tier.clip").waitFor({ timeout: 20000 });
  const mask = await page.locator(".tier.clip").evaluate((img) => img.decode().then(() => `${img.naturalWidth}x${img.naturalHeight}`));
  console.log(`[tools] clipping warning: ${mask}, button on: ${await page.locator('.dock [aria-label="过曝 / 欠曝警告"]').getAttribute("aria-pressed")}`);
  await shot("clipping");
  await page.keyboard.press("H");
  console.log(`[tools] clipping off: ${(await page.locator(".tier.clip").count()) === 0}`);
  const save = page.getByRole("button", { name: "存入滤镜库" });
  if (await save.count()) {
    await save.click();
    await page.locator(".toast").waitFor();
    console.log(`[tools] kept the camera's recipe: "${await page.locator(".toast").textContent()}", button gone: ${(await save.count()) === 0}`);
    await shot("recipe-saved");
  } else console.log("[tools] this photo has no recipe to keep (or the library has it already)");
  const t0 = Date.now();
  await page.keyboard.press("W");
  await page.locator(".look-wall").waitFor();
  await page.waitForFunction(() => document.querySelectorAll(".look-tile-image.loading").length === 0, null, { timeout: 120000 });
  console.log(`[tools] wall: ${await page.locator(".look-tile").count()} tiles in ${((Date.now() - t0) / 1000).toFixed(1)}s`);
  await shot("wall");
  await page.locator(".look-tile").last().click();
  await page.locator(".status-pill.quiet").waitFor({ timeout: 60000 });
  console.log(`[tools] chosen from the wall: "${await page.locator(".status-pill").textContent()}", wall closed: ${(await page.locator(".look-wall").count()) === 0}`);
  await page.keyboard.press("Escape");
  await page.keyboard.press("ControlOrMeta+A");
  await page.locator('.toolbar [aria-label="给所选照片套用滤镜"]').click();
  await shot("batch-menu");
  await page.getByRole("menuitem").nth(2).click();
  console.log(`[tools] batch: "${await page.locator(".toast").textContent()}", cells with a look: ${await page.locator(".cell-look").count()} of ${await page.locator(".cell").count()}`);
  await shot("batch-applied");
});

// Selecting in the grid: clicks, ranges, ticks, dragging a rectangle, the bar, sorting.
// Best with a folder of a few hundred photos.
await session("select", desktop, async (page, shot) => {
  await openLibrary(page);
  const count = () => page.evaluate(() => window.__sift?.selection?.() ?? document.querySelectorAll(".cell.selected").length);
  const picked = async () => (await page.locator(".selection-bar strong").count()) ? await page.locator(".selection-bar strong").textContent() : `${await count()} (no bar)`;
  const cell = (i) => page.locator(".cell").nth(i);
  console.log(`[select] opened on the first photo: ${await count()} selected`);
  await cell(1).click();
  await cell(3).click({ modifiers: ["ControlOrMeta"] });
  console.log(`[select] click + mod-click: ${await picked()}`);
  await cell(6).click({ modifiers: ["Shift"] });
  console.log(`[select] shift-click to the 7th keeps the first pick: ${await picked()} (want 5)`);
  await cell(4).click({ modifiers: ["Shift"] });
  console.log(`[select] shift-click back shrinks the range: ${await picked()} (want 3)`);
  await cell(9).hover();
  await cell(9).locator(".cell-check").click();
  console.log(`[select] tick without a modifier: ${await picked()} (want 4)`);
  await shot("ticked");
  await cell(9).locator(".cell-check").click();
  console.log(`[select] untick: ${await picked()} (want 3)`);
  await cell(1).dblclick();
  await page.locator(".viewer").waitFor();
  await page.keyboard.press("Escape");
  await page.locator(".viewer").waitFor({ state: "detached" });
  console.log(`[select] after a look at one of them: ${await picked()} (want 3)`);
  await page.keyboard.press("Escape");
  console.log(`[select] Escape: ${await picked()} (want 1, no bar)`);
  await page.keyboard.press("Shift+ArrowRight");
  await page.keyboard.press("Shift+ArrowRight");
  await page.keyboard.press("Shift+ArrowRight");
  await page.keyboard.press("Shift+ArrowLeft");
  console.log(`[select] shift+arrows 3 right, 1 left: ${await picked()} (want 3)`);
  // Drag a rectangle from the gap left of the first cell across two rows.
  const a = await cell(0).boundingBox();
  const b = await cell(3).boundingBox();
  const rowBelow = a.y + a.height * 1.5;
  await page.mouse.move(a.x - 8, a.y + 20);
  await page.mouse.down();
  await page.mouse.move(b.x + 30, rowBelow, { steps: 8 });
  await shot("marquee");
  const during = await picked();
  await page.mouse.up();
  console.log(`[select] dragged over 4 columns x 2 rows: ${during}, after release: ${await picked()} (want 8)`);
  await page.mouse.move(a.x + 30, a.y + 30);
  await page.mouse.down();
  await page.mouse.move(b.x + 30, a.y + 40, { steps: 6 });
  await page.mouse.up();
  console.log(`[select] a drag that starts on a photo: ${await picked()} (want 4)`);
  await page.locator(".selection-bar .stars button").nth(3).click();
  await page.locator(".selection-bar .flag-button.pick").click();
  console.log(`[select] bar: ${await page.locator(".cell-stars").count()} rated, ${await page.locator(".cell-flag.pick").count()} picked (want 4, 4)`);
  await shot("bar");
  // Dragging to the bottom edge scrolls and keeps selecting.
  const view = page.viewportSize();
  await page.mouse.move(a.x - 8, a.y + 20);
  await page.mouse.down();
  await page.mouse.move(view.width - 40, view.height - 12, { steps: 6 });
  await page.waitForTimeout(1500);
  const scrolled = await page.locator(".grid-scroller").evaluate((el) => el.scrollTop);
  await page.mouse.up();
  console.log(`[select] drag to the bottom edge: scrolled ${Math.round(scrolled)} px, ${await picked()}`);
  await shot("drag-scrolled");
  await page.keyboard.press("ControlOrMeta+A");
  console.log(`[select] select all: ${await picked()}`);
  await page.locator(".grid-scroller").evaluate((el) => (el.scrollTop = 0));
  await page.waitForTimeout(300);
  // A click on empty space (the gap beside the first column) clears.
  const c = await page.locator(".cell").first().boundingBox();
  await page.mouse.click(c.x - 8, c.y + 30);
  console.log(`[select] click on empty space: ${await picked()} (want 0)`);
  // Sorting.
  const names = () => page.locator(".cell").evaluateAll((cells) => cells.slice(0, 3).map((el) => el.querySelector("img")?.src.match(/[^/%]+\.(JPG|NEF)/i)?.[0] ?? "?").join(", "));
  await page.locator(".grid-scroller").evaluate((el) => (el.scrollTop = 0));
  await page.waitForTimeout(300);
  console.log(`[select] by name: ${await names()}`);
  await page.locator('.toolbar [aria-label="排序"]').click();
  await shot("sort-menu");
  await page.getByRole("menuitem", { name: "降序" }).click();
  const top = async () => {
    await page.waitForTimeout(200);
    await page.locator(".grid-scroller").evaluate((el) => (el.scrollTop = 0));
    await page.waitForTimeout(300);
  };
  await top();
  console.log(`[select] name, descending: ${await names()}`);
  await page.locator('.toolbar [aria-label="排序"]').click();
  await page.getByRole("menuitem", { name: "星级" }).click();
  await top();
  console.log(`[select] by rating, descending: ${await page.locator(".cell").evaluateAll((cells) => cells.slice(0, 5).map((el) => el.querySelector(".cell-stars")?.textContent ?? "-").join(" "))}`);
  await shot("sorted-by-rating");
  await page.locator('.toolbar [aria-label="排序"]').click();
  await page.getByRole("menuitem", { name: "拍摄时间" }).click();
  await top();
  console.log(`[select] by time, descending: ${await names()}`);
  // Scrolling: how many thumbnails are asked for when the grid is flung to the end.
  await page.locator('.toolbar [aria-label="排序"]').click();
  await page.getByRole("menuitem", { name: "文件名" }).click();
  let asked = 0;
  page.on("request", (r) => r.url().includes("/thumb") && asked++);
  const t0 = Date.now();
  await page.locator(".grid-scroller").evaluate(async (el) => {
    for (let y = 0; y < el.scrollHeight; y += 400) {
      el.scrollTop = y;
      await new Promise((r) => requestAnimationFrame(r));
    }
  });
  await page.waitForFunction(() => [...document.querySelectorAll(".cell-frame")].every((f) => !f.classList.contains("pending")), null, { timeout: 30000 });
  console.log(`[select] flung to the end: ${asked} thumbnails asked for, settled in ${Date.now() - t0} ms, ${await page.locator(".cell").count()} cells mounted`);
});

await session("select-phone", phone, async (page, shot) => {
  await openLibrary(page);
  const first = await page.locator(".cell").first().boundingBox();
  const client = await page.context().newCDPSession(page);
  const touch = (type, x, y) => client.send("Input.dispatchTouchEvent", { type, touchPoints: type === "touchEnd" ? [] : [{ x, y }] });
  await touch("touchStart", first.x + 30, first.y + 30);
  await page.waitForTimeout(600);
  await touch("touchEnd");
  await page.waitForTimeout(200);
  console.log(`[select-phone] long press: "${await page.locator(".selection-bar strong").textContent()}", viewer open: ${(await page.locator(".viewer").count()) > 0}`);
  await page.locator(".cell").nth(2).tap();
  await page.locator(".cell").nth(4).tap();
  console.log(`[select-phone] two taps more: "${await page.locator(".selection-bar strong").textContent()}"`);
  await shot("selecting");
  await page.locator(".selection-bar").getByText("完成").tap();
  await page.locator(".cell").nth(1).tap();
  console.log(`[select-phone] after Done a tap opens the photo: ${(await page.locator(".viewer").count()) > 0}`);
});

// Trackpad: a sideways two-finger swipe turns one page per swipe; pinch zooms about the pointer.
await session("trackpad", desktop, async (page) => {
  await openLibrary(page);
  await page.locator(".cell").nth(1).dblclick();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  const title = () => page.locator(".viewer-title b, .viewer-title strong").first().textContent();
  const zoom = async () => Number((await page.locator(".zoom-pill").textContent()).replace("%", ""));
  const box = await page.locator(".pane").boundingBox();
  await page.mouse.move(box.x + box.width * 0.7, box.y + box.height * 0.4);
  const first = await title();
  // One swipe: many small events and a momentum tail, well past the threshold.
  for (let i = 0; i < 30; i++) await page.mouse.wheel(i < 12 ? 18 : 4, 0);
  await page.waitForTimeout(500);
  const second = await title();
  for (let i = 0; i < 30; i++) await page.mouse.wheel(i < 12 ? -18 : -4, 0);
  await page.waitForTimeout(500);
  const back = await title();
  console.log(`[trackpad] swipe: ${first} -> ${second} -> ${back} (one photo each way: ${second !== first && back === first})`);
  // Vertical scrolling while fitted does nothing.
  for (let i = 0; i < 10; i++) await page.mouse.wheel(0, 30);
  await page.waitForTimeout(300);
  console.log(`[trackpad] vertical scroll keeps the photo: ${(await title()) === first}`);
  // Pinch (Chromium sends ctrl+wheel) zooms in around the pointer; scrolling then pans instead of turning pages.
  const fit = await zoom();
  await page.keyboard.down("Control");
  for (let i = 0; i < 12; i++) await page.mouse.wheel(0, -10);
  await page.keyboard.up("Control");
  await page.waitForTimeout(300);
  const zoomed = await zoom();
  for (let i = 0; i < 20; i++) await page.mouse.wheel(20, 0);
  await page.waitForTimeout(400);
  console.log(`[trackpad] pinch: ${fit}% -> ${zoomed}%; scrolling while zoomed keeps the photo: ${(await title()) === first}`);
});

// Mouse: what the wheel and a double click do in the viewer is chosen in Settings.
await session("mouse", { ...desktop, platform: "windows" }, async (page, shot) => {
  await openLibrary(page);
  const settings = async (...choices) => {
    await page.locator('[aria-label="更多"]').click();
    await page.getByRole("menuitem", { name: "设置" }).click();
    for (const name of choices) await page.locator(".sheet").getByRole("tab", { name, exact: true }).click();
    const chosen = await page.locator('.sheet [role="tab"][aria-selected="true"]').allTextContents();
    return chosen.slice(1).join(", ");
  };
  const position = async () => Number((await page.locator(".viewer-title span").first().textContent()).split("/")[0]);
  const zoom = async () => Number((await page.locator(".zoom-pill").textContent()).replace("%", ""));
  const open = async () => {
    await page.locator(".cell").nth(4).dblclick();
    await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
    const box = await page.locator(".pane").boundingBox();
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    return [box.x + box.width / 2, box.y + box.height / 2];
  };
  const notches = async (n, dy, wait) => {
    for (let i = 0; i < n; i++) {
      await page.mouse.wheel(0, dy);
      if (wait) await page.waitForTimeout(wait);
    }
    await page.waitForTimeout(250);
  };

  console.log(`[mouse] as installed on Windows: ${await settings()}`);
  await page.keyboard.press("Escape");
  let [x, y] = await open();
  const fit = await zoom();
  await notches(3, -100, 120);
  console.log(`[mouse] wheel zooms: ${fit}% -> ${await zoom()}%, photo ${await position()} (stays 5)`);
  await page.keyboard.press("Z");
  await page.mouse.dblclick(x, y);
  await page.waitForTimeout(300);
  console.log(`[mouse] double click zooms: ${fit}% -> ${await zoom()}%, viewer open: ${(await page.locator(".viewer").count()) > 0}`);
  await page.keyboard.press("Escape");

  console.log(`[mouse] chosen: ${await settings("切换照片", "返回列表")}`);
  await shot("settings");
  await page.keyboard.press("Escape");
  [x, y] = await open();
  const seen = [await position()];
  await notches(3, 100, 120);
  seen.push(await position());
  await notches(2, -100, 120);
  seen.push(await position());
  // Spun fast: every notch still counts (100 here is a notch on Windows whatever the display scale).
  await notches(6, 100, 0);
  seen.push(await position());
  await notches(6, -100, 0);
  seen.push(await position());
  // Sped up by the system, as on a Mac: slow notches are small and each turns a page.
  await notches(3, 4, 120);
  seen.push(await position());
  await notches(3, -4, 120);
  seen.push(await position());
  console.log(`[mouse] wheel turns pages: ${seen.join(" -> ")} (expected 5 -> 8 -> 6 -> 12 -> 6 -> 9 -> 6)`);
  // A trackpad sends a dense stream of small steps: a page per notch of travel, not one per event.
  await page.evaluate(async () => {
    const pane = document.querySelector(".pane");
    for (let i = 0; i < 40; i++) {
      pane.dispatchEvent(new WheelEvent("wheel", { deltaY: 5, bubbles: true, cancelable: true }));
      await new Promise((r) => setTimeout(r, 10));
    }
  });
  await page.waitForTimeout(250);
  console.log(`[mouse] forty small steps 10 ms apart, four notches of travel: ${seen.at(-1)} -> ${await position()} (4 photos, not 40)`);
  await page.keyboard.down("Control");
  await notches(8, -10, 0);
  await page.keyboard.up("Control");
  const before = await position();
  console.log(`[mouse] Ctrl + wheel still zooms: ${fit}% -> ${await zoom()}%`);
  await notches(2, 100, 120);
  console.log(`[mouse] wheel turns pages from a zoomed photo too: ${before} -> ${await position()}`);
  const name = await page.locator(".viewer-title strong").textContent();
  await page.mouse.dblclick(x, y);
  await page.waitForTimeout(400);
  const cursor = await page.locator(".cell.selected, .cell.cursor").first().locator("img").first().getAttribute("src");
  console.log(`[mouse] double click goes back: viewer open ${(await page.locator(".viewer").count()) > 0}, the grid is on ${name}: ${decodeURIComponent(cursor ?? "").includes(name)}`);
  await page.locator(".cell.selected, .cell.cursor").first().dblclick();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  console.log(`[mouse] and a double click in the grid opens it again: ${(await page.locator(".viewer-title strong").textContent()) === name}`);
  // Cropping keeps the double click for itself.
  await page.keyboard.press("R");
  await page.mouse.dblclick(x, y);
  await page.waitForTimeout(300);
  console.log(`[mouse] double click while cropping stays in the viewer: ${(await page.locator(".viewer").count()) > 0}`);
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  await page.locator('[aria-label="更多"]').click();
  await page.getByRole("menuitem", { name: "帮助" }).click();
  const rows = await page.locator(".key-list > div").allTextContents();
  console.log(`[mouse] help lists: ${rows.filter((r) => /滚轮|双击/.test(r)).join(" | ")}`);
  await shot("help");
  await page.keyboard.press("Escape");

  // A setting saved by 0.4.2, where the wheel either zoomed or did not.
  await page.addInitScript(() => localStorage.setItem("sift.settings", JSON.stringify({ onboarded: true, wheelZoom: false })));
  await page.reload();
  await page.locator(".cell img").first().waitFor();
  console.log(`[mouse] "wheel zooms" switched off in 0.4.2 becomes: ${await settings()}`);
});

// Turning pages: holding a key, or spinning a wheel set to turn them, passes no photo unseen and never
// shows an empty pane, also when the pictures arrive slowly (SLOW=<ms> on each picture and its metadata).
await session("hold", { ...desktop, platform: "windows" }, async (page) => {
  await page.addInitScript(() => localStorage.setItem("sift.settings", JSON.stringify({ onboarded: true, wheel: "turn", autoAdvance: true })));
  const slow = Number(process.env.SLOW ?? 150);
  let heavy = 0;
  page.on("request", (r) => /\/img\/(full|dev|orig|look|clip|live)\b/.test(r.url()) && heavy++);
  const position = async () => Number((await page.locator(".viewer-title span").first().textContent()).split("/")[0]);
  // A reload forgets the pictures the page holds; the folder, and the photo that was open, come back by themselves.
  const open = async (first) => {
    await page.reload();
    await page.locator(".welcome button.primary, .cell img, .viewer").first().waitFor();
    await page.locator(".welcome button.primary").click({ timeout: 1000 }).catch(() => {});
    await page.locator(".cell img, .viewer").first().waitFor();
    if (first && (await page.locator(".viewer").count())) await page.keyboard.press("Escape");
    if (!(await page.locator(".viewer").count())) await page.locator(".cell").first().dblclick();
    await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
    const box = await page.locator(".pane").boundingBox();
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.waitForTimeout(600);
  };
  // What the pane shows, frame by frame: the photo the title names, whole, in its own shape.
  const watch = (pane = 0) =>
    page.evaluate((pane) => {
      const log = (window.__turns = { frames: [], on: true });
      const tick = () => {
        const at = Number(document.querySelector(".viewer-title span")?.textContent.split("/")[0]);
        const name = document.querySelector(".viewer-title strong")?.textContent ?? "";
        const img = document.querySelectorAll(".pane")[pane]?.querySelector(".canvas .tier:not(.full):not(.clip)");
        const full = img?.parentElement;
        const there = !!img && img.complete && img.naturalWidth > 0 && decodeURIComponent(img.getAttribute("src")).includes(name);
        const shaped = there && Math.abs(full.offsetWidth / full.offsetHeight / (img.naturalWidth / img.naturalHeight) - 1) < 0.02;
        log.frames.push([at, there, shaped, performance.now()]);
        if (log.on) requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    }, pane);
  const watched = async () => {
    // Let the last page asked for arrive.
    for (let last = -1, at = await position(); at !== last; last = at, at = await position()) await page.waitForTimeout(400 + slow * 2);
    const frames = await page.evaluate(() => ((window.__turns.on = false), window.__turns.frames));
    const passed = [...new Set(frames.map((f) => f[0]))];
    const seen = new Set(frames.filter((f) => f[1] && f[2]).map((f) => f[0]));
    const turning = frames.filter((f) => f[0] !== frames[0][0] && f[0] !== frames.at(-1)[0]);
    const pace = turning.length > 1 ? ((passed.length - 2) / (turning.at(-1)[3] - turning[0][3])) * 1000 : 0;
    const unseen = passed.filter((p) => !seen.has(p));
    const text = `${passed[0]} to ${passed.at(-1)}, ${passed.length} photos, ${unseen.length} never shown${unseen.length ? ` (${unseen.join(" ")})` : ""}, ${frames.filter((f) => !f[1]).length} of ${frames.length} frames without the picture, ${frames.filter((f) => f[1] && !f[2]).length} out of shape, ${pace.toFixed(0)} photos a second`;
    return { text, passed };
  };
  const hold = async (key, events) => {
    for (let i = 0; i < events; i++) {
      await page.keyboard.down(key);
      await page.waitForTimeout(33);
    }
    await page.keyboard.up(key);
  };
  const spin = async (notches, dy, gap) => {
    for (let i = 0; i < notches; i++) {
      await page.mouse.wheel(0, dy);
      await page.waitForTimeout(gap);
    }
  };
  const pass = async (label, run, pane) => {
    heavy = 0;
    await watch(pane);
    await run();
    console.log(`[hold] ${label}: ${(await watched()).text}; ${heavy} full-size pictures asked for on the way`);
  };

  await open(true);
  await pass("→ held, 24 repeats", () => hold("ArrowRight", 24));
  await pass("← held, 24 repeats", () => hold("ArrowLeft", 24));
  await pass("wheel, 12 notches", () => spin(12, 100, 40));

  // The same with every picture and its metadata arriving late.
  await page.route(/\/img\/(screen|thumb)|\/invoke\/read_meta/, async (route) => {
    await new Promise((r) => setTimeout(r, slow));
    await route.continue();
  });
  await open(true);
  await pass(`→ held, 30 repeats, pictures ${slow} ms late`, () => hold("ArrowRight", 30));
  await open();
  await pass(`← held, 12 repeats, pictures ${slow} ms late`, () => hold("ArrowLeft", 12));
  await open();
  await pass(`wheel, 8 notches, pictures ${slow} ms late`, () => spin(8, 100, 40));
  await pass(`wheel back, 5 notches a quarter second apart, pictures ${slow} ms late`, () => spin(5, -100, 250));
  // A single press on a photo that was rested on turns at once: its neighbours are fetched ahead.
  await page.waitForTimeout(800 + slow * 2);
  const before = await position();
  const took = await page.evaluate(
    () =>
      new Promise((done) => {
        const title = document.querySelector(".viewer-title span");
        const was = title.textContent;
        const start = performance.now();
        const look = () => (title.textContent !== was && document.querySelector(".pane .canvas .tier").complete ? done(Math.round(performance.now() - start)) : requestAnimationFrame(look));
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
        look();
      }),
  );
  console.log(`[hold] one press after a rest, pictures ${slow} ms late: ${before} -> ${await position()}, shown after ${took} ms`);

  // Going from stack to stack waits for its pictures the same way.
  await open(true);
  await watch();
  await hold("ArrowDown", 30);
  const stacks = await watched();
  console.log(`[hold] ↓ held, stack to stack, pictures ${slow} ms late: ${stacks.text}; stopped at ${stacks.passed.join(" ")}`);

  // Comparing, the pane in use is the one that waits; the other keeps its photo.
  await open(true);
  await page.keyboard.press("C");
  await page.keyboard.press("Tab");
  await page.waitForTimeout(600 + slow * 2);
  const names = () => page.locator(".slot-name span").allTextContents();
  const [left] = await names();
  await pass(`→ held in the second of two panes, pictures ${slow} ms late`, () => hold("ArrowRight", 12), 1);
  console.log(`[hold] the first pane kept its photo: ${(await names())[0] === left}`);
  await page.keyboard.press("Escape");

  // Marking with "go to the next photo" switched on turns the page at once: the next key is for the next photo.
  await page.waitForTimeout(600 + slow * 2);
  const marked = await position();
  for (let i = 0; i < 5; i++) {
    await page.keyboard.press("P");
    await page.waitForTimeout(50);
  }
  await page.waitForTimeout(300 + slow * 2);
  const moved = await position();
  const picks = [];
  for (let i = 0; i < 5; i++) {
    await page.keyboard.press("ArrowLeft");
    await page.waitForTimeout(250 + slow);
    picks.push(await page.locator(".dock .pick.on").count());
  }
  console.log(`[hold] P five times 50 ms apart, pictures ${slow} ms late: ${marked} -> ${moved}; picked, going back over them: ${picks.join(" ")} (want 1 1 1 1 1)`);

  // A picture that cannot be had holds nothing up; one that never arrives holds the page for two seconds, not for ever.
  const lost = { how: "", url: "" };
  await page.route(/\/img\/screen/, (route) => {
    const url = route.request().url();
    if (lost.how && !lost.url) lost.url = url;
    if (url !== lost.url) return route.fallback();
    if (lost.how === "broken") return route.abort();
  });
  const lostName = () => decodeURIComponent(lost.url).match(/[^/\\]+(?=&v=)/)?.[0];
  await page.waitForTimeout(600 + slow * 2);
  lost.how = "broken";
  await watch();
  await hold("ArrowRight", 12);
  const broken = await watched();
  console.log(`[hold] → held past a picture that cannot be read (${lostName()}): ${broken.text}`);
  await page.waitForTimeout(600 + slow * 2);
  Object.assign(lost, { how: "stalled", url: "" });
  const from = await position();
  let start = Date.now();
  await hold("ArrowRight", 12);
  await page.waitForFunction((name) => document.querySelector(".viewer-title strong").textContent === name, lostName(), { timeout: 10000 });
  const waited = Date.now() - start;
  const stuck = await position();
  start = Date.now();
  await page.keyboard.press("ArrowRight");
  await page.waitForFunction((was) => !document.querySelector(".viewer-title span").textContent.startsWith(`${was} `), stuck, { timeout: 10000 });
  console.log(`[hold] → held into a picture that never arrives (${lostName()}): ${from} to ${stuck}, turned to after ${waited} ms; pressed again there, the page turned after ${Date.now() - start} ms to ${await position()}`);
});
// Touch: pinch keeps the point between the fingers in place, never turns the page, and a flick glides.
await session("touch", phone, async (page, shot) => {
  await openLibrary(page);
  await page.locator(".cell").nth(1).tap();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  const cdp = await page.context().newCDPSession(page);
  const touch = (type, points) => cdp.send("Input.dispatchTouchEvent", { type, touchPoints: points.map(([x, y], id) => ({ x, y, id })) });
  const view = () =>
    page.evaluate(() => {
      const m = new DOMMatrix(getComputedStyle(document.querySelector(".canvas")).transform);
      return { scale: m.a, tx: m.e, ty: m.f };
    });
  const title = () => page.locator(".viewer-title b, .viewer-title strong").first().textContent();
  const box = await page.locator(".pane").boundingBox();
  const [cx, cy] = [box.x + box.width / 2, box.y + box.height / 2];
  const first = await title();
  const before = await view();
  // The image point under the midpoint of the fingers at the start.
  const anchor = [(cx - box.x - before.tx) / before.scale, (cy - box.y - before.ty) / before.scale];
  await touch("touchStart", [[cx - 30, cy], [cx + 30, cy]]);
  // Spread the fingers to 4x while dragging the pair 40 px right and 60 px down.
  for (let i = 1; i <= 12; i++) {
    const spread = 30 + (i / 12) * 90;
    const [mx, my] = [cx + (i / 12) * 40, cy + (i / 12) * 60];
    await touch("touchMove", [[mx - spread, my], [mx + spread, my]]);
    await page.waitForTimeout(16);
  }
  await page.waitForTimeout(80);
  const pinched = await view();
  const where = [anchor[0] * pinched.scale + pinched.tx + box.x, anchor[1] * pinched.scale + pinched.ty + box.y];
  // Sideways the photo can always follow; up and down it stays centred until it is taller than the pane.
  console.log(`[touch] pinch x${(pinched.scale / before.scale).toFixed(2)}; anchor is ${(where[0] - (cx + 40)).toFixed(1)} px sideways and ${(where[1] - (cy + 60)).toFixed(1)} px vertically from the fingers' midpoint`);
  // Lifting the fingers after a pinch neither turns the page nor changes the zoom.
  await touch("touchEnd", []);
  await page.waitForTimeout(400);
  console.log(`[touch] after the pinch the photo is still ${first}: ${(await title()) === first}, zoom kept: ${((await view()).scale / before.scale).toFixed(2)}x`);
  await shot("pinched");
  // A flick keeps moving after the finger lifts.
  const start = await view();
  await touch("touchStart", [[cx, cy]]);
  for (let i = 1; i <= 6; i++) {
    await touch("touchMove", [[cx - i * 14, cy]]);
    await page.waitForTimeout(10);
  }
  await touch("touchEnd", []);
  const released = await view();
  await page.waitForTimeout(700);
  const settled = await view();
  console.log(`[touch] flick: dragged ${(released.tx - start.tx).toFixed(0)} px, glided a further ${(settled.tx - released.tx).toFixed(0)} px`);
  // Double tap returns to fit; double tap again goes to the focus-check zoom at the tapped point.
  await page.touchscreen.tap(cx, cy);
  await page.touchscreen.tap(cx, cy);
  await page.waitForTimeout(400);
  const fitted = await view();
  await page.touchscreen.tap(cx + 60, cy - 80);
  await page.touchscreen.tap(cx + 60, cy - 80);
  await page.waitForTimeout(400);
  const close = await view();
  console.log(`[touch] double tap: fit x${(fitted.scale / before.scale).toFixed(2)}, then x${(close.scale / before.scale).toFixed(2)} (1 image px = ${(close.scale * 2).toFixed(2)} device px)`);
  // Fitted again, a sideways swipe turns the page and a vertical one switches the look.
  await page.touchscreen.tap(cx, cy);
  await page.touchscreen.tap(cx, cy);
  await page.waitForTimeout(400);
  await touch("touchStart", [[cx + 80, cy]]);
  for (let i = 1; i <= 5; i++) await touch("touchMove", [[cx + 80 - i * 30, cy]]);
  await touch("touchEnd", []);
  await page.waitForTimeout(500);
  console.log(`[touch] swipe while fitted turns the page: ${(await title()) !== first}`);
});

// Crop is confirmed explicitly; guides and looks cycle with key combinations.
await session("crop", desktop, async (page, shot) => {
  await openLibrary(page);
  await page.locator(".cell", { has: page.locator(".cell-raw") }).first().dblclick();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  await page.locator('.dock [aria-label="裁剪预览"]').click();
  await page.getByRole("menuitem", { name: "1:1" }).click();
  await shot("editing");
  await page.keyboard.press("Enter");
  await page.waitForTimeout(300);
  await shot("confirmed");
  for (let i = 0; i < 5; i++) await page.keyboard.press("Alt+D");
  await shot("golden-triangle");
  await page.keyboard.press("Alt+R");
  await shot("golden-triangle-rotated");
  await page.keyboard.press("Alt+A");
  await page.keyboard.press("Alt+A");
  await page.keyboard.press("Alt+R");
  await shot("spiral-rotated");
  await page.keyboard.press("Meta+D");
  await page.waitForTimeout(400);
  await page.locator(".tier.full.ready").waitFor({ timeout: 60000 });
  const look = await page.locator(".status-pill").textContent();
  console.log(`[crop] Cmd+D switched the look to: ${look}`);
  await shot("look-cycled");
  const keys = await page.evaluate(() => JSON.parse(localStorage.getItem("sift.settings")).keys);
  const used = Object.values(keys).filter(Boolean);
  console.log(`[crop] ${used.length} bindings, ${new Set(used).size} distinct`);
});

// The digit 0 clears a rating and nothing else; the letter O is unassigned;
// N shows the original. An old settings file with "original" on O is migrated.
await session("keys", desktop, async (page) => {
  await page.addInitScript(() => {
    if (sessionStorage.getItem("seeded")) return;
    sessionStorage.setItem("seeded", "1");
    localStorage.setItem("sift.settings", JSON.stringify({ onboarded: true, keysVersion: 2, keys: { original: "O", pick: "K" } }));
  });
  await page.reload();
  await openLibrary(page);
  await page.locator(".cell", { has: page.locator(".cell-raw") }).first().dblclick();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  const state = async () => ({
    stars: await page.locator(".dock .stars button.on").count(),
    look: (await page.locator(".status-pill").count()) ? await page.locator(".status-pill").textContent() : "",
    picked: await page.locator(".dock .pick.on").count(),
  });
  await page.keyboard.press("3");
  await page.waitForTimeout(300);
  const rated = await state();
  await page.keyboard.press("O");
  await page.waitForTimeout(300);
  const afterO = await state();
  await page.keyboard.press("0");
  await page.waitForTimeout(300);
  const afterZero = await state();
  await page.keyboard.press("N");
  await page.waitForTimeout(600);
  const afterN = await state();
  await page.keyboard.press("N");
  await page.keyboard.press("K");
  await page.waitForTimeout(300);
  const afterK = await state();
  await page.keyboard.press("K");
  const keys = await page.evaluate(() => JSON.parse(localStorage.getItem("sift.settings")).keys);
  console.log(`[keys] after 3: ${JSON.stringify(rated)}`);
  console.log(`[keys] after letter O: ${JSON.stringify(afterO)}`);
  console.log(`[keys] after digit 0: ${JSON.stringify(afterZero)}`);
  console.log(`[keys] after N: ${JSON.stringify(afterN)}`);
  console.log(`[keys] after K (customised pick, kept by migration): ${JSON.stringify(afterK)}`);
  console.log(`[keys] migrated: original=${keys.original} pick=${keys.pick} rate0=${keys.rate0}`);
});

// The ~ key shows the shortcut list on any screen and hides it again.
await session("help", desktop, async (page, shot) => {
  const open = async () => {
    await page.waitForTimeout(300);
    return page.locator(".help").count();
  };
  await page.locator(".welcome button.primary").waitFor();
  await page.keyboard.press("Backquote");
  const onWelcome = await open();
  await page.keyboard.press("Backquote");
  const closed = await open();
  await openLibrary(page);
  await page.locator(".cell").first().dblclick();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  await page.keyboard.press("Shift+Backquote");
  const inViewer = await open();
  await shot("in-viewer");
  await page.keyboard.press("Backquote");
  console.log(`[help] welcome: ${onWelcome}, toggled off: ${closed === 0}, viewer (with Shift): ${inViewer}, viewer still open: ${await page.locator(".viewer").count()}`);
});

// Progress survives a restart: flags, filter position and the open photo come back.
await session("resume", desktop, async (page, shot) => {
  await openLibrary(page);
  await page.locator(".cell").nth(2).dblclick();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  await page.keyboard.press("P");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("4");
  await page.waitForTimeout(600);
  await page.reload();
  await page.locator(".viewer .tier.full.ready").waitFor({ timeout: 20000 });
  await shot("after-restart");
  const title = await page.locator(".viewer-title strong").textContent();
  await page.keyboard.press("Escape");
  const picked = await page.locator(".cell-flag.pick").count();
  console.log(`[resume] reopened on ${title}, ${picked} pick restored`);
});

// Read-only walk through a real folder: never rates, so no file is modified.
await session("browse", desktop, async (page, shot) => {
  const t0 = Date.now();
  await page.locator(".welcome button.primary").click();
  await page.locator(".cell img").first().waitFor();
  const count = await page.locator(".toolbar-title span").textContent();
  await page.waitForFunction(() => [...document.querySelectorAll(".cell img")].every((i) => i.complete && i.naturalWidth > 0), null, { timeout: 60000 });
  console.log(`[browse] ${count}; first screen of thumbnails after ${Date.now() - t0} ms`);
  await shot("library");
  const total = parseInt(count, 10) || 0;
  const start = Math.min(7, Math.max(0, total - 2));
  await page.locator(".cell").nth(start).dblclick();
  await page.locator(".tier.full.ready").waitFor({ timeout: 30000 });
  await shot("viewer");
  const steps = [];
  // Stop at the last photo: small folders have fewer than 12 to step through.
  for (let i = 0; i < Math.min(12, total - start - 1); i++) {
    const before = await page.locator(".viewer-title strong").textContent();
    const t = Date.now();
    await page.keyboard.press("ArrowRight");
    await page.waitForFunction((b) => document.querySelector(".viewer-title strong")?.textContent !== b && document.querySelector(".tier.full.ready")?.naturalWidth > 0, before, { timeout: 30000 });
    steps.push(Date.now() - t);
  }
  console.log(`[browse] full-resolution ready after stepping: ${steps.join(", ")} ms`);
  await shot("viewer-later");
  await page.keyboard.press("Escape");
  await page.locator(".grid-scroller").evaluate((el) => (el.scrollTop = el.scrollHeight));
  await page.waitForFunction(() => [...document.querySelectorAll(".cell img")].every((i) => i.complete), null, { timeout: 60000 });
  await shot("library-end");
});

await session("light", { ...desktop, scheme: "light", lang: "en-US", platform: "windows" }, async (page, shot) => {
  await shot("welcome");
  await openLibrary(page);
  await page.locator(".cell").nth(2).click();
  await page.keyboard.press("5");
  await shot("library");
  await page.locator('[aria-label="More"]').click();
  await page.getByRole("menuitem", { name: "Find RAW" }).click();
  await shot("find-raw");
});


await session("phone", phone, async (page, shot) => {
  await shot("welcome");
  await openLibrary(page);
  await shot("library");
  await page.locator(".cell").first().tap();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  await shot("viewer");
  await page.locator('[aria-label="信息"]').tap();
  await shot("viewer-info");
  await page.locator('[aria-label="信息"]').tap();
  await page.locator('.viewer [aria-label="返回"]').tap();
  await page.locator('[aria-label="导出"]').tap();
  await page.locator(".frame-preview img").waitFor({ timeout: 20000 });
  await shot("export");
});

await session("tablet", { width: 1024, height: 768, mobile: true, platform: "ios", scheme: "light" }, async (page, shot) => {
  await openLibrary(page);
  await shot("library");
  await page.locator(".cell").nth(4).tap();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  await shot("viewer");
});

await session("compare", desktop, async (page, shot) => {
  await openLibrary(page);
  const names = () => page.locator(".slot-name span").allTextContents();
  const cellOf = (name) => page.locator(".cell", { has: page.locator(`img[src*="${encodeURIComponent(name)}"]`) }).first();
  // The first cell of a burst (BURST=<part of its file name>).
  await cellOf(process.env.BURST ?? "pied-kingfisher-0").dblclick();
  await page.locator(".tier.full.ready").first().waitFor({ timeout: 30000 });
  await page.keyboard.press("C");
  console.log(`[compare] C: ${JSON.stringify(await names())} (want the photo and the next)`);
  await page.locator(".slot-name").nth(1).click();
  const option = page.locator(".compare-picker button").nth(Number(process.env.PICK ?? 0));
  const picked = await option.getAttribute("title");
  await shot("picker");
  await option.click();
  console.log(`[compare] picker put ${picked} in pane 2: ${JSON.stringify(await names())}, picker open: ${await page.locator(".compare-picker").count()}`);
  await page.locator(".pane-count button", { hasText: "4" }).click();
  await page.locator(".slot").nth(3).locator(".tier.full.ready").waitFor({ timeout: 30000 });
  const before = await names();
  console.log(`[compare] 4 panes: ${JSON.stringify(before)}`);
  await shot("four");
  await page.keyboard.press("Z");
  await page.waitForTimeout(300);
  console.log(`[compare] Z with linked zoom: ${await page.locator(".pane.zoomed").count()} of 4 panes zoomed (want 4)`);
  await page.keyboard.press("Z");
  await page.waitForTimeout(300);
  await page.locator(".slot").nth(2).locator(".slot-step").nth(1).click();
  const after = await names();
  console.log(`[compare] next in pane 3 only: changed ${after.map((n, i) => n !== before[i]).join(",")} (want false,false,true,false)`);
  console.log(`[compare] active after its arrow: ${await page.locator(".slot-bar.active .slot-number").textContent()} (want 3)`);
  await page.keyboard.press("Tab");
  await page.keyboard.press("3");
  console.log(`[compare] Tab then 3: pane ${await page.locator(".slot-bar.active .slot-number").textContent()} has ${await page.locator(".slot-bar.active .slot-stars").textContent().catch(() => "none")} (want 4, ★3)`);
  await page.keyboard.press("ArrowRight");
  console.log(`[compare] → moves pane 4 only: ${JSON.stringify(await names())}`);
  await page.locator(".slot-bar").first().locator(".slot-number").click();
  const frames = page.locator(".stack-strip-frames button");
  if (await frames.count()) {
    console.log(`[compare] strip marks: ${JSON.stringify(await page.locator(".stack-strip-frames i.slot-mark").allTextContents())}`);
    const last = frames.last();
    const title = await last.getAttribute("title");
    await last.click();
    console.log(`[compare] last burst frame ${title} into pane 1: ${(await names())[0]}`);
    await shot("burst");
  } else console.log("[compare] no burst strip on pane 1");
  await page.keyboard.press("Shift+C");
  console.log(`[compare] Shift+C from 4: ${await page.locator(".slot").count()} panes (want 2)`);
  await page.keyboard.press("Shift+C");
  console.log(`[compare] Shift+C again: ${await page.locator(".slot").count()} panes (want 3)`);
  await page.locator(".slot").nth(2).locator(".slot-number").click();
  const third = (await names())[2];
  await page.keyboard.press("Escape");
  console.log(`[compare] Escape leaves on pane 3's photo: ${await page.locator(".viewer-title strong").textContent()} (want ${third}), panes ${await page.locator(".slot").count()}`);
  await page.keyboard.press("Escape");
  await page.locator(".viewer").waitFor({ state: "detached" });
  await page.locator(".cell").nth(1).click();
  await page.locator(".cell").nth(4).click({ modifiers: ["ControlOrMeta"] });
  await page.locator(".cell").nth(6).click({ modifiers: ["ControlOrMeta"] });
  await page.locator(".selection-bar .icon-button").last().click();
  await page.locator(".slot").nth(2).locator(".tier.full.ready").waitFor({ timeout: 30000 });
  console.log(`[compare] from the grid selection: ${JSON.stringify(await names())}`);
  await shot("from-grid");
});

await session("compare-phone", phone, async (page, shot) => {
  await openLibrary(page);
  await page.locator(".cell").nth(2).tap();
  await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
  await page.locator('.dock [aria-label="对比"]').tap();
  const boxes = await page.locator(".slot").evaluateAll((els) => els.map((e) => [Math.round(e.getBoundingClientRect().x), Math.round(e.getBoundingClientRect().y)]));
  console.log(`[compare-phone] pane positions: ${JSON.stringify(boxes)} (want one above the other)`);
  await shot("two");
  await page.locator(".pane-count button", { hasText: "4" }).tap();
  await shot("four");
});

// The updater exists in official builds only; the bridge shim plays its part
// (see src/dev/mock.ts). Offer, download, cancel, later, skip, as printed.
await session("update", { ...desktop, query: "&update=0.4.0&install=stall" }, async (page, shot) => {
  const dialog = page.locator('.sheet[aria-label="发现新版本"]');
  const settings = async () => {
    await page.locator('[aria-label="更多"]').click();
    await page.getByRole("menuitem", { name: "设置" }).click();
    await page.getByText("软件更新").scrollIntoViewIfNeeded();
  };
  const t0 = Date.now();
  await dialog.waitFor({ timeout: 10000 });
  console.log(`[update] offered ${((Date.now() - t0) / 1000).toFixed(1)} s after launch: ${(await dialog.locator(".update-versions").textContent()).trim()}`);
  await shot("offer");
  await dialog.getByRole("button", { name: "更新并重启" }).click();
  await dialog.locator(".progress small", { hasText: "74.8 / 187.0 MB" }).waitFor();
  await page.keyboard.press("Escape");
  console.log(`[update] downloading: close button ${await dialog.locator(".sheet-close").count()} (want 0), still open after Esc: ${await dialog.count()}`);
  await shot("downloading");
  await dialog.getByRole("button", { name: "取消" }).click();
  await dialog.getByRole("button", { name: "更新并重启" }).waitFor();
  await dialog.getByRole("button", { name: "以后再说" }).click();
  console.log(`[update] cancelled, then later: dialog ${await dialog.count()} (want 0)`);
  await openLibrary(page);
  await settings();
  await shot("settings-available");
  await page.locator(".sheet .row", { hasText: "有新版本 0.4.0" }).getByRole("button", { name: "查看" }).click();
  await dialog.getByRole("button", { name: "跳过这个版本" }).click();
  // Every load starts from fresh settings (see `session`), so the saved ones are put back.
  const saved = await page.evaluate(() => localStorage.getItem("sift.settings"));
  const skipped = JSON.parse(saved).skippedUpdate;
  await page.addInitScript((settings) => localStorage.setItem("sift.settings", settings), saved);
  await page.reload();
  await page.locator(".cell img").first().waitFor();
  await page.waitForTimeout(5000);
  console.log(`[update] skipped ${skipped}; dialog after the next launch: ${await dialog.count()} (want 0)`);
  await settings();
  await page.locator(".sheet .row", { hasText: "有新版本 0.4.0" }).getByRole("button", { name: "查看" }).click();
  console.log(`[update] a skipped release can still be opened from Settings: ${await dialog.count()}`);
});

await session("update-install", { ...desktop, query: "&update=0.4.0" }, async (page, shot) => {
  const dialog = page.locator('.sheet[aria-label="发现新版本"]');
  await dialog.waitFor({ timeout: 10000 });
  await dialog.getByRole("button", { name: "更新并重启" }).click();
  await dialog.locator(".progress small", { hasText: "正在安装" }).waitFor();
  console.log(`[update-install] installing: buttons left ${await dialog.locator("footer button").count()} (want 0)`);
  await shot("installing");
});

await session("update-en", { ...desktop, scheme: "light", lang: "en-US", platform: "windows", query: "&update=0.4.0&install=fail" }, async (page, shot) => {
  const dialog = page.locator('.sheet[aria-label="Update available"]');
  await dialog.waitFor({ timeout: 10000 });
  await shot("offer");
  await dialog.getByRole("button", { name: "Update and restart" }).click();
  await dialog.locator(".note.bad").waitFor();
  console.log(`[update-en] failed: ${(await dialog.locator(".note.bad").textContent()).slice(0, 90)}`);
  await shot("failed");
  await dialog.getByRole("button", { name: "Close" }).last().click();
  await openLibrary(page);
  await page.locator('[aria-label="More"]').click();
  await page.getByRole("menuitem", { name: "Settings" }).click();
  await page.getByText("Updates", { exact: true }).scrollIntoViewIfNeeded();
  await shot("settings");
});

// Nothing newer, a failing check, a build that does not update itself, a phone.
for (const [name, query, view] of [
  ["update-none", "&update=none", desktop],
  ["update-fail", "&update=fail", desktop],
  ["update-off", "", desktop],
  ["update-phone", "&update=0.4.0", phone],
]) {
  await session(name, { ...view, query }, async (page, shot) => {
    await openLibrary(page);
    await page.waitForTimeout(4500);
    const prompted = await page.locator(".sheet").count();
    await page.locator('[aria-label="更多"]').click();
    await page.getByRole("menuitem", { name: "设置" }).click();
    const section = page.locator(".sheet h4", { hasText: "软件更新" });
    const shown = await section.count();
    let result = "";
    if (shown) {
      await section.scrollIntoViewIfNeeded();
      await page.getByRole("button", { name: "检查更新" }).click();
      await page.waitForTimeout(900);
      result = (await page.locator(".sheet .row", { hasText: "Sift " }).first().locator("small").textContent()) ?? "";
      await shot("settings");
    }
    console.log(`[${name}] prompt at launch: ${prompted}, section in Settings: ${shown}, check now: ${result || "-"}`);
  });
}
// Photos in folders inside the opened one, and a folder whose photos come and
// go while it is open. The session builds its own folder from the first JPEGs
// of the one given:
//   <tmp>/day 1/{a,b,c}   <tmp>/day 2/{d,e, a again}   <tmp>/day 2/more/{f}
if (!only || only.includes("folders")) {
  const jpegs = readdirSync(folder).filter((f) => /\.jpe?g$/i.test(f)).sort();
  if (jpegs.length < 10) console.log("[folders] skipped: needs a folder with 10 JPEGs");
  else {
    const root = join(tmpdir(), `sift-folders-${process.pid}`);
    rmSync(root, { recursive: true, force: true });
    const put = (n, to) => (mkdirSync(dirname(join(root, to)), { recursive: true }), copyFileSync(join(folder, jpegs[n]), join(root, to)));
    [[0, "day 1/a.jpg"], [1, "day 1/b.jpg"], [2, "day 1/c.jpg"], [3, "day 2/d.jpg"], [4, "day 2/e.jpg"], [6, "day 2/a.jpg"], [5, "day 2/more/f.jpg"]].forEach(([n, to]) => put(n, to));
    await session("folders", { ...desktop, pick: root }, async (page, shot) => {
      // Photos, not cells: frames shot in one burst fold into one cell.
      const cells = async () => Number((await page.locator(".toolbar-title span").textContent()).match(/(\d+) 张/)?.[1] ?? -1);
      const until = async (want) => {
        const t0 = Date.now();
        while ((await cells()) !== want && Date.now() - t0 < 9000) await page.waitForTimeout(200);
        return `${await cells()} after ${((Date.now() - t0) / 1000).toFixed(1)} s`;
      };
      const toast = async () => ((await page.locator(".toast").count()) ? await page.locator(".toast").textContent() : "(none)");
      const subtitle = () => page.locator(".toolbar-title span").textContent();
      await openLibrary(page);
      console.log(`[folders] a folder with no photos of its own: ${await cells()} photos (want 7), "${await toast()}"`);
      await page.locator(".cell").nth(0).click();
      await page.keyboard.press("P");
      console.log(`[folders] picking day 1/a.jpg leaves day 2/a.jpg alone: ${await page.locator(".cell-flag.pick").count()} picked (want 1)`);
      await shot("all");
      await page.locator('[aria-label="只看其中一个文件夹"]').click();
      console.log(`[folders] folders offered: ${JSON.stringify(await page.getByRole("menuitem").allTextContents())}`);
      await page.getByRole("menuitem", { name: "day 2 · 3" }).click();
      console.log(`[folders] day 2 alone: ${await cells()} photos (want 3), "${await subtitle()}"`);
      await shot("one");
      await page.locator('[aria-label="只看其中一个文件夹"]').click();
      await page.getByRole("menuitem", { name: /^全部文件夹/ }).click();

      put(7, "day 1/new.jpg");
      console.log(`[folders] a photo copied in: ${await until(8)} (want 8, on its own), toast "${await toast()}"`);
      rmSync(join(root, "day 2/d.jpg"));
      console.log(`[folders] a photo deleted: ${await until(7)} (want 7), toast "${await toast()}"`);

      // The photo on screen goes away.
      await page.locator(".cell").last().dblclick();
      await page.locator(".tier.full.ready").waitFor({ timeout: 20000 });
      const title = () => page.locator(".viewer-title strong").textContent();
      const first = `${await title()} ${await page.locator(".viewer-title span").textContent()}`;
      rmSync(join(root, "day 2/more/f.jpg"));
      const t0 = Date.now();
      while ((await title()) === "f.jpg" && Date.now() - t0 < 9000) await page.waitForTimeout(200);
      console.log(`[folders] viewer on "${first}", file deleted: now "${await title()}" "${await page.locator(".viewer-title span").textContent()}", toast "${await toast()}"`);
      await shot("viewer-moved-on");
      await page.keyboard.press("Escape");
      console.log(`[folders] back in the grid: ${await cells()} photos (want 6), ${await page.locator(".cell.selected").count()} selected (want 1)`);

      await page.keyboard.press("Meta+R");
      await page.locator(".toast", { hasText: "已刷新" }).waitFor({ timeout: 5000 }).catch(() => {});
      console.log(`[folders] refresh with nothing new: "${await toast()}"`);
      await page.waitForTimeout(300);
      put(8, "day 2/late.jpg");
      rmSync(join(root, "day 1/b.jpg"));
      await page.keyboard.press("Meta+R");
      await page.locator(".toast", { hasText: "新增" }).waitFor({ timeout: 5000 }).catch(() => {});
      console.log(`[folders] refresh after one came and one went: "${await toast()}", ${await cells()} photos (want 6)`);

      // A photo in the folder itself: next time only that one shows, until subfolders are asked for.
      put(9, "top.jpg");
      console.log(`[folders] a photo in the folder itself: ${await until(7)} (want 7)`);
      await page.waitForTimeout(600);
      await page.reload();
      await page.locator(".cell img").first().waitFor();
      console.log(`[folders] reopened: ${await cells()} photos (want 1), folder button: ${await page.locator('[aria-label="只看其中一个文件夹"]').count()} (want 0)`);
      await page.locator('[aria-label="更多"]').click();
      await page.getByRole("menuitem", { name: "包含子文件夹里的照片" }).click();
      console.log(`[folders] subfolders switched on: ${await until(7)} (want 7), picked kept: ${await page.locator(".cell-flag.pick").count()} (want 1)`);
      await page.locator('[aria-label="只看其中一个文件夹"]').click();
      console.log(`[folders] folders offered: ${JSON.stringify(await page.getByRole("menuitem").allTextContents())}`);
      await shot("with-subfolders");
      await page.keyboard.press("Escape");
      await page.locator('[aria-label="更多"]').click();
      await page.getByRole("menuitem", { name: "设置" }).click();
      await shot("settings");
    });
    rmSync(root, { recursive: true, force: true });
  }
}
// Tags: making them while culling, several on one photo, narrowing the grid to
// some, and an export with a folder for each. The session builds its own
// folder from the first JPEGs and the first RAW of the one given, and exports
// next to it.
if (!only || only.includes("tags") || only.includes("tags-phone")) {
  const files = readdirSync(folder).sort();
  const jpegs = files.filter((f) => /\.jpe?g$/i.test(f));
  const raw = files.find((f) => /\.(nef|arw|cr2|cr3|raf|dng)$/i.test(f));
  if (jpegs.length < 8) console.log("[tags] skipped: needs a folder with 8 JPEGs");
  else {
    const root = join(tmpdir(), `sift-tags-${process.pid}`);
    const dest = `${root}-out`;
    rmSync(root, { recursive: true, force: true });
    rmSync(dest, { recursive: true, force: true });
    mkdirSync(root, { recursive: true });
    "abcdefgh".split("").forEach((name, n) => copyFileSync(join(folder, jpegs[n]), join(root, `${name}.jpg`)));
    if (raw) copyFileSync(join(folder, raw), join(root, `i.${raw.split(".").pop()}`));
    const total = raw ? 9 : 8;
    const tree = (dir) =>
      Object.fromEntries(
        readdirSync(dir, { withFileTypes: true })
          .filter((e) => e.isDirectory())
          .map((e) => [e.name, readdirSync(join(dir, e.name)).sort()]),
      );
    await session("tags", { ...desktop, pick: root }, async (page, shot) => {
      const cell = (i) => page.locator(".cell").nth(i);
      const cells = () => page.locator(".cell").count();
      const bar = () => page.locator(".tag-bar button:not(.clear)").allTextContents();
      const on = (i) => cell(i).locator(".cell-tags").allTextContents();
      const toast = async () => ((await page.locator(".toast").count()) ? await page.locator(".toast").textContent() : "(none)");
      const add = async (name) => {
        await page.locator(".tag-new input").fill(name);
        await page.locator(".tag-new input").press("Enter");
      };
      await openLibrary(page);
      console.log(`[tags] photos: ${await cells()} (want ${total}), tag bar before any tag: ${await page.locator(".tag-bar").count()} (want 0)`);

      // T opens the menu on the photo the keys act on; a name and Enter makes a tag and puts it on.
      await cell(0).click();
      await page.keyboard.press("T");
      await page.locator(".tag-menu").waitFor();
      await shot("menu-empty");
      console.log(`[tags] T opens the menu, typing goes to the name: ${await page.evaluate(() => document.activeElement?.closest(".tag-new") != null)}`);
      await add("小明");
      await add("合影");
      console.log(`[tags] two tags made on the first photo: bar ${JSON.stringify(await bar())}, cell ${JSON.stringify(await on(0))}`);
      await shot("menu-two");
      await page.keyboard.press("Escape");
      console.log(`[tags] Escape closes the menu only: menu ${await page.locator(".tag-menu").count()} (want 0), cells ${await cells()}`);

      await cell(1).click();
      await page.keyboard.press("Shift+1");
      const put = await toast();
      await page.keyboard.press("Shift+1");
      const off = await toast();
      await page.keyboard.press("Shift+1");
      console.log(`[tags] Shift+1 on the second photo: "${put}", again: "${off}", a third time: ${JSON.stringify(await on(1))}`);

      // Several photos at once, from the bar that comes up for them.
      await cell(2).click();
      await cell(3).click({ modifiers: ["ControlOrMeta"] });
      await page.locator(".selection-bar [aria-label='给所选照片打标签']").click();
      await page.locator(".selection-bar .tag-menu").getByRole("menuitemcheckbox", { name: "合影" }).click();
      console.log(`[tags] 合影 on two selected photos: ${JSON.stringify([await on(2), await on(3)])}, bar ${JSON.stringify(await bar())}`);
      await shot("selection-bar");
      await page.keyboard.press("Escape");
      // A key for a tag that does not exist yet opens the menu to make it.
      await page.keyboard.press("Shift+3");
      console.log(`[tags] Shift+3 with two tags opens the menu: ${await page.locator(".selection-bar .tag-menu").count()} (want 1)`);
      await add("客户");
      await page.keyboard.press("Escape");
      await page.keyboard.press("Escape");
      console.log(`[tags] three tags: bar ${JSON.stringify(await bar())}, third photo ${JSON.stringify(await on(2))}`);
      await shot("grid");

      // Narrowing the grid.
      await page.locator(".tag-bar button", { hasText: "小明" }).click();
      const one = await cells();
      await page.locator(".tag-bar button", { hasText: "客户" }).click();
      const two = await cells();
      await shot("filtered");
      await page.locator(".tag-bar .clear").click();
      console.log(`[tags] only 小明: ${one} (want 2), 小明 or 客户: ${two} (want 4), all again: ${await cells()} (want ${total})`);

      // In the viewer a tag goes on the photo on screen.
      await cell(4).dblclick();
      await page.locator(".viewer").waitFor();
      await page.keyboard.press("Shift+2");
      await page.locator(".dock [aria-label='标签']").click();
      await page.locator(".dock .tag-menu").getByRole("menuitemcheckbox", { name: "小明" }).click();
      await shot("viewer-menu");
      console.log(`[tags] viewer, Shift+2 then 小明 from the dock: title ${JSON.stringify(await page.locator(".viewer-title em.tag").allTextContents())}, info ${JSON.stringify(await page.locator(".info-tags").allTextContents())}`);
      await page.keyboard.press("Escape");
      console.log(`[tags] Escape closes the menu, not the viewer: ${await page.locator(".viewer").count()} (want 1)`);
      await page.keyboard.press("Escape");
      await page.locator(".viewer").waitFor({ state: "detached" });

      // A crop on the first photo (two tags), so the export has something to render.
      await cell(0).dblclick();
      await page.locator(".viewer").waitFor();
      await page.locator(".dock [aria-label='裁剪预览']").click();
      await page.getByRole("menuitem", { name: "1:1" }).click();
      await page.keyboard.press("Enter");
      await page.keyboard.press("Escape");
      await page.locator(".viewer").waitFor({ state: "detached" });

      // Tags are part of the folder's progress.
      await page.waitForTimeout(600);
      const kept = await page.evaluate(() => localStorage.getItem("sift.settings"));
      await page.addInitScript((settings) => localStorage.setItem("sift.settings", settings), kept);
      await page.reload();
      await page.locator(".cell img").first().waitFor();
      console.log(`[tags] reopened: bar ${JSON.stringify(await bar())}, first photo ${JSON.stringify(await on(0))}`);

      // Renaming, recolouring and deleting.
      await page.locator(".toolbar [aria-label='给所选照片打标签']").click();
      await page.getByRole("button", { name: "修改标签" }).click();
      const names = page.locator(".tag-edit input");
      await names.nth(2).fill("小明");
      await names.nth(2).blur();
      console.log(`[tags] renaming 客户 to a name in use: "${await toast()}", still ${JSON.stringify(await names.nth(2).inputValue())}`);
      await names.nth(2).fill("Client: A");
      await names.nth(2).blur();
      await page.locator(".tag-edit .tag-color").nth(2).click();
      await shot("edit");
      console.log(`[tags] renamed: bar ${JSON.stringify(await bar())}, third photo ${JSON.stringify(await on(2))}`);

      // Export sorted by tag, renamed {tag}_{seq}.
      await page.keyboard.press("Escape");
      await page.evaluate((dir) => localStorage.setItem("sift.exportDest", dir), dest);
      await page.locator(".toolbar [aria-label='导出']").click();
      await page.getByRole("switch").first().waitFor();
      await page.locator(".row", { hasText: "按标签分文件夹" }).getByRole("switch").click();
      await page.locator(".row-label small", { hasText: "示例" }).waitFor({ timeout: 5000 }).catch(() => {});
      console.log(`[tags] export sheet: folders ${JSON.stringify(await page.locator(".tag-folders button").allTextContents())}, ${JSON.stringify(await page.locator(".row", { hasText: "重命名" }).locator("small").allTextContents())}, notes ${JSON.stringify(await page.locator(".sheet .note").allTextContents())}`);
      await shot("export");
      await page.getByRole("button", { name: "开始" }).click();
      await page.locator(".report").waitFor({ timeout: 60000 });
      console.log(`[tags] exported: "${await page.locator(".report strong").textContent()}" ${JSON.stringify(tree(dest))}`);
      const same = (x, y) => readFileSync(x).equals(readFileSync(y));
      console.log(`[tags] the cropped photo is one rendering in both folders: ${same(join(dest, "小明", "小明_001.jpg"), join(dest, "合影", "合影_001.jpg"))}, and not the file itself: ${!same(join(dest, "小明", "小明_001.jpg"), join(root, "a.jpg"))}; an untouched photo is a plain copy: ${same(join(dest, "小明", "小明_002.jpg"), join(root, "b.jpg"))}`);
      await page.getByRole("button", { name: "完成" }).click();

      // Again without the photos that have no tag, names kept, and one tag left out.
      rmSync(dest, { recursive: true, force: true });
      await page.locator(".toolbar [aria-label='导出']").click();
      await page.locator(".row", { hasText: "按标签分文件夹" }).getByRole("switch").click();
      await page.locator(".tag-folders button", { hasText: "未分类" }).click();
      await page.locator(".tag-folders button", { hasText: "合影" }).click();
      await page.locator(".row", { hasText: "重命名" }).getByRole("switch").click();
      console.log(`[tags] 未分类 and 合影 switched off: ${JSON.stringify(await page.locator(".tag-folders button[aria-pressed=true]").allTextContents())}, notes ${JSON.stringify(await page.locator(".sheet .note").allTextContents())}`);
      await page.getByRole("button", { name: "开始" }).click();
      await page.locator(".report").waitFor({ timeout: 60000 });
      console.log(`[tags] no untagged, 合影 left out, names kept: "${await page.locator(".report strong").textContent()}" ${JSON.stringify(tree(dest))}`);
      await page.getByRole("button", { name: "完成" }).click();

      // Framed, sorted by tag.
      rmSync(dest, { recursive: true, force: true });
      await page.locator(".toolbar [aria-label='导出']").click();
      await page.getByRole("tab", { name: "加参数边框" }).click();
      await page.locator(".row", { hasText: "按标签分文件夹" }).getByRole("switch").click();
      await page.getByRole("button", { name: "开始" }).click();
      await page.locator(".report").waitFor({ timeout: 120000 });
      console.log(`[tags] framed: "${await page.locator(".report strong").textContent()}" ${JSON.stringify(tree(dest))}`);
      console.log(`[tags] framed once, copied to the other folder: ${same(join(dest, "小明", "小明_001.jpg"), join(dest, "合影", "合影_001.jpg"))}`);
      await page.getByRole("button", { name: "完成" }).click();

      // Deleting a tag takes it off its photos.
      await page.locator(".toolbar [aria-label='给所选照片打标签']").click();
      await page.getByRole("button", { name: "修改标签" }).click();
      await page.locator(".tag-edit .tag-remove").nth(2).click();
      const asked = await page.locator(".tag-edit .tag-remove").nth(2).textContent();
      await page.locator(".tag-edit .tag-remove").nth(2).click();
      console.log(`[tags] delete asks first ("${asked}"), then: bar ${JSON.stringify(await bar())}, third photo ${JSON.stringify(await on(2))}`);
      await page.keyboard.press("Escape");

      // Another folder is offered the names used before.
      await page.locator(".toolbar [aria-label='给所选照片打标签']").click();
      console.log(`[tags] names remembered for other folders, minus the ones this folder has: ${JSON.stringify(await page.locator(".tag-suggest button").allTextContents())}`);
      await page.keyboard.press("Escape");
      await page.locator('[aria-label="更多"]').click();
      await page.getByRole("menuitem", { name: "设置" }).click();
      await page.locator(".group.keys .row", { hasText: "打上 / 去掉标签 1" }).scrollIntoViewIfNeeded();
      await shot("settings-keys");
    });
    await session("tags-phone", { ...phone, pick: root }, async (page, shot) => {
      await page.locator(".welcome button.primary").click().catch(() => {});
      await page.locator(".cell img").first().waitFor();
      await page.locator(".cell").nth(0).tap();
      await page.locator(".viewer").waitFor();
      await page.locator(".dock [aria-label='标签']").tap();
      await shot("viewer-menu-empty");
      await page.locator(".tag-new input").fill("小明");
      await page.locator(".tag-new input").press("Enter");
      await page.locator(".tag-new input").fill("很长很长的一个标签名字用来看看放不放得下");
      await page.locator(".tag-new input").press("Enter");
      await shot("viewer-menu");
      const menu = await page.locator(".tag-menu").boundingBox();
      console.log(`[tags-phone] menu inside the screen: ${menu.x >= 0 && menu.x + menu.width <= 390} (${Math.round(menu.x)}..${Math.round(menu.x + menu.width)} of 390), title ${JSON.stringify(await page.locator(".viewer-title em.tag").allTextContents())}`);
      await page.locator(".viewer-title").tap();
      await page.locator(".viewer-bar [aria-label='返回']").tap();
      await page.locator(".viewer").waitFor({ state: "detached" });
      await shot("grid");
      await page.locator('[aria-label="更多"]').tap();
      await page.getByRole("menuitem", { name: "选择照片" }).tap();
      await page.locator(".cell").nth(1).tap();
      await page.locator(".cell").nth(2).tap();
      await page.locator(".selection-bar [aria-label='给所选照片打标签']").tap();
      await shot("selection-menu");
      const bar = await page.locator(".selection-bar").boundingBox();
      const pop = await page.locator(".selection-bar .tag-menu").boundingBox();
      const reach = await page.evaluate(() => Math.max(...[...document.querySelector(".selection-bar").children].map((el) => el.getBoundingClientRect().right)));
      console.log(`[tags-phone] selection bar ${Math.round(bar.x)}..${Math.round(bar.x + bar.width)}, its buttons end at ${Math.round(reach)}, its menu ${Math.round(pop.x)}..${Math.round(pop.x + pop.width)} of 390`);
      await page.locator(".selection-bar .tag-menu").getByRole("menuitemcheckbox", { name: "小明" }).tap();
      console.log(`[tags-phone] tagged from the bar: ${JSON.stringify(await page.locator(".tag-bar button:not(.clear)").allTextContents())}`);
    });
    rmSync(root, { recursive: true, force: true });
    rmSync(dest, { recursive: true, force: true });
  }
}
await browser.close();
console.log(`screenshots in ${out}`);
