// ui-shipping-options · API 2027-01
//   purchase.checkout.shipping-option-list.render-before: how many options, and the chosen one
//     (`deliveryGroups`, `selectedDeliveryOption`).
//   purchase.checkout.shipping-option-list.render-after: every option with its cost before and
//     after discounts (`deliverySelectionGroups`).
//   purchase.checkout.shipping-option-item.render-after: under each option, its handle and
//     whether it is the selected one (`target`, `isTargetSelected`).
//   purchase.checkout.shipping-option-item.details.render: details of an option.
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

  const options = () => shopify.deliveryGroups.value.flatMap((group) => group.deliveryOptions);
  const selectedHandle = () => {
    const group = shopify.deliveryGroups.value[0];
    return group && group.selectedDeliveryOption ? group.selectedDeliveryOption.handle : null;
  };

  shopify.extend('purchase.checkout.shipping-option-list.render-before', () => {
    const text = h('s-text', { color: 'subdued' });
    shopify.deliveryGroups.subscribe(() => {
      const all = options();
      const chosen = all.find((option) => option.handle === selectedHandle());
      text.textContent = `${all.length} shipping option${all.length === 1 ? '' : 's'}; chosen: ${chosen ? chosen.title : 'none'}.`;
    });
    mount(text);
  });

  shopify.extend('purchase.checkout.shipping-option-list.render-after', () => {
    const list = h('s-unordered-list');
    shopify.deliverySelectionGroups.subscribe((groups) => {
      list.replaceChildren(...groups.map((entry) =>
        h('s-list-item', {},
          `${entry.title} (${entry.handle}): ${money(entry.cost)} → ${money(entry.costAfterDiscounts)}${entry.selected ? ' · selected' : ''}`)));
    });
    mount(h('s-stack', { gap: 'small-200' }, h('s-text', { type: 'strong' }, 'deliverySelectionGroups'), list));
  });

  shopify.extend('purchase.checkout.shipping-option-item.render-after', () => {
    const badge = h('s-badge', { tone: 'success' }, 'Selected');
    const text = h('s-text', { color: 'subdued' });
    const show = () => {
      const option = shopify.target.value;
      text.textContent = option ? `handle ${option.handle}, type ${option.type}` : 'no option';
      badge.hidden = !shopify.isTargetSelected.value;
    };
    shopify.target.subscribe(show);
    shopify.isTargetSelected.subscribe(show);
    mount(h('s-stack', { direction: 'inline', gap: 'small-200', alignItems: 'center' }, badge, text));
  });

  shopify.extend('purchase.checkout.shipping-option-item.details.render', () => {
    const text = h('s-text');
    shopify.target.subscribe((option) => {
      text.textContent = option
        ? `Details of ${option.title}: leaves the warehouse within 2 business days.`
        : 'No option.';
    });
    mount(h('s-box', { padding: 'small-200', background: 'subdued', borderRadius: 'base' }, text));
  });
})();
