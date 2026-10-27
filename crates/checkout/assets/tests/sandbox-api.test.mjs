// The `shopify` global: what an extension reads, calls and is called with.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { assets, checkoutState, engineCheckout, init, pageState, plain, sandboxWindow, settle } from './harness.mjs';
import { update } from '../preview/mock-engine.mjs';

function runtime(options = {}) {
  const win = sandboxWindow();
  const posted = [];
  const created = win.context.LsfSandbox.createRuntime({
    post: (message) => posted.push(plain(message)),
    init: init({ handle: 'x', target: 'purchase.checkout.block.render', ...options }),
  });
  return { ...created, posted, win };
}

const ADDRESS = { lastName: 'Martin', address1: '1 rue de Rivoli', city: 'Paris', countryCode: 'FR' };
const next = (...operations) => pageState(update(engineCheckout(), operations).state);

test('reads the checkout as signals', () => {
  const { shopify } = runtime();
  assert.equal(shopify.lines.value.length, 2);
  const [shirt] = shopify.lines.value;
  assert.equal(shirt.id, 'gid://shopify/CartLine/1');
  assert.equal(shirt.merchandise.id, 'gid://shopify/ProductVariant/101');
  assert.equal(shirt.merchandise.title, 'Linen shirt');
  assert.equal(shirt.merchandise.subtitle, 'M / Sand');
  assert.equal(shirt.merchandise.sku, 'LIN-SH-M-SAND');
  assert.deepEqual(plain(shirt.cost.totalAmount), { amount: 49, currencyCode: 'EUR' });
  assert.deepEqual(plain(shopify.cost.totalAmount.value), { amount: 73, currencyCode: 'EUR' });
  assert.equal(shopify.checkoutToken.value, 'preview');
  assert.equal(shopify.shippingAddress.value, undefined);
  assert.equal(shopify.buyerIdentity.email.value, undefined);
  assert.equal(shopify.extensionPoint, 'purchase.checkout.block.render');
  assert.equal(shopify.extension.target, 'purchase.checkout.block.render');
  assert.equal(shopify.extension.apiVersion, '2026-10');
  assert.deepEqual(plain(shopify.extension.capabilities.value), ['api_access', 'block_progress']);
});

test('merchandise carries what the Cart Lines API documents, for lines and bundle components', () => {
  const base = checkoutState();
  const component = {
    ...base.lines[1],
    id: 'gid://shopify/CartLine/9',
    vendor: 'Acme',
    productType: 'Gifts',
    selectedOptions: [{ name: 'Size', value: 'L' }],
    sellingPlan: { id: 'gid://shopify/SellingPlan/7', recurringDeliveries: true },
    image: '/cdn/a.png',
    imageAlt: 'A bag',
    requiresShipping: false,
  };
  const state = { ...base, lines: [{ ...base.lines[0], components: [component] }, base.lines[1]] };
  const { shopify } = runtime({ state });
  const [shirt, tote] = shopify.lines.value;
  assert.equal(shirt.merchandise.product.vendor, 'Northline');
  assert.equal(shirt.merchandise.product.productType, 'Shirts');
  assert.deepEqual(plain(shirt.merchandise.selectedOptions), [
    { name: 'Size', value: 'M' },
    { name: 'Color', value: 'Sand' },
  ]);
  assert.equal(shirt.merchandise.sellingPlan, undefined, 'no plan: no key');
  assert.equal('sellingPlan' in tote.merchandise, false);
  const part = shirt.lineComponents[0].merchandise;
  assert.equal(part.product.vendor, 'Acme');
  assert.equal(part.product.productType, 'Gifts');
  assert.equal(part.product.id, 'gid://shopify/Product/22');
  assert.deepEqual(plain(part.selectedOptions), [{ name: 'Size', value: 'L' }]);
  assert.deepEqual(plain(part.sellingPlan), { id: 'gid://shopify/SellingPlan/7', recurringDeliveries: true });
  assert.deepEqual(plain(part.image), { url: '/cdn/a.png', altText: 'A bag' });
  assert.equal(part.requiresShipping, false);
  assert.equal(part.sku, 'TOTE-01');
  assert.equal(part.subtitle, undefined);
});

