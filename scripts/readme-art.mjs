// Lays the README screenshots out as finished images: window frames, the hero with a
// phone beside the desktop window, the performance chart and the social preview.
//
//   node scripts/readme-art.mjs <shots dir> <out dir> [perf.json]
//
// <shots dir> is what readme-shots.mjs wrote (one folder per language).
import { chromium } from "playwright-core";
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";
import { dirname, join } from "node:path";

const [shots, out, perfFile] = process.argv.slice(2);
if (!shots || !out) throw new Error("usage: node scripts/readme-art.mjs <shots dir> <out dir> [perf.json]");
mkdirSync(out, { recursive: true });
const root = join(dirname(new URL(import.meta.url).pathname), "..");
const icon = pathToFileURL(join(root, "design/icon.png")).href;
const only = process.env.ONLY?.split(",");
const browser = await chromium.launch({ channel: "chrome" });
const page = await browser.newPage({ deviceScaleFactor: 2 });

const font = `-apple-system, BlinkMacSystemFont, "SF Pro Display", "PingFang SC", "Helvetica Neue", "Segoe UI", sans-serif`;
const url = (lang, name) => pathToFileURL(join(shots, lang, `${name}.png`)).href;
const has = (lang, name) => existsSync(join(shots, lang, `${name}.png`));

// Renders `body` at `width` x `height` CSS px (2x) and writes a WebP.
async function render(file, width, height, body, { transparent = false, quality = 82 } = {}) {
  if (only && !only.some((o) => file.startsWith(o))) return;
  await page.setViewportSize({ width, height });
  const html = join(out, `.${file}.html`);
  writeFileSync(html, `<!doctype html><html><head><meta charset="utf-8"><style>
    * { box-sizing: border-box; margin: 0; }
    html, body { width: ${width}px; height: ${height}px; background: ${transparent ? "transparent" : "#0e0f12"}; font-family: ${font}; overflow: hidden; }
    .win { position: absolute; border-radius: 12px; overflow: hidden; box-shadow: 0 0 0 1px rgba(255,255,255,.10), 0 24px 60px rgba(0,0,0,.45), 0 6px 16px rgba(0,0,0,.25); background: #131314; }
    .win.light { box-shadow: 0 0 0 1px rgba(0,0,0,.10), 0 24px 60px rgba(0,0,0,.22), 0 6px 16px rgba(0,0,0,.12); background: #f5f5f7; }
    .win img { display: block; width: 100%; }
    .lights { position: absolute; top: 18px; left: 18px; display: flex; gap: 8px; }
    .lights i { width: 12px; height: 12px; border-radius: 50%; background: #ff5f57; box-shadow: inset 0 0 0 .5px rgba(0,0,0,.25); }
    .lights i:nth-child(2) { background: #febc2e; } .lights i:nth-child(3) { background: #28c840; }
    .phone { position: absolute; border-radius: 46px; padding: 9px; background: #050506; box-shadow: 0 0 0 1.5px #3a3b40, 0 30px 70px rgba(0,0,0,.55); }
    .phone img { display: block; width: 100%; border-radius: 38px; }
    .glow { position: absolute; inset: 0; background:
      radial-gradient(60% 55% at 85% 100%, rgba(48,209,88,.20), transparent 70%),
      radial-gradient(55% 60% at 0% 0%, rgba(90,96,120,.45), transparent 70%),
      linear-gradient(180deg, #17181d, #0c0d10); }
  </style></head><body>${body}</body></html>`);
  await page.goto(pathToFileURL(html).href);
  await page.evaluate(() => Promise.all([...document.images].map((i) => i.decode())));
  const png = join(out, `.${file}.png`);
  await page.screenshot({ path: png, omitBackground: transparent });
  const webp = join(out, `${file}.webp`);
  execFileSync("cwebp", ["-quiet", "-q", String(quality), "-m", "6", "-mt", "-alpha_q", "90", png, "-o", webp]);
  execFileSync("rm", [png, html]);
  console.log(webp);
}

// The shots are 1440 px wide; macOS draws its window buttons over the toolbar, so they are
// added at the same place and scaled with the shot. Windows shots keep their own controls.
const window = (src, { x, y, w, mac = true, light = false }) => {
  const s = w / 1440;
  const lights = `<div class="lights" style="top:${20 * s}px;left:${20 * s}px;gap:${8 * s}px">${"<i></i>".repeat(3).replaceAll("<i>", `<i style="width:${12 * s}px;height:${12 * s}px">`)}</div>`;
  return `<div class="win${light ? " light" : ""}" style="left:${x}px;top:${y}px;width:${w}px;border-radius:${mac ? 12 : 8}px"><img src="${src}">${mac ? lights : ""}</div>`;
};

