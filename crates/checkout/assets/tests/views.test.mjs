// The pages as virtual DOM: what each state of the checkout shows.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { withCurrency } from '../format.js';
import { update } from '../preview/mock-engine.mjs';
import { RENDERED_TARGETS } from '../extension-host.js';
import { h } from '../vdom.js';
import { defaultRunView, jsonCode, renderPage, runView, selectedRun } from '../views.js';
import { checkoutState, engineCheckout, pageState } from './harness.mjs';

const walk = (node, visit) => {
  visit(node);
  for (const child of node.children) walk(child, visit);
};
const findAll = (tree, predicate) => {
  const found = [];
  walk(tree, (node) => predicate(node) && found.push(node));
  return found;
};
const has = (node, name) => (node.props.class ?? '').split(/\s+/).includes(name);
const byClass = (tree, name) => findAll(tree, (node) => has(node, name));
const text = (node) => (node.tag === '#text' ? node.text : node.children.map(text).join(''));

function page(state, ui = {}, { slots = [], refusals } = {}) {
  const actions = new Proxy({}, { get: () => () => {} });
  const slot = (target, instance) => {
    slots.push([target, instance]);
    return null;
  };
  return renderPage({ state, ui: { draft: { address: {} }, ...ui }, slot, actions, refusals, location: 'http://localhost/checkouts/cn/preview' });
}

/** The sample checkout after `operations`, as the page gets it. */
function after(...operations) {
  return pageState(update(engineCheckout(), operations).state);
}

const ADDRESS = { lastName: 'Martin', address1: '1 rue de Rivoli', city: 'Paris', countryCode: 'FR' };

test('the one-page checkout has its steps in order', () => {
  const tree = page(checkoutState());
  assert.equal(tree.props['data-variant'], 'standard');
  assert.deepEqual(findAll(tree, (node) => node.tag === 'section').map((node) => node.props['data-section']), ['contact', 'delivery', 'shipping', 'payment']);
  assert.equal(text(byClass(tree, 'lsf-pay')[0]), 'Pay now');
  assert.match(text(byClass(tree, 'lsf-summary-toggle')[0]), /Show order summary.*€73\.00/);
});

test('the Shop skin changes the words and the data attribute, not the steps', () => {
  const tree = page(checkoutState({ variant: 'shop' }));
  assert.equal(tree.props['data-variant'], 'shop');
  assert.equal(text(byClass(tree, 'lsf-shop-logo')[0]), 'shop');
  assert.match(text(byClass(tree, 'lsf-pay')[0]), /^Pay now · €73\.00$/);
  assert.equal(text(findAll(tree, (node) => node.tag === 'h2')[1]), 'Ship to');
});

const countryOptions = (tree) =>
  findAll(tree, (node) => node.tag === 'select' && node.props.name === 'countryCode')[0]
    .children.filter((node) => node.tag === 'option' && node.props.value)
    .map((node) => [node.props.value, text(node)]);

test('the country selector lists the countries of the store', () => {
  const state = checkoutState();
  state.shop = { ...state.shop, countries: [{ code: 'FR', name: 'France' }, { code: 'BE', name: 'Belgique' }] };
  assert.deepEqual(countryOptions(page(state)), [['FR', 'France'], ['BE', 'Belgique']]);
  // The country already on the address stays selectable.
  state.shippingAddress = { ...ADDRESS, countryCode: 'JP' };
  assert.deepEqual(countryOptions(page(state)).map(([code]) => code), ['FR', 'BE', 'JP']);
  // Without countries, or with none, the default list.
  for (const countries of [undefined, []]) {
    const bare = checkoutState();
    bare.shop = { ...bare.shop, countries };
    assert.ok(countryOptions(page(bare)).length >= 20);
    assert.ok(countryOptions(page(bare)).some(([code]) => code === 'US'));
  }
});

test('shipping methods wait for the address', () => {
  const before = page(checkoutState());
  assert.match(text(byClass(before, 'lsf-placeholder')[0]), /Enter your shipping address/);
  const withAddress = page(after({ type: 'shippingAddress', address: ADDRESS }));
  assert.equal(byClass(withAddress, 'lsf-placeholder').length, 0);
  const cards = byClass(withAddress, 'lsf-radio-card').filter((card) => card.props.key === 'standard' || card.props.key === 'express');
  assert.equal(cards.length, 2);
  assert.match(text(cards[0]), /Standard.*3 to 5 business days.*€4\.90/);
  assert.match(text(cards[1]), /Express.*€12\.00/);
});

test('shows the chosen shipping method and the total with it', () => {
  const state = after({ type: 'shippingAddress', address: ADDRESS }, { type: 'deliveryOption', group: 'shipping', handle: 'standard' });
  const tree = page(state);
  assert.equal(byClass(tree, 'is-selected').filter((node) => node.props.key === 'standard').length, 1);
  const total = byClass(tree, 'lsf-total')[0];
  assert.match(text(total), /EUR.*€77\.90/);
});

