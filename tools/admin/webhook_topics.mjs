#!/usr/bin/env node
// Writes the webhook topics of a version of the Admin API
// (crates/app/schemas/admin/<version>/topics.json), which `lsf_app::admin::webhooks::topics` reads:
// every value of its `WebhookSubscriptionTopic` enum, with the topic as Shopify writes it
// (`ORDERS_CREATE` is `orders/create`, `CUSTOMER_TAGS_ADDED` is `customer.tags_added`), the access
// scopes it requires and what it reports. `--version <v>` (default: lsf-app's default version).
//
// The written form is not derived from the enum value: the split between the resource and the
// action is not mechanical (`CUSTOMERS_EMAIL_MARKETING_CONSENT_UPDATE` is
// `customers_email_marketing_consent/update`). It is read from the description of each value,
// which starts with "The webhook topic for `<topic>` events."; the run fails when one does not.
//
// Source: the introspection of the version (tools/admin/source.mjs: the pinned @shopify/dev-mcp,
// or shopify.dev's public proxy), with its descriptions.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";

import { introspection, isMain, root, versionArg } from "./source.mjs";

/** Writes topics.json of `version` from its introspection. */
export function generate(version, { schema, source }) {
  const out = join(root, "crates/app/schemas/admin", version, "topics.json");
  const topicType = schema.types.find((type) => type.name === "WebhookSubscriptionTopic");
  if (!topicType) throw new Error(`${version} has no WebhookSubscriptionTopic enum`);
  const rows = topicType.enumValues.map((value) => {
    const description = (value.description ?? "").replace(/\s+/g, " ").trim();
    const written = /^The webhook topic for `([^`]+)` events\. ?/.exec(description);
    if (!written) throw new Error(`${value.name}: no topic in its description: ${description}`);
    const rest = description.slice(written[0].length);
    const scopes = [];
    const one = /Requires the `([a-z_]+)` scope\./.exec(rest);
    const any = /Requires at least one of the following scopes: ([a-z_, ]+)\./.exec(rest);
    if (one) scopes.push(one[1]);
    if (any) scopes.push(...any[1].split(",").map((scope) => scope.trim()));
    const summary = rest
      .replace(/ ?Requires the `[a-z_]+` scope\./, "")
      .replace(/ ?Requires at least one of the following scopes: [a-z_, ]+\./, "")
      .trim();
    return { name: value.name, written: written[1], scopes, summary, deprecated: Boolean(value.isDeprecated) };
  });
  // One topic a line, so that the topics a new version adds or drops are lines of the diff.
  const lines = rows.map((row) => `  ${JSON.stringify(row)}`);
  const body = `{\n "source": ${JSON.stringify(source)},\n "topics": [\n${lines.join(",\n")}\n ]\n}\n`;
  JSON.parse(body);
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, body);
  console.log(`${out}: ${rows.length} topics`);
}

if (isMain(import.meta.url)) {
  const version = versionArg();
  generate(version, await introspection(version));
}