test('signals tell subscribers about changes, and only changes', () => {
  const { shopify, refresh } = runtime();
  const seen = [];
  const unsubscribe = shopify.cost.totalAmount.subscribe((value) => seen.push(value.amount));
  assert.deepEqual(seen, [73], 'a signal calls back at once');
  refresh(next({ type: 'discountCode', action: 'add', code: 'SAVE10' }));
  refresh(next({ type: 'discountCode', action: 'add', code: 'SAVE10' }));
  assert.deepEqual(seen, [73, 65.7], 'the same value twice is one change');
  unsubscribe();
  refresh(checkoutState());
  assert.deepEqual(seen, [73, 65.7]);
  assert.equal(shopify.cost.totalAmount.value.amount, 73);
});

test('maps address, buyer, delivery and payment', () => {
  const state = next(
    { type: 'email', value: 'a@b.c' },
    { type: 'shippingAddress', address: { ...ADDRESS, firstName: 'Ada' } },
    { type: 'deliveryOption', group: 'shipping', handle: 'express' },
    { type: 'paymentMethod', handle: 'cod' },
    { type: 'note', change: { type: 'updateNote', note: 'ring' } },
    { type: 'attribute', change: { type: 'updateAttribute', key: 'k', value: 'v' } },
  );
  const { shopify } = runtime({ state });
  assert.equal(shopify.buyerIdentity.email.value, 'a@b.c');
  assert.equal(shopify.shippingAddress.value.name, 'Ada Martin');
  assert.equal(shopify.shippingAddress.value.countryCode, 'FR');
  assert.equal(shopify.localization.country.value.isoCode, 'FR');
  assert.equal(shopify.note.value, 'ring');
  assert.deepEqual(plain(shopify.attributes.value), [{ key: 'k', value: 'v' }]);
  const [group] = shopify.deliveryGroups.value;
  assert.equal(group.selectedDeliveryOption.handle, 'express');
  assert.deepEqual(group.targetedCartLines.map((line) => line.id), ['gid://shopify/CartLine/1', 'gid://shopify/CartLine/2']);
  const express = group.deliveryOptions.find((option) => option.handle === 'express');
  assert.equal(express.type, 'shipping');
  assert.deepEqual(plain(express.cost), { amount: 12, currencyCode: 'EUR' });
  assert.deepEqual(plain(shopify.selectedPaymentOptions.value), [{ type: 'paymentOnDelivery', handle: 'cod' }]);
  assert.deepEqual(plain(shopify.availablePaymentOptions.value).map((option) => option.type), ['creditCard', 'paymentOnDelivery', 'manualPayment']);
});

test('discount codes, allocations and cart metafields', () => {
  const state = next(
    { type: 'discountCode', action: 'add', code: 'SAVE10' },
    { type: 'metafield', change: { type: 'updateCartMetafield', namespace: 'n', key: 'k', valueType: 'boolean', value: 'true' } },
  );
  const { shopify } = runtime({ state });
  assert.deepEqual(plain(shopify.discountCodes.value), [{ code: 'SAVE10' }]);
  const [line] = shopify.lines.value;
  assert.deepEqual(plain(line.discountAllocations), [{ type: 'code', code: 'SAVE10', discountedAmount: { amount: 4.9, currencyCode: 'EUR' } }]);
  assert.equal(next({ type: 'discountCode', action: 'add', code: 'SAVE10' }).lines[0].discountAllocations[0].class, 'PRODUCT', 'the state tells the class of each allocation');
  assert.deepEqual(plain(shopify.appMetafields.value)[0].metafield, { namespace: 'n', key: 'k', value: 'true', type: 'boolean', valueType: 'boolean' });
});