test('shows discount codes, what they took off and an ignored code', () => {
  const state = after({ type: 'discountCode', action: 'add', code: 'save10' }, { type: 'discountCode', action: 'add', code: 'nope' });
  const tree = page(state, { discountError: 'Enter a valid discount code' });
  const codes = byClass(tree, 'lsf-code');
  assert.deepEqual(codes.map((code) => has(code, 'is-ignored')), [false, true]);
  assert.match(text(codes[1]), /NOPE.*Not applied/);
  assert.match(text(byClass(tree, 'lsf-cost-discount')[0]), /SAVE10.*−€7\.30/);
  assert.match(text(byClass(tree, 'lsf-field-error')[0]), /Enter a valid discount code/);
  assert.match(text(byClass(tree, 'lsf-line')[0]), /SAVE10 \(−€4\.90\)/);
});

test('the pay button says what paying will do', () => {
  assert.equal(text(byClass(page(after({ type: 'paymentMethod', handle: 'card' })), 'lsf-pay')[0]), 'Pay now');
  assert.equal(text(byClass(page(after({ type: 'paymentMethod', handle: 'cod' })), 'lsf-pay')[0]), 'Complete order');
  assert.equal(text(byClass(page(checkoutState(), { busy: true }), 'lsf-pay')[0]), 'Processing…');
});

test('what is missing shows once the buyer tried to pay', () => {
  const quiet = page(checkoutState());
  assert.equal(byClass(quiet, 'lsf-field-error').length, 0);
  const tried = page(checkoutState(), { tried: true });
  const errors = byClass(tried, 'lsf-field-error').map(text);
  assert.ok(errors.includes('Enter an email'));
  assert.ok(errors.includes('Enter an address'));
  assert.ok(errors.includes('Select a payment method'));
  assert.equal(findAll(tried, (node) => node.props['aria-invalid'] === 'true').length >= 3, true);
});

test('errors of the checkout go to their field or to a banner', () => {
  const state = checkoutState({
    errors: [
      { message: 'Use a work email', target: '$.cart.buyerIdentity.email', function: 'x' },
      { message: 'Order too large', target: '$.cart', function: 'y' },
    ],
  });
  const tree = page(state);
  const email = byClass(tree, 'lsf-field').find((node) => node.props['data-field'] === 'email');
  assert.match(text(byClass(email, 'lsf-field-error')[0]), /Use a work email/);
  assert.match(text(findAll(tree, (node) => node.props['data-banner'] === 'errors')[0]), /Order too large/);
});

test('extensions that block the buyer are heard', () => {
  const tree = page(checkoutState(), { block: { reason: 'The gift message is empty.', errors: [{ message: 'Write a message.' }] } });
  assert.match(text(findAll(tree, (node) => node.props['data-banner'] === 'blocked')[0]), /The gift message is empty\.Write a message\./);
});

test('asks for a slot at each target, with an instance for each line', () => {
  const slots = [];
  page(after({ type: 'shippingAddress', address: ADDRESS }, { type: 'deliveryOption', group: 'shipping', handle: 'standard' }), {}, { slots });
  const targets = slots.map(([target]) => target);
  for (const expected of [
    'purchase.checkout.header.render-after',
    'purchase.checkout.contact.render-after',
    'purchase.checkout.delivery-address.render-before',
    'purchase.checkout.delivery-address.render-after',
    'purchase.checkout.shipping-option-list.render-before',
    'purchase.checkout.shipping-option-list.render-after',
    'purchase.checkout.shipping-option-item.render-after',
    'purchase.checkout.shipping-option-item.details.render',
    'purchase.checkout.block.render',
    'purchase.checkout.payment-method-list.render-before',
    'purchase.checkout.payment-method-list.render-after',
    'purchase.checkout.actions.render-before',
    'purchase.checkout.footer.render-after',
    'purchase.checkout.cart-line-item.render-after',
    'purchase.checkout.cart-line-list.render-after',
    'purchase.checkout.reductions.render-before',
    'purchase.checkout.reductions.render-after',
  ]) {
    assert.ok(targets.includes(expected), `${expected} is rendered`);
  }
  const lineSlots = slots.filter(([target]) => target === 'purchase.checkout.cart-line-item.render-after');
  assert.deepEqual(lineSlots.map(([, instance]) => instance.lineId), ['gid://shopify/CartLine/1', 'gid://shopify/CartLine/2']);
  const options = slots.filter(([target]) => target === 'purchase.checkout.shipping-option-item.render-after');
  assert.deepEqual(options.map(([, instance]) => instance.option), ['standard', 'express']);
});

