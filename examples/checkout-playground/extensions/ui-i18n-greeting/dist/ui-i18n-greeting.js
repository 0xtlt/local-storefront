// ui-i18n-greeting · API 2027-01 · purchase.checkout.block.render
// Translations from `locales/en.default.json` and `locales/fr.json` with
// `shopify.i18n.translate`: a greeting with the customer's first name, a plural (`count`), a
// key that only English has (falls back to the default locale) and one that no file has (shows
// the key). Formats money, a number and a date with `formatCurrency`, `formatNumber`,
// `formatDate`, and shows `localization.language` and `extensionLanguage`. Open the French
// scenario to see it in French.
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

  shopify.extend('purchase.checkout.block.render', () => {
    const { i18n } = shopify;
    const body = h('s-stack', { gap: 'small-200' });
    const draw = () => {
      const customer = shopify.buyerIdentity.customer.value;
      const count = shopify.lines.value.reduce((sum, line) => sum + line.quantity, 0);
      body.replaceChildren(
        h('s-heading', {}, i18n.translate('heading')),
        h('s-text', { type: 'strong' }, i18n.translate('greeting', { name: customer && customer.firstName ? customer.firstName : i18n.translate('guest') })),
        h('s-text', {}, i18n.translate('items', { count })),
        h('s-text', {}, i18n.translate('total', { amount: i18n.formatCurrency(shopify.cost.totalAmount.value.amount) })),
        h('s-text', {}, i18n.translate('number', { number: i18n.formatNumber(1234567.891) })),
        h('s-text', {}, i18n.translate('today', { date: i18n.formatDate(new Date(), { dateStyle: 'full' }) })),
        h('s-text', { color: 'subdued' }, i18n.translate('languages', {
          buyer: shopify.localization.language.value.isoCode,
          extension: shopify.localization.extensionLanguage.value.isoCode,
        })),
        h('s-text', { color: 'subdued' }, i18n.translate('english_only')),
        h('s-text', { color: 'subdued' }, `No file has this key: ${i18n.translate('missing.key')}`),
      );
    };
    shopify.buyerIdentity.customer.subscribe(draw);
    shopify.lines.subscribe(draw);
    shopify.cost.totalAmount.subscribe(draw);
    mount(body);
  });
})();
