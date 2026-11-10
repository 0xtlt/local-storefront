// A browser, as far as the run needs one: it keeps cookies per host, does not follow redirects
// (the run follows them, to see where they go), and says it is Chrome. Shopify's libraries
// answer `410 Gone` to a user agent that looks like a bot (`isbot`): curl's or Node's included.
export class Browser {
  static userAgent =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Safari/537.36";

  #cookies = new Map();

  async fetch(url, init = {}) {
    const target = new URL(url);
    const headers = new Headers(init.headers);
    headers.set("User-Agent", Browser.userAgent);
    const jar = this.#cookies.get(target.host);
    if (jar?.size) headers.set("Cookie", [...jar].map(([name, value]) => `${name}=${value}`).join("; "));
    const response = await fetch(target, { ...init, headers, redirect: "manual" });
    for (const cookie of response.headers.getSetCookie()) {
      const [pair] = cookie.split(";");
      const at = pair.indexOf("=");
      if (!this.#cookies.has(target.host)) this.#cookies.set(target.host, new Map());
      this.#cookies.get(target.host).set(pair.slice(0, at).trim(), pair.slice(at + 1).trim());
    }
    return response;
  }
}

/**
 * Runs `run` with a Chromium context (playwright-core, a dev dependency) that says it is Chrome,
 * as `Browser` does, and refuses every request to cdn.shopify.com, as a machine without the
 * internet would: `refused` lists them, `served` every answer of lsf's `/__lsf/cdn/`. The browser
 * is closed after. `npx playwright-core install --only-shell chromium` installs it.
 */
export async function withChromium(run) {
  const { chromium } = await import("playwright-core");
  const browser = await chromium.launch();
  try {
    const context = await browser.newContext({ userAgent: Browser.userAgent });
    const refused = [];
    const served = [];
    await context.route(/^https:\/\/cdn\.shopify\.com\//, (route) => {
      refused.push(route.request().url());
      return route.abort("internetdisconnected");
    });
    context.on("response", (response) => {
      if (new URL(response.url()).pathname.startsWith("/__lsf/cdn/")) served.push({ url: response.url(), status: response.status() });
    });
    return await run({ context, refused, served });
  } finally {
    await browser.close();
  }
}