test('the thank-you page shows the order and the thank-you targets', () => {
  const slots = [];
  const done = withCurrency({ ...after({ type: 'email', value: 'a@b.c' }, { type: 'shippingAddress', address: { ...ADDRESS, firstName: 'Ada' } }, { type: 'deliveryOption', group: 'shipping', handle: 'express' }, { type: 'paymentMethod', handle: 'card' }), status: 'completed', order: { id: 'gid://shopify/Order/1', number: '#1001' } });
  const tree = page(done, {}, { slots });
  assert.equal(tree.props['data-status'], 'completed');
  assert.equal(text(findAll(tree, (node) => node.tag === 'h1')[0]), 'Thank you, Ada!');
  assert.equal(text(byClass(tree, 'lsf-muted').find((node) => node.props['data-test'] === 'confirmation')), 'Confirmation #1001');
  const details = text(byClass(tree, 'lsf-details')[0]);
  assert.match(details, /a@b\.c/);
  assert.match(details, /1 rue de Rivoli/);
  assert.match(details, /Express · €12\.00/);
  assert.match(details, /Credit card/);
  assert.equal(byClass(tree, 'lsf-pay').filter((node) => node.tag === 'button').length, 0, 'nothing to pay any more');
  const targets = slots.map(([target]) => target);
  for (const expected of ['purchase.thank-you.announcement.render', 'purchase.thank-you.header.render-after', 'purchase.thank-you.block.render', 'purchase.thank-you.customer-information.render-after', 'purchase.thank-you.cart-line-item.render-after', 'purchase.thank-you.cart-line-list.render-after', 'purchase.thank-you.footer.render-after']) {
    assert.ok(targets.includes(expected), `${expected} is rendered`);
  }
  assert.ok(!targets.some((target) => target.startsWith('purchase.checkout.')), 'no checkout target on the thank-you page');
});

/** The sample with its first line (49.00) sold as a pre-order whose deposit is 20.00. */
function preorder(overrides = {}) {
  const state = structuredClone(checkoutState(overrides));
  const eur = (amount) => ({ amount, currencyCode: 'EUR' });
  state.lines[0].sellingPlan = { id: 'gid://shopify/SellingPlan/5', name: 'Pre-order with a deposit', recurringDeliveries: false };
  state.lines[0].dueLater = eur(29);
  state.cost = { ...state.cost, dueNow: eur(state.cost.total.amount - 29), dueLater: eur(29), checkoutChargeAmount: eur(state.cost.subtotal.amount - 29) };
  return state;
}

test('a cart without a deferred payment shows one total, as before', () => {
  const tree = page(checkoutState());
  assert.equal(byClass(tree, 'lsf-cost-later').length, 0);
  assert.equal(byClass(tree, 'lsf-cost-order-total').length, 0);
  assert.match(text(byClass(tree, 'lsf-total')[0]), /^Total.*€73\.00$/);
  assert.equal(byClass(tree, 'lsf-plan').length, 0);
  // A state that predates the split is all due now.
  const old = structuredClone(checkoutState());
  delete old.cost.dueNow;
  delete old.cost.dueLater;
  assert.match(text(byClass(page(old), 'lsf-total')[0]), /€73\.00$/);
});

test('a deferred payment shows the total, what is due today and what is due later', () => {
  const tree = page(preorder());
  assert.match(text(byClass(tree, 'lsf-cost-order-total')[0]), /^Total€73\.00$/);
  assert.match(text(byClass(tree, 'lsf-total')[0]), /^Due today.*€44\.00$/);
  assert.match(text(byClass(tree, 'lsf-cost-later')[0]), /^Due later€29\.00$/);
  const line = text(byClass(tree, 'lsf-line')[0]);
  assert.match(line, /Pre-order with a deposit/);
  assert.match(line, /Due later €29\.00/);
  assert.doesNotMatch(text(byClass(tree, 'lsf-line')[1]), /Due later/);
});

test('the pay button of the Shop skin charges what is due today', () => {
  assert.match(text(byClass(page(preorder({ variant: 'shop' })), 'lsf-pay')[0]), /^Pay now · €44\.00$/);
  assert.match(text(byClass(page(preorder()), 'lsf-summary-toggle')[0]), /€73\.00/, 'the summary shows the order total');
});

test('the thank-you page shows the balance the order still owes', () => {
  const eur = (amount) => ({ amount, currencyCode: 'EUR' });
  const done = withCurrency({ ...preorder(), status: 'completed', selectedPayment: 'card', order: { id: 'gid://shopify/Order/1', number: '#1001', totalReceived: eur(44), totalOutstanding: eur(29) } });
  const tree = page(done);
  assert.match(text(findAll(tree, (node) => node.props['data-test'] === 'balance')[0]), /remaining balance of €29\.00 is due later/);
  const details = text(byClass(tree, 'lsf-details')[0]);
  assert.match(details, /Credit card · €44\.00/);
  assert.match(details, /Remaining balance€29\.00 due later/);
  assert.match(text(byClass(tree, 'lsf-total')[0]), /^Due today.*€44\.00$/);
  // An order without a balance has none of it.
  const paid = withCurrency({ ...checkoutState(), status: 'completed', selectedPayment: 'card', order: { id: 'gid://shopify/Order/2', number: '#1002' } });
  const plain = page(paid);
  assert.equal(findAll(plain, (node) => node.props['data-test'] === 'balance').length, 0);
  assert.doesNotMatch(text(byClass(plain, 'lsf-details')[0]), /Remaining balance/);
});

// --- The Functions: the badge and the dialog ---

const badge = (tree) => byClass(tree, 'lsf-fn-badge')[0];
const dialog = (tree) => byClass(tree, 'lsf-fn-dialog')[0];
const pres = (tree) => findAll(tree, (node) => node.tag === 'pre');
const withRuns = (runs, overrides = {}) => {
  const state = checkoutState(overrides);
  state.runs = runs.map((run) => ({ ...state.runs[0], ...run }));
  return state;
};

