// Captures the screenshots and screen recordings used on the README from a folder of
// public sample photos, in headless Chrome against the dev bridge (see AGENTS.md).
//
//   cargo run -p sift --example bridge &
//   pnpm dev &
//   LOOKS=a.cube,b.cube node scripts/readme-shots.mjs <photo folder> [outDir]
//
// LANG_UI=en switches the UI to English; ONLY=library,viewer picks sessions.
import { chromium } from "playwright-core";
import { mkdirSync, writeFileSync } from "node:fs";

const folder = process.argv[2];
const lang = process.env.LANG_UI ?? "zh";
const out = `${process.argv[3] ?? "/tmp/sift-readme"}/${lang}`;
if (!folder) throw new Error("usage: node scripts/readme-shots.mjs <photo folder> [outDir]");
mkdirSync(out, { recursive: true });
const only = process.env.ONLY?.split(",");
const lookFiles = (process.env.LOOKS ?? "").split(",").filter(Boolean);
const bridge = "http://127.0.0.1:4777";
const base = `http://localhost:1420/?bridge=${bridge}&pick=${encodeURIComponent(folder)}${lookFiles.map((l) => `&look=${encodeURIComponent(l)}`).join("")}`;
const T = {
  zh: { looks: "滤镜", importLooks: "导入滤镜…", export: "导出", frame: "加参数边框", livePhoto: "实况照片", locale: "zh-CN" },
  en: { looks: "Looks", importLooks: "Import looks…", export: "Export", frame: "Add info frame", livePhoto: "Live Photo", locale: "en-US" },
}[lang];

const browser = await chromium.launch({ channel: "chrome" });
const desktop = { width: 1440, height: 900 };
const phone = { width: 390, height: 844, mobile: true, platform: "ios" };

async function session(name, { width, height, mobile = false, scheme = "dark", platform = "macos", scale = 2 }, steps) {
  if (only && !only.includes(name)) return;
  await new Promise((r) => setTimeout(r, 500));
  await fetch(`${bridge}/invoke/dev_reset`, { method: "POST", body: "{}" });
  const context = await browser.newContext({ viewport: { width, height }, deviceScaleFactor: scale, colorScheme: scheme, locale: T.locale, hasTouch: mobile, isMobile: mobile });
  const page = await context.newPage();
  page.on("pageerror", (e) => console.log(`[${name}] page error: ${e.message}`));
  page.on("response", async (r) => r.status() >= 400 && console.log(`[${name}] ${r.status()} ${decodeURIComponent(r.url()).slice(0, 200)} ${(await r.text().catch(() => "")).slice(0, 200)}`));
  await page.addInitScript((settings) => localStorage.setItem("sift.settings", settings), JSON.stringify({ onboarded: true }));
  await page.goto(`${base}&platform=${platform}${mobile ? "&mobile" : ""}`);
  const shot = async (label) => {
    await page.waitForTimeout(400);
    await page.screenshot({ path: `${out}/${name}-${label}.png` });
    console.log(`[${name}] ${label}`);
  };
  try {
    await steps(page, shot);
  } catch (e) {
    console.log(`[${name}] FAILED: ${e.message.split("\n")[0]}`);
    await page.screenshot({ path: `${out}/${name}-failure.png` });
  }
  await context.close();
}

// Records the page as JPEG frames with their timestamps (Chrome's screencast), for
// turning into an animation afterwards.
async function record(page, dir) {
  mkdirSync(dir, { recursive: true });
  const cdp = await page.context().newCDPSession(page);
  const frames = [];
  cdp.on("Page.screencastFrame", async ({ data, metadata, sessionId }) => {
    const file = `${dir}/${String(frames.length).padStart(4, "0")}.jpg`;
    writeFileSync(file, Buffer.from(data, "base64"));
    frames.push({ file, t: metadata.timestamp });
    await cdp.send("Page.screencastFrameAck", { sessionId }).catch(() => {});
  });
  const size = page.viewportSize();
  await cdp.send("Page.startScreencast", { format: "jpeg", quality: 92, maxWidth: size.width * 2, maxHeight: size.height * 2, everyNthFrame: 1 });
  return async () => {
    await cdp.send("Page.stopScreencast");
    writeFileSync(`${dir}/frames.json`, JSON.stringify(frames));
    console.log(`recorded ${frames.length} frames in ${dir}`);
  };
}

