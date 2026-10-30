// Copies the npm packages lsf builds extensions with when an app has no node_modules into
// crates/checkout/javascript-packages/node_modules, and writes the list lsf embeds
// (crates/checkout/src/javascript/packages.rs). Only the files the entries of a Function or a
// checkout or customer account UI extension reach are copied, with each package's package.json and license.
//
//   node tools/checkout/javascript_packages.mjs        (or: mise run checkout:packages)
//
// The versions are those Shopify's templates resolve to (`shopify app generate extension`):
// change them in PACKAGES, then run it again. It needs npm and the network; lsf does not.

import { execFileSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const PACKAGES = {
  '@shopify/shopify_function': '2.0.1',
  preact: '10.29.8',
  '@preact/signals': '2.11.4',
  '@preact/signals-core': '1.14.5',
  '@shopify/ui-extensions': '2026.10.0',
};

// What a Function's entry and a Preact checkout or customer account UI extension import.
const ENTRIES = [
  '@shopify/shopify_function/run',
  'preact',
  'preact/hooks',
  'preact/jsx-runtime',
  '@preact/signals',
  '@preact/signals-core',
  '@shopify/ui-extensions',
  '@shopify/ui-extensions/preact',
  '@shopify/ui-extensions/checkout',
  '@shopify/ui-extensions/checkout/preact',
  '@shopify/ui-extensions/customer-account',
  '@shopify/ui-extensions/customer-account/preact',
];

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');
const target = join(root, 'crates', 'checkout', 'javascript-packages', 'node_modules');
const scratch = mkdtempSync(join(tmpdir(), 'lsf-packages-'));
writeFileSync(join(scratch, 'package.json'), JSON.stringify({ name: 'packages', private: true, dependencies: PACKAGES }));
execFileSync('npm', ['install', '--no-audit', '--no-fund', '--ignore-scripts', '--loglevel=error'], { cwd: scratch, stdio: 'inherit' });
const modules = join(scratch, 'node_modules');

/** The package of a specifier (`@scope/name/sub` is `@scope/name`), and the rest. */
function split(specifier) {
  const parts = specifier.split('/');
  const name = specifier.startsWith('@') ? parts.slice(0, 2).join('/') : parts[0];
  return [name, `.${specifier.slice(name.length)}`];
}

/** The file a bare specifier leads to, with the conditions lsf's resolver reads. */
function entry(specifier) {
  const [name, sub] = split(specifier);
  const dir = join(modules, name);
  const manifest = JSON.parse(readFileSync(join(dir, 'package.json'), 'utf8'));
  const pick = (value) =>
    typeof value === 'string'
      ? value
      : Array.isArray(value)
        ? value.map(pick).find(Boolean)
        : value && (pick(value.browser) ?? pick(value.module) ?? pick(value.import) ?? pick(value.default));
  let file;
  if (manifest.exports) {
    const map = typeof manifest.exports === 'object' && Object.keys(manifest.exports).some((key) => key.startsWith('.'));
    file = pick(map ? manifest.exports[sub] : sub === '.' ? manifest.exports : undefined);
  }
  file ??= sub === '.' ? manifest.browser ?? manifest.module ?? manifest.main ?? 'index.js' : sub;
  for (const candidate of [file, `${file}.ts`, `${file}.js`, `${file}.mjs`]) {
    const path = join(dir, candidate);
    if (existsSync(path) && statSync(path).isFile()) return path;
  }
  throw new Error(`${specifier}: ${file} not found`);
}

const files = new Set();
const queue = [];
const add = (file) => {
  if (!files.has(file)) {
    files.add(file);
    queue.push(file);
  }
};
for (const specifier of ENTRIES) add(entry(specifier));
while (queue.length > 0) {
  const file = queue.shift();
  const text = readFileSync(file, 'utf8');
  for (const match of text.matchAll(/(?:^|[;\s])(?:import|export)\s*(?:[^'";]*?\sfrom\s*)?['"]([^'"]+)['"]/g)) {
    const specifier = match[1];
    if (specifier.startsWith('.')) {
      const path = resolve(dirname(file), specifier);
      add([path, `${path}.mjs`, `${path}.js`, `${path}.ts`].find((candidate) => existsSync(candidate) && statSync(candidate).isFile()));
    } else {
      add(entry(specifier));
    }
  }
}
for (const name of Object.keys(PACKAGES)) {
  const dir = join(modules, name);
  files.add(join(dir, 'package.json'));
  for (const license of readdirSync(dir).filter((file) => /^licen[cs]e/i.test(file))) files.add(join(dir, license));
}

rmSync(target, { recursive: true, force: true });
const paths = [...files].map((file) => relative(modules, file).split(sep).join('/')).sort();
for (const path of paths) {
  mkdirSync(dirname(join(target, path)), { recursive: true });
  cpSync(join(modules, path), join(target, path));
}
rmSync(scratch, { recursive: true, force: true });

const rust = `// Written by tools/checkout/javascript_packages.mjs: do not edit.
//
// The files of crates/checkout/javascript-packages/node_modules, by path in node_modules.

/// The versions of the packages.
pub const VERSIONS: &[(&str, &str)] = &[
${Object.entries(PACKAGES).map(([name, version]) => `    (${JSON.stringify(name)}, ${JSON.stringify(version)}),`).join('\n')}
];

/// Each file, by its path in node_modules.
pub const FILES: &[(&str, &[u8])] = &[
${paths.map((path) => `    (${JSON.stringify(path)}, include_bytes!("../../../javascript-packages/node_modules/${path}")),`).join('\n')}
];
`;
writeFileSync(join(root, 'crates', 'checkout', 'src', 'javascript', 'packages', 'files.rs'), rust);
console.log(`${paths.length} files in ${relative(root, target)}`);