test('calls of the API become messages to the page and resolve with the answer', async () => {
  const { shopify, posted, receive } = runtime();
  const pending = shopify.applyCartLinesChange({ type: 'addCartLine', merchandiseId: 'gid://shopify/ProductVariant/202', quantity: 1 });
  assert.deepEqual(posted, [{ type: 'change', id: 1, method: 'applyCartLinesChange', change: { type: 'addCartLine', merchandiseId: 'gid://shopify/ProductVariant/202', quantity: 1 } }]);
  const second = shopify.applyNoteChange({ type: 'updateNote', note: 'x' });
  receive({ type: 'result', id: 2, result: { type: 'error', message: 'no' } });
  receive({ type: 'result', id: 1, result: { type: 'success' } });
  assert.deepEqual(plain(await pending), { type: 'success' });
  assert.deepEqual(plain(await second), { type: 'error', message: 'no' });
  for (const method of ['applyAttributeChange', 'applyDiscountCodeChange', 'applyMetafieldChange', 'applyShippingAddressChange', 'applyGiftCardChange']) {
    assert.equal(typeof shopify[method], 'function', method);
  }
});

test('storage, session token and the Storefront API', async () => {
  const { shopify, posted, receive } = runtime();
  const read = shopify.storage.read('k');
  assert.deepEqual(posted.at(-1), { type: 'storage', id: 1, op: 'read', key: 'k' });
  receive({ type: 'result', id: 1, result: { a: 1 } });
  assert.deepEqual(plain(await read), { a: 1 });
  const write = shopify.storage.write('k', [1]);
  assert.deepEqual(posted.at(-1), { type: 'storage', id: 2, op: 'write', key: 'k', value: [1] });
  receive({ type: 'result', id: 2, result: null });
  assert.equal(await write, undefined);
  const token = shopify.sessionToken.get();
  assert.equal(posted.at(-1).type, 'sessionToken');
  receive({ type: 'result', id: 3, result: 'a.b.c' });
  assert.equal(await token, 'a.b.c');
  await assert.rejects(() => shopify.query('{ shop { name } }'), /Storefront API is not available/);
  assert.equal(await shopify.analytics.publish('event', { a: 1 }), true);
  assert.deepEqual(posted.at(-1), { type: 'analytics', name: 'event', data: { a: 1 } });
});

test('targets add their own members', () => {
  const state = next({ type: 'shippingAddress', address: ADDRESS }, { type: 'deliveryOption', group: 'shipping', handle: 'standard' });
  const line = runtime({ target: 'purchase.checkout.cart-line-item.render-after', ctx: { lineId: 'gid://shopify/CartLine/2' }, state }).shopify;
  assert.equal(line.target.value.merchandise.title, 'Canvas tote');
  assert.equal(line.target.value.quantity, 2);
  assert.equal(typeof line.applyCartLinesChange, 'function');

  const item = runtime({ target: 'purchase.checkout.shipping-option-item.render-after', ctx: { group: 'shipping', option: 'standard' }, state }).shopify;
  assert.equal(item.target.value.handle, 'standard');
  assert.equal(item.isTargetSelected.value, true);
  assert.deepEqual(plain(item.renderMode), { overlay: false });
  const other = runtime({ target: 'purchase.checkout.shipping-option-item.render-after', ctx: { group: 'shipping', option: 'express' }, state }).shopify;
  assert.equal(other.isTargetSelected.value, false);

  const list = runtime({ target: 'purchase.checkout.shipping-option-list.render-after', state }).shopify;
  assert.equal(list.target.value.deliveryGroups.length, 1);
  assert.deepEqual(plain(list.deliverySelectionGroups.value).map((group) => [group.handle, group.selected]), [['standard', true], ['express', false]]);
});

test('thank-you targets see the order and cannot change the checkout', () => {
  const state = pageState({ ...engineCheckout(), status: 'completed', order: { id: 'gid://shopify/Order/9', number: '#1001' } });
  const { shopify } = runtime({ target: 'purchase.thank-you.block.render', state });
  assert.deepEqual(plain(shopify.orderConfirmation.value), { order: { id: 'gid://shopify/Order/9' }, number: '#1001', isFirstOrder: true });
  assert.equal(shopify.buyerJourney.completed.value, true);
  assert.equal(shopify.applyNoteChange, undefined);
  assert.equal(shopify.applyCartLinesChange, undefined);
});

test('a thank-you announcement can close itself', () => {
  const { shopify, posted } = runtime({ target: 'purchase.thank-you.announcement.render' });
  let closed = 0;
  shopify.announcement.addEventListener('close', () => closed++);
  shopify.announcement.close();
  assert.equal(closed, 1);
  assert.deepEqual(posted, [{ type: 'dismiss' }]);
});

