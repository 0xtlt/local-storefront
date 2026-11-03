// Writes what graphql-codegen produces for every case of `cases.json`: `expected.json` (the
// SHA-256 of the output, or the error) and `expected/<case>.ts` (the whole output of the cases
// that have `"full": true`). It runs graphql-codegen the way Shopify's CLI does for a Function:
//
//     npm exec -- graphql-code-generator --config <tmp>/codegen.json
//
// in a copy of the folder of a Function (its `package.json` with the `codegen` section, the
// schema, `src/*.graphql`), with the config Shopify's CLI merges into the one of the Function.
//
// Usage: node regenerate.mjs <oracle> [case ...]
//
// `<oracle>` is a folder with `@shopify/shopify_function@2.0.1` installed (`npm install
// --save-exact @shopify/shopify_function@2.0.1` in an empty folder): it brings the graphql-codegen
// that a Function uses. See README.md.

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const schemasOfApis = path.join(here, '../../../schemas/2026-10');
const [oracleArg, ...only] = process.argv.slice(2);
if (!oracleArg) {
  console.error('usage: node regenerate.mjs <oracle> [case ...]');
  process.exit(2);
}
const oracle = path.resolve(oracleArg);

const pinned = {
  '@graphql-codegen/cli': '5.0.6',
  '@graphql-codegen/typescript': '4.1.6',
  '@graphql-codegen/typescript-operations': '4.6.1',
  '@graphql-codegen/visitor-plugin-common': '5.8.0',
  graphql: '16.14.2',
};
for (const [name, version] of Object.entries(pinned)) {
  const found = JSON.parse(fs.readFileSync(path.join(oracle, 'node_modules', name, 'package.json'), 'utf8')).version;
  if (found !== version) {
    console.error(`${name} is ${found} in ${oracle}, the fixtures are made with ${version}`);
    process.exit(1);
  }
}

// What Shopify's CLI merges into the config of a Function.
const shopifyDefaults = {
  defaultScalarType: 'unknown',
  scalars: {
    Date: 'string',
    DateTime: 'string',
    DateTimeWithoutTimezone: 'string',
    Decimal: 'string',
    Handle: 'string',
    ID: 'string',
    JSON: 'unknown',
    TimeWithoutTimezone: 'string',
    URL: 'string',
    Void: 'null',
  },
};
// The `codegen` section of the Function of a case, unless the case changes some of its keys.
const functionCodegen = {
  schema: 'schema.graphql',
  documents: 'src/*.graphql',
  generates: { './generated/api.ts': { plugins: ['typescript', 'typescript-operations'] } },
  config: { omitOperationSuffix: true },
};

function schemaFile(name) {
  const own = path.join(here, 'schemas', `${name}.graphql`);
  return fs.existsSync(own) ? own : path.join(schemasOfApis, `${name}.graphql`);
}

function run(testCase) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), 'lsf-typegen-'));
  try {
    const folder = path.join(tmp, 'function');
    fs.mkdirSync(path.join(folder, 'src'), { recursive: true });
    fs.symlinkSync(path.join(oracle, 'node_modules'), path.join(folder, 'node_modules'));
    fs.copyFileSync(schemaFile(testCase.schema), path.join(folder, 'schema.graphql'));
    for (const document of testCase.documents) {
      fs.copyFileSync(path.join(here, 'documents', document), path.join(folder, 'src', path.basename(document)));
    }
    const codegen = { ...functionCodegen, ...testCase.codegen };
    fs.writeFileSync(path.join(folder, 'package.json'), JSON.stringify({ name: 'function', codegen }));
    const config = {
      ...shopifyDefaults,
      ...codegen.config,
      scalars: { ...shopifyDefaults.scalars, ...codegen.config?.scalars },
    };
    const configPath = path.join(tmp, 'codegen.json');
    fs.writeFileSync(configPath, JSON.stringify({ ...codegen, config }));
    const result = spawnSync('npm', ['exec', '--', 'graphql-code-generator', '--config', configPath], {
      cwd: folder,
      encoding: 'utf8',
    });
    if (result.status !== 0) {
      const text = `${result.stdout}${result.stderr}`.split(tmp).join('<function>');
      const lines = text
        .split('\n')
        .filter((line) => line.startsWith('[FAILED]'))
        .map((line) => line.replace('[FAILED]', '').trim())
        .filter((line) => line && !line.startsWith('Failed to load') && !line.startsWith('at '));
      return { error: lines.slice(0, 2).join(' ') || text.trim().split('\n')[0] };
    }
    const output = Object.keys(codegen.generates)[0];
    return { output: fs.readFileSync(path.join(folder, output), 'utf8') };
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
}

const cases = JSON.parse(fs.readFileSync(path.join(here, 'cases.json'), 'utf8'));
const expectedPath = path.join(here, 'expected.json');
const expected = only.length && fs.existsSync(expectedPath) ? JSON.parse(fs.readFileSync(expectedPath, 'utf8')) : {};
fs.mkdirSync(path.join(here, 'expected'), { recursive: true });
for (const testCase of cases) {
  if (only.length && !only.includes(testCase.name)) continue;
  const { output, error } = run(testCase);
  const full = path.join(here, 'expected', `${testCase.name}.ts`);
  fs.rmSync(full, { force: true });
  if (error !== undefined) {
    expected[testCase.name] = { error };
  } else if (testCase.full) {
    fs.writeFileSync(full, output);
    delete expected[testCase.name];
  } else {
    expected[testCase.name] = createHash('sha256').update(output).digest('hex');
  }
  console.log(`${testCase.name}: ${error !== undefined ? `error: ${error}` : testCase.full ? 'full output' : expected[testCase.name]}`);
}
const lines = cases
  .filter((testCase) => testCase.name in expected)
  .map((testCase) => `  ${JSON.stringify(testCase.name)}: ${JSON.stringify(expected[testCase.name])}`);
fs.writeFileSync(expectedPath, `{\n${lines.join(',\n')}\n}\n`);
