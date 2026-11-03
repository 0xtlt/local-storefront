// The end-to-end run: starts lsf (with the app, a small theme and the demo store) and the
// backend, plays the merchant and the buyer (the admin's launch, OAuth's browser, the charge
// page, the storefront, the checkout), checks what the backend and lsf answered, prints a table,
// and stops both. Run it with `npm run e2e` (after `cargo build -p lsf-cli`).
//
// Options (environment): LSF_BIN (default: debug/lsf of `CARGO_TARGET_DIR`, where `cargo build`
// put it, else target/debug/lsf of the repository), E2E_VERBOSE=1
// (print the logs of lsf and the backend), E2E_ONLY=<substring> (only the checks whose name has
// it, and those they depend on run anyway).
import { spawn, spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { createServer } from "node:net";
import { join, resolve } from "node:path";
import { createAdminApiClient } from "@shopify/admin-api-client";
import { appRoot, config } from "../server/config.mjs";
import { lsfFetch } from "../server/lsf.mjs";
import { Browser } from "./browser.mjs";

const repoRoot = join(appRoot, "..", "..");
const targetDir = process.env.CARGO_TARGET_DIR
  ? resolve(process.env.CARGO_TARGET_DIR)
  : join(repoRoot, "target");
const lsfBin = process.env.LSF_BIN ?? join(targetDir, "debug", "lsf");
const verbose = Boolean(process.env.E2E_VERBOSE);
const SHOP = "local-supply-co.myshopify.com";

// Known gaps: checks that fail for a reason recorded in the README ("What does not work yet").
// They run, and show as KNOWN rather than FAIL while they fail; a known check that passes is
// reported, so that the list stays true.
const KNOWN = {
  "billing: billing.require from a page (exit-iframe)":
    "the template's exit-iframe page only redirects to https admin URLs, lsf's charge page is http://127.0.0.1",
};

function freePort() {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.unref();
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

function start(name, command, args, options, ready) {
  const child = spawn(command, args, { ...options, stdio: ["ignore", "pipe", "pipe"] });
  const log = [];
  return new Promise((resolve, reject) => {
    let done = false;
    const onData = (chunk) => {
      const text = chunk.toString();
      log.push(text);
      if (verbose) process.stderr.write(`[${name}] ${text}`);
      const match = !done && ready(log.join(""));
      if (match) {
        done = true;
        resolve({ child, log, match });
      }
    };
    child.stdout.on("data", onData);
    child.stderr.on("data", onData);
    child.on("exit", (code) => {
      if (!done) reject(new Error(`${name} exited with ${code}:\n${log.join("")}`));
    });
    setTimeout(() => {
      if (!done) reject(new Error(`${name} did not start in 60 s:\n${log.join("")}`));
    }, 60_000).unref();
  });
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

function assert(condition, message, details) {
  if (!condition) {
    const error = new Error(message);
    error.details = details;
    throw error;
  }
}

async function main() {
  if (!existsSync(lsfBin)) {
    console.error(`lsf is not built: ${lsfBin}\nRun \`cargo build -p lsf-cli\` first, or set LSF_BIN.`);
    process.exit(2);
  }
  const backendPort = await freePort();
  const appUrl = `http://localhost:${backendPort}`;
  const lsfProcess = await start(
    "lsf",
    lsfBin,
    ["serve", "--theme", "theme", "--app", ".", "--app-url", appUrl, "--port", "0", "--static"],
    { cwd: appRoot, env: { ...process.env, LSF_APP_URL: undefined, LSF_CLIENT_SECRET: undefined } },
    // The banner's token line and its `ready:` line may come on different streams, in either
    // order: wait for both.
    (text) => {
      const ready = text.match(/^ready:\s+(http:\/\/127\.0\.0\.1:\d+)\//m);
      const token = text.match(/X-Shopify-Access-Token: (shpat_[0-9a-f]+)/);
      return ready && token && { lsf: ready[1], token: token[1] };
    },
  );
  const { lsf, token } = lsfProcess.match;
  let backendProcess;
  const stop = () => {
    for (const child of [backendProcess?.child, lsfProcess.child]) child?.kill("SIGTERM");
  };
  process.on("SIGINT", () => {
    stop();
    process.exit(130);
  });
  try {
    backendProcess = await start(
      "backend",
      process.execPath,
      ["server/index.mjs"],
      { cwd: appRoot, env: { ...process.env, PORT: String(backendPort), LSF_URL: lsf, SHOPIFY_APP_URL: appUrl } },
      (text) => text.includes("backend: "),
    );
    const results = await runChecks({ lsf, appUrl, token });
    const failed = report(results);
    if (failed && !verbose) {
      console.log("\nThe last lines of the backend's log:\n" + backendProcess.log.join("").split("\n").slice(-25).join("\n"));
    }
    process.exitCode = failed ? 1 : 0;
  } finally {
    stop();
  }
}

// The checks, in order: later ones use what earlier ones made (`ctx`).
async function runChecks({ lsf, appUrl, token }) {
  const browser = new Browser();
  const ctx = {};
  const results = [];
  const only = process.env.E2E_ONLY;

  const sessionToken = async (shop = SHOP) => {
    const answer = await fetch(`${lsf}/__lsf/admin/session-token`, { method: "POST", body: JSON.stringify({ shop }) });
    return (await answer.json()).token;
  };
  // A call of the app's front end: fetch with the session token App Bridge adds.
  const app = async (method, path, body, { shop } = {}) => {
    const answer = await browser.fetch(`${appUrl}${path}`, {
      method,
      headers: { Authorization: `Bearer ${await sessionToken(shop)}`, "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await answer.text();
    let value;
    try {
      value = JSON.parse(text);
    } catch {
      value = text;
    }
    return { status: answer.status, headers: answer.headers, body: value };
  };
  const appOk = async (method, path, body, options) => {
    const answer = await app(method, path, body, options);
    assert(answer.status === 200, `${method} ${path} answered ${answer.status}`, answer.body);
    return answer.body;
  };
  const admin = async (query, variables, accessToken = token) => {
    const answer = await fetch(`${lsf}/admin/api/2026-10/graphql.json`, {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Shopify-Access-Token": accessToken },
      body: JSON.stringify({ query, variables }),
    });
    return answer.json();
  };
  const backendState = async () => (await fetch(`${appUrl}/debug/state`)).json();
  // A shop of its own (`POST /__lsf/admin/shops`): its domain and its token.
  const newShop = async (name) =>
    (await fetch(`${lsf}/__lsf/admin/shops`, { method: "POST", body: JSON.stringify({ shop: `${name}-${Date.now()}` }) })).json();
  // Waits until the backend accepted a webhook of `topic` that `match` keeps.
  const received = async (topic, match = () => true, timeout = 8000) => {
    const deadline = Date.now() + timeout;
    while (Date.now() < deadline) {
      const state = await backendState();
      const found = state.webhooks.find((webhook) => webhook.topic === topic && match(webhook));
      if (found) return found;
      await sleep(100);
    }
    const state = await backendState();
    const deliveries = await (await fetch(`${lsf}/__lsf/webhooks?topic=${topic.toLowerCase().replace("_", "/")}`)).json();
    throw Object.assign(new Error(`no ${topic} webhook accepted by the backend`), {
      details: { accepted: state.webhooks.map((w) => w.topic), rejected: state.rejectedWebhooks, deliveries },
    });
  };

  const check = async (family, name, run) => {
    if (only && !name.includes(only) && !family.includes(only)) return;
    const started = Date.now();
    let status = "PASS";
    let detail = "";
    try {
      detail = (await run()) ?? "";
    } catch (error) {
      status = "FAIL";
      detail = error.message;
      if (verbose && error.details) console.error(name, JSON.stringify(error.details, null, 2));
      if (!verbose && error.details) detail += ` ${JSON.stringify(error.details).slice(0, 300)}`;
    }
    if (KNOWN[name]) {
      status = status === "FAIL" ? "KNOWN" : "FIXED?";
      detail = status === "KNOWN" ? KNOWN[name] : `passes now: remove it from KNOWN (${detail})`;
    }
    results.push({ family, name, status, detail, ms: Date.now() - started });
  };

  // Auth.
  await check("auth", "auth: embedded launch → authenticate.admin (token exchange)", async () => {
    const launch = await browser.fetch(`${lsf}/__lsf/admin/launch?shop=${SHOP}`);
    assert(launch.status === 302, `launch answered ${launch.status}`);
    const page = await browser.fetch(launch.headers.get("location"));
    const html = await page.text();
    assert(page.status === 200, `the app's home answered ${page.status}`, html.slice(0, 300));
    assert(html.includes(`data-shop="${SHOP}"`), "the home page did not query the shop", html.slice(0, 300));
    const sessions = await (await fetch(`${appUrl}/debug/sessions?shop=${SHOP}`)).json();
    assert(sessions.some((session) => !session.isOnline), "no offline session stored", sessions);
    return `home page rendered "${html.match(/>([^<]+)<\/h1>/)[1]}", offline session stored`;
  });
  await check("auth", "auth: OAuth authorize → callback → access token", async () => {
    const begin = await browser.fetch(`${appUrl}/oauth/install?shop=${SHOP}`);
    assert(begin.status === 302, `auth.begin answered ${begin.status}`);
    const authorize = new URL(begin.headers.get("location"));
    assert(authorize.origin === lsf && authorize.pathname === "/admin/oauth/authorize", `auth.begin redirected to ${authorize}`);
    const approved = await browser.fetch(authorize);
    assert(approved.status === 302, `authorize answered ${approved.status}`, await approved.text());
    const callback = await browser.fetch(approved.headers.get("location"));
    const body = await callback.json();
    assert(callback.status === 200 && body.ok, `auth.callback answered ${callback.status}`, body);
    return `token for ${body.shop}, scope ${body.scope.split(",").length} scopes, shop "${body.shopName}"`;
  });
  await check("auth", "auth: decodeSessionToken + tokenExchange (online)", async () => {
    const answer = await appOk("POST", "/api/token-exchange", { kind: "online" });
    assert(answer.isOnline && answer.user === Number(answer.sub), "not an online session of the token's user", answer);
    return `online session of user ${answer.user}, expires ${answer.expires}`;
  });
  await check("auth", "auth: expiring offline token → refreshToken", async () => {
    const answer = await appOk("POST", "/api/token-exchange", { kind: "expiring" });
    assert(answer.hasRefreshToken && answer.refreshed && answer.expires, "no refresh", answer);
    return `expires ${answer.expires}, refreshed, query ok`;
  });
  await check("auth", "auth: bad session token → 401 + retry header", async () => {
    const answer = await browser.fetch(`${appUrl}/api/cost`, { headers: { Authorization: "Bearer not-a-jwt" } });
    assert(answer.status === 401, `answered ${answer.status}`);
    assert(answer.headers.get("x-shopify-retry-invalid-session-request") === "1", "no retry header");
    return "401, X-Shopify-Retry-Invalid-Session-Request: 1";
  });
  await check("auth", "auth: revoked access token → 401, then a new exchange", async () => {
    await fetch(`${appUrl}/debug/break-session`, { method: "POST", body: JSON.stringify({ shop: SHOP }) });
    const first = await app("GET", "/api/cost");
    assert(first.status === 401, `with a bad stored token: ${first.status}`, first.body);
    const second = await app("GET", "/api/cost");
    assert(second.status === 200, `after the library dropped the session: ${second.status}`, second.body);
    return "401 (session invalidated), then 200 after token exchange";
  });

  // Errors and cost, through the libraries.
  await check("errors", "errors: bad token with @shopify/admin-api-client", async () => {
    const client = createAdminApiClient({
      storeDomain: SHOP,
      apiVersion: "2026-10",
      accessToken: "shpat_00000000000000000000000000000000",
      customFetchApi: lsfFetch(lsf),
    });
    const { errors } = await client.request(`query { shop { name } }`);
    assert(errors?.networkStatusCode === 401, "not a 401", errors);
    return `${errors.networkStatusCode} "${errors.message}"`;
  });
  await check("errors", "errors: query cost in extensions.cost", async () => {
    const { cost } = await appOk("GET", "/api/cost");
    assert(cost?.requestedQueryCost > 0 && cost.throttleStatus?.maximumAvailable > 0, "no cost", cost);
    return `requested ${cost.requestedQueryCost}, actual ${cost.actualQueryCost}, ${cost.throttleStatus.currentlyAvailable}/${cost.throttleStatus.maximumAvailable}`;
  });
  await check("errors", "errors: MAX_COST_EXCEEDED and THROTTLED", async () => {
    const shop = await newShop("throttle");
    const client = createAdminApiClient({ storeDomain: shop.shop, apiVersion: "2026-10", accessToken: shop.token, customFetchApi: lsfFetch(lsf) });
    const tooBig = await client.request(`query { products(first: 250) { nodes { variants(first: 3) { nodes { id } } } } }`);
    const maxCost = tooBig.errors?.graphQLErrors?.[0]?.extensions?.code;
    assert(maxCost === "MAX_COST_EXCEEDED", "not MAX_COST_EXCEEDED", tooBig.errors);
    // A query that asks for 998 points and costs what the demo store returns: the bucket takes
    // the requested cost, then gives back the rest. It runs out once less than 998 is left.
    const heavy = `query { products(first: 166) { nodes { variants(first: 3) { nodes { id } } } } }`;
    let sent = 0;
    let throttled;
    while (!throttled && sent < 5000) {
      const answers = await Promise.all(Array.from({ length: 50 }, () => client.request(heavy)));
      sent += answers.length;
      throttled = answers.find((answer) => answer.errors?.graphQLErrors?.[0]?.extensions?.code === "THROTTLED");
    }
    assert(throttled, `never THROTTLED in ${sent} requests`);
    const status = throttled.extensions?.cost;
    assert(status?.requestedQueryCost === 998 && status.actualQueryCost === null, "the cost of a throttled answer", status);
    return `MAX_COST_EXCEEDED; THROTTLED after ${sent} requests (${status.throttleStatus.currentlyAvailable}/${status.throttleStatus.maximumAvailable} left)`;
  });
  await check("errors", "errors: fewer scopes → ACCESS_DENIED, app/scopes_update", async () => {
    await fetch(`${lsf}/__lsf/admin/install`, { method: "POST", body: JSON.stringify({ scopes: "read_products" }) });
    await received("APP_SCOPES_UPDATE");
    const products = await app("GET", "/api/products");
    const customers = await app("POST", "/api/customers", { email: "denied@example.com" });
    await fetch(`${lsf}/__lsf/admin/install`, { method: "POST", body: JSON.stringify({}) });
    assert(products.status === 200, `products with read_products: ${products.status}`, products.body);
    const denied = customers.body?.details?.errors?.graphQLErrors?.[0];
    assert(customers.status === 500 && denied?.extensions?.code === "ACCESS_DENIED", "not ACCESS_DENIED", customers.body);
    return `products 200; customerCreate: "${denied.message}"`;
  });

  // Admin GraphQL, family by family.
  await check("catalog", "catalog: products with pagination", async () => {
    const answer = await appOk("GET", "/api/products");
    assert(answer.pages > 1 && answer.count === answer.productsCount, "pages do not add up", answer);
    return `${answer.count} products in ${answer.pages} pages, productsCount ${answer.productsCount}`;
  });
  await check("catalog", "catalog: productCreate, productUpdate, inventorySetQuantities, publish", async () => {
    ctx.product = await appOk("POST", "/api/products", { title: "E2E Mug", tag: "e2e-sale", price: "25.00", quantity: 7 });
    const { product } = ctx.product;
    assert(product.title === "E2E Mug (updated)" && product.totalInventory === 7 && product.status === "ACTIVE", "not as made", product);
    return `${product.handle}: ${product.variants.nodes[0].price}, ${product.totalInventory} in stock, ${product.onlineStoreUrl}`;
  });
  await check("catalog", "catalog: webhooks products/create and products/update (toml, HMAC checked)", async () => {
    const id = ctx.product.product.id;
    await received("PRODUCTS_CREATE", (webhook) => webhook.id === id);
    await received("PRODUCTS_UPDATE", (webhook) => webhook.id === id);
    return "both accepted by authenticate.webhook";
  });
  await check("catalog", "storefront: the new product's page and .js", async () => {
    const { handle } = ctx.product.product;
    const html = await (await fetch(`${lsf}/products/${handle}`)).text();
    const read = (id) => html.match(new RegExp(`id="${id}">([^<]*)<`))?.[1];
    assert(read("product-title") === "E2E Mug (updated)", "title", read("product-title"));
    assert(read("product-price") === "$25.00", "price", read("product-price"));
    assert(read("product-inventory") === "7", "inventory", read("product-inventory"));
    const js = await (await fetch(`${lsf}/products/${handle}.js`)).json();
    assert(js.available && js.price === 2500 && js.tags.includes("e2e-sale"), "product.js", js);
    const home = await (await fetch(`${lsf}/`)).text();
    assert(home.includes(`/products/${handle}`), "not in collections.all");
    return `page: "${read("product-title")}" ${read("product-price")}, ${read("product-inventory")} left; in collections.all`;
  });
  await check("webhooks", "webhooks: registerWebhooks (afterAuth) → products/delete", async () => {
    const { data } = await admin(`query { webhookSubscriptions(first: 10) { nodes { topic uri } } }`);
    const registered = data.webhookSubscriptions.nodes.find((s) => s.topic === "PRODUCTS_DELETE");
    assert(registered?.uri === `${appUrl}/webhooks/products`, "afterAuth did not register PRODUCTS_DELETE", data);
    const made = await appOk("POST", "/api/products", { title: "E2E Doomed", tag: "e2e", price: "1.00", quantity: 1 });
    const answer = await appOk("POST", "/api/products/delete", { id: made.product.id });
    const webhook = await received("PRODUCTS_DELETE", (w) => String(w.id) === made.product.id.split("/").pop());
    const page = await fetch(`${lsf}/products/${made.product.handle}`);
    assert(page.status === 404, `the deleted product's page answered ${page.status}`);
    return `${answer.deletedProductId} → ${webhook.path}; its page 404`;
  });
  await check("customers", "customers: customerCreate, metafieldsSet, metaobjectUpsert", async () => {
    ctx.customer = await appOk("POST", "/api/customers", { email: "ada@example.com" });
    const { customer, metaobject } = ctx.customer;
    assert(customer.metafield?.value === "gold", "metafield", customer);
    assert(metaobject.handle === "first-review" && metaobject.displayName === "Works offline", "metaobject", metaobject);
    return `${customer.email} tier ${customer.metafield.value}; ${metaobject.type}/${metaobject.handle}`;
  });
  await check("webhooks", "webhooks: webhookSubscriptionCreate → customers/update", async () => {
    const answer = await appOk("POST", "/api/webhooks", { topic: "CUSTOMERS_UPDATE", uri: `${appUrl}/webhooks/customers` });
    assert(answer.listed.some((s) => s.id === answer.subscription.id), "not listed", answer);
    await appOk("POST", "/api/customers/update", { id: ctx.customer.customer.id, note: "VIP" });
    const webhook = await received("CUSTOMERS_UPDATE", (w) => w.id === ctx.customer.customer.id);
    return `${answer.subscription.id} → ${webhook.path}`;
  });
  await check("discounts", "discounts: discountAutomaticAppCreate for the Function", async () => {
    ctx.discount = await appOk("POST", "/api/discounts", { tag: "e2e-sale", percentage: 20 });
    assert(ctx.discount.discount.status === "ACTIVE", "not active", ctx.discount);
    return `${ctx.discount.discount.title} (${ctx.discount.function.handle}, ${ctx.discount.discount.discountId})`;
  });
  await check("orders", "checkout: an order of the new product, with the app discount", async () => {
    const buyer = new Browser();
    const variantId = Number(ctx.product.product.variants.nodes[0].id.split("/").pop());
    const added = await buyer.fetch(`${lsf}/cart/add.js`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ items: [{ id: variantId, quantity: 2 }] }),
    });
    assert(added.status === 200, `cart/add.js answered ${added.status}`);
    const checkout = await buyer.fetch(`${lsf}/checkout`);
    const path = new URL(checkout.headers.get("location"), lsf).pathname;
    const update = await buyer.fetch(`${lsf}${path}/update`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        operations: [
          { type: "email", value: "ada@example.com" },
          {
            type: "shippingAddress",
            address: { firstName: "Ada", lastName: "Lovelace", address1: "1 Main St", city: "Ottawa", countryCode: "CA", provinceCode: "ON", zip: "K1A 0B1" },
          },
          { type: "paymentMethod", handle: "card" },
        ],
      }),
    });
    const state = await update.json();
    assert(state.cost?.discounts?.amount === 10, "the Function's discount is not applied", state.cost);
    const complete = await buyer.fetch(`${lsf}${path}/complete`, { method: "POST" });
    assert(complete.status === 200, `complete answered ${complete.status}`, await complete.text());
    const webhook = await received("ORDERS_CREATE", (w) => w.at > Date.now() - 10_000);
    ctx.checkoutOrder = webhook.id;
    return `${webhook.id}: 2 × $25 − $${state.cost.discounts.amount} + shipping = $${state.cost.total.amount}; orders/create → ${webhook.path}`;
  });
  await check("orders", "orders: fulfillmentCreate and refundCreate on the checkout's order", async () => {
    const answer = await appOk("POST", "/api/orders", { orderId: ctx.checkoutOrder });
    assert(answer.fulfillment.status === "SUCCESS", "fulfillment", answer);
    assert(answer.refund.totalRefundedSet.shopMoney.amount === "20.0", "refund of one discounted unit", answer.refund);
    assert(answer.financialStatusAfterRefund === "PARTIALLY_REFUNDED", "financial status", answer);
    return `${answer.order.name}: fulfilled, ${answer.refund.totalRefundedSet.shopMoney.amount} refunded, ${answer.financialStatusAfterRefund}`;
  });
  await check("orders", "orders: orderCreate, fulfillmentCreate, refundCreate", async () => {
    const answer = await appOk("POST", "/api/orders", { variantId: ctx.product.product.variants.nodes[0].id, email: "ada@example.com" });
    assert(answer.fulfillment.status === "SUCCESS" && answer.refund.totalRefundedSet.shopMoney.amount === "25.0", "not as expected", answer);
    return `${answer.order.name}: fulfilled, 25.0 refunded, ${answer.financialStatusAfterRefund}`;
  });
  await check("orders", "storefront: the stock orderCreate took (the checkout's order takes none)", async () => {
    const html = await (await fetch(`${lsf}/products/${ctx.product.product.handle}`)).text();
    const left = html.match(/id="product-inventory">([^<]*)</)?.[1];
    assert(left === "5", `the page says ${left} left, not 7 − 2`);
    return "7 − 2 = 5 on the product page";
  });
  await check("webhooks", "webhooks: POST /__lsf/webhooks/trigger orders/create of an order", async () => {
    const answer = await (await fetch(`${lsf}/__lsf/webhooks/trigger`, {
      method: "POST",
      body: JSON.stringify({ topic: "orders/create", id: ctx.checkoutOrder, wait: true }),
    })).json();
    assert(answer.deliveries?.[0]?.state === "delivered", "not delivered", answer);
    return `${answer.deliveries[0].url}: ${answer.deliveries[0].attempts?.[0]?.status}`;
  });

  // The app: billing, bulk operations, files.
  await check("billing", "billing: billing.request → approval page → returnUrl → billing.check", async () => {
    const requested = await app("POST", "/api/billing/request");
    const confirmation = requested.headers.get("x-shopify-api-request-failure-reauthorize-url");
    assert(requested.status === 401 && confirmation, `billing.request answered ${requested.status}`, requested.body);
    const page = await browser.fetch(confirmation);
    const html = await page.text();
    const action = html.match(/<form method="post" action="([^"]*\/approve[^"]*)"/)?.[1];
    assert(action, "no Approve form on the charge page", html.slice(0, 300));
    const approved = await browser.fetch(new URL(action, confirmation), {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ shop: SHOP }),
    });
    assert(approved.status === 303, `approve answered ${approved.status}`);
    const back = await browser.fetch(approved.headers.get("location"));
    assert(back.status === 200, `the return URL answered ${back.status}`);
    const check = await appOk("GET", "/api/billing/check");
    assert(check.hasActivePayment && check.appSubscriptions[0].status === "ACTIVE", "not active", check);
    const returned = (await backendState()).billingReturns.at(-1);
    return `charge_id=${returned?.charge_id}, ${check.appSubscriptions[0].name} ${check.appSubscriptions[0].status}, trial ${check.appSubscriptions[0].trialDays} days`;
  });
  await check("billing", "billing: billing.require from a page (exit-iframe)", async () => {
    const { shop } = await newShop("billing");
    const launch = await browser.fetch(`${lsf}/__lsf/admin/launch?shop=${shop}&path=/billing`);
    const page = await browser.fetch(launch.headers.get("location"));
    assert(page.status === 302, `the loader answered ${page.status}`);
    const exit = await browser.fetch(new URL(page.headers.get("location"), appUrl));
    const html = await exit.text();
    assert(exit.status === 200 && html.includes("/__lsf/admin/charges/"), `exit-iframe answered ${exit.status}`, html.slice(0, 200));
    return "exit-iframe page opens the charge page";
  });
  await check("bulk", "bulk: bulkOperationRunQuery → poll → JSONL", async () => {
    const answer = await appOk("POST", "/api/bulk");
    assert(answer.operation.status === "COMPLETED" && answer.lines === Number(answer.operation.objectCount), "not complete", answer);
    assert(answer.products === (ctx.product ? 9 : 8), "products", answer);
    return `${answer.initialStatus} → ${answer.operation.status}: ${answer.products} products, ${answer.variants} variants (__parentId)`;
  });
  await check("bulk", "bulk: staged JSONL → bulkOperationRunMutation (productCreate)", async () => {
    const answer = await appOk("POST", "/api/bulk/import", { titles: ["E2E Bulk One", "E2E Bulk Two"] });
    const created = answer.results.map((line) => line.data?.productCreate?.product?.title);
    assert(answer.operation.type === "MUTATION" && created.join() === "E2E Bulk One,E2E Bulk Two", "not both created", answer);
    const { data } = await admin(`query { productsCount(query: "tag:e2e-bulk") { count } }`);
    assert(data.productsCount.count === 2, "the products are not in the shop", data);
    return `${answer.operation.objectCount} lines, each {data, __lineNumber}; 2 products tagged e2e-bulk`;
  });
  await check("files", "files: stagedUploadsCreate → multipart POST → fileCreate", async () => {
    const png = readFileSync(join(appRoot, "e2e", "pixel.png"));
    const answer = await appOk("POST", "/api/files", { filename: "pixel.png", mimeType: "image/png", base64: png.toString("base64") });
    assert(answer.file.fileStatus === "READY" && answer.sameBytes, "not ready, or other bytes", answer);
    return `${answer.file.id} READY, ${png.length} bytes back from ${new URL(answer.file.image.url).pathname.slice(0, 40)}…`;
  });

  // Webhooks from the command line and the control API.
  await check("webhooks", "webhooks: bad HMAC → 401 from authenticate.webhook", async () => {
    const answer = await fetch(`${appUrl}/webhooks/products`, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Shopify-Topic": "products/update",
        "X-Shopify-Hmac-Sha256": "bm90IHRoZSBobWFj",
        "X-Shopify-Shop-Domain": SHOP,
        "X-Shopify-API-Version": "2026-10",
        "X-Shopify-Webhook-Id": "forged",
      },
      body: "{}",
    });
    assert(answer.status === 401, `answered ${answer.status}`);
    return "401";
  });
  for (const topic of ["customers/data_request", "customers/redact", "shop/redact"]) {
    await check("webhooks", `webhooks: lsf app webhook trigger ${topic} (compliance)`, async () => {
      const run = spawnSync(lsfBin, ["app", "webhook", "trigger", "--topic", topic, "--app", ".", "--app-url", appUrl, "--json"], {
        cwd: appRoot,
        encoding: "utf8",
        env: { ...process.env, LSF_APP_URL: undefined },
      });
      assert(run.status === 0, `exit ${run.status}`, run.stderr || run.stdout);
      const enumName = topic.toUpperCase().replace(/[/.]/g, "_");
      const webhook = await received(enumName, (w) => w.at > Date.now() - 10_000);
      return `exit 0 → ${webhook.path}, HMAC checked`;
    });
  }
  await check("webhooks", "webhooks: uninstall → app/uninstalled, reinstall", async () => {
    await fetch(`${lsf}/__lsf/admin/uninstall`, { method: "POST", body: "{}" });
    await received("APP_UNINSTALLED", (w) => w.at > Date.now() - 10_000);
    const revoked = await admin(`query { shop { name } }`);
    assert(revoked.errors, "the default token still works", revoked);
    const sessions = await (await fetch(`${appUrl}/debug/sessions?shop=${SHOP}`)).json();
    assert(sessions.length === 0, "the backend kept its sessions", sessions);
    await fetch(`${lsf}/__lsf/admin/install`, { method: "POST", body: "{}" });
    const again = await app("GET", "/api/cost");
    assert(again.status === 200, `after reinstall: ${again.status}`, again.body);
    return "token revoked, sessions deleted, token exchange again after install";
  });

  // App proxy.
  await check("proxy", "proxy: /apps/reviews through the storefront (appProxy, liquid)", async () => {
    const page = await fetch(`${lsf}/apps/reviews/product/1?page=2`);
    const html = await page.text();
    assert(page.status === 200, `answered ${page.status}`, html.slice(0, 300));
    assert(html.includes('<header id="shop-name">') && html.includes("Reviews of Local Supply Co."), "not rendered in the theme", html.slice(0, 400));
    assert(/id="proxy-products">[^<]+</.test(html), "the proxy's admin context did not query", html.slice(0, 400));
    const proxied = (await backendState()).proxied.at(-1);
    return `200 in layout/theme.liquid, ${proxied.path}, admin context from the offline session`;
  });
  await check("proxy", "proxy: logged-in customer", async () => {
    const buyer = new Browser();
    await buyer.fetch(`${lsf}/__lsf/login?customer=ada@example.com&return_to=/`);
    const html = await (await buyer.fetch(`${lsf}/apps/reviews`)).text();
    const shown = html.match(/id="proxy-customer">([^<]*)</)?.[1];
    assert(shown === "ada@example.com", "not the customer the Admin API created", shown);
    const tier = html.match(/id="proxy-tier">([^<]*)</)?.[1];
    assert(tier === "gold", "customer.metafields.loyalty.tier", tier);
    const proxied = (await backendState()).proxied.at(-1);
    return `customer ${shown} (tier ${tier} from metafieldsSet), logged_in_customer_id=${proxied.customer}`;
  });
  await check("proxy", "proxy: form POST → metaobjectUpsert → redirect", async () => {
    const posted = await fetch(`${lsf}/apps/reviews/reviews`, {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({ handle: "from-the-form", title: "Lovely", rating: "4" }),
      redirect: "manual",
    });
    assert(posted.status === 302 && posted.headers.get("location") === "/apps/reviews?posted=1", `answered ${posted.status}`, await posted.text());
    const { data } = await admin(`query { metaobjectByHandle(handle: {type: "$app:review", handle: "from-the-form"}) { field(key: "rating") { value } } }`);
    assert(data?.metaobjectByHandle?.field?.value === "4", "the review was not kept", data);
    return "302 back to the page; the review is a metaobject of the app";
  });
  await check("proxy", "proxy: forged signature → 400", async () => {
    const answer = await fetch(`${appUrl}/proxy?shop=${SHOP}&timestamp=${Math.floor(Date.now() / 1000)}&signature=00`);
    assert(answer.status === 400, `answered ${answer.status}`);
    return "400";
  });

  return results;
}

function report(results) {
  const widths = [8, Math.max(...results.map((r) => r.name.length), 10)];
  console.log("");
  console.log(`${"result".padEnd(widths[0])}  ${"check".padEnd(widths[1])}  detail`);
  console.log(`${"-".repeat(widths[0])}  ${"-".repeat(widths[1])}  ${"-".repeat(40)}`);
  for (const result of results) {
    console.log(`${result.status.padEnd(widths[0])}  ${result.name.padEnd(widths[1])}  ${result.detail}`);
  }
  const count = (status) => results.filter((r) => r.status === status).length;
  console.log(`\n${count("PASS")} passed, ${count("FAIL")} failed, ${count("KNOWN")} known gaps${count("FIXED?") ? `, ${count("FIXED?")} known gaps that pass` : ""}`);
  return count("FAIL") + count("FIXED?");
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
