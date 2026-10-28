// ui-contact-newsletter · API 2027-01 · purchase.checkout.contact.render-after
// Shows, under the contact, who the order confirmation goes to (`buyerIdentity.email` and
// `buyerIdentity.customer`) and a newsletter checkbox that writes the cart attribute
// `newsletter` (`applyAttributeChange`). Log in or type another email: the text follows.
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

  const KEY = 'newsletter';
  const attribute = (list) => list.find((entry) => entry.key === KEY);

  shopify.extend('purchase.checkout.contact.render-after', () => {
    const who = h('s-text', { color: 'subdued' });
    const box = h('s-checkbox', { label: 'Email me about new products (cart attribute "newsletter")' });
    const result = h('s-text', { color: 'subdued' });

    const describe = () => {
      const email = shopify.buyerIdentity.email.value;
      const customer = shopify.buyerIdentity.customer.value;
      if (customer) {
        who.textContent = `Logged in as ${customer.fullName || customer.email} (${customer.ordersCount} orders). Confirmation to ${email || customer.email}.`;
      } else {
        who.textContent = email ? `Guest. Confirmation to ${email}.` : 'Guest, no email yet: type one above.';
      }
    };
    shopify.buyerIdentity.email.subscribe(describe);
    shopify.buyerIdentity.customer.subscribe(describe);
    shopify.attributes.subscribe((list) => {
      const on = Boolean(attribute(list));
      if (box.checked !== on) box.checked = on;
    });

    box.addEventListener('change', async (event) => {
      const on = event.currentTarget.checked;
      const outcome = await shopify.applyAttributeChange(
        on ? { type: 'updateAttribute', key: KEY, value: 'yes' } : { type: 'removeAttribute', key: KEY },
      );
      result.textContent = `applyAttributeChange → ${outcome.type}${outcome.message ? `: ${outcome.message}` : ''}`;
    });

    mount(h('s-stack', { gap: 'small-200' }, who, box, result));
  });
})();
