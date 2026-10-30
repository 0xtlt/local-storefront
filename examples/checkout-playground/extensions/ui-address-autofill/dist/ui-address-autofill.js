// ui-address-autofill · API 2026-10
//   purchase.checkout.delivery-address.render-before: the country of the checkout
//     (`localization.country`) and what the extension reads of the address.
//   purchase.checkout.delivery-address.render-after: buttons that fill the whole address, change
//     only the ZIP, or move it to France (`applyShippingAddressChange`, whole or partial).
(() => {
  // h(tag, attributes, ...children) makes a Polaris web component: attributes are set as
  // attributes, functions become listeners (`onClick` listens to `click`), children are
  // appended. Each bundle carries its own copy, as a real build carries its runtime.
  const h = (tag, attributes, ...children) => {
    const node = document.createElement(tag);
    for (const [name, value] of Object.entries(attributes || {})) {
      if (value === undefined || value === null || value === false) continue;
      if (typeof value === 'function') node.addEventListener(name.slice(2).toLowerCase(), value);
      else node.setAttribute(name, value === true ? '' : String(value));
    }
    for (const child of children.flat(Infinity)) {
      if (child === undefined || child === null || child === false) continue;
      node.append(typeof child === 'object' ? child : String(child));
    }
    return node;
  };
  const mount = (...nodes) => document.body.replaceChildren(...nodes);

  const ADDRESSES = {
    ottawa: {
      label: 'Fill: our Ottawa store',
      address: { firstName: 'Pat', lastName: 'Pickup', address1: '150 Elgin Street', address2: '8th floor', city: 'Ottawa', provinceCode: 'ON', zip: 'K2P 1L4', countryCode: 'CA' },
    },
    zip: { label: 'Change only the ZIP to 10001', address: { zip: '10001' } },
    paris: {
      label: 'Move to Paris (another country)',
      address: { firstName: 'Camille', lastName: 'Martin', address1: '8 rue de Rivoli', city: 'Paris', address2: '', zip: '75004', countryCode: 'FR', provinceCode: '' },
    },
  };

  shopify.extend('purchase.checkout.delivery-address.render-before', () => {
    const text = h('s-text', { color: 'subdued' });
    const show = () => {
      const country = shopify.localization.country.value;
      const address = shopify.shippingAddress.value;
      text.textContent = `Country of the checkout: ${country ? country.isoCode : 'none yet'}. ` +
        (address && address.city ? `Shipping to ${address.name || ''} ${address.city} ${address.zip || ''}`.replace(/\s+/g, ' ') : 'No address yet.');
    };
    shopify.localization.country.subscribe(show);
    shopify.shippingAddress.subscribe(show);
    mount(text);
  });

  shopify.extend('purchase.checkout.delivery-address.render-after', () => {
    const result = h('s-text', { color: 'subdued' }, 'Pick one: the form above follows.');
    const buttons = Object.values(ADDRESSES).map(({ label, address }) =>
      h('s-button', {
        variant: 'secondary',
        onClick: async () => {
          const outcome = await shopify.applyShippingAddressChange({ type: 'updateShippingAddress', address });
          result.textContent = outcome.type === 'success'
            ? `${label}: done.`
            : `${label}: ${(outcome.errors || []).map((error) => error.message).join(', ')}`;
        },
      }, label));
    mount(h('s-stack', { gap: 'small-200' },
      h('s-stack', { direction: 'inline', gap: 'small-200' }, buttons),
      result));
  });
})();