test('every page has the badge of the Functions, last, with the dialog after it', () => {
  const done = withCurrency({ ...checkoutState(), status: 'completed', order: { number: '#1001' } });
  const pages = {
    checkout: page(checkoutState()),
    shop: page(checkoutState({ variant: 'shop' })),
    'thank-you': page(done),
    'shop thank-you': page({ ...done, variant: 'shop' }),
    'empty cart': page({ ...checkoutState(), lines: [] }),
    'every target': everyTarget(checkoutState()).tree,
  };
  for (const [name, tree] of Object.entries(pages)) {
    const last = tree.children.at(-1);
    assert.equal(last.key, 'functions', `${name}: the badge and the dialog come last`);
    assert.deepEqual(last.children.map((node) => node.tag), ['button', 'dialog'], name);
    assert.equal(last.children[0].props['aria-haspopup'], 'dialog', name);
  }
  const button = badge(pages.checkout);
  assert.match(text(button), /Functions.*2 runs/);
  assert.equal(button.props['aria-label'], 'Functions, 2 runs');
  assert.equal(button.props['data-status'], 'ok');
});

test('the badge turns critical when a run failed or the checkout has errors', () => {
  const failed = badge(page(withRuns([{ handle: 'a', error: 'unreachable executed' }, { handle: 'b' }])));
  assert.equal(failed.props['data-status'], 'failed');
  assert.match(text(failed), /2 runs.*1 failed/);
  assert.equal(failed.props['aria-label'], 'Functions, 2 runs, 1 failed');

  const errors = badge(page(checkoutState({ errors: [{ message: 'No', target: '$.cart' }, { message: 'Never', target: '$.cart' }] })));
  assert.equal(errors.props['data-status'], 'failed');
  assert.match(text(errors), /2 errors/);
  assert.equal(errors.props['aria-label'], 'Functions, 2 runs, 2 checkout errors');

  const none = badge(page({ ...checkoutState(), runs: [] }));
  assert.equal(none.props['data-status'], 'ok');
  assert.match(text(none), /No runs/);
});

test('the badge has a short face (ƒ and the number of runs) and a full one, and its name says it all', () => {
  const state = withRuns([{ handle: 'a', error: 'unreachable executed' }, { handle: 'b' }, { handle: 'c' }, { handle: 'd' }], {
    errors: [{ message: 'No', target: '$.cart' }],
  });
  const button = badge(page(state));
  const [short] = byClass(button, 'lsf-fn-badge-short');
  const [full] = byClass(button, 'lsf-fn-badge-full');
  assert.equal(text(short), 'ƒ4');
  assert.equal(byClass(short, 'lsf-chip').length, 0, 'no chips on the short face');
  assert.match(text(full), /^Functions4 runs1 failed1 error$/);
  for (const face of [short, full]) assert.equal(face.props['aria-hidden'], 'true');
  assert.equal(button.props['aria-label'], 'Functions, 4 runs, 1 failed, 1 checkout error');
  assert.equal(button.props.title, button.props['aria-label'], 'the short face says it on hover');
  assert.equal(text(byClass(badge(page({ ...checkoutState(), runs: [] })), 'lsf-fn-badge-short')[0]), 'ƒ0');
});

test('the dialog is empty while it is closed', () => {
  const closed = dialog(page(checkoutState()));
  assert.equal(closed.props.closedby, 'any');
  assert.equal('open' in closed.props, false, 'the page opens it with showModal(), never with the attribute');
  assert.equal(closed.children.length, 0);
});

test('the dialog shows the runs on the left and the JSON of the selected one only, side by side', () => {
  const state = checkoutState();
  const tree = page(state, { functionsOpen: true, functionsView: runView(state.runs, 1) });
  const open = dialog(tree);
  assert.match(text(byClass(open, 'lsf-fn-head')[0]), /Functions.*2 runs/);
  const entries = byClass(open, 'lsf-fn-entry');
  assert.deepEqual(entries.map((node) => node.props['data-run'] ?? node.props['data-view']), ['overview', 'volume-discount', 'order-rules']);
  assert.deepEqual(entries.map((node) => node.props['aria-current']), [null, null, 'true']);
  assert.match(text(entries[1]), /volume-discount.*ok.*cart\.lines\.discounts\.generate\.run.*41,876 instructions/);

  const run = byClass(open, 'lsf-fn-run')[0];
  assert.equal(run.props['data-run'], 'order-rules');
  const panes = byClass(run, 'lsf-fn-panes');
  assert.equal(panes.length, 1, 'no HTTP exchange for a run that is not a fetch');
  assert.deepEqual(panes[0].children.map((node) => node.props['data-pane']), ['input', 'output']);
  assert.deepEqual(panes[0].children.map((node) => text(node.children[0].children[0])), ['Input', 'Output']);
  const json = pres(open).map(text);
  assert.equal(json.length, 2, 'only the JSON of the selected run is built');
  assert.equal(json[0], JSON.stringify(state.runs[1].input, null, 2));
  assert.equal(json[1], JSON.stringify(state.runs[1].output, null, 2));
});

