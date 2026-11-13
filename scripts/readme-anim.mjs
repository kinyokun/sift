// Turns a screen recording from readme-shots.mjs (JPEG frames plus frames.json) into
// an animated WebP, keeping each frame on screen for as long as it was recorded.
//
//   node scripts/readme-anim.mjs <frames dir> <out.webp> [width] [fps]
//
// Needs ffmpeg built with libwebp.
import { readFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { join } from "node:path";

const [dir, out, width = "880", fps = "15"] = process.argv.slice(2);
if (!dir || !out) throw new Error("usage: node scripts/readme-anim.mjs <frames dir> <out.webp> [width] [fps]");
const frames = JSON.parse(readFileSync(join(dir, "frames.json"), "utf8"));
const lines = frames.map((f, i) => {
  // The last frame is held a little so the loop does not restart abruptly.
  const duration = i + 1 < frames.length ? frames[i + 1].t - f.t : 1.2;
  return `file '${f.file}'\nduration ${Math.max(duration, 0.001).toFixed(4)}`;
});
// The concat demuxer ignores the last duration unless the file is listed once more.
lines.push(`file '${frames.at(-1).file}'`);
const list = join(dir, "concat.txt");
writeFileSync(list, `${lines.join("\n")}\n`);
execFileSync("ffmpeg", [
  "-loglevel", "error", "-y", "-f", "concat", "-safe", "0", "-i", list,
  "-vf", `fps=${fps},scale=${width}:-2:flags=lanczos`,
  "-c:v", "libwebp_anim", "-loop", "0", "-quality", "62", "-compression_level", "6", out,
], { stdio: "inherit" });
console.log(out);
