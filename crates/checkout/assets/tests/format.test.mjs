import assert from 'node:assert/strict';
import { test } from 'node:test';
import { countryName, formatMoney, price, priceOrFree, withCurrency } from '../format.js';

test('formats money like the money formats of a shop', () => {
  assert.equal(formatMoney(1234.5, '€{{amount}}'), '€1,234.50');
  assert.equal(formatMoney(1234.5, '{{ amount_with_comma_separator }} €'), '1.234,50 €');
  assert.equal(formatMoney(1234.5, '${{amount_no_decimals}}'), '$1,235');
  assert.equal(formatMoney(1234.5, '{{amount_with_apostrophe_separator}} CHF'), "1'234.50 CHF");
  assert.equal(formatMoney(1234.5, '{{amount_with_space_separator}}'), '1 234,50');
  assert.equal(formatMoney(0.5, '{{amount}}'), '0.50');
  assert.equal(formatMoney(-5, '€{{amount}}'), '-€5.00');
  assert.equal(formatMoney(5, '{{unknown}} {{amount}}'), '{{unknown}} 5.00');
});

test('writes prices and free', () => {
  const shop = { moneyFormat: '€{{amount}}' };
  assert.equal(price({ amount: 4.9 }, shop), '€4.90');
  assert.equal(priceOrFree({ amount: 0 }, shop), 'Free');
  assert.equal(priceOrFree({ amount: 3 }, shop), '€3.00');
});

test('writes the currency of the checkout into every amount', () => {
  const state = {
    cost: { currencyCode: 'EUR', total: { amount: 5 } },
    lines: [{ total: { amount: 1 }, tag: { amount: 1, other: true } }],
    runs: [{ input: { price: { amount: 9 } } }],
  };
  const out = withCurrency(state);
  assert.deepEqual(out.cost.total, { amount: 5, currencyCode: 'EUR' });
  assert.deepEqual(out.lines[0].total, { amount: 1, currencyCode: 'EUR' });
  assert.deepEqual(out.lines[0].tag, { amount: 1, other: true }, 'an object with more keys is not money');
  assert.deepEqual(out.runs[0].input.price, { amount: 9 }, 'what a Function saw is left alone');
  assert.equal(state.cost.total.currencyCode, undefined, 'the input is not changed');
});

test('names countries', () => {
  assert.equal(countryName('FR'), 'France');
  assert.equal(countryName('ZZ'), 'ZZ');
});
