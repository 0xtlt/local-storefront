#!/usr/bin/env node
// Refreshes lsf's copy of Polaris, the web components of an embedded app's pages, which
// `lsf serve --polaris offline` serves at /__lsf/cdn/<path> (crates/cli/src/server/polaris.rs).
// For the maintainers of lsf only: lsf itself never downloads it. See
// crates/cli/assets/shopify-cdn/README.md.
//
// What it fetches from https://cdn.shopify.com/, and writes under crates/cli/assets/shopify-cdn/
// at the same paths:
// - each major of Polaris (`shopifycloud/polaris-<major>.js`), kept under the name of the
//   version it says it is (`polaris-2.0.js` for "2.0.0"), which the CDN must serve the same;
// - what each one loads from the CDN as it runs: its icons (`shopifycloud/admin-ui-foundations/
//   <folder>/<hash>.svg`, every one its tables name) and its stylesheets of fonts, with the files
//   they point at.
// A URL of the CDN in a script or a stylesheet that it does not know how to follow fails the
// run: the copy would be incomplete.
//
// Then it writes SHA256SUMS, the table of README.md between its `refresh` markers, and
// crates/cli/src/server/polaris/files.rs (the list of the files the binary embeds), which it
// formats with rustfmt. Run `cargo test -p lsf-cli polaris` after.
//
//   node tools/polaris/refresh.mjs
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const out = join(root, "crates/cli/assets/shopify-cdn");
const listFile = join(root, "crates/cli/src/server/polaris/files.rs");
const CDN = "https://cdn.shopify.com/";
// The majors kept, newest first. `polaris.js` is whichever the CDN serves it as.
const MAJORS = [2, 1];
// The folders of the CDN the copy is written to: everything else in `out` stays.
const FOLDERS = ["shopifycloud", "static"];
const TYPES = { js: "text/javascript", css: "text/css", svg: "image/svg+xml", woff2: "font/woff2" };

async function get(path) {
  const response = await fetch(CDN + path);
  if (!response.ok) throw new Error(`${CDN}${path}: ${response.status}`);
  const extension = path.split(".").pop();
  const type = response.headers.get("content-type") ?? "";
  if (!TYPES[extension] || !type.startsWith(TYPES[extension])) {
    throw new Error(`${CDN}${path}: content type ${type}, not one lsf serves for .${extension}`);
  }
  return Buffer.from(await response.arrayBuffer());
}

/** Runs `task` on each item, `limit` at a time. */
async function pool(items, limit, task) {
  const results = [];
  let next = 0;
  const worker = async () => {
    while (next < items.length) {
      const index = next++;
      results[index] = await task(items[index]);
    }
  };
  await Promise.all(Array.from({ length: limit }, worker));
  return results;
}

/** The icons a bundle names: `"<name> <hash>"` tables, each turned into URLs by a loop. */
function icons(source, name) {
  const tables = new Map();
  for (const match of source.matchAll(/([A-Za-z_$][\w$]*)=\[((?:"[a-z0-9-]+ [0-9a-f]{5}",?)+)\]/g)) {
    tables.set(match[1], [...match[2].matchAll(/"[a-z0-9-]+ ([0-9a-f]{5})"/g)].map((entry) => entry[1]));
  }
  const loop = /for\(let \w+ of ([\w$]+)\)\{const\[\w+,\w+\]=\w+\.split\(" "\);[\w$]+\[\w+\]="https:\/\/cdn\.shopify\.com\/(shopifycloud\/admin-ui-foundations\/[a-z-]+\/)"\+\w+\+"\.svg"\}/g;
  const paths = [];
  const prefixes = new Set();
  let entries = 0;
  for (const match of source.matchAll(loop)) {
    const hashes = tables.get(match[1]);
    if (!hashes) throw new Error(`${name}: the icon loop over ${match[1]} has no table`);
    prefixes.add(match[2]);
    entries += hashes.length;
    for (const hash of hashes) paths.push(`${match[2]}${hash}.svg`);
  }
  const all = [...source.matchAll(/"[a-z0-9-]+ [0-9a-f]{5}"/g)].length;
  if (!entries || entries !== all) {
    throw new Error(`${name}: ${all} icon entries, ${entries} of them in a table a loop reads: the bundle changed`);
  }
  return { paths, prefixes };
}