test('each JSON has a button that copies it, and says when it did', () => {
  const state = checkoutState();
  const copied = [];
  const actions = new Proxy({}, { get: (_, name) => (name === 'copyJson' ? (...args) => copied.push(args) : () => {}) });
  const tree = renderPage({ state, ui: { draft: { address: {} }, functionsOpen: true, functionsView: runView(state.runs, 0) }, slot: () => null, actions, location: 'http://localhost/' });
  const buttons = byClass(tree, 'lsf-fn-copy');
  assert.deepEqual(buttons.map(text), ['Copy', 'Copy']);
  buttons[1].props.onClick();
  assert.equal(copied[0][1], JSON.stringify(state.runs[0].output, null, 2));
  const id = copied[0][0];
  const after = page(state, { functionsOpen: true, functionsView: runView(state.runs, 0), copied: id });
  assert.deepEqual(byClass(after, 'lsf-fn-copy').map(text), ['Copy', 'Copied']);
});

test('the dialog opens on the first run that failed, else on the first run, else on the overview', () => {
  const state = withRuns([{ handle: 'a' }, { handle: 'b', error: 'boom' }, { handle: 'c', error: 'bang' }]);
  assert.deepEqual(defaultRunView(state.runs), { handle: 'b', target: state.runs[0].target, occurrence: 0 });
  assert.deepEqual(defaultRunView(withRuns([{ handle: 'a' }, { handle: 'b' }]).runs).handle, 'a');
  assert.equal(defaultRunView([]), 'overview');
  // With no choice made, the page shows the same.
  const tree = page(state, { functionsOpen: true });
  assert.equal(byClass(tree, 'lsf-fn-run')[0].props['data-run'], 'b');
  const empty = page({ ...checkoutState(), runs: [] }, { functionsOpen: true, functionsView: 'overview' });
  assert.match(text(byClass(empty, 'lsf-fn-overview')[0]), /No Function ran\./);
});

test('the run shown is found again by its handle and target after an update, else the default', () => {
  const target = 'cart.lines.discounts.generate.run';
  const before = withRuns([{ handle: 'a', target }, { handle: 'b', target }, { handle: 'b', target }]);
  const view = runView(before.runs, 2);
  assert.deepEqual(view, { handle: 'b', target, occurrence: 1 });
  // A run before it went: the same run is now the second.
  const after = withRuns([{ handle: 'b', target }, { handle: 'b', target, logs: 'second' }]);
  assert.equal(selectedRun(after.runs, view), 1);
  const tree = page(after, { functionsOpen: true, functionsView: view });
  assert.deepEqual(byClass(tree, 'lsf-fn-entry').map((node) => node.props['aria-current']), [null, null, 'true']);
  // It went: the first that failed, else the first.
  const gone = withRuns([{ handle: 'a', target }, { handle: 'c', target, error: 'boom' }]);
  assert.equal(selectedRun(gone.runs, view), 1);
  assert.equal(selectedRun(withRuns([{ handle: 'a', target }]).runs, view), 0);
  assert.equal(selectedRun([], view), -1);
  assert.equal(selectedRun(after.runs, 'overview'), -1);
});

test('a fetch run shows the request it asked for and the response, side by side under its input and output', () => {
  const state = checkoutState();
  state.runs[0] = {
    ...state.runs[0],
    target: 'cart.lines.discounts.generate.fetch',
    exchange: {
      request: { method: 'POST', url: 'https://app.example/check', headers: [], body: null, readTimeoutMs: 500 },
      response: { status: 200, headers: [], body: '{"ok":true}' },
    },
  };
  const tree = page(state, { functionsOpen: true, functionsView: runView(state.runs, 0) });
  const [run, exchange] = byClass(tree, 'lsf-fn-panes');
  assert.equal(run.props['data-panes'], 'run');
  assert.equal(exchange.props['data-panes'], 'exchange');
  assert.deepEqual(exchange.children.map((node) => node.props['data-pane']), ['request', 'response']);
  assert.match(text(exchange.children[0]), /HTTP request.*"url": "https:\/\/app\.example\/check"/s);
  assert.match(text(exchange.children[1]), /HTTP response.*"status": 200/s);
  const other = page(state, { functionsOpen: true, functionsView: runView(state.runs, 1) });
  assert.equal(byClass(other, 'lsf-fn-panes').length, 1, 'only the fetch run has an exchange');
});

test('a failed run shows its error and counts as failed', () => {
  const state = checkoutState();
  state.runs[0] = { ...state.runs[0], error: 'unreachable executed', output: null };
  const tree = page(state, { functionsOpen: true });
  assert.match(text(byClass(dialog(tree), 'lsf-fn-head')[0]), /1 failed/);
  assert.match(text(findAll(tree, (node) => node.tag === 'pre' && has(node, 'is-error'))[0]), /unreachable executed/);
  const output = byClass(tree, 'lsf-fn-pane').find((node) => node.props['data-pane'] === 'output');
  assert.match(text(output), /Output\(none\)/);
  assert.equal(byClass(output, 'lsf-fn-copy').length, 0, 'nothing to copy');
});

