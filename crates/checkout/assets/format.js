// Money, as the shop's `money_format` writes it, and the few other strings the page formats.

/** Groups the digits of `value`: `1234567.5` → `1,234,567.50` for `(',', '.', 2)`. */
function number(value, thousands, decimal, decimals) {
  const [integer, fraction] = Math.abs(value).toFixed(decimals).split('.');
  const grouped = integer.replace(/\B(?=(\d{3})+(?!\d))/g, thousands);
  return fraction === undefined ? grouped : grouped + decimal + fraction;
}

const PLACEHOLDERS = {
  amount: (v) => number(v, ',', '.', 2),
  amount_no_decimals: (v) => number(v, ',', '.', 0),
  amount_with_comma_separator: (v) => number(v, '.', ',', 2),
  amount_no_decimals_with_comma_separator: (v) => number(v, '.', ',', 0),
  amount_with_apostrophe_separator: (v) => number(v, "'", '.', 2),
  amount_no_decimals_with_space_separator: (v) => number(v, ' ', ',', 0),
  amount_with_space_separator: (v) => number(v, ' ', ',', 2),
  amount_with_period_and_space_separator: (v) => number(v, ' ', '.', 2),
};

/** Formats `amount` (major units) with a Liquid money format such as `€{{ amount }}`. */
export function formatMoney(amount, format = '${{amount}}') {
  const text = format.replace(/\{\{\s*(\w+)\s*\}\}/g, (whole, name) => {
    const placeholder = PLACEHOLDERS[name];
    return placeholder ? placeholder(amount ?? 0) : whole;
  });
  return (amount ?? 0) < 0 ? `-${text}` : text;
}

/** A price as the checkout shows it. `money` is `{amount, currencyCode}`. */
export function price(money, shop) {
  return formatMoney(money?.amount ?? 0, shop?.moneyFormat);
}

/** `Free` for nothing to pay, the price otherwise. */
export function priceOrFree(money, shop) {
  return (money?.amount ?? 0) === 0 ? 'Free' : price(money, shop);
}

/** The state of the checkout with the currency written into every amount (the server only
 * writes `{amount}`: the currency is the checkout's). Returns a new object. */
export function withCurrency(state) {
  const currencyCode = state?.cost?.currencyCode ?? state?.shop?.currencyCode ?? 'USD';
  const walk = (value) => {
    if (Array.isArray(value)) return value.map(walk);
    if (value === null || typeof value !== 'object') return value;
    const keys = Object.keys(value);
    if (keys.length === 1 && keys[0] === 'amount' && typeof value.amount === 'number') {
      return { amount: value.amount, currencyCode };
    }
    const out = {};
    for (const key of keys) out[key] = key === 'runs' ? value[key] : walk(value[key]);
    return out;
  };
  return walk(state);
}

/** `gid://shopify/ProductVariant/12` for `("ProductVariant", 12)`. */
export const gid = (type, id) => `gid://shopify/${type}/${id}`;

/** What the selector lists when the checkout does not say which countries the store sells in. */
export const COUNTRIES = [
  ['AU', 'Australia'], ['AT', 'Austria'], ['BE', 'Belgium'], ['BR', 'Brazil'], ['CA', 'Canada'],
  ['DK', 'Denmark'], ['FI', 'Finland'], ['FR', 'France'], ['DE', 'Germany'], ['IE', 'Ireland'],
  ['IT', 'Italy'], ['JP', 'Japan'], ['LU', 'Luxembourg'], ['MX', 'Mexico'], ['NL', 'Netherlands'],
  ['NZ', 'New Zealand'], ['NO', 'Norway'], ['PL', 'Poland'], ['PT', 'Portugal'], ['ES', 'Spain'],
  ['SE', 'Sweden'], ['CH', 'Switzerland'], ['GB', 'United Kingdom'], ['US', 'United States'],
];

/** The countries of the store as `[code, name]`, in the order it configures them
 * (`state.shop.countries`), else the default list. */
export function countriesOf(state) {
  const configured = state?.shop?.countries;
  if (!Array.isArray(configured)) return COUNTRIES;
  const countries = configured
    .filter((country) => country && country.code)
    .map((country) => [country.code, country.name || country.code]);
  return countries.length > 0 ? countries : COUNTRIES;
}

/** The name of a country: the store's, else the default list's, else the code. */
export const countryName = (code, countries = COUNTRIES) =>
  countries.find(([iso]) => iso === code)?.[1] ?? COUNTRIES.find(([iso]) => iso === code)?.[1] ?? code ?? '';
