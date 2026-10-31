#!/usr/bin/env node
// Writes the example payloads of Shopify's webhook reference for a version of the Admin API, for
// the topics lsf-admin builds (crates/admin/tests/fixtures/webhooks/examples-<version>.json): the
// tests of the payload builders compare what they build with them. `--version <v>` (default:
// lsf-admin's default version).
//
// Source: https://shopify.dev/docs/api/webhooks/<version>.md, the reference as Markdown, where
// each topic has a "Sample Payload". The topics are those of `BUILT_TOPICS`
// (crates/admin/src/webhooks/payload/mod.rs). Needs network access.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

import { isMain, root, versionArg } from "./source.mjs";

/** Writes examples-<version>.json from shopify.dev's webhook reference of `version`. */
export async function generate(version) {
  const source = `https://shopify.dev/docs/api/webhooks/${version}.md`;
  const out = join(root, `crates/admin/tests/fixtures/webhooks/examples-${version}.json`);
  const builders = readFileSync(join(root, "crates/admin/src/webhooks/payload/mod.rs"), "utf8");
  const list = /pub const BUILT_TOPICS: &\[&str\] = &\[([^\]]*)\];/.exec(builders);
  if (!list) throw new Error("BUILT_TOPICS not found");
  const built = [...list[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]);

  const response = await fetch(source);
  if (!response.ok) throw new Error(`${source}: ${response.status}`);
  const markdown = await response.text();

  // `* #### orders/create: Sample Payload`, then a fenced block, indented by two spaces.
  const examples = {};
  const pattern = /^\* #### (.+?): Sample Payload\n[\s\S]*?```[a-z]*\n([\s\S]*?)\n\s*```/gm;
  for (const [, title, body] of markdown.matchAll(pattern)) {
    const topic = title.replaceAll("\\_", "_").trim();
    const text = body
      .split("\n")
      .map((line) => (line.startsWith("  ") ? line.slice(2) : line))
      .join("\n");
    // Parsed to check it is JSON, but written as it is: `JSON.parse` would round the ids of 18
    // digits, which a number of JavaScript cannot hold.
    JSON.parse(text);
    examples[topic] = text;
  }
  const entries = built.map((topic) => {
    if (!(topic in examples)) throw new Error(`${topic}: no example payload in ${source}`);
    return `    ${JSON.stringify(topic)}: ${examples[topic].replaceAll("\n", "\n    ")}`;
  });
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, `{\n  "source": ${JSON.stringify(source)},\n  "examples": {\n${entries.join(",\n")}\n  }\n}\n`);
  console.log(`${out}: ${entries.length} topics`);
}

if (isMain(import.meta.url)) {
  await generate(versionArg());
}