test('the overview has the skin switch, the extensions, the blocking errors and the discounts not applied', () => {
  const state = checkoutState({
    errors: [{ message: 'Too many', target: '$.cart', function: 'order-rules' }],
    skippedDiscounts: [{ title: 'Half', code: null, class: 'Shipping', amount: { amount: 2.45 }, function: 'volume-discount', reason: 'only one shipping discount applies per order: "Three off" takes 3.00 USD off the shipping' }],
  });
  const closed = page(state);
  assert.equal(findAll(closed, (node) => node.props['data-test'] === 'skipped-discounts').length, 0, 'nothing built while the dialog is closed');
  const onRun = page(state, { functionsOpen: true, functionsView: runView(state.runs, 0) });
  assert.equal(byClass(onRun, 'lsf-fn-overview').length, 0, 'the overview shows on its own');

  const tree = page(state, { functionsOpen: true, functionsView: 'overview' });
  assert.deepEqual(byClass(tree, 'lsf-fn-entry').map((node) => node.props['aria-current']), ['true', null, null]);
  assert.equal(pres(tree).length, 0, 'no JSON on the overview');
  const overview = byClass(tree, 'lsf-fn-overview')[0];
  const link = byClass(overview, 'lsf-link')[0];
  assert.equal(link.props.href, '/checkouts/cn/preview?variant=shop');
  assert.equal(text(link), 'Switch to the shop skin');
  assert.match(text(overview), /Errors that block the checkout.*Too many.*order-rules/);
  const [skipped] = findAll(overview, (node) => node.props['data-test'] === 'skipped-discounts');
  assert.match(text(skipped), /Discounts not applied.*Half.*only one shipping discount applies per order.*Three off.*volume-discount/);
  const none = page(checkoutState(), { functionsOpen: true, functionsView: 'overview' });
  assert.equal(findAll(none, (node) => node.props['data-test'] === 'skipped-discounts').length, 0);
});

test('problems with the app show as banners', () => {
  const tree = page(checkoutState({ diagnostics: ['gift-message: no bundle, run `shopify app build`'] }));
  assert.match(text(byClass(tree, 'lsf-diagnostics')[0]), /shopify app build/);
});

test('a build of --watch that failed shows in a banner, in the badge and on the overview', () => {
  const builds = [{ handle: 'volume-discount', reason: 'build failed (exit status: 101)', output: 'error[E0425]: cannot find value `x`' }];
  const tree = page(checkoutState({ builds }));
  const [banner] = findAll(tree, (node) => node.props['data-banner'] === 'build');
  assert.equal(banner.props['data-build'], 'volume-discount');
  assert.match(text(banner), /volume-discount: build failed \(exit status: 101\)\. The checkout runs the last module that built\..*E0425/);
  const [badge] = byClass(tree, 'lsf-fn-badge');
  assert.equal(badge.props['data-status'], 'failed');
  assert.match(badge.props['aria-label'], /1 build error/);
  const overview = byClass(page(checkoutState({ builds }), { functionsOpen: true, functionsView: 'overview' }), 'lsf-fn-overview')[0];
  const [listed] = findAll(overview, (node) => node.props['data-test'] === 'build-errors');
  assert.match(text(listed), /Builds that failed.*volume-discount.*E0425/);
  const none = page(checkoutState({ builds: [] }));
  assert.equal(findAll(none, (node) => node.props['data-banner'] === 'build').length, 0);
  assert.equal(byClass(none, 'lsf-diagnostics').length, 0);
});

test('the banners open the main column, so the summary panel starts under the header', () => {
  const tree = page(checkoutState({ diagnostics: ['gift-message: no bundle'] }));
  const main = byClass(tree, 'lsf-main')[0];
  assert.ok(has(main.children.find(Boolean), 'lsf-diagnostics'), 'the first thing of the main column');
  assert.equal(byClass(tree, 'lsf-diagnostics').length, 1);
  const thanks = page(checkoutState({ status: 'completed', diagnostics: ['gift-message: no bundle'] }));
  assert.ok(has(byClass(thanks, 'lsf-main')[0].children.find(Boolean), 'lsf-diagnostics'), 'the thank-you page too');
  assert.equal(byClass(thanks, 'lsf-diagnostics').length, 1);
});

test('an extension the host does not load shows why, next to the problems with the app', () => {
  const refusals = [{ handle: 'order-notice', apiVersion: '2025-07', reason: 'API version 2025-07 is not supported: the local checkout runs 2026-10 and 2027-01.' }];
  const tree = page(checkoutState({ diagnostics: ['gift-message: no bundle'] }), { functionsOpen: true, functionsView: 'overview' }, { refusals });
  const banners = byClass(tree, 'lsf-diagnostics')[0];
  assert.match(text(banners), /gift-message: no bundle/);
  assert.match(text(banners), /order-notice: not loaded\. API version 2025-07 is not supported/);
  const listed = findAll(tree, (node) => node.tag === 'li' && node.props.key === 'order-notice')[0];
  assert.match(text(listed), /not loaded/);
  const kept = findAll(tree, (node) => node.tag === 'li' && node.props.key === 'gift-message')[0];
  assert.doesNotMatch(text(kept), /not loaded/);
  assert.equal(byClass(page(checkoutState()), 'lsf-diagnostics').length, 0, 'nothing to say, nothing shown');
});

// --- Every extension target (?targets=all) ---