test('buyer journey interceptors answer the page and are told the outcome', async () => {
  const { shopify, posted, receive } = runtime();
  const performed = [];
  const stop = await shopify.buyerJourney.intercept(({ canBlockProgress }) => ({
    behavior: canBlockProgress ? 'block' : 'allow',
    reason: 'No gifts today',
    errors: [{ message: 'Pick again', target: '$.cart' }],
    perform: (result) => performed.push(result.behavior),
  }));
  assert.deepEqual(posted.at(-1), { type: 'interceptors', count: 1 });
  receive({ type: 'intercept', id: 'a', canBlockProgress: true });
  await settle();
  assert.deepEqual(posted.at(-1), { type: 'intercepted', id: 'a', results: [{ behavior: 'block', reason: 'No gifts today', errors: [{ message: 'Pick again', target: '$.cart' }] }] });
  receive({ type: 'perform', behavior: 'block' });
  await settle();
  assert.deepEqual(performed, ['block']);

  receive({ type: 'intercept', id: 'b', canBlockProgress: false });
  await settle();
  assert.deepEqual(posted.at(-1).results.map((result) => result.behavior), ['allow'], 'asked not to block: the interceptor allows');
  assert.equal(posted.some((message) => message.type === 'log' && message.level === 'warn'), false, 'asked not to block: nothing to warn about');

  await stop();
  assert.deepEqual(posted.at(-1), { type: 'interceptors', count: 0 });
});

test('an extension that blocks without the capability is overruled and warned', async () => {
  const { shopify, posted, receive } = runtime();
  await shopify.buyerJourney.intercept(() => ({ behavior: 'block', reason: 'Nope' }));
  receive({ type: 'intercept', id: 'a', canBlockProgress: false });
  await settle();
  assert.deepEqual(posted.at(-1).results, [{ behavior: 'allow' }]);
  assert.match(posted.find((message) => message.type === 'log').args[0], /block_progress/);
});

test('an interceptor that throws does not stop the others', async () => {
  const { shopify, posted, receive } = runtime();
  await shopify.buyerJourney.intercept(() => {
    throw new Error('boom');
  });
  await shopify.buyerJourney.intercept(() => ({ behavior: 'allow' }));
  receive({ type: 'intercept', id: 'a', canBlockProgress: true });
  await settle();
  assert.deepEqual(posted.at(-1).results.map((result) => result.behavior), ['allow']);
  assert.match(posted.find((message) => message.type === 'log').args[0], /boom/);
});

test('uses the Signal class the extension hands over', () => {
  const { shopify, refresh } = runtime();
  const early = shopify.note;
  class Signal {
    constructor(value) {
      this.value = value;
      Signal.made.push(this);
    }
  }
  Signal.made = [];
  shopify.setSignals(Signal);
  const lines = shopify.lines;
  assert.ok(lines instanceof Signal, 'signals made after setSignals are of that class');
  assert.notEqual(shopify.note, early, 'signals read before are made again');
  assert.equal(lines.value.length, 2);
  refresh(next({ type: 'cartLines', change: { type: 'removeCartLine', id: 'gid://shopify/CartLine/2', quantity: 2 } }));
  assert.equal(lines.value.length, 1, 'the extension\'s own signal is the one that is updated');
});

test('settings and translations', () => {
  const { shopify } = runtime({
    ext: { settings: { max_length: 80 }, translations: { en: { greeting: 'Hello {{name}}', items: { one: '{{count}} item', other: '{{count}} items' }, nested: { deep: 'Deep' } }, fr: { greeting: 'Bonjour {{name}}' } } },
  });
  assert.deepEqual(plain(shopify.settings.value), { max_length: 80 });
  assert.equal(shopify.i18n.translate('greeting', { name: 'Ada' }), 'Hello Ada');
  assert.equal(shopify.i18n.translate('items', { count: 1 }), '1 item');
  assert.equal(shopify.i18n.translate('items', { count: 3 }), '3 items');
  assert.equal(shopify.i18n.translate('nested.deep'), 'Deep');
  assert.equal(shopify.i18n.translate('missing'), 'missing');
  assert.equal(shopify.i18n.formatCurrency(12.5), '€12.50');
  assert.equal(shopify.i18n.formatNumber(1234.5), '1,234.5');
});

