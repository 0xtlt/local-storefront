#!/usr/bin/env node
// Writes the access scopes the Admin API 2026-10 requires (crates/admin/schemas/2026-10/access.json),
// which `lsf_admin::auth::scopes` enforces.
//
// Source: the npm package @shopify/dev-mcp (ISC, Shopify), pinned below. Its introspection of the
// Admin API carries, for each type and each field, the `requiredAccess` text shopify.dev prints on
// the page of a query, a mutation or an object ("Requires `read_products` access scope.") and the
// one an `ACCESS_DENIED` error repeats. The texts are written as they are: lsf parses them
// (`RequiredAccess::parse`), so that the message of an error is Shopify's, word for word. The
// tarball is checked against the pinned SHA-256, so the result is reproducible. Needs network
// access and `npm`.
//
// The file has three parts:
// - `types`: the text of each type that has one (`Product`: "`read_products` access scope.");
// - `fields`: the text of each field that has one, by type (`Mutation.productCreate`,
//   `Shop.shopPolicies`);
// - `roots`: for each root query field without a text of its own, the type its answer is made of
//   (`products` -> `Product`, the node of `ProductConnection`), whose text applies: Shopify says
//   "Access denied for products field. Required access: `read_products` access scope.".
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";

const PACKAGE = "@shopify/dev-mcp@1.17.0";
const SHA256 = "f651c221109dbc55601ab7a85353f3b82fec8ec9bbf9f260440139e28c7e8a7a";
const VERSION = "2026-10";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const out = join(root, "crates/admin/schemas", VERSION, "access.json");

const work = mkdtempSync(join(tmpdir(), "lsf-admin-access-"));
try {
  execFileSync("npm", ["pack", PACKAGE, "--silent", "--pack-destination", work], {
    stdio: ["ignore", "ignore", "inherit"],
  });
  const tarball = join(work, readdirSingle(work));
  const sha = createHash("sha256").update(readFileSync(tarball)).digest("hex");
  if (sha !== SHA256) throw new Error(`${PACKAGE}: SHA-256 is ${sha}, expected ${SHA256}`);
  const unpacked = join(work, "unpacked");
  mkdirSync(unpacked);
  execFileSync("tar", ["xzf", tarball, "-C", unpacked]);

  const path = join(unpacked, "package/dist/data", `admin_${VERSION}.json.gz`);
  const schema = JSON.parse(gunzipSync(readFileSync(path))).data.__schema;
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
    source: `${PACKAGE}, package/dist/data/admin_${VERSION}.json.gz (sha256 ${SHA256})`,
    query: schema.queryType.name,
    mutation: schema.mutationType.name,
    types: typeTexts,
    fields: fieldTexts,
    roots,
  };
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, `${JSON.stringify(body, null, 1)}\n`);
  console.log(
    `${out}: ${Object.keys(typeTexts).length} types, ` +
      `${Object.values(fieldTexts).reduce((n, fields) => n + Object.keys(fields).length, 0)} fields, ` +
      `${Object.keys(roots).length} root fields through their type`,
  );
} finally {
  rmSync(work, { recursive: true, force: true });
}

// The `requiredAccess` of a type or a field, without the blanks around it; `null` when it has none.
function text(item) {
  const value = (item.requiredAccess ?? "").trim();
  return value === "" ? null : value;
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

function readdirSingle(dir) {
  const names = execFileSync("ls", [dir]).toString().trim().split("\n");
  if (names.length !== 1) throw new Error(`expected one tarball in ${dir}, got ${names}`);
  return names[0];
}
