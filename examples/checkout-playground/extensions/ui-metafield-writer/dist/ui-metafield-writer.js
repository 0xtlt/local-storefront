// ui-metafield-writer · API 2027-01 · purchase.checkout.block.render
// Writes the cart metafield `$app:playground.packaging` from a select
// (`applyMetafieldChange` updateCartMetafield) and removes it (removeCartMetafield). The toml
// declares it with `owner_type = "CART"`, which 2027-01 requires, and lists `appMetafields`.
// No Function of the playground reads it.
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

  const NAMESPACE = '$app:playground';
  const KEY = 'packaging';

  shopify.extend('purchase.checkout.block.render', () => {
    const result = h('s-text', { color: 'subdued' });
    const report = (what) => (outcome) => {
      result.textContent = `${what} → ${outcome.type}${outcome.message ? `: ${outcome.message}` : ''}`;
    };
    const select = h('s-select', { label: 'Packaging (cart metafield $app:playground.packaging)', value: '' },
      h('s-option', { value: '' }, 'Choose…'),
      h('s-option', { value: 'recycled' }, 'Recycled box'),
      h('s-option', { value: 'minimal' }, 'Minimal, no box'));
    select.addEventListener('change', (event) => {
      const value = event.currentTarget.value;
      if (!value) return;
      shopify.applyMetafieldChange({
        type: 'updateCartMetafield',
        metafield: { namespace: NAMESPACE, key: KEY, type: 'single_line_text_field', value },
      }).then(report(`updateCartMetafield ${value}`));
    });
    const clear = h('s-button', {
      variant: 'secondary',
      onClick: () => shopify.applyMetafieldChange({ type: 'removeCartMetafield', namespace: NAMESPACE, key: KEY })
        .then(report('removeCartMetafield')),
    }, 'Remove the metafield');

    const list = h('s-stack', { gap: 'small-300' });
    shopify.appMetafields.subscribe((entries) => {
      const mine = entries.find((entry) => entry.metafield.namespace === NAMESPACE && entry.metafield.key === KEY);
      if (select.value !== (mine ? mine.metafield.value : '')) select.value = mine ? mine.metafield.value : '';
      list.replaceChildren(...(entries.length ? entries.map(({ target, metafield }) =>
        h('s-text', {}, `${target.type} · ${metafield.namespace}.${metafield.key} = ${metafield.value} (${metafield.type})`))
        : [h('s-text', { color: 'subdued' }, 'No app metafield.')]));
    });

    mount(h('s-stack', { gap: 'base' },
      h('s-heading', {}, 'Cart metafield (2027-01, owner_type CART)'),
      select, clear, h('s-text', { type: 'strong' }, 'shopify.appMetafields'), list, result));
  });
})();
