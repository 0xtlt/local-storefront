// ui-change-refusals · API 2026-10 · purchase.checkout.block.render
// One button per change the checkout refuses, and the answer it gave (`{type: "error",
// message}`): a gift card and payment method attributes (not supported locally), a line with
// quantity 0, a line id that does not exist, an unknown attribute change, an empty discount
// code, a note change of an unknown type, an address in a country that does not exist (ZZ).
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

  const CASES = [
    ['applyGiftCardChange', { type: 'addGiftCardChange', code: 'GIFT-0000' }],
    ['applyPaymentMethodAttributesChange', { type: 'updatePaymentMethodAttributes', attributes: [] }],
    ['applyCartLinesChange', { type: 'addCartLine', merchandiseId: 'gid://shopify/ProductVariant/9100000021', quantity: 0 }],
    ['applyCartLinesChange', { type: 'updateCartLine', id: 'gid://shopify/CartLine/does-not-exist', quantity: 2 }],
    ['applyAttributeChange', { type: 'renameAttribute', key: 'a' }],
    ['applyDiscountCodeChange', { type: 'addDiscountCode', code: '  ' }],
    ['applyNoteChange', { type: 'appendNote', note: 'x' }],
    ['applyShippingAddressChange', { type: 'updateShippingAddress', address: { countryCode: 'ZZ' } }],
  ];

  shopify.extend('purchase.checkout.block.render', () => {
    mount(h('s-stack', { gap: 'small-200' },
      h('s-heading', {}, 'Changes the checkout refuses'),
      CASES.map(([method, change]) => {
        const answer = h('s-text', { color: 'subdued' });
        return h('s-stack', { gap: 'small-400' },
          h('s-button', {
            variant: 'secondary',
            onClick: async () => {
              answer.textContent = '…';
              try {
                answer.textContent = `→ ${JSON.stringify(await shopify[method](change))}`;
              } catch (error) {
                answer.textContent = `→ threw ${error.message}`;
              }
            },
          }, `${method} ${change.type}`),
          answer);
      })));
  });
})();