function inLocale(locale, translations, defaultLocale) {
  const state = structuredClone(checkoutState());
  state.shop = { ...state.shop, locale };
  return runtime({ state, ext: { translations, defaultLocale } }).shopify;
}

test('translations fall back from the locale to its language to the declared default', () => {
  const translations = { fr: { hello: 'Bonjour' }, en: { hello: 'Hello' }, 'pt-BR': { hello: 'Oi' } };
  // fr.default.json, en.json and a buyer in German: the default is French, not English.
  const german = inLocale('de', translations, 'fr');
  assert.equal(german.i18n.translate('hello'), 'Bonjour');
  assert.equal(german.localization.extensionLanguage.value.isoCode, 'fr');
  assert.equal(german.localization.language.value.isoCode, 'de', 'the language of the buyer is still theirs');
  // The exact locale, then the language of the locale.
  const brazil = inLocale('pt-BR', translations, 'fr');
  assert.equal(brazil.i18n.translate('hello'), 'Oi');
  assert.equal(brazil.localization.extensionLanguage.value.isoCode, 'pt-BR');
  const english = inLocale('en-GB', translations, 'fr');
  assert.equal(english.i18n.translate('hello'), 'Hello');
  assert.equal(english.localization.extensionLanguage.value.isoCode, 'en');
  // A key that no locale has is the key.
  assert.equal(german.i18n.translate('missing'), 'missing');
  // No declared default: English, as before.
  assert.equal(inLocale('de', translations).localization.extensionLanguage.value.isoCode, 'en');
  // No translations at all: the language of the buyer.
  assert.equal(inLocale('de', {}).localization.extensionLanguage.value.isoCode, 'de');
});

test('translations are resolved key by key: locale, language, default, key', () => {
  const translations = {
    'fr-CA': { a: 'a fr-CA' },
    fr: { a: 'a fr', b: 'b fr' },
    en: { a: 'a en', b: 'b en', c: 'c en' },
  };
  const buyer = inLocale('fr-CA', translations, 'en');
  assert.equal(buyer.i18n.translate('a'), 'a fr-CA');
  assert.equal(buyer.i18n.translate('b'), 'b fr', 'only in the language');
  assert.equal(buyer.i18n.translate('c'), 'c en', 'only in the default');
  assert.equal(buyer.i18n.translate('d'), 'd');
  // The extension is still rendered in the best dictionary.
  assert.equal(buyer.localization.extensionLanguage.value.isoCode, 'fr-CA');
  // A dictionary without the key does not hide it from the next one, nested keys included.
  const nested = inLocale('fr-CA', { 'fr-CA': { x: { y: 'y fr-CA' } }, en: { x: { y: 'y en', z: 'z en' } } }, 'en');
  assert.equal(nested.i18n.translate('x.y'), 'y fr-CA');
  assert.equal(nested.i18n.translate('x.z'), 'z en');
});

test('a declared default ends the fallback, and no default is what the server sends as null', () => {
  const translations = { fr: { hello: 'Bonjour' }, en: { hello: 'Hello', only: 'English only' } };
  // French is the default: a key it lacks is the key, not the English string.
  const german = inLocale('de', translations, 'fr');
  assert.equal(german.i18n.translate('only'), 'only');
  // No `.default.json`: the server sends `defaultLocale: null`, and English stands in.
  const none = inLocale('de', { en: { hello: 'Hello' } }, null);
  assert.equal(none.i18n.translate('hello'), 'Hello');
  assert.equal(none.localization.extensionLanguage.value.isoCode, 'en');
});

