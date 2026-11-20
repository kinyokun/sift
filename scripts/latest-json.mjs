#!/usr/bin/env node
// Writes latest.json, the manifest the in-app updater reads, into a folder of release files.
//   node scripts/latest-json.mjs <dir> <tag> <owner/repo> [notes.md]
// Every update package needs its signature next to it (<file>.sig, written by
// `tauri build` when the update key is set). With no signature at all, as in a
// fork without the key, nothing is written. When the folder has a signed
// updates.json (scripts/deltas.mjs), latest.json points to it.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [dir, tag, repo, notesFile] = process.argv.slice(2);
if (!dir || !tag || !repo) {
  console.error("usage: node scripts/latest-json.mjs <dir> <tag> <owner/repo> [notes.md]");
  process.exit(2);
}
const fail = (message) => {
  console.error(message);
  process.exit(1);
};

const version = tag.replace(/^v/, "");
const files = readdirSync(dir);
const signed = files.filter((f) => f.endsWith(".sig")).map((f) => f.slice(0, -".sig".length));
if (!signed.length) {
  console.log("no signed update packages: latest.json not written");
  process.exit(0);
}

// The macOS app is a universal binary, so both architectures take the same package.
const packages = [
  { suffix: ".app.tar.gz", targets: ["darwin-aarch64", "darwin-x86_64"] },
  { suffix: "-setup.exe", targets: ["windows-x86_64"] },
];

// SIFT_RELEASE_URL, for tests, is where the files are served instead of GitHub.
const base = process.env.SIFT_RELEASE_URL ?? `https://github.com/${repo}/releases/download/${tag}/`;
const url = (file) => `${base}${encodeURIComponent(file)}`;

// The app only takes what is signed for the version it is offered as, so a
// file signed for another version would be refused by everyone.
function signature(file) {
  const signature = readFileSync(join(dir, `${file}.sig`), "utf8").trim();
  const comment = Buffer.from(signature, "base64")
    .toString("utf8")
    .split("\n")
    .find((line) => line.startsWith("trusted comment:"));
  const signedVersion = comment?.split("\t").find((field) => field.startsWith("version:"));
  if (signedVersion !== `version:${version}`) {
    fail(`${file}.sig is signed for ${signedVersion ?? "no version"}, not ${version}`);
  }
  return signature;
}

const platforms = {};
for (const { suffix, targets } of packages) {
  const found = signed.filter((f) => f.endsWith(suffix) && files.includes(f));
  if (found.length !== 1) fail(`expected one signed *${suffix} in ${dir}, found ${found.length}`);
  const [file] = found;
  const entry = { signature: signature(file), url: url(file) };
  for (const target of targets) platforms[target] = entry;
}

const manifest = {
  version,
  notes: notesFile ? readFileSync(notesFile, "utf8").trim() : "",
  pub_date: new Date().toISOString(),
  platforms,
};
if (signed.includes("updates.json") && files.includes("updates.json")) {
  manifest.catalogue = { url: url("updates.json"), signature: signature("updates.json") };
}
writeFileSync(join(dir, "latest.json"), `${JSON.stringify(manifest, null, 2)}\n`);
console.log(`latest.json: ${version} for ${Object.keys(platforms).join(", ")}${manifest.catalogue ? ", with updates.json" : ""}`);
