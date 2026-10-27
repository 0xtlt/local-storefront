// The look of the page: one set of design tokens, read by the page and by the frame of the
// extensions, and no color of its own outside it.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const assets = join(dirname(fileURLToPath(import.meta.url)), '..');
const read = (name) => readFileSync(join(assets, name), 'utf8');

test('the tokens set the defaults of a store with no branding', () => {
  const tokens = read('tokens.css');
  const value = (name) => tokens.match(new RegExp(`${name}:\\s*([^;]+);`))?.[1].trim();
  assert.equal(value('--lsf-color-accent'), '#005bd1');
  assert.equal(value('--lsf-color-text'), '#000');
  assert.equal(value('--lsf-color-text-subdued'), '#707070');
  assert.equal(value('--lsf-color-border'), '#dedede');
  assert.equal(value('--lsf-color-critical'), '#d91c1c');
  assert.equal(value('--lsf-summary-bg'), '#f5f5f5');
  assert.equal(value('--lsf-font-size-base'), '14px');
  assert.deepEqual(['small', 'base', 'large'].map((size) => value(`--lsf-radius-${size}`)), ['8px', '12px', '16px']);
});

test('what follows from the accent is derived from it, not written down', () => {
  const tokens = read('tokens.css');
  for (const name of ['accent-hover', 'accent-tint-1', 'accent-tint-2', 'accent-tint-3', 'accent-wash', 'focus-ring']) {
    assert.match(tokens, new RegExp(`--lsf-color-${name}:[^;]*var\\(--lsf-color-accent\\)`), name);
  }
});

test('the page and the frame of the extensions read the same tokens', () => {
  assert.match(read('checkout.css'), /@import url\('tokens\.css'\)/);
  assert.match(read('sandbox.html'), /<link rel="stylesheet" href="tokens\.css"/);
});

test('the page keeps its colors in the tokens', () => {
  // The Shop skin and the thank-you page may still draw a few violet washes and the code blocks
  // of the Functions panel are dark: everything else is a token.
  const own = read('checkout.css').match(/#[0-9a-fA-F]{3,8}\b/g) ?? [];
  assert.ok(own.length <= 10, `colors written in checkout.css: ${own.join(', ')}`);
  const components = read('sandbox-components.js').match(/#[0-9a-fA-F]{3,8}\b/g) ?? [];
  assert.deepEqual(components, [], 'the components of the extensions use tokens');
});

test('less motion: transitions end at once and spinners turn slower, on the page and in the frame', () => {
  const tokens = read('tokens.css');
  const block = tokens.match(/@media \(prefers-reduced-motion: reduce\)\s*\{([\s\S]*?)\n\}/)?.[1] ?? '';
  assert.match(block, /--lsf-duration-fast:\s*1ms/);
  assert.match(block, /--lsf-duration-base:\s*1ms/);
  assert.match(block, /--lsf-duration-spin:/);
  for (const name of ['checkout.css', 'sandbox-components.js']) {
    const source = read(name);
    assert.doesNotMatch(source, /(transition|animation)[^;{}]*\b\d*\.?\d+m?s\b/, `${name} writes a duration down`);
  }
});