test('plurals follow the language of the string, numbers the locale of the buyer', () => {
  const translations = {
    fr: { items: { one: '{{count}} article', other: '{{count}} articles' } },
    en: { items: { one: '{{count}} item', other: '{{count}} items' } },
  };
  // French counts 0 as `one`, German and English as `other`.
  const german = inLocale('de', translations, 'fr');
  assert.equal(german.i18n.translate('items', { count: 0 }), '0 article');
  assert.equal(german.i18n.translate('items', { count: 2 }), '2 articles');
  assert.equal(german.i18n.formatNumber(1234.5), '1.234,5');
  // The string of a partial dictionary that falls back to another language: that one's rules.
  const partial = inLocale('de', { de: { hello: 'Hallo' }, ...translations }, 'fr');
  assert.equal(partial.i18n.translate('items', { count: 0 }), '0 article');
  const english = inLocale('en', translations, 'fr');
  assert.equal(english.i18n.translate('items', { count: 0 }), '0 items');
  // Strings with no locale at all use the rules of the buyer.
  const flat = inLocale('fr', { items: { one: '{{count}} one', other: '{{count}} other' } });
  assert.equal(flat.i18n.translate('items', { count: 0 }), '0 one');
});

test('the one-page checkout is the step `checkout`, the thank-you page `thank-you`', () => {
  const { shopify } = runtime();
  assert.equal(shopify.buyerJourney.activeStep.value.handle, 'checkout');
  assert.deepEqual(plain(shopify.buyerJourney.steps.value).map((step) => step.handle), ['checkout']);
  const state = pageState({ ...engineCheckout(), status: 'completed', order: { id: 'gid://shopify/Order/9', number: '#1001' } });
  const done = runtime({ target: 'purchase.thank-you.block.render', state }).shopify;
  assert.equal(done.buyerJourney.activeStep.value.handle, 'thank-you');
  assert.deepEqual(plain(done.buyerJourney.steps.value).map((step) => step.handle), ['thank-you']);
});

test('registers targets with extend and offers the rest of the global', () => {
  const { shopify, registered, posted } = runtime();
  const render = () => {};
  shopify.extend('purchase.checkout.block.render', render);
  assert.equal(registered.get('purchase.checkout.block.render'), render);
  shopify.reload();
  assert.deepEqual(posted.at(-1), { type: 'reload' });
  assert.equal(shopify.localization.currency.value.isoCode, 'EUR');
  assert.equal(shopify.shop.name, 'Local Store');
  assert.equal(shopify.shop.myshopifyDomain, 'local-store.myshopify.com');
  assert.equal(shopify.experimentalIsShopAppStyle, false);
});

test('capabilities become names', () => {
  const { capabilityNames } = runtime().win.context.LsfSandbox;
  assert.deepEqual(plain(capabilityNames({ api_access: true, network_access: false, collect_buyer_consent: { sms_marketing: true, customer_personal_data: false } })), ['api_access', 'collect_buyer_consent.sms_marketing']);
});

test('the cost of a line is before order discounts, after product ones', () => {
  const state = structuredClone(checkoutState());
  const eur = (amount) => ({ amount, currencyCode: 'EUR' });
  // 49.00 - 2.00 (product) - 5.00 (order) = 42.00 in the state.
  state.lines[0].total = eur(42);
  state.lines[0].discountAllocations = [
    { title: 'Summer', amount: eur(2), class: 'PRODUCT' },
    { title: 'Welcome', code: 'WELCOME', amount: eur(5), class: 'ORDER' },
  ];
  // A line without any order discount keeps its total, and so does one that has no class at all.
  state.lines[1].total = eur(22);
  state.lines[1].discountAllocations = [{ title: 'Old', amount: eur(2) }];
  const { shopify } = runtime({ state });
  const [shirt, mug] = shopify.lines.value;
  assert.deepEqual(plain(shirt.cost.totalAmount), eur(47), 'the order discount is not taken off');
  assert.deepEqual(plain(mug.cost.totalAmount), eur(22));
  assert.deepEqual(plain(shirt.discountAllocations).map((entry) => entry.discountedAmount.amount), [2, 5], 'the allocations still list both');
  assert.equal(shopify.cost.totalAmount.value.amount, 73, 'the cart total is not a line cost');
});

