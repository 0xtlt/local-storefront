// The stand-in `shopify_common.js`: the province selector of an address form.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';

const here = dirname(fileURLToPath(import.meta.url));
const source = readFileSync(join(here, '..', 'themes_support', 'shopify_common.js'), 'utf8');

/** Just enough of a DOM for a pair of `<select>`s: elements by id, options, change events. */
function page() {
  const element = (tag, attributes = {}) => {
    const node = {
      tag,
      attributes: { ...attributes },
      style: { display: '' },
      listeners: {},
      options: [],
      selectedIndex: 0,
      value: attributes.value ?? '',
      textContent: '',
      get text() {
        return this.textContent;
      },
      getAttribute(name) {
        return this.attributes[name] ?? null;
      },
      addEventListener(type, listener) {
        (this.listeners[type] ??= []).push(listener);
      },
      dispatch(type) {
        for (const listener of this.listeners[type] ?? []) listener();
      },
      appendChild(child) {
        this.options.push(child);
      },
      remove(index) {
        this.options.splice(index, 1);
      },
    };
    return node;
  };
  const option = (value, provinces) => {
    const node = element('option', { value, 'data-provinces': provinces });
    node.textContent = value;
    return node;
  };
  const country = element('select', { 'data-default': 'Canada' });
  country.options = [
    option('France', '[]'),
    option('Canada', '[["Ontario","Ontario"],["Quebec","Québec"]]'),
  ];
  const province = element('select', { 'data-default': 'Quebec' });
  const container = element('div');
  const elements = { country, province, container };
  const document = {
    getElementById: (id) => elements[id] ?? null,
    createElement: (tag) => element(tag),
  };
  const window = {};
  vm.runInNewContext(source, { window, document });
  return { Shopify: window.Shopify, country, province, container };
}

test('the province selector lists the provinces of the country selected', () => {
  const { Shopify, country, province, container } = page();
  new Shopify.CountryProvinceSelector('country', 'province', { hideElement: 'container' });
  // Each starts on its `data-default`.
  assert.equal(country.selectedIndex, 1);
  assert.deepEqual(province.options.map((option) => [option.value, option.textContent]), [
    ['Ontario', 'Ontario'],
    ['Quebec', 'Québec'],
  ]);
  assert.equal(province.selectedIndex, 1);
  assert.equal(container.style.display, '');
  // A country without provinces hides them.
  country.selectedIndex = 0;
  country.dispatch('change');
  assert.equal(province.options.length, 0);
  assert.equal(container.style.display, 'none');
});