// One screenshot in a window, on a transparent canvas so it sits on GitHub's light and dark themes.
const framed = (file, src, opts = {}) => {
  const w = 1200, h = Math.round((w * 900) / 1440), m = 36;
  return render(file, w + m * 2, h + m * 2 + 12, window(src, { x: m, y: m - 8, w, ...opts }), { transparent: true });
};

for (const lang of ["zh", "en"]) {
  if (!has(lang, "library-grid")) continue;
  const L = (n) => url(lang, n);
  // Hero: the library on the desktop, the viewer on a phone beside it.
  await render(`hero-${lang}`, 1280, 760, `<div class="glow"></div>
    ${window(L("library-grid"), { x: 44, y: 44, w: 1040 })}
    <div class="phone" style="left:1000px;top:150px;width:250px"><img src="${L("phone-viewer")}"></div>`);
  for (const [name, src, opts] of [
    ["viewer", "viewer-photo"],
    ["focus", "viewer-af"],
    ["eye-bird", "eyes-kingfisher-on-a-rock-eye"],
    ["eye-fox", "eyes-cross-fox-eye"],
    ["look-wall", "looks-wall"],
    ["burst", "stack-strip"],
    ["compare", "stack-compare"],
    ["live", "stack-live"],
    ["clipping", "clipping-on"],
    ["frame", "export-frame"],
    ["windows", "light-grid", { mac: false, light: true }],
  ]) if (has(lang, src)) await framed(`${name}-${lang}`, L(src), opts);
  if (has(lang, "phone-grid")) {
    await render(`phone-${lang}`, 640, 640, `
      <div class="phone" style="left:40px;top:28px;width:270px"><img src="${L("phone-grid")}"></div>
      <div class="phone" style="left:330px;top:28px;width:270px"><img src="${L("phone-viewer")}"></div>`, { transparent: true });
  }
}

// Social preview (1280x640): the icon, the name, one line, and the hero shot.
for (const lang of ["en"]) {
  if (!has(lang, "library-grid")) continue;
  await render(`social-preview`, 1280, 640, `<div class="glow"></div>
    <img src="${icon}" style="position:absolute;left:56px;top:70px;width:132px">
    <div style="position:absolute;left:76px;top:212px;color:#fff;font-size:76px;font-weight:700;letter-spacing:-1.5px">Sift</div>
    <div style="position:absolute;left:78px;top:312px;width:420px;color:#c9ccd4;font-size:28px;line-height:1.35">Fast photo culling for JPEG and RAW.<br><span style="color:#6ee7a0">快速选片，翻页不等解码。</span></div>
    <div style="position:absolute;left:78px;top:470px;color:#8b8f99;font-size:20px">macOS · Windows · iOS · Android · GPL-3.0</div>
    ${window(url(lang, "viewer-photo"), { x: 540, y: 70, w: 840 })}`, { quality: 90 });
}

// Performance chart from perf.json: { lang: { title, unit, rows: [{ label, value, note, accent }] } }
if (perfFile) {
  const perf = JSON.parse(readFileSync(perfFile, "utf8"));
  for (const [lang, chart] of Object.entries(perf)) {
    const max = Math.max(...chart.rows.map((r) => r.value));
    const log = (v) => Math.log10(v + 1) / Math.log10(max + 1);
    const rows = chart.rows.map((r) => `
      <div style="display:grid;grid-template-columns:330px 1fr;align-items:center;gap:18px;margin:14px 0">
        <div style="color:#e6e7ea;font-size:17px;text-align:right">${r.label}<div style="color:#8b8f99;font-size:13px;margin-top:3px">${r.note ?? ""}</div></div>
        <div style="display:flex;align-items:center;gap:12px">
          <div style="height:26px;border-radius:7px;width:${Math.max(log(r.value) * 640, 6)}px;background:${r.accent ? "linear-gradient(90deg,#30d158,#6ee7a0)" : "#4a4d59"}"></div>
          <div style="color:${r.accent ? "#6ee7a0" : "#c9ccd4"};font-size:18px;font-weight:600;white-space:nowrap">${r.text}</div>
        </div>
      </div>`).join("");
    const height = 150 + chart.rows.length * 62;
    await render(`performance-${lang}`, 1200, height, `<div class="glow"></div>
      <div style="position:absolute;left:48px;top:36px;right:48px">
        <div style="color:#fff;font-size:26px;font-weight:700">${chart.title}</div>
        <div style="color:#8b8f99;font-size:15px;margin-top:6px">${chart.subtitle}</div>
        <div style="margin-top:22px">${rows}</div>
      </div>`, { quality: 90 });
  }
}

await browser.close();
