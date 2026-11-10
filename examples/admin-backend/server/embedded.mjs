// The embedded app's page: what the React Router template's `app/routes/app.tsx` renders with
// `AppProvider`, written by hand (no React here). Its head loads lsf's App Bridge when
// APP_BRIDGE_URL is set, then Shopify's script as AppProvider puts it: Shopify's finds
// `shopify.ready` and does nothing (online), or does not load (offline); then Polaris, from
// POLARIS_URL (lsf's `/__lsf/cdn/shopifycloud/polaris-2.js`) or Shopify's CDN. The body has the
// app's navigation (`s-app-nav`), its title bar (`ui-title-bar`), a few Polaris components with
// icons, and a section per route, switched in the browser as a router does: the admin's nav items
// click the links of `s-app-nav`.
//
// What it uses of App Bridge: toast, loading, user, resourcePicker, the title bar and the
// navigation, a `data-save-bar` form, and fetch() to routes behind authenticate.admin (App
// Bridge adds the session token).

const escape = (text) =>
  String(text).replace(/[&<>"]/g, (char) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[char]);

// AppProvider's default `polarisUrl`.
const SHOPIFY_POLARIS = "https://cdn.shopify.com/shopifycloud/polaris-2.js";

export function embeddedPage({ apiKey, shop, shopName, appBridgeUrl, polarisUrl = SHOPIFY_POLARIS, settings }) {
  const lsfScript = appBridgeUrl ? `<script src="${escape(appBridgeUrl)}"></script>\n    ` : "";
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Admin backend</title>
    ${lsfScript}<script src="https://cdn.shopify.com/shopifycloud/app-bridge.js" data-api-key="${escape(apiKey)}"></script>
    <script src="${escape(polarisUrl)}"></script>
    <style>
      body { font: 14px/1.5 system-ui, sans-serif; margin: 0; padding: 16px 20px; color: #1c1e21; }
      .card { background: #fff; border: 1px solid #dcdfe3; border-radius: 10px; padding: 16px 20px; margin-bottom: 16px; }
      h1 { font-size: 18px; margin: 0 0 4px; }
      h2 { font-size: 15px; margin: 0 0 8px; }
      p { margin: 4px 0 12px; color: #5f646b; }
      button { font: inherit; padding: 6px 12px; border: 1px solid #c9ccd1; border-radius: 8px; background: #fff; cursor: pointer; }
      label { display: block; margin: 8px 0; }
      input[type=number] { font: inherit; padding: 4px 8px; width: 6em; }
      ul { margin: 8px 0 0; padding-left: 20px; }
    </style>
  </head>
  <body>
    <s-app-nav>
      <a href="/app" rel="home">Home</a>
      <a href="/app/products">Products</a>
      <a href="/app/settings">Settings</a>
    </s-app-nav>
    <ui-title-bar title="Home"></ui-title-bar>

    <section data-route="/app">
      <div class="card">
        <h1 data-shop="${escape(shop)}">${escape(shopName)}</h1>
        <p id="user">An embedded app of lsf's admin.</p>
        <button id="toast" type="button">Show a toast</button>
        <button id="pick" type="button">Pick products</button>
        <button id="about-open" type="button">About this app</button>
        <ul id="picked"></ul>
        <p><a href="shopify://admin/products" target="_top">The shop's products in the admin</a></p>
        <p><a id="plans" href="shopify://admin/charges/admin-backend/pricing_plans" target="_top">Choose a plan</a> (managed pricing: the plan selection page of the admin)</p>
      </div>
      <s-section heading="Polaris" id="polaris">
        <s-stack direction="inline" gap="base" alignItems="center">
          <s-button id="polaris-button" icon="product" href="/app/products">Products</s-button>
          <s-badge tone="success" icon="check">Synced</s-badge>
          <s-icon type="cart"></s-icon>
          <s-text>Web components, icons and fonts from POLARIS_URL.</s-text>
        </s-stack>
        <s-banner id="polaris-banner" tone="info" heading="Polaris loads its icons with fetch()">
          From POLARIS_URL's origin, with the headers App Bridge adds to the app's own requests.
          <s-button slot="secondary-actions" id="banner-action" href="/app/settings">Open the settings</s-button>
        </s-banner>
      </s-section>
      <ui-modal id="about">
        <p style="padding: 0 4px">A Shopify app backend run against lsf: its Admin API, its webhooks, and this embedded page in lsf's admin.</p>
        <ui-title-bar title="About this app">
          <button variant="primary" onclick="shopify.modal.hide('about')">Close</button>
        </ui-title-bar>
      </ui-modal>
    </section>

    <section data-route="/app/products" hidden>
      <div class="card">
        <h2>Products of the shop</h2>
        <p>Read by the backend, from a route behind authenticate.admin: App Bridge's fetch() sends the session token.</p>
        <ul id="products"></ul>
      </div>
    </section>

    <section data-route="/app/settings" hidden>
      <div class="card">
        <h2>Settings</h2>
        <p>A form with data-save-bar: change it, and the admin shows its save bar.</p>
        <form id="settings" data-save-bar data-discard-confirmation>
          <label>Low stock threshold <input name="threshold" type="number" min="0" value="${escape(settings.threshold)}"></label>
          <label><input name="notify" type="checkbox"${settings.notify ? " checked" : ""}> Email me when a product runs low</label>
        </form>
      </div>
    </section>

    <script>
      const bar = document.querySelector("ui-title-bar");
      const titleBars = {
        "/app": { title: "Home", buttons: [["Show a toast", "toast"], ["Pick products", "pick", "primary"]] },
        "/app/products": { title: "Products", buttons: [["Home", "home", "breadcrumb"], ["Reload", "reload", "primary"]] },
        "/app/settings": { title: "Settings", buttons: [["Home", "home", "breadcrumb"]] },
      };

      // The router: one page, a section per route, as a React Router app switches routes.
      function show(path) {
        const route = titleBars[path] ? path : "/app";
        for (const section of document.querySelectorAll("section[data-route]")) section.hidden = section.dataset.route !== route;
        const { title, buttons } = titleBars[route];
        bar.setAttribute("title", title);
        bar.replaceChildren(...buttons.map(([label, action, variant]) => {
          const button = document.createElement("button");
          button.textContent = label;
          button.dataset.action = action;
          if (variant) button.setAttribute("variant", variant);
          return button;
        }));
        if (route === "/app/products") loadProducts();
      }
      function go(path) {
        history.pushState(null, "", path);
        show(path);
      }
      document.addEventListener("click", (event) => {
        const link = event.target.closest("a[href^='/app']");
        if (!link || event.defaultPrevented || link.target) return;
        event.preventDefault();
        go(link.getAttribute("href"));
      });
      document.addEventListener("shopify:navigate", (event) => go(event.target.getAttribute("href")));
      window.addEventListener("popstate", () => show(location.pathname));

      const actions = {
        toast: () => shopify.toast.show("Hello from the app", { action: "Undo", onAction: () => shopify.toast.show("Undone") }),
        pick: async () => {
          const selected = await shopify.resourcePicker({ type: "product", multiple: true, action: "select" });
          const list = document.getElementById("picked");
          list.replaceChildren(...(selected ?? []).map((product) => {
            const item = document.createElement("li");
            item.textContent = product.title + " (" + product.variants.length + " variants)";
            return item;
          }));
          if (selected) shopify.toast.show(selected.length + " picked");
        },
        home: () => go("/app"),
        reload: () => loadProducts(),
      };
      bar.addEventListener("click", (event) => actions[event.target.dataset?.action]?.());
      document.getElementById("toast").addEventListener("click", actions.toast);
      document.getElementById("pick").addEventListener("click", actions.pick);
      document.getElementById("about-open").addEventListener("click", () => shopify.modal.show("about"));

      async function loadProducts() {
        shopify.loading(true);
        try {
          const response = await fetch("/api/products");
          const { titles, productsCount } = await response.json();
          const list = document.getElementById("products");
          list.replaceChildren(...titles.map((title) => Object.assign(document.createElement("li"), { textContent: title })));
          shopify.toast.show(productsCount + " products");
        } catch (error) {
          shopify.toast.show("Could not load the products: " + error.message, { isError: true });
        } finally {
          shopify.loading(false);
        }
      }

      const form = document.getElementById("settings");
      form.addEventListener("submit", async (event) => {
        event.preventDefault();
        const values = Object.fromEntries(new FormData(form));
        const response = await fetch("/api/settings", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ threshold: Number(values.threshold), notify: values.notify === "on" }),
        });
        shopify.toast.show(response.ok ? "Settings saved" : "Not saved (" + response.status + ")", { isError: !response.ok });
      });

      shopify.user().then((user) => {
        document.getElementById("user").textContent = "Signed in as " + user.name + " (" + user.accountAccess + ").";
      }, () => {});
      show(location.pathname === "/" ? "/app" : location.pathname);
    </script>
  </body>
</html>
`;
}
