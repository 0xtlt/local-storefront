// The stand-in of the `<shopify-account>` web component: the avatar of the visitor, which
// leads to the sign-in page or to the account.

import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const source = readFileSync(join(dirname(fileURLToPath(import.meta.url)), '..', 'account.js'), 'utf8');

/** Just enough of a DOM node for the component: attributes, children and a shadow root. */
class Node {
  constructor(tag) {
    this.tag = tag;
    this.attributes = {};
    this.children = [];
  }
  setAttribute(name, value) {
    this.attributes[name] = String(value);
  }
  getAttribute(name) {
    return this.attributes[name] ?? null;
  }
  append(...nodes) {
    this.children.push(...nodes);
  }
  attachShadow() {
    this.shadowRoot = new Node('#shadow-root');
    return this.shadowRoot;
  }
}

/** Renders `<shopify-account>` with `attributes`, on a page whose data names `customer`. */
const render = (attributes, customer) => {
  const defined = new Map();
  const data = { textContent: JSON.stringify({ customer, shop: { id: 'gid://shopify/Shop/1' } }) };
  const document = {
    createElement: (tag) => new Node(tag),
    getElementById: (id) => (id === 'shopify-customer-account-data' ? data : null),
  };
  const customElements = { get: (name) => defined.get(name), define: (name, element) => defined.set(name, element) };
  const window = { Shopify: { routes: { root: '/fr/' } } };
  new Function('window', 'document', 'customElements', 'HTMLElement', source)(
    window,
    document,
    customElements,
    Node,
  );
  const Element = defined.get('shopify-account');
  const element = new Element('shopify-account');
  Object.assign(element.attributes, attributes);
  element.connectedCallback();
  const [, link] = element.shadowRoot.children;
  return link;
};

test('a visitor who is not logged in gets the signed-out avatar, which leads to the sign-in page', () => {
  const link = render({ menu: 'customer-account-main-menu' }, null);
  assert.equal(link.href, '/customer_authentication/login');
  assert.equal(link.getAttribute('aria-label'), 'Sign in');
  const [slot] = link.children;
  assert.equal(slot.name, 'signed-out-avatar');
  assert.match(slot.innerHTML, /part="signed-out-avatar"/);
  // `sign-in-url` replaces the default.
  assert.equal(render({ 'sign-in-url': '/login' }, null).href, '/login');
});

test("a logged-in customer gets their initial, which leads to the account in the page's language", () => {
  const link = render({}, { email: 'jane@example.com', firstName: 'Jane', lastName: 'Doe' });
  assert.equal(link.href, '/fr/account');
  const [avatar] = link.children;
  assert.equal(avatar.getAttribute('part'), 'signed-in-avatar');
  assert.equal(avatar.textContent, 'J');
});
