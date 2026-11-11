// The backend of the app: the routes a React Router app has (its embedded home, its API for the
// front end, webhooks, the app proxy, billing), served by node:http. Each route hands the Web
// `Request` to the template's library (`authenticate.admin`, `authenticate.webhook`,
// `authenticate.public.appProxy`, `billing`), as a React Router loader does, and answers the
// `Response` it returns or throws. No React Router build: the library is the same.
import { createServer } from "node:http";
import { config } from "./config.mjs";
import { RequestedTokenType } from "@shopify/shopify-api";
import { PLAN, oauthApi, sessionStorage, shopify } from "./shopify.mjs";
import * as api from "./api.mjs";
import { embeddedPage } from "./embedded.mjs";

// What the backend saw, for the end-to-end runner (GET /debug/state). A real app has none.
const seen = { webhooks: [], rejectedWebhooks: [], billingReturns: [], welcomes: [], proxied: [], oauth: [], settings: [] };

// The settings of the embedded page's form, kept in memory.
const settings = { threshold: 5, notify: false };

// The headers of a page of the embedded app, as the template's app/entry.server.tsx sets them,
// unchanged: `addDocumentResponseHeaders` lets only Shopify's admin frame the app
// (`Content-Security-Policy: frame-ancestors https://<shop> https://admin.shopify.com …`), and so
// do the pages the library throws (the bounce page, the page of `redirect(…, { target: "_top" })`).
// lsf's admin frames them all the same: its frame goes through a proxy that drops
// `frame-ancestors`.
function documentHeaders(request) {
  const headers = new Headers({ "Content-Type": "text/html; charset=utf-8" });
  shopify.addDocumentResponseHeaders(request, headers);
  return headers;
}

const json = (value, init = {}) =>
  new Response(JSON.stringify(value), {
    ...init,
    headers: { "Content-Type": "application/json", ...init.headers },
  });

// A route of the front end's API: `authenticate.admin`, as a loader calls it, then `call` with the
// admin context and the request, answered as JSON.
const adminApi = (call) => async (request) => {
  const { admin } = await shopify.authenticate.admin(request);
  return json(await call(admin, request));
};
// The same, `call` taking the JSON body of the request.
const adminApiWithBody = (call) => adminApi(async (admin, request) => call(admin, await request.json()));

