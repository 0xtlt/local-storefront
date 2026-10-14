// Turns the tests of Shopify's Prettier plugin into cases for
// crates/fmt/tests/plugin_fixtures.rs: for each paragraph of each `index.liquid`, the source,
// the options its title asks for, and what Prettier makes of it.
//
// The paragraphs are cut and their options read the way the plugin's own test helper does
// (packages/prettier-plugin-liquid/src/test/test-helpers.ts).
import fs from 'node:fs';
import path from 'node:path';
import { format, root } from './oracle.mjs';

const tests = path.join(root, '.cache/theme-tools/packages/prettier-plugin-liquid/src/test');
const PARAGRAPH_SPLITTER = /(?:\r?\n){2,}(?=\/\/|It|When|If|focus|debug|skip|<)/i;
const TEST_MESSAGE = /^(\/\/|It|When|If|focus|debug|skip)((\s|\S)(?!<)(?!{)(?!---))*./i;

const cases = [];
for (const directory of fs.readdirSync(tests).sort()) {
  const file = path.join(tests, directory, 'index.liquid');
  if (!fs.existsSync(file)) continue;
  const paragraphs = fs
    .readFileSync(file, 'utf8')
    .split(PARAGRAPH_SPLITTER)
    .map((paragraph) => paragraph.trimEnd());
  for (const [index, paragraph] of paragraphs.entries()) {
    const message = (TEST_MESSAGE.exec(paragraph) ?? [''])[0]
      .replace(/^\/\/\s*/, '')
      .replace(/\r?\n/g, ' ')
      .trimEnd()
      .replace(/\.$/, '');
    // The defaults the tests of the plugin were written with.
    const options = { printWidth: 80, indentSchema: true, trailingComma: 'es5' };
    for (const match of message.matchAll(/(?<name>\w+): (?<value>[^\s]*)/g)) {
      try {
        options[match.groups.name] = JSON.parse(match.groups.value);
      } catch {
        // Not an option: a word of the title followed by a colon.
      }
    }
    const source = paragraph.replace(TEST_MESSAGE, '').trimStart();
    try {
      const expected = await format(source, options);
      cases.push({ name: `${directory}#${index}`, options, source, expected });
    } catch {
      // Prettier refuses the source: nothing to compare with.
    }
  }
}
const output = path.join(root, '.cache/prettier-plugin-liquid-cases.json');
fs.writeFileSync(output, JSON.stringify(cases, null, 1) + '\n');
console.log(`${cases.length} cases written to ${path.relative(root, output)}`);
