// Writes a folder of random cases and what graphql-codegen makes of each, to compare with the
// typegen: the ignored test `the_output_is_what_graphql_codegen_writes_for_random_documents`
// reads the folder. Use it when graphql-codegen or the schemas change; the fixed cases of
// `cases.json` are what the normal tests run.
//
// Usage: node fuzz.mjs <oracle> <schema.graphql> <out> [count] [seed]
//        LSF_TYPEGEN_CORPUS=<out> cargo test -p lsf-checkout typegen -- --ignored
//
// A case is a folder with `schema.graphql`, `src/*.graphql` (random valid documents: aliases,
// fragments, inline fragments on interfaces and unions, `@include` and `@skip`, arguments),
// `case.json` (a random `config`) and either `generated/api.ts` or `error.txt` (graphql-codegen
// refuses the documents: random documents can break `OverlappingFieldsCanBeMerged`).

import fs from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';

const [oracleArg, schemaFile, outArgument, countArg = '200', seedArg = '1'] = process.argv.slice(2);
if (!outArgument) {
  console.error('usage: node fuzz.mjs <oracle> <schema.graphql> <out> [count] [seed]');
  process.exit(2);
}
const outArg = path.resolve(outArgument);
const require = createRequire(path.join(path.resolve(oracleArg), 'node_modules/'));
const g = require('graphql');
const { generate } = require('@graphql-codegen/cli');

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
const configs = [
  { omitOperationSuffix: true },
  { omitOperationSuffix: false },
  { dedupeOperationSuffix: true },
  { omitOperationSuffix: true, enumsAsTypes: true },
  { omitOperationSuffix: true, enumsAsConst: true },
  { omitOperationSuffix: true, immutableTypes: true },
  { omitOperationSuffix: true, skipTypename: true },
  { omitOperationSuffix: true, skipTypeNameForRoot: true },
  { omitOperationSuffix: true, nonOptionalTypename: true },
  { omitOperationSuffix: true, skipTypename: true, immutableTypes: true, nonOptionalTypename: true },
];

const sdl = fs.readFileSync(schemaFile, 'utf8');
const schema = g.buildSchema(sdl, { assumeValidSDL: true });
let seed = Number(seedArg) || 1;
const random = () => {
  seed = (seed * 1664525 + 1013904223) % 4294967296;
  return seed / 4294967296;
};
const pick = (items) => items[Math.floor(random() * items.length)];
const chance = (probability) => random() < probability;

const composites = Object.values(schema.getTypeMap()).filter(
  (type) => !type.name.startsWith('__') && g.isCompositeType(type),
);

// A literal for an argument. Always the same for a type, so two fields with one name have the
// same arguments (or the documents would conflict).
function literal(type, depth = 0) {
  if (g.isNonNullType(type)) return literal(type.ofType, depth);
  if (g.isListType(type)) return '[]';
  if (g.isEnumType(type)) return type.getValues()[0].name;
  if (g.isInputObjectType(type)) {
    const fields = Object.values(type.getFields());
    if (type.isOneOf || type.astNode?.directives?.some((d) => d.name.value === 'oneOf')) {
      return `{${fields[0].name}: ${literal(fields[0].type, depth + 1)}}`;
    }
    const required = fields.filter((f) => g.isNonNullType(f.type) && f.defaultValue === undefined);
    return `{${required.map((f) => `${f.name}: ${depth > 3 ? 'null' : literal(f.type, depth + 1)}`).join(', ')}}`;
  }
  return { Int: '1', Float: '1.5', Boolean: 'true', ID: '"1"', String: '"s"' }[type.name] ?? '"x"';
}

const argumentsOf = (field) =>
  field.args.length ? `(${field.args.map((a) => `${a.name}: ${literal(a.type)}`).join(', ')})` : '';

const objectTypes = (type) => (g.isObjectType(type) ? [type] : schema.getPossibleTypes(type));

// The types an inline fragment inside `parent` can be on.
function typeConditions(parent) {
  const objects = objectTypes(parent);
  const found = [parent, ...objects];
  for (const type of composites) {
    if (!g.isObjectType(type) && objectTypes(type).some((object) => objects.includes(object))) found.push(type);
  }
  return found;
}

let fragments = [];
const conditional = () => (chance(0.5) ? ' @include(if: $a)' : ' @skip(if: $b)');

