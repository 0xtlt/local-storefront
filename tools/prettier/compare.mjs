// Formats every Liquid file of a directory with Prettier and with `lsf fmt`, and lists the
// files the two disagree on: `mise run prettier:compare -- <directory> [options as JSON]`.
// Nothing is written to the directory.
//
// Both run with the same options and without configuration files. With
// `{"embeddedLanguageFormatting":"off"}` the comparison is that of Liquid and HTML alone.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { format, root } from './oracle.mjs';

const [directory, optionsJson] = process.argv.slice(2);
if (!directory) {
  console.error('usage: compare.mjs <directory> [options as JSON]');
  process.exit(2);
}
const options = optionsJson ? JSON.parse(optionsJson) : {};
const lsf = process.env.LSF ?? path.join(root, 'target/release/lsf');
if (!fs.existsSync(lsf)) {
  console.error(`${lsf} is missing: run \`mise run build\` first, or set LSF to the binary.`);
  process.exit(1);
}
const flags = Object.entries(options).flatMap(([name, value]) => ['--option', `${name}=${JSON.stringify(value)}`]);

function* liquidFiles(dir) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
    if (entry.name === 'node_modules' || entry.name === '.git') continue;
    const file = path.join(dir, entry.name);
    if (entry.isDirectory()) yield* liquidFiles(file);
    else if (entry.name.endsWith('.liquid')) yield file;
  }
}

let same = 0;
let refused = 0;
const different = [];
for (const file of liquidFiles(directory)) {
  const source = fs.readFileSync(file, 'utf8');
  let expected;
  try {
    expected = await format(source, options);
  } catch {
    refused += 1;
    continue;
  }
  const actual = execFileSync(lsf, ['fmt', '--stdin', '--no-config', ...flags], {
    input: source,
    encoding: 'utf8',
    maxBuffer: 1 << 28,
  });
  if (actual === expected) same += 1;
  else different.push(path.relative(directory, file));
}
for (const file of different) console.log(file);
console.error(
  `${same} of ${same + different.length} file(s) formatted as Prettier formats them` +
    (refused ? `, ${refused} that Prettier refuses left out` : ''),
);
process.exit(different.length === 0 ? 0 : 1);
