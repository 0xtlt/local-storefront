import assert from 'node:assert/strict';
import { test } from 'node:test';
import { fieldForTarget, missingFields, op, operationsForChange, placeErrors } from '../operations.js';

const ops = (method, change, state) => operationsForChange(method, change, state);

test('cart line changes go through as they are', () => {
  const add = { type: 'addCartLine', merchandiseId: 'gid://shopify/ProductVariant/1', quantity: 2 };
  assert.deepEqual(ops('applyCartLinesChange', add), { operations: [{ type: 'cartLines', change: add }] });
  const update = { type: 'updateCartLine', id: 'gid://shopify/CartLine/1', quantity: 0 };
  assert.deepEqual(ops('applyCartLinesChange', update).operations[0].change, update);
});

test('refuses cart line changes the API refuses', () => {
  assert.match(ops('applyCartLinesChange', { type: 'addCartLine', quantity: 1 }).error, /merchandiseId/);
  assert.match(ops('applyCartLinesChange', { type: 'addCartLine', merchandiseId: 'x', quantity: 0 }).error, /quantity/);
  assert.match(ops('applyCartLinesChange', { type: 'removeCartLine', id: 'x' }).error, /quantity/);
  assert.match(ops('applyCartLinesChange', { type: 'nope' }).error, /Unknown cart line change/);
});

test('attributes and the note', () => {
  assert.deepEqual(ops('applyAttributeChange', { type: 'updateAttribute', key: 'k', value: 5 }).operations, [
    { type: 'attribute', change: { type: 'updateAttribute', key: 'k', value: '5' } },
  ]);
  assert.deepEqual(ops('applyAttributeChange', { type: 'removeAttribute', key: 'k' }).operations[0].change, { type: 'removeAttribute', key: 'k' });
  assert.match(ops('applyAttributeChange', { type: 'updateAttribute', value: '1' }).error, /key/);
  assert.deepEqual(ops('applyNoteChange', { type: 'updateNote', note: 'hi' }).operations, [{ type: 'note', change: { type: 'updateNote', note: 'hi' } }]);
  assert.deepEqual(ops('applyNoteChange', { type: 'removeNote' }).operations, [{ type: 'note', change: { type: 'removeNote' } }]);
});

test('discount codes become add and remove', () => {
  assert.deepEqual(ops('applyDiscountCodeChange', { type: 'addDiscountCode', code: ' SAVE10 ' }).operations, [op.discountCode('add', 'SAVE10')]);
  assert.deepEqual(ops('applyDiscountCodeChange', { type: 'removeDiscountCode', code: 'SAVE10' }).operations, [op.discountCode('remove', 'SAVE10')]);
  assert.match(ops('applyDiscountCodeChange', { type: 'addDiscountCode', code: '' }).error, /code/);
});

test('metafield changes, current and older, become one flat shape', () => {
  assert.deepEqual(
    ops('applyMetafieldChange', { type: 'updateCartMetafield', metafield: { namespace: 'n', key: 'k', type: 'boolean', value: 'true' } }).operations,
    [{ type: 'metafield', change: { type: 'updateCartMetafield', namespace: 'n', key: 'k', valueType: 'boolean', value: 'true' } }],
  );
  assert.deepEqual(
    ops('applyMetafieldChange', { type: 'updateMetafield', namespace: 'n', key: 'k', valueType: 'string', value: 'v' }).operations[0].change,
    { type: 'updateMetafield', namespace: 'n', key: 'k', valueType: 'string', value: 'v' },
  );
  assert.deepEqual(ops('applyMetafieldChange', { type: 'removeCartMetafield', namespace: 'n', key: 'k' }).operations[0].change, {
    type: 'removeCartMetafield', namespace: 'n', key: 'k',
  });
  assert.match(ops('applyMetafieldChange', { type: 'removeCartMetafield' }).error, /key/);
});

test('a shipping address change merges into the address and splits a name', () => {
  const state = { shippingAddress: { city: 'Paris', countryCode: 'FR' } };
  assert.deepEqual(ops('applyShippingAddressChange', { type: 'updateShippingAddress', address: { zip: '75001', name: 'Ada Lovelace King', oneTimeUse: true } }, state).operations, [
    op.shippingAddress({ city: 'Paris', countryCode: 'FR', zip: '75001', firstName: 'Ada', lastName: 'Lovelace King' }),
  ]);
});

test('says what a local checkout cannot do', () => {
  assert.match(ops('applyGiftCardChange', { type: 'addGiftCard', code: 'x' }).error, /not supported/);
  assert.match(ops('applyPaymentMethodAttributesChange', { type: 'x' }).error, /not supported/);
  assert.match(ops('applyWhatever', { type: 'x' }).error, /not part of/);
  assert.match(ops('applyNoteChange', null).error, /no change/);
});

test('finds the field a validation target points at', () => {
  assert.equal(fieldForTarget('$.cart.buyerIdentity.email'), 'email');
  assert.equal(fieldForTarget('$.cart.buyerIdentity.phone'), 'phone');
  assert.equal(fieldForTarget('$.cart.deliveryGroups[0].deliveryAddress.zip'), 'zip');
  assert.equal(fieldForTarget('$.cart.deliveryGroups[0].deliveryAddress.provinceCode'), 'provinceCode');
  assert.equal(fieldForTarget('$.cart.paymentMethods'), 'payment');
  assert.equal(fieldForTarget('$.cart'), null);
  assert.equal(fieldForTarget(undefined), null);
});

test('places errors by field, the rest as general', () => {
  const placed = placeErrors([
    { message: 'Enter an email', target: '$.cart.buyerIdentity.email' },
    { message: 'Too many', target: '$.cart' },
    { message: 'Nothing to say where' },
  ]);
  assert.deepEqual(placed, { fields: { email: ['Enter an email'] }, general: ['Too many', 'Nothing to say where'] });
});

test('lists what is missing before paying', () => {
  const state = { lines: [{ requiresShipping: true }], shippingAddress: null, deliveryGroups: [], paymentMethods: [{ handle: 'card' }], selectedPayment: null, buyer: {} };
  assert.deepEqual(missingFields(state), ['email', 'lastName', 'address1', 'city', 'countryCode', 'payment']);
  const filled = {
    ...state,
    buyer: { email: 'a@b.c' },
    shippingAddress: { lastName: 'L', address1: 'A', city: 'C', countryCode: 'FR' },
    deliveryGroups: [{ selected: null }],
    selectedPayment: 'card',
  };
  assert.deepEqual(missingFields(filled), ['delivery']);
  assert.deepEqual(missingFields({ ...filled, deliveryGroups: [{ selected: 'standard' }] }), []);
  assert.deepEqual(missingFields({ ...state, lines: [{ requiresShipping: false }], buyer: { email: 'a@b.c' }, selectedPayment: 'card' }), [], 'nothing to ship, no address');
  assert.deepEqual(missingFields(state, { email: 'typed@b.c', address: { lastName: 'L', address1: 'A', city: 'C', countryCode: 'FR' } }), ['payment'], 'drafts count');
});
