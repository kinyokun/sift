// Drives the web UI in headless Chromium against the dev bridge and saves
// screenshots of every main screen. Used to review layout on desktop and
// phone-sized viewports.
//
//   cargo run -p sift --example bridge &
//   pnpm dev &
//   node scripts/shots.mjs /path/to/photos [outDir]
import { chromium } from "playwright-core";
import { mkdirSync } from "node:fs";

const folder = process.argv[2];
const out = process.argv[3] ?? "/tmp/sift-shots";
if (!folder) throw new Error("usage: node scripts/shots.mjs <photo folder> [outDir]");
mkdirSync(out, { recursive: true });

const only = process.env.ONLY?.split(",");
const lookFiles = (process.env.LOOKS ?? "").split(",").filter(Boolean);
const base = `http://localhost:1420/?bridge=http://127.0.0.1:4777&pick=${encodeURIComponent(folder)}${lookFiles.map((l) => `&look=${encodeURIComponent(l)}`).join("")}`;
const bridge = "http://127.0.0.1:4777";
const reset = () => fetch(`${bridge}/invoke/dev_reset`, { method: "POST", body: "{}" });
const browser = await chromium.launch({ channel: process.env.CHANNEL ?? "chrome" });

async function session(name, { width, height, mobile = false, scheme = "dark", lang = "zh-CN", platform = "macos" }, steps) {
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
  await page.goto(`${base}&platform=${platform}${mobile ? "&mobile" : ""}`);
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

await browser.close();
console.log(`screenshots in ${out}`);
