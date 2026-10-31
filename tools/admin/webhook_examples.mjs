#!/usr/bin/env node
// Writes the example payloads of Shopify's webhook reference for API 2026-10, for the topics
// lsf-admin builds (crates/admin/tests/fixtures/webhooks/examples-2026-10.json): the tests of the
// payload builders compare what they build with them.
//
// Source: https://shopify.dev/docs/api/webhooks/2026-10.md, the reference as Markdown, where each
// topic has a "Sample Payload". The topics are those of `BUILT_TOPICS`
// (crates/admin/src/webhooks/payload/mod.rs). Needs network access.
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SOURCE = "https://shopify.dev/docs/api/webhooks/2026-10.md";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const out = join(root, "crates/admin/tests/fixtures/webhooks/examples-2026-10.json");
const builders = readFileSync(join(root, "crates/admin/src/webhooks/payload/mod.rs"), "utf8");
const list = /pub const BUILT_TOPICS: &\[&str\] = &\[([^\]]*)\];/.exec(builders);
if (!list) throw new Error("BUILT_TOPICS not found");
const built = [...list[1].matchAll(/"([^"]+)"/g)].map((match) => match[1]);

const response = await fetch(SOURCE);
if (!response.ok) throw new Error(`${SOURCE}: ${response.status}`);
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
// The topics the reference has no "Sample Payload" for: the payloads of Shopify's own
// subscriptions reference app (https://github.com/Shopify/subscriptions-reference-app,
// `app/types/webhooks.ts` for the keys and their order, the fixtures of its tests,
// `app/jobs/dunning/tests/fixtures/`, for the values). `/challenged` is the same resource, with
// no order: the app has no fixture of it.
const attempt = (fields) =>
  JSON.stringify(
    {
      id: 1,
      admin_graphql_api_id: "gid://shopify/SubscriptionBillingAttempt/1",
      idempotency_key: "foo-bar-baz",
      order_id: 1,
      admin_graphql_api_order_id: "gid://shopify/Order/1",
      subscription_contract_id: 1,
      admin_graphql_api_subscription_contract_id: "gid://shopify/SubscriptionContract/1",
      ready: true,
      error_message: null,
      error_code: null,
      ...fields,
    },
    null,
    2,
  );
const SUPPLEMENTS = {
  "subscription_billing_attempts/challenged": attempt({ order_id: null, admin_graphql_api_order_id: null }),
  "subscription_billing_attempts/failure": attempt({
    error_message: "insufficient_funds",
    error_code: "insufficient_funds",
  }),
  "subscription_billing_attempts/success": attempt({}),
};
for (const [topic, text] of Object.entries(SUPPLEMENTS)) {
  if (!(topic in examples)) examples[topic] = text;
}
const entries = built.map((topic) => {
  if (!(topic in examples)) throw new Error(`${topic}: no example payload in ${SOURCE}`);
  return `    ${JSON.stringify(topic)}: ${examples[topic].replaceAll("\n", "\n    ")}`;
});
mkdirSync(dirname(out), { recursive: true });
writeFileSync(out, `{\n  "source": ${JSON.stringify(SOURCE)},\n  "examples": {\n${entries.join(",\n")}\n  }\n}\n`);
console.log(`${out}: ${entries.length} topics`);
