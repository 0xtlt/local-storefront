// Prettier with Shopify's Liquid plugin, as installed by `mise run prettier:install`: the
// reference `lsf fmt` is compared with.
import { createRequire } from 'node:module';
import { existsSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');

const modules = path.join(root, '.oracle/prettier/node_modules');
if (!existsSync(path.join(modules, 'prettier'))) {
  console.error('Prettier is not installed: run `mise run prettier:install` first.');
  process.exit(1);
}
const require = createRequire(path.join(modules, 'x.js'));
const prettier = require('prettier');
const pluginModule = require('@shopify/prettier-plugin-liquid');
const plugin = pluginModule.default ?? pluginModule;

/** Formats a Liquid source with Prettier's options. */
export function format(source, options = {}) {
  return prettier.format(source, { ...options, parser: 'liquid-html', plugins: [plugin] });
}
