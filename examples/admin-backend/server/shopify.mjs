// The app's Shopify objects, as the React Router template's app/shopify.server.ts makes them.
import "@shopify/shopify-app-react-router/adapters/node";
import { ApiVersion, BillingInterval, DeliveryMethod, shopifyApp } from "@shopify/shopify-app-react-router/server";
import { shopifyApi } from "@shopify/shopify-api";
import { MemorySessionStorage } from "@shopify/shopify-app-session-storage-memory";
import { config } from "./config.mjs";
import { pointAtLsf } from "./lsf.mjs";

// After the imports: the adapters set the runtime's fetch when they load.
pointAtLsf(config.lsfUrl);

export const PLAN = "Pro";

export const sessionStorage = new MemorySessionStorage();

export const shopify = shopifyApp({
  apiKey: config.apiKey,
  apiSecretKey: config.apiSecretKey,
  apiVersion: ApiVersion.October26,
  scopes: config.scopes,
  appUrl: config.appUrl,
  authPathPrefix: "/auth",
  sessionStorage,
  // What the pages' `Link` header preloads: the Polaris AppProvider loads (unset: the CDN's).
  polarisUrl: config.polarisUrl,
  // A subscription the app makes when it is installed, besides those of shopify.app.toml.
  webhooks: {
    PRODUCTS_DELETE: { deliveryMethod: DeliveryMethod.Http, callbackUrl: "/webhooks/products" },
  },
  hooks: {
    afterAuth: async ({ session }) => {
      await shopify.registerWebhooks({ session });
    },
  },
  billing: {
    [PLAN]: {
      trialDays: 7,
      lineItems: [{ amount: 9.99, currencyCode: "USD", interval: BillingInterval.Every30Days }],
    },
  },
  logger: { level: process.env.SHOPIFY_LOG_LEVEL ? Number(process.env.SHOPIFY_LOG_LEVEL) : 0 },
});

// The authorization code grant (OAuth): the React Router package has none (embedded apps install
// through Shopify-managed installation and token exchange), so a non-embedded app does it with
// @shopify/shopify-api, as Express apps do.
export const oauthApi = shopifyApi({
  apiKey: config.apiKey,
  apiSecretKey: config.apiSecretKey,
  apiVersion: ApiVersion.October26,
  scopes: config.scopes,
  hostName: new URL(config.appUrl).host,
  hostScheme: new URL(config.appUrl).protocol.replace(":", ""),
  isEmbeddedApp: false,
  logger: { level: 0 },
});
