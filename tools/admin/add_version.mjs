#!/usr/bin/env node
// Writes everything a version of the Admin API needs in lsf-app (`mise run admin:version --
// 2027-01`), from one introspection (tools/admin/source.mjs: the pinned @shopify/dev-mcp, or
// shopify.dev's public proxy for a version the package does not ship):
//
// - crates/app/schemas/admin/<version>/: admin.graphql, admin.graphql.br, introspection.graphql
//   (admin_schema.mjs), access.json and search.json (access_scopes.mjs), topics.json
//   (webhook_topics.mjs);
// - crates/app/tests/fixtures/<version>/introspection.json (admin_schema.mjs) and
//   crates/app/tests/fixtures/webhooks/examples-<version>.json (webhook_examples.mjs, from
//   shopify.dev's webhook reference of the version).
//
// Then it prints what Shopify says of the version (from the package's list of supported versions),
// how its schema differs from the default version's (what an adapter or a test may need), and the
// steps left, which are code: see "Add a version" in crates/app/docs/admin/design.md.
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { generate as schemaFiles } from "./admin_schema.mjs";
import { generate as accessFiles } from "./access_scopes.mjs";
import { generate as topicFiles } from "./webhook_topics.mjs";
import { generate as exampleFiles } from "./webhook_examples.mjs";
import { defaultVersion, introspection, root, supportedVersions, versionArg } from "./source.mjs";

const version = versionArg();
const source = await introspection(version);
schemaFiles(version, source);
accessFiles(version, source);
topicFiles(version, source);
await exampleFiles(version);

const listed = supportedVersions();
const entry = listed.find((item) => item.name === version);
const status =
  version === "unstable"
    ? "Unstable"
    : entry?.releaseCandidate
      ? "ReleaseCandidate"
      : entry
        ? "Stable"
        : "Unsupported";
const dated = listed.filter((item) => /^\d{4}-\d{2}$/.test(item.name) && !item.releaseCandidate);
const oldest = dated.map((item) => item.name).sort()[0];
console.log(`\n${version}: ${entry ? JSON.stringify(entry) : "not in the package's list of supported versions"}`);
const registry = readFileSync(join(root, "crates/app/src/admin/version.rs"), "utf8");
const current = /pub const OLDEST_ACCESSIBLE: &str = "([^"]+)";/.exec(registry)?.[1];
if (oldest && current !== oldest) {
  console.log(`The oldest stable version Shopify supports is now ${oldest}: set OLDEST_ACCESSIBLE (it is ${current}).`);
}

const base = defaultVersion();
if (base !== version) {
  const other = (await introspection(base)).schema;
  console.log(`\nWhat ${version} changes from ${base} (the records' version):`);
  for (const [title, items] of Object.entries(differences(other, source.schema))) {
    if (items.length) console.log(`  ${title} (${items.length}):\n${items.map((item) => `    ${item}`).join("\n")}`);
  }
}

console.log(`
Left to do, in code (crates/app/docs/admin/design.md, "Add a version"):
  1. crates/app/src/admin/version.rs: add \`version!("${version}", ${status}, &[])\` to KNOWN, in date order, and raise COUNT.
  2. For each field, enum value or payload above that ${version} reads differently from the records,
     an Adapter in crates/app/src/admin/version/adapt.rs (a removed field or mutation needs none: the schema refuses it).
  3. Add to ALL_SCOPES (crates/app/src/admin/auth/scopes.rs) what the test of every scope names.
  4. cargo test -p lsf-app: the payload test compares the builders with the examples of ${version}.`);

/** What differs between two introspections: types, fields, arguments, input fields, enum values. */
function differences(before, after) {
  const str = (type) =>
    type.kind === "NON_NULL" ? `${str(type.ofType)}!` : type.kind === "LIST" ? `[${str(type.ofType)}]` : type.name;
  const index = (schema) => new Map(schema.types.map((type) => [type.name, type]));
  const [a, b] = [index(before), index(after)];
  const out = {
    "types removed": [],
    "types added": [],
    "fields removed": [],
    "fields added": [],
    "types changed": [],
    "arguments removed": [],
    "arguments added": [],
    "input fields removed": [],
    "input fields added": [],
    "enum values removed": [],
    "enum values added": [],
    "newly deprecated": [],
  };
  for (const name of a.keys()) if (!b.has(name)) out["types removed"].push(name);
  for (const name of b.keys()) if (!a.has(name)) out["types added"].push(name);
  for (const [name, type] of a) {
    const other = b.get(name);
    if (!other) continue;
    const fields = new Map((type.fields ?? []).map((field) => [field.name, field]));
    const otherFields = new Map((other.fields ?? []).map((field) => [field.name, field]));
    for (const [field, def] of fields) {
      const next = otherFields.get(field);
      if (!next) {
        out["fields removed"].push(`${name}.${field}: ${str(def.type)}`);
        continue;
      }
      if (str(def.type) !== str(next.type)) out["types changed"].push(`${name}.${field}: ${str(def.type)} -> ${str(next.type)}`);
      if (!def.isDeprecated && next.isDeprecated) out["newly deprecated"].push(`${name}.${field}: ${next.deprecationReason}`);
      const args = new Map(def.args.map((arg) => [arg.name, arg]));
      const otherArgs = new Map(next.args.map((arg) => [arg.name, arg]));
      for (const [arg, value] of args) {
        if (!otherArgs.has(arg)) out["arguments removed"].push(`${name}.${field}(${arg}: ${str(value.type)})`);
        else if (str(value.type) !== str(otherArgs.get(arg).type)) {
          out["types changed"].push(`${name}.${field}(${arg}): ${str(value.type)} -> ${str(otherArgs.get(arg).type)}`);
        }
      }
      for (const [arg, value] of otherArgs) if (!args.has(arg)) out["arguments added"].push(`${name}.${field}(${arg}: ${str(value.type)})`);
    }
    for (const [field, def] of otherFields) if (!fields.has(field)) out["fields added"].push(`${name}.${field}: ${str(def.type)}`);
    const values = new Set((type.enumValues ?? []).map((value) => value.name));
    const otherValues = new Set((other.enumValues ?? []).map((value) => value.name));
    for (const value of values) if (!otherValues.has(value)) out["enum values removed"].push(`${name}.${value}`);
    for (const value of otherValues) if (!values.has(value)) out["enum values added"].push(`${name}.${value}`);
    const inputs = new Map((type.inputFields ?? []).map((field) => [field.name, field]));
    const otherInputs = new Map((other.inputFields ?? []).map((field) => [field.name, field]));
    for (const [field, def] of inputs) {
      if (!otherInputs.has(field)) out["input fields removed"].push(`${name}.${field}: ${str(def.type)}`);
      else if (str(def.type) !== str(otherInputs.get(field).type)) {
        out["types changed"].push(`input ${name}.${field}: ${str(def.type)} -> ${str(otherInputs.get(field).type)}`);
      }
    }
    for (const [field, def] of otherInputs) if (!inputs.has(field)) out["input fields added"].push(`${name}.${field}: ${str(def.type)}`);
  }
  return out;
}