const routes = [
  // The embedded app's pages, as the admin opens them: authenticate.admin reads the session token
  // of the URL and exchanges it for an access token. One page for every route: the browser
  // switches between them, as a router does (server/embedded.mjs).
  ["GET", /^\/(app(\/(products|settings))?)?$/, async (request) => {
    const { admin } = await shopify.authenticate.admin(request);
    // The welcome link of a plan of managed pricing: Shopify appends plan_handle (and charge_id
    // for the apps enrolled before April 2026).
    const params = new URL(request.url).searchParams;
    if (params.has("plan_handle")) {
      seen.welcomes.push({ path: new URL(request.url).pathname, plan_handle: params.get("plan_handle"), charge_id: params.get("charge_id") });
    }
    const { shop } = await api.gql(admin, `#graphql
      query { shop { name myshopifyDomain } }`);
    const html = embeddedPage({
      apiKey: config.apiKey,
      shop: shop.myshopifyDomain,
      shopName: shop.name,
      appBridgeUrl: config.appBridgeUrl,
      polarisUrl: config.polarisUrl,
      settings,
    });
    return new Response(html, { headers: documentHeaders(request) });
  }],
  // The settings of the embedded page: the front end's fetch() sends the session token.
  ["GET", /^\/api\/settings$/, adminApi(async () => settings)],
  ["POST", /^\/api\/settings$/, adminApiWithBody(async (_admin, body) => {
    settings.threshold = Number(body.threshold ?? settings.threshold);
    settings.notify = Boolean(body.notify);
    seen.settings.push({ ...settings });
    return settings;
  })],
  // The template's auth paths: the bounce page and the exit-iframe page.
  ["GET", /^\/auth\/(session-token|exit-iframe)$/, (request) => shopify.authenticate.admin(request)],

  // The authorization code grant, with @shopify/shopify-api.
  ["GET", /^\/oauth\/install$/, async (request) => {
    const shop = oauthApi.utils.sanitizeShop(new URL(request.url).searchParams.get("shop"), true);
    const response = await oauthApi.auth.begin({ shop, callbackPath: "/auth/callback", isOnline: false, rawRequest: request });
    // The browser goes to the shop's authorization page: on lsf when the backend runs against it.
    if (config.lsfUrl) {
      const authorize = new URL(response.headers.get("location"));
      response.headers.set("location", `${config.lsfUrl}${authorize.pathname}${authorize.search}`);
    }
    return response;
  }],
  ["GET", /^\/auth\/callback$/, async (request) => {
    const { session } = await oauthApi.auth.callback({ rawRequest: request });
    await sessionStorage.storeSession(session);
    const client = new oauthApi.clients.Graphql({ session });
    const { data } = await client.request(`query { shop { name } }`);
    seen.oauth.push({ shop: session.shop, scope: session.scope, token: `${session.accessToken.slice(0, 6)}…`, shopName: data.shop.name });
    return json({ ok: true, shop: session.shop, scope: session.scope, shopName: data.shop.name });
  }],

  // Token exchange without the template: decode the session token, ask for an online access
  // token (or an expiring offline one, then refresh it), and query with it.
  ["POST", /^\/api\/token-exchange$/, async (request) => {
    const { kind } = await request.json();
    const sessionToken = (request.headers.get("authorization") ?? "").replace(/^Bearer /, "");
    const payload = await oauthApi.session.decodeSessionToken(sessionToken);
    const shop = new URL(payload.dest).hostname;
    const { session } = await oauthApi.auth.tokenExchange({
      shop,
      sessionToken,
      requestedTokenType: kind === "online" ? RequestedTokenType.OnlineAccessToken : RequestedTokenType.OfflineAccessToken,
      expiring: kind === "expiring",
    });
    let current = session;
    let refreshed = false;
    if (kind === "expiring") {
      ({ session: current } = await oauthApi.auth.refreshToken({ shop, refreshToken: session.refreshToken }));
      refreshed = current.accessToken !== session.accessToken;
    }
    const client = new oauthApi.clients.Graphql({ session: current });
    const { data } = await client.request(`query { shop { name } }`);
    return json({
      id: current.id,
      isOnline: current.isOnline,
      user: current.onlineAccessInfo?.associated_user?.id ?? null,
      sub: payload.sub,
      expires: current.expires ?? null,
      hasRefreshToken: Boolean(session.refreshToken),
      refreshed,
      shopName: data.shop.name,
    });
  }],

  // The API the app's front end calls with `fetch` (App Bridge adds the session token).
  ["GET", /^\/api\/products$/, adminApi(api.listProducts)],
  ["POST", /^\/api\/products$/, adminApiWithBody(api.createProduct)],
  ["POST", /^\/api\/products\/delete$/, adminApiWithBody(api.deleteProduct)],
  ["POST", /^\/api\/customers$/, adminApiWithBody(api.createCustomer)],
  ["POST", /^\/api\/customers\/update$/, adminApiWithBody(api.updateCustomer)],
  ["POST", /^\/api\/orders$/, adminApiWithBody(api.processOrder)],
  ["POST", /^\/api\/discounts$/, adminApiWithBody(api.createDiscount)],
  ["POST", /^\/api\/bulk$/, adminApi(api.bulkExport)],
  ["POST", /^\/api\/bulk\/import$/, adminApiWithBody(api.bulkImport)],
  ["POST", /^\/api\/files$/, adminApiWithBody((admin, { filename, base64, mimeType }) =>
    api.uploadFile(admin, { filename, mimeType, bytes: Buffer.from(base64, "base64") }))],
  ["POST", /^\/api\/webhooks$/, adminApiWithBody(api.subscribe)],
  ["GET", /^\/api\/cost$/, adminApi(api.queryCost)],

  // Billing: request a plan (the library answers the confirmation URL), then check it.
  ["POST", /^\/api\/billing\/request$/, async (request) => {
    const { billing } = await shopify.authenticate.admin(request);
    await billing.request({ plan: PLAN, isTest: true, returnUrl: `${config.appUrl}/billing/return` });
    return json({ ok: false, error: "billing.request did not redirect" }, { status: 500 });
  }],
  ["GET", /^\/api\/billing\/check$/, async (request) => {
    const { billing } = await shopify.authenticate.admin(request);
    const check = await billing.check({ plans: [PLAN], isTest: true });
    return json(check);
  }],
  // The same request from a document (the loader of a page of the embedded app).
  ["GET", /^\/billing$/, async (request) => {
    const { billing } = await shopify.authenticate.admin(request);
    await billing.require({
      plans: [PLAN],
      isTest: true,
      onFailure: () => billing.request({ plan: PLAN, isTest: true, returnUrl: `${config.appUrl}/billing/return` }),
    });
    return new Response("subscribed");
  }],
  // Managed pricing (Shopify App Pricing): the plan selection page is Shopify's. The template's
  // way to send the merchant there, out of the app's frame.
  ["GET", /^\/app\/plans$/, async (request) => {
    const { redirect } = await shopify.authenticate.admin(request);
    return redirect(`shopify://admin/charges/${config.toml.handle}/pricing_plans`, { target: "_top" });
  }],
  // What the app knows of its plan: its active subscription, in the Admin API.
  ["GET", /^\/api\/billing\/subscriptions$/, adminApi(async (admin) => {
    const { currentAppInstallation } = await api.gql(admin, `#graphql
      query { currentAppInstallation { activeSubscriptions { id name status test trialDays
        lineItems { plan { pricingDetails { ... on AppRecurringPricing { planHandle interval price { amount currencyCode } } } } } } } }`);
    return currentAppInstallation.activeSubscriptions;
  })],
  // What the app knows of its plan under Shopify App Pricing: the Partner API's
  // `activeSubscription` and the history of the shop (`events`), for the app's own GID and the
  // shop's, read from the Admin API.
  ["GET", /^\/api\/billing\/partner$/, adminApi(async (admin) => {
    const { currentAppInstallation, shop } = await api.gql(admin, `#graphql
      query { currentAppInstallation { app { id } } shop { id } }`);
    const answer = await fetch(`${config.partnerApiUrl}/${config.partnerId}/api/2026-10/graphql.json`, {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Shopify-Access-Token": config.partnerToken ?? "" },
      body: JSON.stringify({
        query: `query Plan($appId: ID!, $shopId: ID!) {
          activeSubscription(appId: $appId, shopId: $shopId) {
            trialEndsAt currentBillingCycle { startTime endTime } legacySubscriptionId
            items { handle price { ... on FlatRatePrice { amount currency } } } }
          events(filter: { subjectId: $appId, shopId: $shopId }, first: 10) {
            edges { node { eventType occurredAt ... on SubscriptionStatus { plan { handle } } } } } }`,
        variables: { appId: currentAppInstallation.app.id, shopId: shop.id },
      }),
    });
    const body = await answer.json();
    if (!answer.ok || body.errors) throw new api.AdminError(`Partner API: ${answer.status}`, body.errors);
    return { activeSubscription: body.data.activeSubscription, events: body.data.events.edges.map((edge) => edge.node) };
  })],
  ["GET", /^\/billing\/return$/, async (request) => {
    seen.billingReturns.push(Object.fromEntries(new URL(request.url).searchParams));
    return new Response("<!doctype html><p>Thanks for subscribing.</p>", { headers: { "Content-Type": "text/html" } });
  }],

  // Webhooks, verified by the library (HMAC of the body with the client secret).
  ["POST", /^\/webhooks\//, async (request) => {
    const path = new URL(request.url).pathname;
    let context;
    try {
      context = await shopify.authenticate.webhook(request);
    } catch (error) {
      if (error instanceof Response) {
        seen.rejectedWebhooks.push({ path, status: error.status, topic: request.headers.get("x-shopify-topic") });
      }
      throw error;
    }
    const { topic, shop, payload, webhookId, session, apiVersion } = context;
    seen.webhooks.push({
      path,
      topic,
      shop,
      webhookId,
      apiVersion,
      hasSession: Boolean(session),
      id: payload?.admin_graphql_api_id ?? payload?.app_subscription?.admin_graphql_api_id ?? payload?.id ?? null,
      subscription: payload?.app_subscription
        ? { name: payload.app_subscription.name, status: payload.app_subscription.status, plan_handle: payload.app_subscription.plan_handle }
        : undefined,
      keys: Object.keys(payload ?? {}).slice(0, 12),
      at: Date.now(),
    });
    if (topic === "APP_UNINSTALLED") {
      for (const stored of await sessionStorage.findSessionsByShop(shop)) {
        await sessionStorage.deleteSession(stored.id);
      }
    }
    if (topic === "APP_SCOPES_UPDATE" && session) {
      session.scope = payload.current.join(",");
      await sessionStorage.storeSession(session);
    }
    return new Response();
  }],

  // The app proxy: the storefront forwards /apps/reviews/* here, signed.
  ["GET", /^\/proxy(\/|$)/, async (request) => {
    const { liquid, session, admin } = await shopify.authenticate.public.appProxy(request);
    const url = new URL(request.url);
    let products = "";
    if (admin) {
      const { products: list } = await api.gql(admin, `#graphql
        query { products(first: 2, sortKey: TITLE) { nodes { title } } }`);
      products = list.nodes.map((product) => product.title).join(", ");
    }
    seen.proxied.push({ path: url.pathname, customer: url.searchParams.get("logged_in_customer_id"), hasSession: Boolean(session) });
    if (url.searchParams.get("format") === "json") {
      return json({ path: url.pathname, shop: url.searchParams.get("shop"), products });
    }
    return liquid(
      `<h1 id="proxy">Reviews of {{ shop.name }}</h1><p id="proxy-products">${products}</p>` +
        `<p id="proxy-customer">{{ customer.email | default: "guest" }}</p>` +
        `<p id="proxy-tier">{{ customer.metafields.loyalty.tier }}</p>`,
    );
  }],
  // A review the storefront's form posts through the app proxy: kept as a metaobject of the
  // app, then back to the page, as a reviews app does.
  ["POST", /^\/proxy\/reviews$/, async (request) => {
    const { admin } = await shopify.authenticate.public.appProxy(request);
    const form = await request.formData();
    const review = await api.gql(
      admin,
      `#graphql
      mutation Review($handle: MetaobjectHandleInput!, $metaobject: MetaobjectUpsertInput!) {
        metaobjectUpsert(handle: $handle, metaobject: $metaobject) {
          metaobject { id handle }
          userErrors { field message code }
        }
      }`,
      {
        handle: { type: "$app:review", handle: String(form.get("handle")) },
        metaobject: { fields: [{ key: "title", value: String(form.get("title")) }, { key: "rating", value: String(form.get("rating")) }] },
      },
      { mutation: "metaobjectUpsert" },
    );
    seen.proxied.push({ path: "/proxy/reviews", method: "POST", review: review.metaobject.handle });
    return new Response(null, { status: 302, headers: { Location: "/apps/reviews?posted=1" } });
  }],

  // For the runner only.
  ["GET", /^\/debug\/state$/, async () => json(seen)],
  ["GET", /^\/debug\/sessions$/, async (request) => {
    const shop = new URL(request.url).searchParams.get("shop");
    const sessions = await sessionStorage.findSessionsByShop(shop);
    return json(sessions.map((s) => ({ id: s.id, isOnline: s.isOnline, scope: s.scope, expires: s.expires, token: `${s.accessToken?.slice(0, 6)}…` })));
  }],
  ["POST", /^\/debug\/break-session$/, async (request) => {
    const { shop } = await request.json();
    const [session] = await sessionStorage.findSessionsByShop(shop);
    session.accessToken = "shpat_00000000000000000000000000000000";
    await sessionStorage.storeSession(session);
    return json({ ok: true, id: session.id });
  }],
  ["DELETE", /^\/debug\/state$/, async () => {
    for (const list of Object.values(seen)) list.length = 0;
    return json({ ok: true });
  }],
];