/** The page with every target asked for, and a slot (a node) only at the `declared` targets. */
function everyTarget(state, { declared = [], refusals } = {}) {
  const asked = [];
  const slot = (target, instance) => {
    asked.push([target, instance]);
    return declared.includes(target) ? h('div', { class: 'lsf-slot', 'data-target': target }) : null;
  };
  const actions = new Proxy({}, { get: () => () => {} });
  const tree = renderPage({
    state, ui: { draft: { address: {} } }, slot, actions, refusals,
    location: 'http://localhost/checkouts/cn/preview?targets=all',
    preview: { all: true, rendered: RENDERED_TARGETS },
  });
  return { tree, asked: asked.map(([target]) => target) };
}

const withExtensions = (state, extensions) => ({ ...state, extensions: extensions.map((ext) => ({ apiVersion: '2026-10', ...ext })) });

test('showing every target shows the steps of a cart that does not ship, and the thank-you page under them', () => {
  const state = checkoutState();
  state.lines = state.lines.map((line) => ({ ...line, requiresShipping: false }));
  // Without it, the steps of the delivery are left out of a cart that does not ship.
  assert.deepEqual(findAll(page(state), (node) => node.tag === 'section').map((node) => node.props['data-section']), ['contact', 'payment']);

  const { tree, asked } = everyTarget(state);
  const sections = findAll(tree, (node) => node.tag === 'section').map((node) => node.props['data-section']);
  assert.deepEqual(sections.filter(Boolean), ['targets', 'contact', 'delivery', 'shipping', 'payment', 'thank-you-preview']);
  for (const target of ['purchase.checkout.delivery-address.render-before', 'purchase.checkout.shipping-option-list.render-after', 'purchase.checkout.block.render']) {
    assert.ok(asked.includes(target), `${target} is asked for`);
  }
  // The thank-you page is there for its own targets, with the checkout's.
  for (const target of ['purchase.thank-you.announcement.render', 'purchase.thank-you.block.render', 'purchase.thank-you.cart-line-item.render-after', 'purchase.thank-you.footer.render-after']) {
    assert.ok(asked.includes(target), `${target} is asked for`);
  }
  assert.equal(text(findAll(tree, (node) => node.props['data-section'] === 'thank-you-preview')[0].children[0]), 'Thank-you page');
});

test('the targets panel says which targets are shown and why the others are not', () => {
  const state = withExtensions(checkoutState(), [
    { handle: 'gift', targets: ['purchase.checkout.block.render', 'purchase.thank-you.block.render'] },
    { handle: 'chat', targets: ['purchase.checkout.chat.render', 'purchase.checkout.shipping-option-item.render-after', 'purchase.checkout.shipping-option-item.details.render'] },
    { handle: 'old', targets: ['purchase.checkout.footer.render-after'] },
  ]);
  const { tree } = everyTarget(state, {
    declared: ['purchase.checkout.block.render', 'purchase.thank-you.block.render', 'purchase.checkout.footer.render-after'],
    refusals: [{ handle: 'old', reason: 'API version 2025-07 is not supported' }],
  });
  const panel = findAll(tree, (node) => node.props['data-section'] === 'targets')[0];
  const row = (handle, target) => text(findAll(panel, (node) => node.tag === 'li' && node.props['data-for'] === handle && node.props['data-for-target'] === target)[0]);
  assert.match(row('gift', 'purchase.checkout.block.render'), /gift.*purchase\.checkout\.block\.render.*shown/);
  assert.match(row('gift', 'purchase.thank-you.block.render'), /shown/);
  assert.match(row('chat', 'purchase.checkout.chat.render'), /no place on the page/);
  assert.match(row('chat', 'purchase.checkout.shipping-option-item.render-after'), /needs shipping methods: enter a delivery address/);
  assert.match(row('chat', 'purchase.checkout.shipping-option-item.details.render'), /needs a selected shipping method/);
  assert.match(row('old', 'purchase.checkout.footer.render-after'), /not loaded/);
  assert.equal(findAll(panel, (node) => node.tag === 'li').length, 6);
});

test('without the parameter there is no targets panel and no thank-you page under the checkout', () => {
  const tree = page(checkoutState());
  assert.equal(findAll(tree, (node) => ['targets', 'thank-you-preview'].includes(node.props['data-section'])).length, 0);
  // Nor on the thank-you page itself.
  const done = withCurrency({ ...checkoutState(), status: 'completed', order: { number: '#1001' } });
  const { tree: thanks } = everyTarget(done);
  assert.equal(findAll(thanks, (node) => ['targets', 'thank-you-preview'].includes(node.props['data-section'])).length, 0);
});

test('a checkout opened from a scenario names it and leads back to the list', () => {
  const state = checkoutState({ scenario: { name: 'b2b', title: 'A hotel orders' }, home: '/__lsf/checkouts' });
  const chip = byClass(page(state), 'lsf-scenario')[0];
  assert.equal(chip.tag, 'a');
  assert.equal(chip.props.href, '/__lsf/checkouts');
  assert.equal(text(chip), 'Scenario: A hotel orders');
  const panel = text(byClass(page(state, { functionsOpen: true, functionsView: 'overview' }), 'lsf-fn-overview')[0]);
  assert.match(panel, /scenario A hotel orders/);
  assert.match(panel, /All checkouts and extensions/);
  assert.equal(byClass(page(checkoutState()), 'lsf-scenario').length, 0);
});