/** The URLs of the CDN in a script or a stylesheet: files, and prefixes (ending with `/`). */
function cdnUrls(source) {
  return [...new Set([...source.matchAll(/https:\/\/cdn\.shopify\.com\/([^"'`)\s]*)/g)].map((match) => match[1]))];
}

const files = new Map(); // path → bytes
const scripts = [];

for (const major of MAJORS) {
  const bundle = await get(`shopifycloud/polaris-${major}.js`);
  const source = bundle.toString("utf8");
  const version = /"(\d+\.\d+\.\d+)",[\w$]+=\{currencyCode:"XXX",locale:"en-US",version:/.exec(source)?.[1];
  if (!version || !version.startsWith(`${major}.`)) throw new Error(`polaris-${major}.js: no version ${major}.x found in it`);
  const path = `shopifycloud/polaris-${version.split(".").slice(0, 2).join(".")}.js`;
  if (!(await get(path)).equals(bundle)) throw new Error(`${path} is not what polaris-${major}.js serves`);
  scripts.push({ path, version, aliases: [`shopifycloud/polaris-${major}.js`] });
  files.set(path, bundle);

  const { paths, prefixes } = icons(source, path);
  for (const url of cdnUrls(source)) {
    if (url === "shopifycloud/admin-ui-foundations/" || prefixes.has(url)) continue; // the check of an icon's URL, the icons
    if (url.endsWith("/")) throw new Error(`${path}: the prefix ${CDN}${url} is not one lsf knows how to follow`);
    paths.push(url);
  }
  for (const icon of paths) if (!files.has(icon)) files.set(icon, null);
}
const unversioned = await get("shopifycloud/polaris.js");
const latest = scripts.find((script) => files.get(script.path).equals(unversioned));
if (!latest) throw new Error("polaris.js is none of the majors fetched");
latest.aliases.unshift("shopifycloud/polaris.js");

// Everything the scripts load, then what the stylesheets load.
const fetchAll = async () => {
  const missing = [...files].filter(([, bytes]) => bytes === null).map(([path]) => path);
  await pool(missing, 12, async (path) => files.set(path, await get(path)));
  return missing;
};
for (const path of await fetchAll()) {
  if (!path.endsWith(".css")) continue;
  const css = files.get(path).toString("utf8");
  if (cdnUrls(css).length) throw new Error(`${path}: an absolute URL of the CDN, which lsf does not rewrite in a stylesheet's url()`);
  for (const match of css.matchAll(/url\(\s*(['"]?)([^'")]+)\1\s*\)/g)) {
    const url = new URL(match[2], CDN + path);
    if (url.origin + "/" !== CDN) throw new Error(`${path}: ${url} is not on the CDN`);
    if (!files.has(url.pathname.slice(1))) files.set(url.pathname.slice(1), null);
  }
}
await fetchAll();

// Written in place of the previous copy.
for (const folder of FOLDERS) rmSync(join(out, folder), { recursive: true, force: true });
const sorted = [...files.keys()].sort();
for (const path of sorted) {
  if (!FOLDERS.includes(path.split("/")[0])) throw new Error(`${path} is outside ${FOLDERS.join(", ")}`);
  mkdirSync(dirname(join(out, path)), { recursive: true });
  writeFileSync(join(out, path), files.get(path));
}
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");
writeFileSync(join(out, "SHA256SUMS"), sorted.map((path) => `${sha256(files.get(path))}  ${path}\n`).join(""));

// The table of README.md.
const date = new Date().toISOString().slice(0, 10);
const size = (paths) => paths.reduce((total, path) => total + files.get(path).length, 0);
const kb = (bytes) => `${(bytes / 1024).toFixed(0)} KB`;
const group = (test) => sorted.filter(test);
const svgs = group((path) => path.endsWith(".svg"));
const fonts = group((path) => path.startsWith("static/"));
const rows = [
  `Fetched on ${date} from ${CDN}: ${sorted.length} files, ${kb(size(sorted))}.`,
  "",
  "| File | Version | Also served at | Size | SHA-256 |",
  "|---|---|---|---|---|",
  ...scripts.map(
    (script) =>
      `| \`${script.path}\` | ${script.version} | ${script.aliases.map((alias) => `\`${alias.split("/").pop()}\``).join(", ")} | ${kb(files.get(script.path).length)} | \`${sha256(files.get(script.path))}\` |`,
  ),
  ...fonts
    .filter((path) => path.endsWith(".css"))
    .map((path) => `| \`${path}\` | | | ${kb(files.get(path).length)} | \`${sha256(files.get(path))}\` |`),
  "",
  `And ${svgs.length} icons (\`shopifycloud/admin-ui-foundations/{icons,internal-only,experimental}/<hash>.svg\`, ${kb(size(svgs))}), ` +
    `${fonts.filter((path) => path.endsWith(".woff2")).length} fonts (\`static/fonts/…/*.woff2\`, ${kb(size(fonts.filter((path) => path.endsWith(".woff2"))))}). ` +
    "The SHA-256 of every file is in `SHA256SUMS`.",
];
const readme = join(out, "README.md");
const text = readFileSync(readme, "utf8");
const start = "<!-- refresh: start -->\n";
const end = "<!-- refresh: end -->";
const [before, rest] = text.split(start);
if (rest === undefined || !rest.includes(end)) throw new Error("README.md has no refresh markers");
writeFileSync(readme, `${before}${start}${rows.join("\n")}\n${end}${rest.split(end)[1]}`);

// The list the binary embeds.
const quote = (text) => JSON.stringify(text);
writeFileSync(
  listFile,
  `// Written by tools/polaris/refresh.mjs: run it again rather than editing this file.

use super::Script;

/// The files, by their path on the CDN, which is their path in assets/shopify-cdn.
macro_rules! files {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_bytes!(concat!("../../../assets/shopify-cdn/", $path)) as &[u8])),*]
    };
}

/// The copies of Polaris, newest first: the path of each on the CDN, the version it says it is,
/// and the other paths the CDN served it at when it was fetched (${date}).
pub const SCRIPTS: &[Script] = &[
${scripts.map((script) => `    Script { path: ${quote(script.path)}, version: ${quote(script.version)}, aliases: &[${script.aliases.map(quote).join(", ")}] },`).join("\n")}
];

/// Every file of the copy, by its path on the CDN.
pub const FILES: &[(&str, &[u8])] = files![
${sorted.map((path) => `    ${quote(path)},`).join("\n")}
];
`,
);
const formatted = spawnSync("rustfmt", ["--edition", "2024", listFile], { stdio: "inherit" });
if (formatted.status !== 0) console.warn(`rustfmt did not format ${listFile}: run cargo fmt`);

console.log(`${sorted.length} files, ${kb(size(sorted))}: ${scripts.map((script) => `${script.path} (${script.version}; ${script.aliases.join(", ")})`).join(", ")}`);