async function handle(request) {
  const { pathname } = new URL(request.url);
  for (const [method, pattern, handler] of routes) {
    if (request.method === method && pattern.test(pathname)) {
      try {
        return await handler(request);
      } catch (error) {
        if (error instanceof Response) return error;
        console.error(`${request.method} ${pathname}: ${error.name}: ${error.message}`);
        return json(
          { error: error.message, name: error.constructor.name, details: error.details ?? error.body ?? error.response ?? null },
          { status: 500 },
        );
      }
    }
  }
  return new Response("Not found", { status: 404 });
}

const server = createServer(async (req, res) => {
  const chunks = [];
  for await (const chunk of req) chunks.push(chunk);
  const body = chunks.length ? Buffer.concat(chunks) : undefined;
  const headers = new Headers();
  for (const [name, value] of Object.entries(req.headers)) {
    headers.set(name, Array.isArray(value) ? value.join(", ") : value);
  }
  const request = new Request(new URL(req.url, config.appUrl), {
    method: req.method,
    headers,
    body: ["GET", "HEAD"].includes(req.method) ? undefined : body,
  });
  const response = await handle(request);
  const outgoing = {};
  response.headers.forEach((value, name) => {
    if (name !== "set-cookie") outgoing[name] = value;
  });
  const cookies = response.headers.getSetCookie();
  if (cookies.length) outgoing["set-cookie"] = cookies;
  res.writeHead(response.status, outgoing);
  res.end(Buffer.from(await response.arrayBuffer()));
});

server.listen(config.port, "127.0.0.1", () => {
  console.log(`backend: ${config.appUrl} (client id ${config.apiKey}${config.lsfUrl ? `, Admin API at ${config.lsfUrl}` : ""})`);
});
