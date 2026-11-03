#!/usr/bin/env node
// Writes the schema of the GraphQL Admin API that lsf-app serves
// (crates/app/schemas/admin/<version>/):
//
// - `admin.graphql`: the SDL of the whole schema, with every description (introspection serves
//   them), deprecation reason, default value, `@oneOf`, and the directives the schema declares,
//   built-in ones included. Types, fields and values are in the order Shopify's introspection
//   lists them, which is the order lsf's introspection answers in.
// - `admin.graphql.br`: the same, compressed with Brotli: what lsf-app embeds (4 MB of SDL
//   would be 4 MB of the binary; compressed, it takes 0.4 MB and is read back in 5 ms). A test of
//   lsf-app checks that it is `admin.graphql`.
// - `introspection.graphql`: the introspection types (`__Schema`, `__Type`, ...) as Shopify
//   defines them. They are not valid in an SDL document (names starting with `__` are reserved),
//   so they have a file of their own, which lsf-app parses after `admin.graphql`.
//
// It also writes crates/app/tests/fixtures/<version>/introspection.json: a few types of the
// introspection as Shopify answers it, which the tests compare lsf's introspection with.
//
// Source: the introspection of the version (tools/admin/source.mjs): the pinned @shopify/dev-mcp,
// or shopify.dev's public proxy for a version the package does not ship.
//
// `--version <v>` writes the files of another version (default: lsf-app's default version).
// `--introspection <file>` also writes the whole introspection, in the shape of the fixture, to
// <file>: `LSF_ADMIN_INTROSPECTION=<file> cargo test -p lsf-app --test introspection` then
// compares every type of it with what lsf answers.
import { mkdirSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { brotliCompressSync, constants } from "node:zlib";

import { PACKAGE, introspection, isMain, root, versionArg } from "./source.mjs";

// The types the fixture keeps whole, and the root fields it keeps (the roots are too large).
const FIXTURE_TYPES = [
  "Product",
  "ProductConnection",
  "ProductEdge",
  "PageInfo",
  "Node",
  "HasMetafields",
  "ProductStatus",
  "MetafieldReference",
  "ProductIdentifierInput",
  "MoneyV2",
  "DateTime",
  "Money",
  "String",
  "ID",
  "__Schema",
  "__Type",
  "__Field",
  "__InputValue",
  "__EnumValue",
  "__Directive",
  "__TypeKind",
  "__DirectiveLocation",
];
const FIXTURE_FIELDS = {
  QueryRoot: ["node", "nodes", "product", "products", "shop"],
  Mutation: ["productCreate", "productSet", "metafieldsSet"],
};

/** Writes the schema files of `version` from its introspection. */
export function generate(version, { schema, source, pinned }, introspectionOut = null) {
  const out = join(root, "crates/app/schemas/admin", version);
  const fixtures = join(root, "crates/app/tests/fixtures", version);
  const origin = pinned ? PACKAGE : source;
  const header = `# Shopify GraphQL Admin API ${version}, from the introspection of ${origin}, by tools/admin/admin_schema.mjs: do not edit.`;

  mkdirSync(out, { recursive: true });
  const sdl = Buffer.from(`${header}\n\n${printSchema(schema)}`);
  writeFileSync(join(out, "admin.graphql"), sdl);
  const brotli = {
    [constants.BROTLI_PARAM_QUALITY]: constants.BROTLI_MAX_QUALITY,
    [constants.BROTLI_PARAM_LGWIN]: 24,
    [constants.BROTLI_PARAM_SIZE_HINT]: sdl.length,
  };
  writeFileSync(join(out, "admin.graphql.br"), brotliCompressSync(sdl, { params: brotli }));
  const meta = schema.types.filter((type) => type.name.startsWith("__"));
  writeFileSync(join(out, "introspection.graphql"), `${header}\n\n${meta.map(printType).join("\n")}`);

  mkdirSync(fixtures, { recursive: true });
  const fixtureSource = pinned
    ? `${PACKAGE} dist/data/admin_${version}.json.gz, by tools/admin/admin_schema.mjs`
    : `${source}, by tools/admin/admin_schema.mjs`;
  writeFileSync(join(fixtures, "introspection.json"), `${JSON.stringify(fixture(schema, fixtureSource), null, 1)}\n`);
  if (introspectionOut) writeFileSync(introspectionOut, JSON.stringify(standard(schema)));
  console.log(`${version}: ${schema.types.length} types, ${schema.directives.length} directives`);
}

if (isMain(import.meta.url)) {
  const version = versionArg();
  const introspectionArg = process.argv.indexOf("--introspection");
  const introspectionOut = introspectionArg > 0 ? resolve(process.argv[introspectionArg + 1]) : null;
  generate(version, await introspection(version), introspectionOut);
}

// ---- SDL ----

function printSchema(schema) {
  const lines = ["schema {", `  query: ${schema.queryType.name}`];
  if (schema.mutationType) lines.push(`  mutation: ${schema.mutationType.name}`);
  if (schema.subscriptionType) lines.push(`  subscription: ${schema.subscriptionType.name}`);
  lines.push("}", "");
  for (const directive of schema.directives) {
    lines.push(...description(directive.description, ""));
    const repeatable = directive.isRepeatable ? " repeatable" : "";
    lines.push(`directive @${directive.name}${args(directive.args, "")}${repeatable} on ${directive.locations.join(" | ")}`, "");
  }
  const types = schema.types.filter((type) => !type.name.startsWith("__"));
  return `${lines.join("\n")}\n${types.map(printType).join("\n")}`;
}

function printType(type) {
  const lines = description(type.description, "");
  switch (type.kind) {
    case "SCALAR": {
      const specified = type.specifiedByURL ? ` @specifiedBy(url: ${JSON.stringify(type.specifiedByURL)})` : "";
      lines.push(`scalar ${type.name}${specified}`);
      break;
    }
    case "ENUM":
      lines.push(`enum ${type.name} {`);
      for (const value of type.enumValues) {
        lines.push(...description(value.description, "  "), `  ${value.name}${deprecated(value)}`);
      }
      lines.push("}");
      break;
    case "UNION":
      lines.push(`union ${type.name} = ${type.possibleTypes.map((t) => t.name).join(" | ")}`);
      break;
    case "INTERFACE":
    case "OBJECT": {
      const keyword = type.kind === "OBJECT" ? "type" : "interface";
      const implemented = type.interfaces?.length ? ` implements ${type.interfaces.map((t) => t.name).join(" & ")}` : "";
      lines.push(`${keyword} ${type.name}${implemented} {`);
      for (const field of type.fields) {
        lines.push(
          ...description(field.description, "  "),
          `  ${field.name}${args(field.args, "  ")}: ${typeRef(field.type)}${deprecated(field)}`,
        );
      }
      lines.push("}");
      break;
    }
    case "INPUT_OBJECT":
      lines.push(`input ${type.name}${type.isOneOf ? " @oneOf" : ""} {`);
      for (const field of type.inputFields) lines.push(...description(field.description, "  "), `  ${input(field)}`);
      lines.push("}");
      break;
    default:
      throw new Error(`${type.name}: unexpected kind ${type.kind}`);
  }
  return `${lines.join("\n")}\n`;
}

function typeRef(type) {
  if (type.kind === "NON_NULL") return `${typeRef(type.ofType)}!`;
  if (type.kind === "LIST") return `[${typeRef(type.ofType)}]`;
  return type.name;
}

function deprecated(item) {
  if (!item.isDeprecated) return "";
  return item.deprecationReason == null ? " @deprecated" : ` @deprecated(reason: ${JSON.stringify(item.deprecationReason)})`;
}

function input(item) {
  const fallback = item.defaultValue == null ? "" : ` = ${item.defaultValue}`;
  return `${item.name}: ${typeRef(item.type)}${fallback}${deprecated(item)}`;
}

/** The arguments of a field or a directive: on one line, or one per line when one has a description. */
function args(list, indent) {
  if (!list?.length) return "";
  if (list.every((arg) => arg.description == null)) return `(${list.map(input).join(", ")})`;
  const inner = `${indent}  `;
  const lines = list.flatMap((arg) => [...description(arg.description, inner), `${inner}${input(arg)}`]);
  return `(\n${lines.join("\n")}\n${indent})`;
}

/**
 * The lines of a description: a block string when the parser gives back the same text (the
 * value of a block string is its lines without their common indentation and without the blank
 * lines around them, and graphql-parser counts any Unicode whitespace as indentation), a quoted
 * string otherwise.
 */
function description(text, indent) {
  if (text == null) return [];
  const lines = text.split("\n");
  const block =
    text !== "" &&
    !text.includes('"""') &&
    !/[^\S \t\n]/.test(text) &&
    lines[0].trim() !== "" &&
    lines[lines.length - 1].trim() !== "" &&
    lines.some((line) => line.trim() !== "" && !/^[ \t]/.test(line));
  if (!block) {
    if (/[\uD800-\uDFFF]/.test(text.replace(/[\uD800-\uDBFF][\uDC00-\uDFFF]/g, ""))) {
      throw new Error(`a description has a lone surrogate: ${JSON.stringify(text)}`);
    }
    return [`${indent}${JSON.stringify(text)}`];
  }
  return [`${indent}"""`, ...lines.map((line) => (line === "" ? "" : `${indent}${line}`)), `${indent}"""`];
}

// ---- the fixture: the introspection as graphql-js's `getIntrospectionQuery()` asks for it ----

function fixture(schema, source) {
  const byName = new Map(schema.types.map((type) => [type.name, type]));
  const roots = Object.entries(FIXTURE_FIELDS).map(([name, fields]) => {
    const type = standardType(byName.get(name));
    return { ...type, fields: type.fields.filter((field) => fields.includes(field.name)) };
  });
  return {
    source,
    queryType: schema.queryType,
    mutationType: schema.mutationType,
    subscriptionType: schema.subscriptionType,
    types: FIXTURE_TYPES.map((name) => standardType(byName.get(name))),
    roots,
    directives: schema.directives.map(standardDirective),
  };
}

/** The whole introspection, with the keys of `getIntrospectionQuery()` only. */
function standard(schema) {
  return {
    queryType: schema.queryType,
    mutationType: schema.mutationType,
    subscriptionType: schema.subscriptionType,
    typeNames: schema.types.map((type) => type.name),
    types: schema.types.map(standardType),
    roots: [],
    directives: schema.directives.map(standardDirective),
  };
}

function standardType(type) {
  if (!type) throw new Error("a fixture type is missing from the schema");
  return {
    kind: type.kind,
    name: type.name,
    description: type.description,
    specifiedByURL: type.specifiedByURL,
    isOneOf: type.isOneOf,
    fields: type.fields?.map((field) => ({
      name: field.name,
      description: field.description,
      args: field.args.map(standardInput),
      type: standardRef(field.type),
      isDeprecated: field.isDeprecated,
      deprecationReason: field.deprecationReason,
    })) ?? null,
    inputFields: type.inputFields?.map(standardInput) ?? null,
    interfaces: type.interfaces?.map(standardRef) ?? null,
    enumValues: type.enumValues?.map((value) => ({
      name: value.name,
      description: value.description,
      isDeprecated: value.isDeprecated,
      deprecationReason: value.deprecationReason,
    })) ?? null,
    possibleTypes: type.possibleTypes?.map(standardRef) ?? null,
  };
}

function standardInput(input) {
  return {
    name: input.name,
    description: input.description,
    type: standardRef(input.type),
    defaultValue: input.defaultValue,
    isDeprecated: input.isDeprecated,
    deprecationReason: input.deprecationReason,
  };
}

function standardRef(type) {
  return { kind: type.kind, name: type.name, ofType: type.ofType ? standardRef(type.ofType) : null };
}

function standardDirective(directive) {
  return {
    name: directive.name,
    description: directive.description,
    isRepeatable: directive.isRepeatable,
    locations: directive.locations,
    args: directive.args.map(standardInput),
  };
}
