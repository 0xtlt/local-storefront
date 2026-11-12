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

  /** Selects the option whose value or text is `value`: `true` when one is. */
  const selectByValue = (select, value) => {
    const options = Array.from(select.options);
    const index = options.findIndex((option) => option.value === value || option.text === value);
    if (index >= 0) select.selectedIndex = index;
    return index >= 0;
  };

  /**
   * `new Shopify.CountryProvinceSelector('AddressCountry', 'AddressProvince', {hideElement:
   * 'AddressProvinceContainer'})`: fills the province selector with the provinces of the country
   * selected (the `data-provinces` of its option, which `country_option_tags` writes), and hides
   * it (or `hideElement`) for a country without. Each selector starts on its `data-default`.
   */
  root.CountryProvinceSelector ??= class CountryProvinceSelector {
    constructor(countryId, provinceId, options = {}) {
      this.countryEl = document.getElementById(countryId);
      this.provinceEl = document.getElementById(provinceId);
      this.provinceContainer = document.getElementById(options.hideElement || provinceId);
      if (!this.countryEl || !this.provinceEl) return;
      this.countryEl.addEventListener('change', () => this.countryHandler());
      selectByValue(this.countryEl, this.countryEl.getAttribute('data-default') || '');
      this.countryHandler();
      const province = this.provinceEl.getAttribute('data-default');
      if (province && this.provinceEl.options.length > 0) selectByValue(this.provinceEl, province);
    }

    countryHandler() {
      const option = this.countryEl.options[this.countryEl.selectedIndex];
      let provinces = [];
      try {
        provinces = JSON.parse((option && option.getAttribute('data-provinces')) || '[]');
      } catch {
        provinces = [];
      }
      while (this.provinceEl.options.length > 0) this.provinceEl.remove(0);
      for (const [value, label] of provinces) {
        const element = document.createElement('option');
        element.value = value;
        element.textContent = label;
        this.provinceEl.appendChild(element);
      }
      if (this.provinceContainer) this.provinceContainer.style.display = provinces.length > 0 ? '' : 'none';
    }
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
