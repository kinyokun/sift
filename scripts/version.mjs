#!/usr/bin/env node
// Sets or checks Sift's version in package.json, Cargo.toml and tauri.conf.json.
//   node scripts/version.mjs 0.2.0         set all three (and Cargo.lock)
//   node scripts/version.mjs --check v0.2.0  fail unless all three equal the tag
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const files = {
  package: `${root}package.json`,
  cargo: `${root}Cargo.toml`,
  tauri: `${root}src-tauri/tauri.conf.json`,
};
const cargoVersion = /(\[workspace\.package\][^[]*?\nversion\s*=\s*")([^"]+)(")/;

function current() {
  return {
    "package.json": JSON.parse(readFileSync(files.package, "utf8")).version,
    "Cargo.toml": readFileSync(files.cargo, "utf8").match(cargoVersion)?.[2],
    "src-tauri/tauri.conf.json": JSON.parse(readFileSync(files.tauri, "utf8")).version,
  };
}

const args = process.argv.slice(2);
if (args[0] === "--check") {
  const want = (args[1] ?? "").replace(/^v/, "");
  const found = current();
  const wrong = Object.entries(found).filter(([, v]) => v !== want);
  for (const [file, v] of wrong) console.error(`${file} has ${v}, expected ${want}`);
  if (!want || wrong.length) process.exit(1);
  console.log(`version ${want} everywhere`);
} else if (/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(args[0] ?? "")) {
  const v = args[0];
  // Edit the text in place so formatting and key order stay as they are.
  const setJson = (path) => {
    const text = readFileSync(path, "utf8");
    writeFileSync(path, text.replace(/("version"\s*:\s*")[^"]+(")/, `$1${v}$2`));
  };
  setJson(files.package);
  setJson(files.tauri);
  writeFileSync(files.cargo, readFileSync(files.cargo, "utf8").replace(cargoVersion, `$1${v}$3`));
  execFileSync("cargo", ["update", "--workspace", "--offline"], { cwd: root, stdio: "inherit" });
  console.log(current());
} else {
  console.error("usage: node scripts/version.mjs <x.y.z> | --check <tag>");
  process.exit(2);
}