const openLibrary = async (page) => {
  await page.locator(".welcome button.primary").click();
  await page.locator(".cell img").first().waitFor();
  await page.waitForLoadState("networkidle");
  await page.waitForFunction(() => [...document.querySelectorAll(".cell img")].every((i) => i.complete), null, { timeout: 60000 });
};
const cell = (page, name) => page.locator(".cell", { has: page.locator(`img[src*="${encodeURIComponent(name)}"]`) }).first();
const ready = (page) => page.locator(".tier.full.ready").first().waitFor({ timeout: 60000 });
const open = async (page, name) => {
  await cell(page, name).dblclick();
  await ready(page);
  await page.waitForTimeout(600);
};
const rate = async (page) => {
  for (const [name, keys] of [["cat-portrait", ["5", "P"]], ["kingfisher-on-a-rock", ["4", "P"]], ["cross-fox", ["4", "P"]], ["boxer", ["X"]], ["horse.jpg", ["3"]], ["squirrel.jpg", ["2"]], ["DSC0018", ["5", "P"]]]) {
    await cell(page, name).click();
    for (const k of keys) await page.keyboard.press(k);
  }
};
const importLooks = async (page) => {
  await page.locator(`.dock [aria-label="${T.looks}"]`).click();
  await page.getByRole("menuitem", { name: T.importLooks }).click();
  await page.waitForTimeout(800);
};
const pickLook = async (page, name) => {
  await page.locator(`.dock [aria-label="${T.looks}"]`).click();
  await page.getByRole("menuitem", { name }).click();
  await page.waitForTimeout(500);
  await ready(page);
  await page.waitForTimeout(800);
};

await session("library", desktop, async (page, shot) => {
  await openLibrary(page);
  await rate(page);
  await cell(page, "cat-portrait").click();
  await page.mouse.move(5, 890);
  await shot("grid");
});

await session("viewer", desktop, async (page, shot) => {
  await openLibrary(page);
  await open(page, "kingfisher-on-a-rock");
  await shot("photo");
  await page.keyboard.press("Escape");
  await open(page, "DSC0070");
  await shot("af");
  await page.keyboard.press("Z");
  await page.waitForTimeout(1500);
  await shot("af-100");
});

await session("eyes", desktop, async (page, shot) => {
  await openLibrary(page);
  for (const name of ["kingfisher-on-a-rock", "cat-portrait", "cross-fox"]) {
    await open(page, name);
    await shot(`${name}-fit`);
    await page.keyboard.press("E");
    await page.waitForTimeout(2200);
    await shot(`${name}-eye`);
    await page.keyboard.press("Escape");
    await page.keyboard.press("Escape");
    await page.locator(".viewer").waitFor({ state: "detached" }).catch(() => {});
  }
});

await session("looks", desktop, async (page, shot) => {
  if (!lookFiles.length) return console.log("[looks] skipped: set LOOKS");
  await openLibrary(page);
  await open(page, "R5A_2034");
  await importLooks(page);
  await pickLook(page, "Teal Orange");
  await shot("applied");
  await page.keyboard.press("W");
  await page.locator(".look-wall").waitFor();
  await page.waitForFunction(() => document.querySelectorAll(".look-tile-image.loading").length === 0, null, { timeout: 180000 });
  // Larger tiles, so the wall fills the window.
  const wall = await page.locator(".look-wall").boundingBox();
  await page.mouse.move(wall.x + wall.width / 2, wall.y + wall.height / 2);
  await page.keyboard.down("Control");
  for (let i = 0; i < Number(process.env.WALL_ZOOM ?? 10); i++) {
    await page.mouse.wheel(0, -12);
    await page.waitForTimeout(30);
  }
  await page.keyboard.up("Control");
  await page.waitForTimeout(800);
  await page.waitForFunction(() => document.querySelectorAll(".look-tile-image.loading").length === 0, null, { timeout: 180000 });
  await page.mouse.move(5, 5);
  await shot("wall");
});

await session("stack", desktop, async (page, shot) => {
  await openLibrary(page);
  await open(page, "pied-kingfisher-03");
  await shot("strip");
  await page.keyboard.press("C");
  await page.waitForTimeout(500);
  await page.locator(".pane").nth(1).locator(".tier.full.ready").waitFor({ timeout: 30000 });
  await shot("compare");
  await page.keyboard.press("C");
  const live = page.getByRole("button", { name: T.livePhoto });
  if (await live.count()) {
    await live.first().click();
    await page.waitForTimeout(2500);
    await shot("live");
  }
});

await session("clipping", desktop, async (page, shot) => {
  await openLibrary(page);
  await open(page, "cat-in-the-sun");
  await page.keyboard.press("H");
  await page.locator(".tier.clip").waitFor({ timeout: 20000 });
  await shot("on");
});

await session("export", desktop, async (page, shot) => {
  await openLibrary(page);
  await cell(page, "kingfisher-on-a-rock").click();
  await page.locator(`[aria-label="${T.export}"]`).click();
  await page.getByRole("tab", { name: T.frame }).click();
  await page.locator(".frame-preview img").waitFor({ timeout: 20000 });
  await shot("frame");
});

