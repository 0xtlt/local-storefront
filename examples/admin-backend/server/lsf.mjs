// Local development against lsf: the Admin API and OAuth of every shop go to it. This is the
// recipe of docs/admin.md ("Point the backend at lsf"): Shopify's libraries build
// `https://<shop>/admin/...` URLs, and every request of @shopify/shopify-api (the GraphQL
// clients, token exchange, OAuth) goes through the runtime's fetch, which this replaces.
import { setAbstractFetchFunc } from "@shopify/shopify-api/runtime";

export function pointAtLsf(lsf) {
  if (!lsf) return;
  setAbstractFetchFunc((input, init) => {
    const url = new URL(input instanceof Request ? input.url : String(input));
    if (!url.hostname.endsWith(".myshopify.com")) return fetch(input, init);
    const headers = new Headers(init?.headers);
    headers.set("x-forwarded-host", url.host); // the shop, for OAuth
    return fetch(`${lsf}${url.pathname}${url.search}`, { ...init, headers });
  });
}

// A fetch for @shopify/admin-api-client, which takes one (`customFetchApi`) and always builds an
// https URL from `storeDomain`.
export function lsfFetch(lsf) {
  return lsf ? (url, init) => fetch(String(url).replace(/^https:\/\/[^/]+/, lsf), init) : fetch;
}