test('the header leads back to the cart with an icon, and the Shop skin says Shop Pay instead', () => {
  const standard = page(checkoutState());
  const cart = byClass(standard, 'lsf-cart-link')[0];
  assert.match(cart.props.href, /\/cart$/);
  assert.equal(cart.props['aria-label'], 'Return to cart');
  assert.equal(byClass(standard, 'lsf-secure').length, 0);
  const shop = page(checkoutState({ variant: 'shop' }));
  assert.equal(byClass(shop, 'lsf-cart-link').length, 0);
  assert.equal(text(byClass(shop, 'lsf-secure')[0]), 'Shop Pay');
});

test('the pay button comes first, the way back to the cart under it', () => {
  const row = byClass(page(checkoutState()), 'lsf-actions-row')[0];
  assert.deepEqual(row.children.map((node) => node.props.class.split(' ')[1] ?? node.props.class), ['lsf-pay', 'lsf-return']);
});

test('the Apply button is inside the discount field, after its label', () => {
  const field = byClass(byClass(page(checkoutState()), 'lsf-discount-form')[0], 'lsf-field')[0];
  assert.deepEqual(field.children.map((node) => node.tag), ['input', 'label', 'button']);
  assert.equal(field.children[2].props.class, 'lsf-apply');
});

test('the Apply button is disabled while the discount field is empty, and follows what is typed', () => {
  const apply = (ui) => byClass(page(checkoutState(), ui), 'lsf-apply')[0];
  assert.equal(apply({}).props.disabled, true);
  assert.equal(apply({ discountInput: '   ' }).props.disabled, true);
  assert.equal(apply({ discountInput: 'SAVE10' }).props.disabled, null);
  assert.equal(apply({ discountInput: 'SAVE10', busy: true }).props.disabled, true);

  const input = byClass(byClass(page(checkoutState()), 'lsf-discount-form')[0], 'lsf-input')[0];
  const button = { disabled: true };
  const target = (value) => ({ value, parentElement: { querySelector: (selector) => (selector === '.lsf-apply' ? button : null) } });
  input.props.onInput({ target: target('SAVE10') });
  assert.equal(button.disabled, false);
  input.props.onInput({ target: target(' ') });
  assert.equal(button.disabled, true);
});

test('the card row shows the brands it takes as three chips', () => {
  const tree = page(checkoutState());
  assert.deepEqual(byClass(tree, 'lsf-card-brand').map(text), ['VISA', 'MC', 'AMEX']);
});

test('the JSON of the Functions dialog is colored by kind, and reads the same as plain JSON', () => {
  const value = { id: 'gid://shopify/CartLine/1', 'say \"hi\"': 'a \"quoted\": 2', quantity: -1.5e3, ok: true, no: false, none: null, list: [1, 'x'] };
  const pre = jsonCode(value);
  assert.equal(pre.tag, 'pre');
  assert.equal(text(pre), JSON.stringify(value, null, 2));
  const kinds = (kind) => byClass(pre, `lsf-json-${kind}`).map(text);
  assert.deepEqual(kinds('key'), ['"id"', '"say \\"hi\\""', '"quantity"', '"ok"', '"no"', '"none"', '"list"']);
  assert.deepEqual(kinds('string'), ['"gid://shopify/CartLine/1"', '"a \\"quoted\\": 2"', '"x"']);
  assert.deepEqual(kinds('number'), ['-1500', '1']);
  assert.deepEqual(kinds('boolean'), ['true', 'false']);
  assert.deepEqual(kinds('null'), ['null']);
});

test('numbers with an exponent are one number token', () => {
  const pre = jsonCode({ small: 1e-7, big: -1e21 });
  assert.equal(text(pre), '{\n  "small": 1e-7,\n  "big": -1e+21\n}');
  assert.deepEqual(byClass(pre, 'lsf-json-number').map(text), ['1e-7', '-1e+21']);
});

test('a very large JSON is shown plain', () => {
  const pre = jsonCode({ big: 'x'.repeat(250_000) });
  assert.equal(pre.children.length, 1);
  assert.equal(pre.children[0].tag, '#text');
});

test('an empty cart says so, with the scenarios, instead of a checkout to pay for nothing', () => {
  const state = { ...checkoutState(), lines: [], home: '/' };
  const tree = page(state);
  const empty = findAll(tree, (node) => node.props['data-section'] === 'empty-cart')[0];
  assert.match(text(empty), /Your cart is empty/);
  assert.equal(findAll(empty, (node) => node.props['data-link'] === 'scenarios')[0].props.href, '/');
  assert.equal(byClass(tree, 'lsf-pay').length, 0, 'nothing to pay');
  assert.equal(findAll(tree, (node) => node.props['data-section'] === 'payment').length, 0);
  assert.equal(findAll(tree, (node) => node.props['data-section'] === 'contact').length, 1, 'the login stays');
});

test('every target still shows with an empty cart', () => {
  const state = { ...checkoutState(), lines: [] };
  const tree = renderPage({ state, ui: { draft: { address: {} } }, slot: () => null, actions: new Proxy({}, { get: () => () => {} }), preview: { all: true, rendered: [] }, location: 'http://localhost/checkouts/cn/preview' });
  assert.equal(findAll(tree, (node) => node.props['data-section'] === 'empty-cart').length, 0);
});

