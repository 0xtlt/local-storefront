// What the generators of tools/admin read: the introspection of the GraphQL Admin API of one
// version, and which version that is.
//
// - `--version <v>` names the version (`2027-01`); without it, the default version of lsf-admin,
//   read from `pub const DEFAULT` in crates/admin/src/version.rs, so that the scripts and the
//   crate have one default.
// - The introspection comes from the npm package @shopify/dev-mcp (ISC, Shopify), pinned below and
//   checked against its SHA-256, which ships the introspection of each Admin API version it
//   supports (`dist/data/admin_<version>.json.gz`, with the `requiredAccess` of each type and
//   field). It is reproducible: the same package gives the same files.
// - A version the package does not ship (a release newer than it) is introspected from Shopify's
//   public proxy of the Admin API, the one shopify.dev's GraphiQL explorer queries
//   (`https://shopify.dev/admin-graphql-direct-proxy/<version>`), which answers introspection
//   without a token and carries `requiredAccess` too. It is not pinned: the source says when it
//   was read, and a release candidate read there may differ from the one the next package ships.
//
// Needs network access, and `npm` for the package.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";

export const PACKAGE = "@shopify/dev-mcp@1.17.0";
export const SHA256 = "f651c221109dbc55601ab7a85353f3b82fec8ec9bbf9f260440139e28c7e8a7a";
export const PROXY = "https://shopify.dev/admin-graphql-direct-proxy";

export const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

/** The default version of lsf-admin: `pub const DEFAULT` of crates/admin/src/version.rs. */
export function defaultVersion() {
  const source = readFileSync(join(root, "crates/admin/src/version.rs"), "utf8");
  const found = /pub const DEFAULT: &str = "([^"]+)";/.exec(source);
  if (!found) throw new Error("crates/admin/src/version.rs has no `pub const DEFAULT`");
  return found[1];
}

/** `--version <v>` of the command line (or the first argument that is a version), else the default. */
export function versionArg(argv = process.argv.slice(2)) {
  const flag = argv.indexOf("--version");
  const given = flag >= 0 ? argv[flag + 1] : argv.find((arg) => /^(\d{4}-\d{2}|unstable)$/.test(arg));
  const version = given ?? defaultVersion();
  if (!/^(\d{4}-\d{2}|unstable)$/.test(version)) {
    throw new Error(`\`${version}\` is not an API version (2027-01, unstable)`);
  }
  return version;
}

let unpacked = null;

/** The pinned package, unpacked once per process into a temporary directory. */
export function devMcp() {
  if (unpacked) return unpacked;
  const work = mkdtempSync(join(tmpdir(), "lsf-admin-dev-mcp-"));
  process.on("exit", () => rmSync(work, { recursive: true, force: true }));
  execFileSync("npm", ["pack", PACKAGE, "--silent", "--pack-destination", work], {
    stdio: ["ignore", "ignore", "inherit"],
  });
  const tarballs = readdirSync(work);
  if (tarballs.length !== 1) throw new Error(`expected one tarball in ${work}, got ${tarballs}`);
  const tarball = join(work, tarballs[0]);
  const sha = createHash("sha256").update(readFileSync(tarball)).digest("hex");
  if (sha !== SHA256) throw new Error(`${PACKAGE}: SHA-256 is ${sha}, expected ${SHA256}`);
  const dir = join(work, "unpacked");
  mkdirSync(dir);
  execFileSync("tar", ["xzf", tarball, "-C", dir]);
  unpacked = join(dir, "package");
  return unpacked;
}

/**
 * The versions of the Admin API the package lists, newest first, as shopify.dev's version
 * picker shows them: `{name, releaseCandidate?, latestVersion?}`.
 */
export function supportedVersions() {
  const file = join(devMcp(), "dist/data/supported-versions-schema.json");
  return JSON.parse(readFileSync(file, "utf8")).admin;
}

/**
 * The introspection of the Admin API of `version` (`data.__schema`), and where it comes from:
 * `{schema, source, pinned}`. From the pinned package when it ships the version, from Shopify's
 * public proxy otherwise.
 */
export async function introspection(version) {
  const file = join(devMcp(), "dist/data", `admin_${version}.json.gz`);
  let compressed = null;
  try {
    compressed = readFileSync(file);
  } catch {
    // Not in the package: read it from shopify.dev.
  }
  if (compressed) {
    return {
      schema: JSON.parse(gunzipSync(compressed)).data.__schema,
      source: `${PACKAGE}, package/dist/data/admin_${version}.json.gz (sha256 ${SHA256})`,
      pinned: true,
    };
  }
  const url = `${PROXY}/${version}`;
  console.warn(`${PACKAGE} does not ship the Admin API ${version}: introspecting ${url}`);
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json", accept: "application/json" },
    body: JSON.stringify({ query: INTROSPECTION_QUERY }),
  });
  if (!response.ok) throw new Error(`${url}: ${response.status} ${await response.text()}`);
  const body = await response.json();
  if (body.errors) throw new Error(`${url}: ${JSON.stringify(body.errors)}`);
  const day = new Date().toISOString().slice(0, 10);
  return { schema: body.data.__schema, source: `${url}, introspected on ${day}`, pinned: false };
}

const typeRef = (depth) => (depth === 0 ? "kind name" : `kind name ofType { ${typeRef(depth - 1)} }`);

/**
 * graphql-js's `getIntrospectionQuery()` with what the generators read besides: `requiredAccess`
 * of types and fields, deprecated arguments and input fields.
 */
const INTROSPECTION_QUERY = `query IntrospectionQuery {
  __schema {
    description
    queryType { name kind }
    mutationType { name kind }
    subscriptionType { name kind }
    types { ...FullType }
    directives { name description isRepeatable locations args(includeDeprecated: true) { ...InputValue } }
  }
}
fragment FullType on __Type {
  kind name description specifiedByURL isOneOf requiredAccess
  fields(includeDeprecated: true) {
    name description args(includeDeprecated: true) { ...InputValue }
    type { ...TypeRef } isDeprecated deprecationReason requiredAccess
  }
  inputFields(includeDeprecated: true) { ...InputValue }
  interfaces { ...TypeRef }
  enumValues(includeDeprecated: true) { name description isDeprecated deprecationReason }
  possibleTypes { ...TypeRef }
}
fragment InputValue on __InputValue {
  name description type { ...TypeRef } defaultValue isDeprecated deprecationReason
}
fragment TypeRef on __Type { ${typeRef(8)} }`;

/** Whether this module is the script node runs (not a module another script imports). */
export function isMain(url) {
  return process.argv[1] && resolve(process.argv[1]) === fileURLToPath(url);
}