function selections(parent, depth) {
  const items = [];
  const used = new Set();
  const count = 1 + Math.floor(random() * 4);
  for (let i = 0; i < count; i++) {
    const roll = random();
    if (roll < 0.55 || depth >= 4) {
      let fields = g.isUnionType(parent) ? [] : Object.values(parent.getFields());
      if (depth >= 4) fields = fields.filter((f) => !g.isCompositeType(g.getNamedType(f.type)));
      if (!fields.length) {
        items.push('__typename');
        continue;
      }
      const field = pick(fields);
      const alias = chance(0.2) ? `${field.name}Alias: ` : '';
      const key = alias || field.name;
      if (used.has(key)) continue;
      used.add(key);
      const named = g.getNamedType(field.type);
      const directive = chance(0.15) ? conditional() : '';
      const sub = g.isCompositeType(named) ? ` ${set(named, depth + 1)}` : '';
      items.push(`${alias}${field.name}${argumentsOf(field)}${directive}${sub}`);
    } else if (roll < 0.62) {
      items.push(chance(0.5) ? '__typename' : 'tn: __typename');
    } else if (roll < 0.85) {
      const on = pick(typeConditions(parent));
      const directive = chance(0.2) ? conditional() : '';
      const untyped = chance(0.1);
      items.push(`... ${untyped ? '' : `on ${on.name}`}${directive} ${set(untyped ? parent : on, depth + 1)}`);
    } else {
      const objects = objectTypes(parent);
      const usable = fragments.filter((f) => objectTypes(f.type).some((o) => objects.includes(o)));
      items.push(usable.length ? `...${pick(usable).name}${chance(0.15) ? ' @include(if: $a)' : ''}` : '__typename');
    }
  }
  return items;
}
const set = (parent, depth) => `{ ${selections(parent, depth).join(' ')} }`;

fs.rmSync(outArg, { recursive: true, force: true });
for (let n = 0; n < Number(countArg); n++) {
  const dir = path.join(outArg, `case${String(n).padStart(4, '0')}`);
  fs.mkdirSync(path.join(dir, 'src'), { recursive: true });
  fs.writeFileSync(path.join(dir, 'schema.graphql'), sdl);

  // A fragment only spreads the fragments made before it.
  fragments = [];
  const definitions = [];
  const fragmentCount = Math.floor(random() * 4);
  for (let i = fragmentCount - 1; i >= 0; i--) {
    const type = pick(composites);
    const body = set(type, 1);
    fragments.unshift({ name: `Frag${i}`, type });
    definitions.push(`fragment Frag${i} on ${type.name} ${body}`);
  }
  const roots = [schema.getQueryType(), ...(chance(0.15) && schema.getMutationType() ? [schema.getMutationType()] : [])];
  const operationCount = 1 + Math.floor(random() * 2);
  for (let i = 0; i < operationCount; i++) {
    const root = pick(roots);
    const kind = root === schema.getMutationType() ? 'mutation' : 'query';
    definitions.push(`${kind} Op${i}($a: Boolean! = true, $b: Boolean = false) ${set(root, 0)}`);
  }
  const split = chance(0.3) && definitions.length > 1 ? Math.floor(definitions.length / 2) : definitions.length;
  fs.writeFileSync(path.join(dir, 'src/a.graphql'), `${definitions.slice(0, split).join('\n')}\n`);
  if (split < definitions.length) fs.writeFileSync(path.join(dir, 'src/b.graphql'), `${definitions.slice(split).join('\n')}\n`);

  const userConfig = pick(configs);
  fs.writeFileSync(path.join(dir, 'case.json'), JSON.stringify({ config: userConfig }));
  const config = { ...shopifyDefaults, ...userConfig, scalars: shopifyDefaults.scalars };
  const codegen = {
    schema: 'schema.graphql',
    documents: 'src/*.graphql',
    generates: { './generated/api.ts': { plugins: ['typescript', 'typescript-operations'] } },
    config,
    silent: true,
    cwd: dir,
  };
  process.chdir(dir);
  try {
    const [file] = await generate(codegen, false);
    fs.mkdirSync(path.join(dir, 'generated'), { recursive: true });
    fs.writeFileSync(path.join(dir, 'generated/api.ts'), file.content);
  } catch (error) {
    fs.writeFileSync(path.join(dir, 'error.txt'), String(error.errors?.map((e) => e.message).join('\n') ?? error.message));
  }
}
console.log(`${countArg} cases in ${outArg}`);
