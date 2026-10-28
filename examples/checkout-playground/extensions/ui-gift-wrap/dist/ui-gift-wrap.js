// ui-gift-wrap · API 2027-01 · purchase.checkout.block.render
// A gift wrap checkbox. Ticked: adds the Gift Wrap product (variant 9100000011, from
// `shopify-local/products/ui-gift-wrap.json`) with `applyCartLinesChange` addCartLine and sets
// the cart attribute `gift_wrap`. Unticked: removes the line and the attribute. The box
// follows the cart, so removing the line elsewhere unticks it.
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

  const WRAP = 'gid://shopify/ProductVariant/9100000011';
  const wrapLine = () => shopify.lines.value.find((line) => line.merchandise.id === WRAP);

  shopify.extend('purchase.checkout.block.render', () => {
    const box = h('s-checkbox', { label: 'Gift wrap this order (+5.00)' });
    const result = h('s-text', { color: 'subdued' });
    shopify.lines.subscribe(() => {
      const on = Boolean(wrapLine());
      if (box.checked !== on) box.checked = on;
    });
    box.addEventListener('change', async (event) => {
      const on = event.currentTarget.checked;
      box.setAttribute('disabled', '');
      const line = wrapLine();
      const outcomes = on
        ? [
            line ? { type: 'success' } : await shopify.applyCartLinesChange({ type: 'addCartLine', merchandiseId: WRAP, quantity: 1 }),
            await shopify.applyAttributeChange({ type: 'updateAttribute', key: 'gift_wrap', value: 'yes' }),
          ]
        : [
            line ? await shopify.applyCartLinesChange({ type: 'removeCartLine', id: line.id, quantity: line.quantity }) : { type: 'success' },
            await shopify.applyAttributeChange({ type: 'removeAttribute', key: 'gift_wrap' }),
          ];
      box.removeAttribute('disabled');
      const failed = outcomes.find((outcome) => outcome.type !== 'success');
      result.textContent = failed ? `Refused: ${failed.message}` : on ? 'Gift wrap added.' : 'Gift wrap removed.';
    });
    mount(h('s-stack', { gap: 'small-200' }, h('s-heading', {}, 'Gift wrap'), box, result));
  });
})();
