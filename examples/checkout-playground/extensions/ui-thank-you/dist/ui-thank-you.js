// ui-thank-you · API 2027-01 · the seven targets of the thank-you page:
//   announcement.render: a dismissible bar (`shopify.announcement.close`).
//   header.render-after: the order number (`orderConfirmation`).
//   block.render: whether it is a first order, and what the buyer journey says.
//   customer-information.render-after: the email and the address of the order.
//   cart-line-list.render-after: how many units were bought.
//   cart-line-item.render-after: each line (`target`).
//   footer.render-after: a link to track the order.
// Place the order, or open the checkout with `&targets=all`, to see them.
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

  const number = () => shopify.orderConfirmation.value.number || shopify.orderConfirmation.value.order.id;

  shopify.extend('purchase.thank-you.announcement.render', () => {
    mount(h('s-stack', { direction: 'inline', gap: 'base', alignItems: 'center' },
      h('s-text', {}, 'Thanks for your order! Free returns for 30 days.'),
      h('s-button', { variant: 'secondary', onClick: () => shopify.announcement.close() }, 'Close')));
  });

  shopify.extend('purchase.thank-you.header.render-after', () => {
    const text = h('s-text', { type: 'strong' });
    shopify.orderConfirmation.subscribe(() => {
      text.textContent = `Order ${number()} is confirmed.`;
    });
    mount(text);
  });

  shopify.extend('purchase.thank-you.block.render', () => {
    const text = h('s-text');
    const show = () => {
      const confirmation = shopify.orderConfirmation.value;
      text.textContent = `isFirstOrder: ${confirmation.isFirstOrder} · buyerJourney.completed: ${shopify.buyerJourney.completed.value} · activeStep: ${shopify.buyerJourney.activeStep.value.handle}`;
    };
    shopify.orderConfirmation.subscribe(show);
    shopify.buyerJourney.completed.subscribe(show);
    mount(h('s-banner', { tone: 'success', heading: 'ui-thank-you: block' }, text));
  });

  shopify.extend('purchase.thank-you.customer-information.render-after', () => {
    const text = h('s-text', { color: 'subdued' });
    const show = () => {
      const address = shopify.shippingAddress.value;
      text.textContent = `Confirmation sent to ${shopify.buyerIdentity.email.value || 'nobody'}; ships to ${address ? [address.name, address.city, address.countryCode].filter(Boolean).join(', ') : 'no address'}.`;
    };
    shopify.buyerIdentity.email.subscribe(show);
    shopify.shippingAddress.subscribe(show);
    mount(text);
  });

  shopify.extend('purchase.thank-you.cart-line-list.render-after', () => {
    const text = h('s-text', { color: 'subdued' });
    shopify.lines.subscribe((lines) => {
      text.textContent = `${lines.reduce((sum, line) => sum + line.quantity, 0)} units bought, ${money(shopify.cost.totalAmount.value)} in all.`;
    });
    mount(text);
  });

  shopify.extend('purchase.thank-you.cart-line-item.render-after', () => {
    const text = h('s-text', { color: 'subdued' });
    shopify.target.subscribe((line) => {
      text.textContent = line ? `${line.merchandise.title} × ${line.quantity}` : '';
    });
    mount(text);
  });

  shopify.extend('purchase.thank-you.footer.render-after', () => {
    mount(h('s-link', { href: 'https://example.com/track' }, 'Track your order'));
  });
})();
