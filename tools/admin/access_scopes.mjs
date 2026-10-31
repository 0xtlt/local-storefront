#!/usr/bin/env node
// Writes the access scopes a version of the Admin API requires
// (crates/admin/schemas/<version>/access.json), which `lsf_admin::auth::scopes` enforces, and the
// search filters each connection documents (crates/admin/schemas/<version>/search.json), which
// `lsf_admin::search` knows. `--version <v>` (default: lsf-admin's default version).
//
// Source: the introspection of the version (tools/admin/source.mjs: the pinned @shopify/dev-mcp,
// or shopify.dev's public proxy). It carries, for each type and each field, the `requiredAccess`
// text shopify.dev prints on the page of a query, a mutation or an object ("Requires
// `read_products` access scope.") and the one an `ACCESS_DENIED` error repeats. The texts are
// written as they are: lsf parses them (`RequiredAccess::parse`), so that the message of an error
// is Shopify's, word for word.
//
// The file has three parts:
// - `types`: the text of each type that has one (`Product`: "`read_products` access scope.");
// - `fields`: the text of each field that has one, by type (`Mutation.productCreate`,
//   `Shop.shopPolicies`);
// - `roots`: for each root query field without a text of its own, the type its answer is made of
//   (`products` -> `Product`, the node of `ProductConnection`), whose text applies: Shopify says
//   "Access denied for products field. Required access: `read_products` access scope.".
//
// The description of the `query` argument of a connection is a table of the filters it accepts
// (`| name | type | description | acceptable_values | default_value | example_use |`):
// search.json keeps the name and the type of each (`["created_at", "time"]`), by type and field.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

import { introspection, isMain, root, versionArg } from "./source.mjs";

/** Writes access.json and search.json of `version` from its introspection. */
export function generate(version, { schema, source }) {
  const out = join(root, "crates/admin/schemas", version, "access.json");
  const searchOut = join(root, "crates/admin/schemas", version, "search.json");
  const types = new Map(schema.types.map((type) => [type.name, type]));

  const typeTexts = {};
  const fieldTexts = {};
  for (const type of [...schema.types].sort(byName)) {
    if (type.name.startsWith("__")) continue;
    if (text(type)) typeTexts[type.name] = text(type);
    const fields = {};
    for (const field of [...(type.fields ?? [])].sort(byName)) {
      if (text(field)) fields[field.name] = text(field);
    }
    if (Object.keys(fields).length) fieldTexts[type.name] = fields;
  }

  const roots = {};
  const query = types.get(schema.queryType.name);
  for (const field of [...query.fields].sort(byName)) {
    if (text(field)) continue;
    const node = nodeType(types, named(field.type));
    if (typeTexts[node]) roots[field.name] = node;
  }

  const body = {
    source,
    query: schema.queryType.name,
    mutation: schema.mutationType.name,
    types: typeTexts,
    fields: fieldTexts,
    roots,
  };
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, `${JSON.stringify(body, null, 1)}\n`);

  const connections = {};
  for (const type of [...schema.types].sort(byName)) {
    for (const field of [...(type.fields ?? [])].sort(byName)) {
      const query = (field.args ?? []).find((arg) => arg.name === "query");
      const filters = query ? filterTable(query.description ?? "") : [];
      if (filters.length) (connections[type.name] ??= {})[field.name] = filters;
    }
  }
  const search = { source: body.source, connections };
  writeFileSync(searchOut, `${JSON.stringify(search, null, 1).replace(/\[\n\s+"([^"]*)",\n\s+"([^"]*)"\n\s+\]/g, '["$1", "$2"]')}\n`);
  console.log(
    `${out}: ${Object.keys(typeTexts).length} types, ` +
      `${Object.values(fieldTexts).reduce((n, fields) => n + Object.keys(fields).length, 0)} fields, ` +
      `${Object.keys(roots).length} root fields through their type`,
  );
  console.log(`${searchOut}: ${Object.values(connections).reduce((n, fields) => n + Object.keys(fields).length, 0)} connections`);
}

if (isMain(import.meta.url)) {
  const version = versionArg();
  generate(version, await introspection(version));
}

// The `requiredAccess` of a type or a field, without the blanks around it; `null` when it has none.
function text(item) {
  const value = (item.requiredAccess ?? "").trim();
  return value === "" ? null : value;
}

// The `[name, type]` of each row of the filter table of a `query` argument's description. A row
// may stop after its type or its description (`| enabled | boolean |`): only the name and the
// type are needed. A line that continues the cell of the row before has no name.
function filterTable(description) {
  if (!description.includes("| name |")) return [];
  const rows = [];
  for (const line of description.split("\n")) {
    const cells = line.split("|").map((cell) => cell.trim());
    // The header row (`| name | type | …`), not a filter called `name`, which many tables have.
    const header = cells[1] === "name" && cells[2] === "type";
    if (cells.length < 4 || cells[1] === "" || cells[2] === "") continue;
    if (header || cells[1].startsWith("--")) continue;
    rows.push([cells[1], cells[2]]);
  }
  return rows;
}

function byName(a, b) {
  return a.name < b.name ? -1 : a.name > b.name ? 1 : 0;
}

function named(type) {
  return type.ofType ? named(type.ofType) : type.name;
}

// The type of the records a field answers: the node of a connection, the type itself otherwise.
function nodeType(types, name) {
  const type = types.get(name);
  if (!name.endsWith("Connection") || !type?.fields) return name;
  const nodes = type.fields.find((field) => field.name === "nodes");
  return nodes ? named(nodes.type) : name;
}
