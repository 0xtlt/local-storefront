// The npm package @shopify/dev-mcp (ISC, Shopify), pinned: what tools/admin reads the Admin API's
// introspection from, and tools/checkout the Function APIs'. The tarball is checked against its
// SHA-256, so the files it gives are the same on every run. Needs network access, and `npm`.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

export const PACKAGE = "@shopify/dev-mcp@1.17.0";
export const SHA256 = "f651c221109dbc55601ab7a85353f3b82fec8ec9bbf9f260440139e28c7e8a7a";

let unpacked = null;

/**
 * The pinned package, downloaded and checked once per process, unpacked into a temporary
 * directory removed when the process exits: its `package` directory.
 */
export function devMcp() {
  if (unpacked) return unpacked;
  const work = mkdtempSync(join(tmpdir(), "lsf-dev-mcp-"));
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