test('the cost of a line adds order discounts up in cents', () => {
  const state = structuredClone(checkoutState());
  const eur = (amount) => ({ amount, currencyCode: 'EUR' });
  state.lines[0].total = eur(10.1);
  state.lines[0].discountAllocations = [
    { title: 'A', amount: eur(0.1), class: 'ORDER' },
    { title: 'B', amount: eur(0.1), class: 'ORDER' },
  ];
  const { shopify } = runtime({ state });
  assert.equal(shopify.lines.value[0].cost.totalAmount.amount, 10.3);
});

test('the components of a bundle line are costed the same way', () => {
  const state = structuredClone(checkoutState());
  const eur = (amount) => ({ amount, currencyCode: 'EUR' });
  state.lines[0].components = [{
    id: 'c1', variantId: 1, productId: 1, title: 'Part', quantity: 1, total: eur(8),
    discountAllocations: [{ title: 'Welcome', amount: eur(2), class: 'ORDER' }],
  }];
  const { shopify } = runtime({ state });
  assert.equal(shopify.lines.value[0].lineComponents[0].cost.totalAmount.amount, 10);
});

// The state the server wrote for a logged-in customer buying in French
// (`crates/cli/src/server/tests.rs` checks the server still writes exactly this file).
const SERVER = JSON.parse(readFileSync(join(assets, '..', 'tests', 'fixtures', 'server-buyer-state.json'), 'utf8'));

function serverRuntime(name) {
  const state = structuredClone(checkoutState());
  state.buyer = { ...state.buyer, ...SERVER[name].buyer };
  state.localization = SERVER[name].localization;
  return runtime({ state }).shopify;
}

test('the Buyer Identity API reads the customer the server serialized', () => {
  const { buyerIdentity } = serverRuntime('customer');
  assert.equal(buyerIdentity.email.value, 'jane.doe@example.com');
  assert.equal(buyerIdentity.phone.value, '+1 555 010 9988');
  assert.deepEqual(plain(buyerIdentity.customer.value), {
    id: 'gid://shopify/Customer/6523834743460',
    email: 'jane.doe@example.com',
    phone: '+1 555 010 9988',
    fullName: 'Jane Doe',
    firstName: 'Jane',
    lastName: 'Doe',
    image: { url: '' },
    acceptsMarketing: true,
    acceptsEmailMarketing: true,
    acceptsSmsMarketing: false,
    storeCreditAccounts: [],
    ordersCount: 2,
  });
  assert.equal(buyerIdentity.purchasingCompany.value, undefined);
});

test('the Buyer Identity API reads the company a B2B customer buys for', () => {
  const { buyerIdentity } = serverRuntime('company');
  const customer = buyerIdentity.customer.value;
  assert.equal(customer.phone, undefined, 'a customer without a phone has none');
  assert.equal(customer.acceptsMarketing, false);
  assert.equal(customer.ordersCount, 0);
  assert.deepEqual(plain(buyerIdentity.purchasingCompany.value), {
    company: { id: 'gid://shopify/Company/8662601194560', name: 'Northwind Hotels', externalId: 'NW-001' },
    location: { id: 'gid://shopify/CompanyLocation/3095219877716', name: 'Northwind Seattle', externalId: 'NW-SEA' },
  });
});

test('the Localization API reads the language, country, currency and market of the server', () => {
  const { localization, i18n } = serverRuntime('customer');
  // The shop is in English: the buyer reads French.
  assert.equal(localization.language.value.isoCode, 'fr');
  assert.equal(localization.extensionLanguage.value.isoCode, 'fr');
  assert.equal(localization.country.value.isoCode, 'CA', 'the country the visitor selected');
  assert.equal(localization.currency.value.isoCode, 'EUR', 'the currency of the checkout');
  assert.deepEqual(plain(localization.market.value), { id: 'gid://shopify/Market/1', handle: 'primary' });
  // Numbers follow the language of the buyer.
  assert.equal(i18n.formatNumber(1234.5), '1\u202f234,5');
});

test('the country of the shipping address wins over the one the visitor selected', () => {
  const state = structuredClone(checkoutState());
  state.localization = { ...SERVER.customer.localization, country: { isoCode: 'GB', name: 'United Kingdom' } };
  assert.equal(runtime({ state }).shopify.localization.country.value.isoCode, 'GB');
});
