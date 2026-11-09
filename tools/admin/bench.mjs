// Measures the RAM and CPU of a release build of lsf: startup, storefront renders, Admin API
// requests, memory growth under mutations, shops, webhook deliveries, the storefront after a
// mutation. Each suite starts its own server (under `/usr/bin/time -l`, for the max RSS) and
// prints one JSON object per line. CPU is the server's own (`ps`), so a busy machine changes the
// latencies more than the CPU times.
//
//   node tools/admin/bench.mjs <suite> --bin <path to lsf> [--label <name>] [--theme <dir>] [--n <count>]
//
// | Suite | What it measures |
// |---|---|
// | `startup` | time to the `ready:` line, idle RSS (`serve` on the fixture theme, the demo store: the theme alone) |
// | `startup-app` | the same with `examples/admin-backend` and 2026-10,2027-01, then the first request of each version |
// | `storefront` | home, product and collection pages: p50, p95, CPU per render (`--n`, 300) |
// | `storefront-ab` | the same, two builds at once, requests alternated: `--against <lsf> --against-label <name>` |
// | `admin` | first request, `shop`, lists of 50, `productCreate`, `orderCreate`, a bulk query |
// | `growth` | RSS and cost per mutation up to `--n` (10,000) mutations: `--kind update`, `create` or `order` |
// | `shops` | RSS after 10 and 100 shops (`POST /__lsf/admin/shops`) |
// | `webhooks` | a burst of `productUpdate` with and without a subscription to a local sink |
// | `sync` | a mutation then a page (the store rebuilt), before and after 1000 products created |
//
// The suites of the Admin API serve it with an app of the bench's own (`serve` without `--app`
// serves the theme alone): no extension, no webhook, the scopes of the workloads.
//
// macOS only (`/usr/bin/time -l`, `ps`). No dependencies. A build without the Admin API (before
// it) runs `startup`, `storefront` and `storefront-ab`.
import { spawn, execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const args = process.argv.slice(2);
const suite = args[0];
const option = (name, fallback) => {
  const index = args.indexOf(`--${name}`);
  return index === -1 ? fallback : args[index + 1];
};
const bin = option("bin", join(repo, "target", "release", "lsf"));
const label = option("label", "lsf");
const theme = option("theme", join(repo, "crates", "theme", "tests", "fixtures", "theme"));
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const emit = (what, values) => console.log(JSON.stringify({ label, suite, what, ...values }));

// --- the server -------------------------------------------------------------------------------

const startLsf = (extra, options) => startLsfWith(bin, extra, options);

async function startLsfWith(binary, extra, { cwd = repo, env = {} } = {}) {
  const started = performance.now();
  const timer = spawn("/usr/bin/time", ["-l", binary, ...extra], {
    cwd,
    env: { ...process.env, LSF_APP_URL: undefined, LSF_CLIENT_SECRET: undefined, ...env },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let text = "";
  const exited = new Promise((resolve) => timer.on("exit", resolve));
  const ready = await new Promise((resolve, reject) => {
    const onData = (chunk) => {
      text += chunk;
      const url = text.match(/^ready:\s+(http:\/\/127\.0\.0\.1:\d+)\//m);
      if (url) resolve({ url: url[1], ms: performance.now() - started });
    };
    timer.stdout.on("data", onData);
    timer.stderr.on("data", onData);
    timer.on("exit", (code) => reject(new Error(`lsf exited with ${code}:\n${text}`)));
    setTimeout(() => reject(new Error(`lsf did not start in 120 s:\n${text}`)), 120_000).unref();
  });
  // The token line may follow the ready line.
  await sleep(200);
  const token = text.match(/X-Shopify-Access-Token: (shpat_[0-9a-f]+)/)?.[1];
  const pid = Number(execFileSync("pgrep", ["-P", String(timer.pid)]).toString().trim().split("\n")[0]);
  return {
    url: ready.url,
    readyMs: ready.ms,
    token,
    pid,
    log: () => text,
    async stop() {
      process.kill(pid, "SIGTERM");
      await exited;
      const maxRss = Number(text.match(/(\d+)\s+maximum resident set size/)?.[1] ?? NaN);
      return { maxRssMB: round(maxRss / 1048576) };
    },
  };
}

// RSS (MB), %CPU and CPU time (s) of a process.
function ps(pid) {
  const [rss, cpu, time] = execFileSync("ps", ["-o", "rss=,%cpu=,time=", "-p", String(pid)])
    .toString()
    .trim()
    .split(/\s+/);
  const parts = time.split(":").map(Number);
  const seconds = parts.reduce((total, part) => total * 60 + part, 0);
  return { rssMB: round(Number(rss) / 1024), cpuPct: Number(cpu), cpuSec: seconds };
}

const round = (value, digits = 1) => Math.round(value * 10 ** digits) / 10 ** digits;

function stats(samples) {
  const sorted = [...samples].sort((a, b) => a - b);
  const at = (q) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))];
  return {
    n: sorted.length,
    p50: round(at(0.5), 2),
    p95: round(at(0.95), 2),
    max: round(sorted[sorted.length - 1], 2),
    mean: round(sorted.reduce((a, b) => a + b, 0) / sorted.length, 2),
  };
}

// Runs `once` n times in sequence: the latencies (ms) and the server's CPU time per call (ms).
async function measure(server, n, once) {
  const before = ps(server.pid);
  const latencies = [];
  for (let i = 0; i < n; i++) {
    const start = performance.now();
    await once(i);
    latencies.push(performance.now() - start);
  }
  const after = ps(server.pid);
  return {
    ...stats(latencies),
    cpuMsPerCall: round(((after.cpuSec - before.cpuSec) * 1000) / n, 2),
    rssMB: after.rssMB,
  };
}

async function admin(server, query, variables, { version = "2026-10", token = server.token } = {}) {
  const answer = await fetch(`${server.url}/admin/api/${version}/graphql.json`, {
    method: "POST",
    headers: { "Content-Type": "application/json", "X-Shopify-Access-Token": token },
    body: JSON.stringify({ query, variables }),
  });
  const body = await answer.json();
  if (body.errors) throw new Error(`${answer.status} ${JSON.stringify(body.errors).slice(0, 400)}`);
  return body.data;
}

async function page(server, path) {
  const answer = await fetch(`${server.url}${path}`, { headers: { "x-lsf-session": "bench" } });
  await answer.arrayBuffer();
  if (answer.status !== 200) throw new Error(`${path} answered ${answer.status}`);
}

async function pagesOf(server) {
  const status = await (await fetch(`${server.url}/__lsf/status`)).json();
  const routes = (status.routes ?? []).map((route) => (typeof route === "string" ? route : route.path ?? route.url));
  return {
    home: "/",
    product: routes.find((path) => path?.startsWith("/products/")),
    collection: routes.find((path) => path?.startsWith("/collections/") && path !== "/collections/all"),
  };
}

// The storefront alone: what every build serves without `--app`, the theme only since `--only`.
const serveArgs = () => ["serve", "--theme", theme, "--port", "0", "--static"];

// The app the suites of the Admin API serve it with, written once, removed at exit.
let benchApp;
function appDir() {
  if (benchApp) return benchApp;
  benchApp = mkdtempSync(join(tmpdir(), "lsf-bench-app-"));
  writeFileSync(
    join(benchApp, "shopify.app.toml"),
    [
      'client_id = "be5c0000be5c0000be5c0000be5c0000"',
      'name = "Bench"',
      "[access_scopes]",
      'scopes = "read_products,write_products,read_orders,write_orders,read_customers,write_customers,read_inventory,write_inventory,read_locations,read_publications,write_publications"',
      "",
    ].join("\n"),
  );
  process.on("exit", () => rmSync(benchApp, { recursive: true, force: true }));
  return benchApp;
}

// The theme and the Admin API, unthrottled, with the bench's app.
const adminArgs = (more = []) => [...serveArgs(), "--app", appDir(), "--admin-throttle", "off", ...more];

// --- workloads --------------------------------------------------------------------------------

const PRODUCT_CREATE = `mutation($product: ProductCreateInput!) {
  productCreate(product: $product) { product { id variants(first: 1) { nodes { id } } } userErrors { field message } } }`;
const PRODUCT_UPDATE = `mutation($product: ProductUpdateInput!) {
  productUpdate(product: $product) { product { id title } userErrors { field message } } }`;
const ORDER_CREATE = `mutation($order: OrderCreateOrderInput!) {
  orderCreate(order: $order) { order { id } userErrors { field message } } }`;
const PRODUCTS_50 = `{ products(first: 50) { nodes { id title handle vendor tags
  variants(first: 5) { nodes { id sku price inventoryQuantity selectedOptions { name value } } }
  metafields(first: 4) { nodes { namespace key type value } } } } }`;
const ORDERS_50 = `{ orders(first: 50) { nodes { id name email createdAt displayFinancialStatus
  totalPriceSet { shopMoney { amount currencyCode } }
  lineItems(first: 10) { nodes { id title quantity sku } } } } }`;

const BULK_RUN = `mutation($q: String!) {
  bulkOperationRunQuery(query: $q) { bulkOperation { id status } userErrors { message } } }`;
const SUBSCRIBE = `mutation($uri: String!) {
  webhookSubscriptionCreate(topic: PRODUCTS_UPDATE, webhookSubscription: { uri: $uri, format: JSON }) {
    webhookSubscription { id } userErrors { message } } }`;

const orderInput = (i, variantId) => ({
  email: `bench${i}@example.com`,
  financialStatus: "PAID",
  lineItems: [{ variantId, quantity: 1 }],
});

const createProduct = (server, i) =>
  admin(server, PRODUCT_CREATE, {
    product: { title: `Bench ${i}`, vendor: "Bench", productType: "Bench", tags: ["bench"], status: "ACTIVE" },
  });

async function suiteStartup() {
  const server = await startLsf(serveArgs());
  await sleep(2000);
  const idle = ps(server.pid);
  emit("startup", { readyMs: round(server.readyMs), idleRssMB: idle.rssMB, cpuSec: idle.cpuSec });
  emit("max-rss", await server.stop());
}

async function suiteStartupApp() {
  const app = join(repo, "examples", "admin-backend");
  const server = await startLsf(
    ["serve", "--theme", "theme", "--app", ".", "--port", "0", "--static", "--admin-versions", "2026-10,2027-01"],
    { cwd: app },
  );
  await sleep(2000);
  const idle = ps(server.pid);
  emit("startup-app", { readyMs: round(server.readyMs), idleRssMB: idle.rssMB, cpuSec: idle.cpuSec });
  // The first request of each version parses its schema.
  for (const version of ["2026-10", "2027-01"]) {
    const start = performance.now();
    await admin(server, "{ shop { name } }", {}, { version });
    emit("first-request", { version, ms: round(performance.now() - start), rssMB: ps(server.pid).rssMB });
  }
  emit("max-rss", await server.stop());
}

async function suiteStorefront() {
  const n = Number(option("n", 300));
  const server = await startLsf(serveArgs());
  const pages = await pagesOf(server);
  for (const [name, path] of Object.entries(pages)) {
    for (let i = 0; i < 20; i++) await page(server, path);
    emit("render", { page: name, path, ...(await measure(server, n, () => page(server, path))) });
  }
  emit("max-rss", await server.stop());
}

async function suiteAdmin() {
  const server = await startLsf(adminArgs(["--admin-versions", "2026-10,2027-01"]));
  const idle = ps(server.pid);
  let start = performance.now();
  await admin(server, "{ shop { name } }");
  emit("first-request", { ms: round(performance.now() - start), rssBeforeMB: idle.rssMB, rssAfterMB: ps(server.pid).rssMB });
  start = performance.now();
  await admin(server, "{ shop { name } }", {}, { version: "2027-01" });
  emit("first-request-2027-01", { ms: round(performance.now() - start), rssAfterMB: ps(server.pid).rssMB });

  emit("query", { query: "shop { name }", ...(await measure(server, 300, () => admin(server, "{ shop { name } }"))) });
  // Some orders and products first, so that the lists have 50 of each.
  const variant = (await admin(server, "{ productVariants(first: 1) { nodes { id } } }")).productVariants.nodes[0].id;
  emit("mutation", { mutation: "productCreate", ...(await measure(server, 300, (i) => createProduct(server, i))) });
  emit("mutation", {
    mutation: "orderCreate",
    ...(await measure(server, 300, (i) =>
      admin(server, ORDER_CREATE, { order: orderInput(i, variant) }),
    )),
  });
  emit("query", {
    query: "products(first: 50) + variants(5) + metafields(4)",
    ...(await measure(server, 100, () => admin(server, PRODUCTS_50))),
  });
  emit("query", { query: "orders(first: 50) + lineItems(10)", ...(await measure(server, 100, () => admin(server, ORDERS_50))) });

  let bulkInfo;
  const bulkQuery = `{ products { edges { node { id title variants { edges { node { id sku price } } } } } } }`;
  const bulk = await measure(server, 5, async () => {
    const run = await admin(server, BULK_RUN, { q: bulkQuery });
    if (run.bulkOperationRunQuery.userErrors.length) throw new Error(JSON.stringify(run.bulkOperationRunQuery.userErrors));
    const { currentBulkOperation } = await admin(server, "{ currentBulkOperation { status objectCount fileSize url } }");
    const file = await fetch(currentBulkOperation.url);
    await file.arrayBuffer();
    bulkInfo = currentBulkOperation;
  });
  emit("bulk", { objects: bulkInfo.objectCount, fileSize: bulkInfo.fileSize, ...bulk });
  emit("max-rss", await server.stop());
}

// RSS while mutations pile up: creates (the shop grows) and updates of one product (it does not).
async function suiteGrowth() {
  const total = Number(option("n", 10_000));
  const marks = new Set([1000, 2000, 5000, 10_000, 20_000].filter((mark) => mark <= total));
  const server = await startLsf(adminArgs());
  await admin(server, "{ shop { name } }");
  emit("baseline", { pid: server.pid, url: server.url, ...ps(server.pid) });
  const kind = option("kind", "update");
  const product = (await createProduct(server, 0)).productCreate.product.id;
  const variant = (await admin(server, "{ productVariants(first: 1) { nodes { id } } }")).productVariants.nodes[0].id;
  emit("cheap-query-before", await measure(server, 100, () => admin(server, "{ shop { name } }")));
  let start = performance.now();
  let cpu = ps(server.pid).cpuSec;
  let last = 0;
  for (let i = 1; i <= total; i++) {
    if (kind === "update") await admin(server, PRODUCT_UPDATE, { product: { id: product, title: `Bench ${i}`, tags: [`t${i % 7}`] } });
    else if (kind === "order")
      await admin(server, ORDER_CREATE, { order: orderInput(i, variant) });
    else await createProduct(server, i);
    if (marks.has(i)) {
      const now = ps(server.pid);
      const count = i - last;
      emit("after", {
        kind,
        mutations: i,
        ...now,
        msPerMutation: round((performance.now() - start) / count, 2),
        cpuMsPerMutation: round(((now.cpuSec - cpu) * 1000) / count, 2),
      });
      start = performance.now();
      cpu = now.cpuSec;
      last = i;
    }
  }
  // What every request pays now that the shop is bigger.
  emit("cheap-query-after", await measure(server, 100, () => admin(server, "{ shop { name } }")));
  // The storefront after the changes: the first page rebuilds the overlay.
  const pages = await pagesOf(server);
  start = performance.now();
  await page(server, pages.home);
  emit("first-page-after", { ms: round(performance.now() - start), rssMB: ps(server.pid).rssMB });
  emit("render-after", { page: "collection", ...(await measure(server, 50, () => page(server, pages.collection))) });
  const status = await (await fetch(`${server.url}/__lsf/admin`)).json();
  emit("sync", { built: status.storeSync?.built, deliveries: status.webhooks?.deliveries });
  emit("max-rss", await server.stop());
}

async function suiteShops() {
  const server = await startLsf(adminArgs());
  await admin(server, "{ shop { name } }");
  emit("baseline", { pid: server.pid, url: server.url, ...ps(server.pid) });
  let made = 0;
  for (const target of [10, 100]) {
    const timing = await measure(server, target - made, async (i) => {
      const answer = await fetch(`${server.url}/__lsf/admin/shops`, { method: "POST", body: JSON.stringify({ shop: `bench-${made + i}` }) });
      if (answer.status !== 201) throw new Error(`shops answered ${answer.status}`);
    });
    made = target;
    emit("shops", { shops: target, ...timing });
  }
  // A shop is used: a query on the last one.
  const list = await (await fetch(`${server.url}/__lsf/admin`)).json();
  const last = list.shops.find((shop) => shop.shop === "bench-99.myshopify.com");
  const start = performance.now();
  await admin(server, "{ shop { name } }", {}, { token: last.token });
  emit("query-on-new-shop", { ms: round(performance.now() - start), rssMB: ps(server.pid).rssMB });
  emit("max-rss", await server.stop());
}

// A burst of updates with a subscription to a local sink, against the same burst without one.
async function suiteWebhooks() {
  const n = Number(option("n", 500));
  let received = 0;
  const sink = createServer((request, response) => {
    request.resume();
    request.on("end", () => {
      received++;
      response.end("ok");
    });
  });
  await new Promise((resolve) => sink.listen(0, "127.0.0.1", resolve));
  const sinkUrl = `http://127.0.0.1:${sink.address().port}/webhooks`;

  for (const subscribed of [false, true]) {
    const server = await startLsf(adminArgs());
    const product = (await createProduct(server, 0)).productCreate.product.id;
    if (subscribed) {
      const { userErrors } = (await admin(server, SUBSCRIBE, { uri: sinkUrl })).webhookSubscriptionCreate;
      if (userErrors.length) throw new Error(JSON.stringify(userErrors));
    }
    await fetch(`${server.url}/__lsf/webhooks`, { method: "DELETE" });
    received = 0;
    const before = ps(server.pid);
    const start = performance.now();
    const burst = await measure(server, n, (i) => admin(server, PRODUCT_UPDATE, { product: { id: product, title: `Burst ${i}` } }));
    if (subscribed) {
      const waited = await fetch(`${server.url}/__lsf/webhooks?wait=${n}&timeout=120000`);
      await waited.arrayBuffer();
    }
    const after = ps(server.pid);
    emit("burst", {
      subscribed,
      updates: n,
      received,
      totalMs: round(performance.now() - start),
      requestP50: burst.p50,
      cpuMsTotal: round((after.cpuSec - before.cpuSec) * 1000),
      rssMB: after.rssMB,
    });
    emit("max-rss", { subscribed, ...(await server.stop()) });
  }
  sink.close();
}

// The developer's loop: a mutation, then a page, which rebuilds the store with what changed.
async function suiteSync() {
  const n = Number(option("n", 100));
  const server = await startLsf(adminArgs());
  const pages = await pagesOf(server);
  for (let i = 0; i < 20; i++) await page(server, pages.collection);
  emit("page-unchanged", { pid: server.pid, ...(await measure(server, n, () => page(server, pages.collection))) });
  const product = (await createProduct(server, 0)).productCreate.product.id;
  const update = (i) => admin(server, PRODUCT_UPDATE, { product: { id: product, title: `Sync ${i}` } });
  emit("update-alone", { ...(await measure(server, n, (i) => update(i))) });
  let pageMs = [];
  const cycle = await measure(server, n, async (i) => {
    await update(i);
    const start = performance.now();
    await page(server, pages.collection);
    pageMs.push(performance.now() - start);
  });
  emit("update-then-page", { ...cycle, page: stats(pageMs) });
  // A bigger overlay: 1000 products created, then the same loop.
  for (let i = 0; i < 1000; i++) await createProduct(server, i);
  await page(server, pages.collection);
  pageMs = [];
  const bigger = await measure(server, n, async (i) => {
    await update(i);
    const start = performance.now();
    await page(server, pages.collection);
    pageMs.push(performance.now() - start);
  });
  const status = await (await fetch(`${server.url}/__lsf/admin`)).json();
  emit("update-then-page-1000-created", { ...bigger, page: stats(pageMs), built: status.storeSync?.built });
  emit("max-rss", await server.stop());
}

// Two builds at once, their requests alternated (`--bin` then `--against`): the load of the
// machine weighs the same on both.
async function suiteStorefrontAb() {
  const n = Number(option("n", 1000));
  const against = option("against");
  const a = await startLsf(serveArgs());
  const b = await startLsfWith(against, serveArgs());
  const pages = await pagesOf(a);
  for (const [name, path] of Object.entries(pages)) {
    for (let i = 0; i < 50; i++) await Promise.all([page(a, path), page(b, path)]);
    const times = { a: [], b: [] };
    const cpu = { a: ps(a.pid).cpuSec, b: ps(b.pid).cpuSec };
    for (let i = 0; i < n; i++) {
      for (const [key, server] of i % 2 ? [["a", a], ["b", b]] : [["b", b], ["a", a]]) {
        const start = performance.now();
        await page(server, path);
        times[key].push(performance.now() - start);
      }
    }
    for (const [key, server, name2] of [["a", a, label], ["b", b, option("against-label", "against")]]) {
      emit("render-ab", {
        build: name2,
        page: name,
        ...stats(times[key]),
        cpuMsPerCall: round(((ps(server.pid).cpuSec - cpu[key]) * 1000) / n, 3),
      });
    }
  }
  emit("max-rss", { [label]: (await a.stop()).maxRssMB, [option("against-label", "against")]: (await b.stop()).maxRssMB });
}

const suites = {
  "storefront-ab": suiteStorefrontAb,
  sync: suiteSync,
  startup: suiteStartup,
  "startup-app": suiteStartupApp,
  storefront: suiteStorefront,
  admin: suiteAdmin,
  growth: suiteGrowth,
  shops: suiteShops,
  webhooks: suiteWebhooks,
};
if (!suites[suite]) {
  console.error(`usage: node tools/admin/bench.mjs <${Object.keys(suites).join("|")}> --bin <lsf>`);
  process.exit(2);
}
await suites[suite]();
