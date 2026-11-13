// The `<shopify-account>` web component, for the local server.
//
// Shopify's component (https://shopify.dev/docs/api/storefront-web-components/components/shopify-account)
// shows an avatar that opens an account sheet, where the buyer signs in through Shopify's
// customer accounts. The local server stands in for those with `/account`, so this one is the
// avatar alone, which leads there: what Shopify's does for a shop on legacy customer accounts
// (https://shopify.dev/docs/storefronts/themes/customer-engagement/account-component). The
// customer comes from the `shopify-customer-account-data` block the page carries.
(() => {
  if (customElements.get('shopify-account')) return;

  const customer = () => {
    try {
      const data = document.getElementById('shopify-customer-account-data');
      return JSON.parse(data?.textContent || '{}').customer ?? null;
    } catch {
      return null;
    }
  };

  const STYLE = `
    :host { display: inline-block; }
    a { display: inline-flex; align-items: center; justify-content: center; color: inherit; text-decoration: none; }
    [part='signed-out-avatar'] {
      width: var(--shopify-account-avatar-size, 24px);
      height: var(--shopify-account-avatar-size, 24px);
    }
    [part='signed-in-avatar'] {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: var(--shopify-account-avatar-size, var(--shopify-account-signed-in-avatar-size, 32px));
      height: var(--shopify-account-avatar-size, var(--shopify-account-signed-in-avatar-size, 32px));
      border-radius: 50%;
      background: var(--shopify-account-signed-in-avatar-color-background, var(--shopify-account-color-accent, #000));
      color: var(--shopify-account-signed-in-avatar-color-text, var(--shopify-account-color-accent-text, #fff));
      font: inherit;
      text-transform: uppercase;
    }
  `;

  const SIGNED_OUT_AVATAR =
    '<svg part="signed-out-avatar" viewBox="0 0 24 24" fill="none" stroke="currentColor" ' +
    'stroke-width="1.5" aria-hidden="true"><circle cx="12" cy="8" r="4"/>' +
    '<path d="M4 21c0-4.4 3.6-8 8-8s8 3.6 8 8"/></svg>';

  class ShopifyAccount extends HTMLElement {
    connectedCallback() {
      if (this.shadowRoot) return;
      const root = this.attachShadow({ mode: 'open' });
      const style = document.createElement('style');
      style.textContent = STYLE;
      const link = document.createElement('a');
      const signedIn = customer();
      if (signedIn) {
        link.href = `${window.Shopify?.routes?.root ?? '/'}account`;
        link.setAttribute('aria-label', 'Account');
        const avatar = document.createElement('span');
        avatar.setAttribute('part', 'signed-in-avatar');
        avatar.textContent = (signedIn.firstName || signedIn.email || '?').charAt(0);
        link.append(avatar);
      } else {
        link.href = this.getAttribute('sign-in-url') || '/customer_authentication/login';
        link.setAttribute('aria-label', 'Sign in');
        const slot = document.createElement('slot');
        slot.name = 'signed-out-avatar';
        slot.innerHTML = SIGNED_OUT_AVATAR;
        link.append(slot);
      }
      root.append(style, link);
    }
  }

  customElements.define('shopify-account', ShopifyAccount);
})();