await session("light", { ...desktop, scheme: "light", platform: "windows" }, async (page, shot) => {
  await openLibrary(page);
  await rate(page);
  await cell(page, "kingfisher-on-a-rock").click();
  await shot("grid");
  await open(page, "kingfisher-on-a-rock");
  await shot("viewer");
});

await session("phone", phone, async (page, shot) => {
  await openLibrary(page);
  await shot("grid");
  await cell(page, process.env.PHONE_PHOTO ?? "cross-fox").scrollIntoViewIfNeeded();
  await cell(page, process.env.PHONE_PHOTO ?? "cross-fox").tap();
  await ready(page);
  await shot("viewer");
});

// Flipping through RAW files straight after opening the folder (nothing cached).
await session("flip", { ...desktop, scale: 1 }, async (page) => {
  await openLibrary(page);
  await cell(page, "CZP_0299").dblclick();
  await ready(page);
  await page.waitForTimeout(800);
  const stop = await record(page, `${out}/flip`);
  await page.waitForTimeout(600);
  for (let i = 0; i < 10; i++) {
    await page.keyboard.press("ArrowRight");
    await page.waitForTimeout(450);
  }
  await page.waitForTimeout(600);
  await stop();
});

// Pinching the grid into a photo and back out (Ctrl + wheel, as Chrome reports a trackpad pinch).
await session("zoom", { ...desktop, scale: 1 }, async (page) => {
  await openLibrary(page);
  await page.waitForTimeout(500);
  const target = await cell(page, process.env.ZOOM_TARGET ?? "kingfisher-on-a-branch").boundingBox();
  await page.mouse.move(target.x + target.width / 2, target.y + target.height / 2);
  const stop = await record(page, `${out}/zoom`);
  const viewer = () => page.locator(".viewer").count();
  const pinch = async (dy, until, max = 200) => {
    for (let i = 0; i < max && !(await until()); i++) {
      await page.mouse.wheel(0, dy);
      await page.waitForTimeout(25);
    }
  };
  await page.waitForTimeout(500);
  await page.keyboard.down("Control");
  // In to two a row, a pause, then on into the photo under the pointer.
  let columns = await page.evaluate(() => getComputedStyle(document.querySelector(".grid") ?? document.body).gridTemplateColumns?.split(" ").length ?? 0);
  await pinch(-12, async () => (await viewer()) > 0, 60);
  await page.waitForTimeout(700);
  await pinch(-12, async () => (await viewer()) > 0);
  await page.keyboard.up("Control");
  await ready(page);
  await page.waitForTimeout(1500);
  // Out again: a pause at fit, then back to the grid, and further out to small thumbnails.
  await page.keyboard.down("Control");
  await pinch(12, async () => (await viewer()) === 0);
  await page.waitForTimeout(400);
  await pinch(12, async () => false, 45);
  await page.waitForTimeout(900);
  // Back to where it started, so the loop has no jump.
  await pinch(-12, async () => false, 45);
  await page.keyboard.up("Control");
  await page.waitForTimeout(1200);
  await stop();
  console.log(`[zoom] opened and closed the viewer, ${columns} columns at the start`);
});

// Timings for the README: opening the folder, and stepping through it in the viewer.
// Run on a large folder with the thumbnail cache cleared and the bridge built with --release.
await session("perf", { ...desktop, scale: 1 }, async (page) => {
  const t0 = Date.now();
  await page.locator(".welcome button.primary").click();
  await page.locator(".cell img").first().waitFor();
  const firstCell = Date.now() - t0;
  await page.waitForFunction(() => [...document.querySelectorAll(".cell img")].every((i) => i.complete && i.naturalWidth > 0), null, { timeout: 60000 });
  const firstScreen = Date.now() - t0;
  const count = await page.locator(".toolbar-title span").textContent();
  await page.locator(".cell").first().dblclick();
  await ready(page);
  const steps = [];
  for (let i = 0; i < Number(process.env.STEPS ?? 30); i++) {
    const before = await page.locator(".viewer-title strong").textContent();
    const t = Date.now();
    await page.keyboard.press("ArrowRight");
    await page.waitForFunction((b) => {
      const title = document.querySelector(".viewer-title strong")?.textContent;
      const img = document.querySelector(".pane .tier:not(.full):not(.clip)");
      return title !== b && img?.complete && img.naturalWidth > 0;
    }, before, { timeout: 30000 });
    const shown = Date.now() - t;
    await page.waitForFunction(() => document.querySelector(".tier.full.ready")?.naturalWidth > 0, null, { timeout: 30000 });
    steps.push({ name: await page.locator(".viewer-title strong").textContent(), shown, full: Date.now() - t });
  }
  console.log(JSON.stringify({ count, firstCell, firstScreen, steps }));
});

await browser.close();
console.log(`output in ${out}`);
