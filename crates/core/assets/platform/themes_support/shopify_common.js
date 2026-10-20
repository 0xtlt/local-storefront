// Stand-in for Shopify's `shopify_common.js`, written for the local server: the helpers of the
// `Shopify` object that themes call, nothing more. Shopify's own file is not reproduced.
// https://shopify.dev/docs/api/liquid/filters/shopify_asset_url
(() => {
  const root = (window.Shopify = window.Shopify || {});

  const group = (digits, separator) => digits.replace(/(\d)(?=(\d{3})+$)/g, `$1${separator}`);

  const number = (cents, thousands, decimal, places) => {
    const value = (Number(cents) / 100).toFixed(places);
    const [whole, fraction] = value.split('.');
    return group(whole, thousands) + (fraction ? decimal + fraction : '');
  };

  /** `Shopify.formatMoney(1999, '${{amount}}')` → `$19.99`. */
  root.formatMoney ??= (cents, format) => {
    const pattern = format || root.money_format || '${{amount}}';
    const text = typeof cents === 'string' ? cents.replace('.', '') : cents;
    return pattern.replace(/\{\{\s*(\w+)\s*\}\}/, (_, name) => {
      switch (name) {
        case 'amount':
          return number(text, ',', '.', 2);
        case 'amount_no_decimals':
          return number(text, ',', '.', 0);
        case 'amount_with_comma_separator':
          return number(text, '.', ',', 2);
        case 'amount_no_decimals_with_comma_separator':
          return number(text, '.', ',', 0);
        case 'amount_with_apostrophe_separator':
          return number(text, "'", '.', 2);
        default:
          return '';
      }
    });
  };

  /** `Shopify.resizeImage(url, 'medium')` → `…_medium.jpg`; the size is `100x100`, `large`, … */
  root.resizeImage ??= (image, size) => {
    if (!image || !size || size === 'original') {
      return image;
    }
    const match = String(image).match(/^(.*?)(\.[a-z0-9]+)?(\?.*)?$/i);
    return `${match[1]}_${size}${match[2] || ''}${match[3] || ''}`;
  };
})();
