// ui-payment-notes · API 2026-10
//   purchase.checkout.payment-method-list.render-before: a line of reassurance.
//   purchase.checkout.payment-method-list.render-after: the payment options the extension
//     reads (`availablePaymentOptions`, their `type`) and the selected one
//     (`selectedPaymentOptions`). Pick another method: the list follows.
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

  shopify.extend('purchase.checkout.payment-method-list.render-before', () => {
    mount(h('s-text', { color: 'subdued' }, 'Every transaction is secure and encrypted. (ui-payment-notes)'));
  });

  shopify.extend('purchase.checkout.payment-method-list.render-after', () => {
    const list = h('s-unordered-list');
    const show = () => {
      const selected = shopify.selectedPaymentOptions.value.map((option) => option.handle);
      list.replaceChildren(...shopify.availablePaymentOptions.value.map((option) =>
        h('s-list-item', {}, `${option.handle}: type ${option.type}${selected.includes(option.handle) ? ' · selected' : ''}`)));
    };
    shopify.availablePaymentOptions.subscribe(show);
    shopify.selectedPaymentOptions.subscribe(show);
    mount(h('s-stack', { gap: 'small-200' }, h('s-text', { type: 'strong' }, 'availablePaymentOptions'), list));
  });
})();
