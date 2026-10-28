// ui-terms-gate · API 2027-01 · purchase.checkout.actions.render-before · block_progress
// A "I accept the terms of sale" checkbox above Pay. `buyerJourney.intercept` blocks the order
// while it is not ticked (behavior "block", with a reason and an error), and the checkbox shows
// the error. The ticked state is kept in the cart attribute `terms_accepted`.
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
  const money = (value) => (value ? shopify.i18n.formatCurrency(value.amount) : '—');

  const accepted = () => shopify.attributes.value.some((entry) => entry.key === 'terms_accepted' && entry.value === 'yes');

  shopify.extend('purchase.checkout.actions.render-before', () => {
    const box = h('s-checkbox', { label: 'I accept the terms of sale' });
    const status = h('s-text', { color: 'subdued' }, 'Pay is blocked until the box is ticked.');
    shopify.attributes.subscribe(() => {
      const on = accepted();
      if (box.checked !== on) box.checked = on;
      if (on) box.removeAttribute('error');
    });
    box.addEventListener('change', (event) => {
      const on = event.currentTarget.checked;
      if (on) box.removeAttribute('error');
      shopify.applyAttributeChange(on
        ? { type: 'updateAttribute', key: 'terms_accepted', value: 'yes' }
        : { type: 'removeAttribute', key: 'terms_accepted' });
    });

    shopify.buyerJourney.intercept(({ canBlockProgress }) => {
      if (canBlockProgress && !accepted()) {
        return {
          behavior: 'block',
          reason: 'The terms of sale are not accepted.',
          errors: [{ message: 'Tick “I accept the terms of sale” to pay.' }],
          perform: (result) => {
            if (result.behavior === 'block') box.setAttribute('error', 'Accept the terms of sale to pay.');
            status.textContent = `Last attempt: ${result.behavior}.`;
          },
        };
      }
      return {
        behavior: 'allow',
        perform: (result) => {
          status.textContent = `Last attempt: ${result.behavior}.`;
        },
      };
    });

    mount(h('s-stack', { gap: 'small-200' }, box, status));
  });
})();
