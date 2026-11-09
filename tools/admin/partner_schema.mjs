#!/usr/bin/env node
// Writes the schema of Shopify's GraphQL Partner API that lsf-app serves
// (crates/app/schemas/partner/<version>/):
//
// - `partner.graphql`: the SDL of the whole schema, with every description, deprecation, default
//   value and directive, in the order of Shopify's introspection (what lsf's introspection
//   answers). The whole schema, not the part lsf answers: a query that is valid on Shopify is
//   valid on lsf, and a field lsf does not simulate answers `LSF_NOT_SIMULATED` instead of
//   "doesn't exist".
// - `introspection.graphql`: the introspection types (`__Schema`, `__Type`, ...), which lsf-app
//   parses after `partner.graphql`.
//
// Source: the introspection the pinned @shopify/dev-mcp package ships
// (`dist/data/partner_<version>.json.gz`, checked against its SHA-256 in tools/dev_mcp.mjs), the
// one the Admin API's schema comes from (tools/admin/admin_schema.mjs).
//
// `--version <v>` writes another version (default 2026-10). The script also says which other
// versions of the package have the same introspection: lsf serves those with the same files
// (`crates/app/src/admin/partner.rs`, `SERVED`).
import { mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { gunzipSync } from "node:zlib";

import { PACKAGE, SHA256, devMcp } from "../dev_mcp.mjs";
import { printSchema, printType } from "./admin_schema.mjs";
import { isMain, root } from "./source.mjs";

const DEFAULT = "2026-10";

/** The introspection (`data.__schema`) of the Partner API of `version` in the pinned package. */
function introspection(version) {
  const file = join(devMcp(), "dist/data", `partner_${version}.json.gz`);
  return JSON.parse(gunzipSync(readFileSync(file))).data.__schema;
}

/** The versions of the Partner API the package ships, oldest first. */
function versions() {
  return readdirSync(join(devMcp(), "dist/data"))
    .map((name) => /^partner_(.+)\.json\.gz$/.exec(name)?.[1])
    .filter(Boolean)
    .sort();
}

export function generate(version) {
  const schema = introspection(version);
  const out = join(root, "crates/app/schemas/partner", version);
  const header = `# Shopify GraphQL Partner API ${version}, from the introspection of ${PACKAGE}, package/dist/data/partner_${version}.json.gz (sha256 ${SHA256}), by tools/admin/partner_schema.mjs: do not edit.`;
  mkdirSync(out, { recursive: true });
  writeFileSync(join(out, "partner.graphql"), `${header}\n\n${printSchema(schema)}`);
  const meta = schema.types.filter((type) => type.name.startsWith("__"));
  writeFileSync(join(out, "introspection.graphql"), `${header}\n\n${meta.map(printType).join("\n")}`);
  const same = versions().filter(
    (other) => other !== version && JSON.stringify(introspection(other)) === JSON.stringify(schema),
  );
  console.log(`partner ${version}: ${schema.types.length} types; the same introspection: ${same.join(", ") || "no other version"}`);
}

if (isMain(import.meta.url)) {
  const flag = process.argv.indexOf("--version");
  generate(flag > 0 ? process.argv[flag + 1] : DEFAULT);
}
