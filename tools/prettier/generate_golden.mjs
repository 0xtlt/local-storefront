// Regenerates crates/fmt/tests/golden/*.json from crates/fmt/tests/cases/*.liquid: every case
// formatted by Prettier with Shopify's Liquid plugin (`mise run prettier:golden`).
//
// A cases file holds several cases. Each starts with a line `## name`, which a line
// `#! {"printWidth": 40}` may follow to give Prettier options; the rest is the source.
import fs from 'node:fs';
import path from 'node:path';
import { format, root } from './oracle.mjs';

const casesDir = path.join(root, 'crates/fmt/tests/cases');
const goldenDir = path.join(root, 'crates/fmt/tests/golden');
let total = 0;
for (const file of fs.readdirSync(casesDir).sort()) {
  if (!file.endsWith('.liquid')) continue;
  const cases = [];
  let current = null;
  for (const line of fs.readFileSync(path.join(casesDir, file), 'utf8').split('\n')) {
    if (line.startsWith('## ')) {
      current = { name: line.slice(3).trim(), options: {}, lines: [] };
      cases.push(current);
    } else if (current && current.lines.length === 0 && line.startsWith('#! ')) {
      current.options = JSON.parse(line.slice(3));
    } else if (current) {
      current.lines.push(line);
    }
  }
  const golden = [];
  for (const { name, options, lines } of cases) {
    const source = lines.join('\n').replace(/\n+$/, '') + '\n';
    // What Prettier makes of its own output is kept too: it is not always the same.
    const expected = await format(source, options);
    golden.push({ name, options, source, expected, again: await format(expected, options) });
  }
  total += golden.length;
  fs.writeFileSync(
    path.join(goldenDir, file.replace(/\.liquid$/, '.json')),
    JSON.stringify(golden, null, 2) + '\n',
  );
}
console.log(`${total} cases written to crates/fmt/tests/golden`);
