#!/usr/bin/env node
// Makes the deltas a release carries (see crates/sift-delta) and its updates.json.
//   node scripts/deltas.mjs <dir> <tag> <owner/repo> <sift-delta binary>
//     [--versions 0.6.3,0.6.4] [--old <dir>] [--previous <updates.json>] [--url <base>] [--changelog <file>]
// <dir> holds the release's update packages. For each of the releases just
// before it, its packages are downloaded from GitHub (or taken from --old) and
// a delta to this release made, checked and signed for each platform. Then
// updates.json lists those deltas and the ones earlier releases published
// (taken from the previous release's updates.json, or --previous), with every
// release's notes from CHANGELOG.md, and is signed in turn. Signing takes
// TAURI_SIGNING_PRIVATE_KEY (and _PASSWORD) and the Tauri CLI; the published
// releases are found with gh unless --versions lists them. The other options
// are for tests.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// Releases before this one cannot apply deltas.
const FIRST_TO_APPLY = "0.6.3";
// Deltas are made from this many releases before the new one…
const FROM_RELEASES = 5;
// …and listed for as long as their base is one of this many newest releases.
const KEEP_RELEASES = 30;

const args = process.argv.slice(2);
const options = {};
for (let i = args.length - 2; i >= 0; i--) {
  if (args[i].startsWith("--")) options[args[i].slice(2)] = args.splice(i, 2)[1];
}
const [dirArg, tag, repo, toolArg] = args;
if (!dirArg || !tag || !repo || !toolArg) {
  console.error("usage: node scripts/deltas.mjs <dir> <tag> <owner/repo> <sift-delta> [--versions a,b] [--old dir] [--previous updates.json] [--url base] [--changelog file]");
  process.exit(2);
}
const dir = resolve(dirArg);
const tool = resolve(toolArg);
const version = tag.replace(/^v/, "");
const base = options.url ?? `https://github.com/${repo}/releases/download/${tag}/`;
const work = resolve(dir, "..", "delta-work");

const run = (cmd, argv, opts = {}) => execFileSync(cmd, argv, { stdio: ["ignore", "pipe", "inherit"], encoding: "utf8", maxBuffer: 1 << 26, ...opts });

/** -1, 0 or 1 as semantic versions compare; enough of SemVer for release tags. */
function compare(a, b) {
  const split = (v) => {
    const [core, pre] = v.split("+")[0].split(/-(.*)/s);
    return [core.split(".").map(Number), pre ? pre.split(".") : []];
  };
  const [[ca, pa], [cb, pb]] = [split(a), split(b)];
  for (let i = 0; i < 3; i++) if (ca[i] !== cb[i]) return ca[i] < cb[i] ? -1 : 1;
  if (!pa.length || !pb.length) return pa.length === pb.length ? 0 : pa.length ? -1 : 1;
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    if (pa[i] === undefined) return -1;
    if (pb[i] === undefined) return 1;
    const [na, nb] = [/^\d+$/.test(pa[i]), /^\d+$/.test(pb[i])];
    if (pa[i] === pb[i]) continue;
    if (na && nb) return Number(pa[i]) < Number(pb[i]) ? -1 : 1;
    if (na !== nb) return na ? -1 : 1;
    return pa[i] < pb[i] ? -1 : 1;
  }
  return 0;
}
const valid = (v) => /^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$/.test(v);

/** Every release in CHANGELOG.md, newest first, as the app shows them. */
function history() {
  const releases = [];
  let current = null;
  const finish = () => {
    if (current) releases.push({ ...current.release, notes: current.lines.join("\n").trim() });
  };
  for (const line of readFileSync(options.changelog ?? new URL("../CHANGELOG.md", import.meta.url), "utf8").split(/\r?\n/)) {
    const head = line.match(/^## \[([^\]]+)\](?:\s+-\s+(.+))?\s*$/);
    if (head) {
      finish();
      current = head[1].toLowerCase() === "unreleased" ? null : { release: { version: head[1], date: head[2]?.trim() ?? null, notes: "" }, lines: [] };
    } else if (/^\[[^\]]+\]:\s+\S+/.test(line)) {
      finish();
      current = null;
    } else {
      current?.lines.push(line);
    }
  }
  finish();
  return releases;
}

const platforms = {
  // The macOS app is a universal binary, one package for both architectures.
  macos: {
    suffix: "_universal.app.tar.gz",
    unpack(file, out) {
      mkdirSync(out, { recursive: true });
      run("tar", ["-xzf", file, "-C", out]);
      const apps = readdirSync(out).filter((name) => name.endsWith(".app"));
      if (apps.length !== 1) throw new Error(`${file} holds ${apps.length} apps`);
      return join(out, apps[0]);
    },
  },
  // What the installer puts in the install folder, without its own working files.
  windows: {
    suffix: "_x64-setup.exe",
    unpack(file, out) {
      run("7z", ["x", "-y", `-o${out}`, file]);
      for (const name of readdirSync(out)) {
        if (name.startsWith("$") || name.toLowerCase() === "uninstall.exe") rmSync(join(out, name), { recursive: true, force: true });
      }
      return out;
    },
  },
};

function sign(file, forVersion) {
  run("pnpm", ["--silent", "tauri", "signer", "sign", "--app-version", forVersion, file], {
    cwd: fileURLToPath(new URL("..", import.meta.url)),
    stdio: ["ignore", "ignore", "inherit"],
  });
  return readFileSync(`${file}.sig`, "utf8").trim();
}

function packageIn(folder, platform, of) {
  const name = readdirSync(folder).find((f) => f.startsWith(`Sift_${of}_`) && f.endsWith(platforms[platform].suffix));
  return name && join(folder, name);
}

// The releases out before this one.
let published;
if (options.versions !== undefined) {
  published = options.versions.split(",").filter(Boolean);
} else {
  const list = JSON.parse(run("gh", ["release", "list", "--repo", repo, "--limit", "200", "--exclude-drafts", "--exclude-pre-releases", "--json", "tagName"]));
  published = list.map((r) => r.tagName.replace(/^v/, ""));
}
published = published.filter((v) => valid(v) && compare(v, version) < 0).sort((a, b) => compare(b, a));
const sources = published.filter((v) => compare(v, FIRST_TO_APPLY) >= 0).slice(0, FROM_RELEASES);
const kept = new Set([version, ...published.slice(0, KEEP_RELEASES - 1)]);
console.log(`deltas to ${version} from: ${sources.join(", ") || "none"}`);

rmSync(work, { recursive: true, force: true });
mkdirSync(work, { recursive: true });
const made = Object.fromEntries(Object.keys(platforms).map((p) => [p, []]));
const full = {};
for (const [platform, { unpack }] of Object.entries(platforms)) {
  const fresh = packageIn(dir, platform, version);
  if (!fresh) throw new Error(`no ${platform} package for ${version} in ${dir}`);
  full[platform] = statSync(fresh).size;
  if (!sources.length) continue;
  const tree = unpack(fresh, join(work, "new"));
  for (const from of sources) {
    let old = options.old && packageIn(options.old, platform, from);
    if (!old) {
      const into = join(work, "download");
      rmSync(into, { recursive: true, force: true });
      run("gh", ["release", "download", `v${from}`, "--repo", repo, "--dir", into, "--pattern", `Sift_${from}${platforms[platform].suffix}`]);
      old = packageIn(into, platform, from);
    }
    const oldTree = unpack(old, join(work, "old"));
    const name = `Sift_${version}_from_${from}_${platform}.delta`;
    const delta = join(dir, name);
    console.log(run(tool, ["make", oldTree, tree, from, version, delta]).trim());
    // What the app will do with it, checked against the real thing.
    const check = join(work, "check");
    run(tool, ["build", delta, oldTree, check]);
    run("diff", ["-r", "--no-dereference", tree, check], { stdio: ["ignore", "inherit", "inherit"] });
    rmSync(check, { recursive: true, force: true });
    rmSync(join(work, "old"), { recursive: true, force: true });
    made[platform].push({ from, to: version, url: `${base}${encodeURIComponent(name)}`, size: statSync(delta).size, signature: sign(delta, version) });
  }
  rmSync(join(work, "new"), { recursive: true, force: true });
}

// The deltas earlier releases published, as long as they are still of use.
let previous = { deltas: {} };
if (options.previous) {
  previous = JSON.parse(readFileSync(options.previous, "utf8"));
} else if (published.length) {
  const into = join(work, "previous");
  try {
    run("gh", ["release", "download", `v${published[0]}`, "--repo", repo, "--dir", into, "--pattern", "updates.json"]);
    previous = JSON.parse(readFileSync(join(into, "updates.json"), "utf8"));
  } catch {
    console.log(`v${published[0]} has no updates.json`);
  }
}
const deltas = {};
for (const platform of Object.keys(platforms)) {
  const older = (previous.deltas?.[platform] ?? []).filter((d) => kept.has(d.from) && kept.has(d.to));
  deltas[platform] = [...made[platform], ...older];
}

const catalogue = { version, full, deltas, history: history() };
if (!catalogue.history.some((r) => r.version === version)) throw new Error(`CHANGELOG.md has no section for ${version}`);
writeFileSync(join(dir, "updates.json"), `${JSON.stringify(catalogue, null, 2)}\n`);
sign(join(dir, "updates.json"), version);
rmSync(work, { recursive: true, force: true });
for (const [platform, list] of Object.entries(deltas)) {
  console.log(`updates.json: ${platform}: ${list.length} deltas, full package ${full[platform]} bytes`);
}
