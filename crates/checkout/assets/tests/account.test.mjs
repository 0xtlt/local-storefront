// Logging in at the checkout: the menu of the customers of the test data that "Log in" opens,
// the account the Contact section shows, the saved addresses, logging out, and what extensions
// read of the buyer when the customer changes. The server is the mock engine of the preview.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { addressLabel, badges, initials, sameAddress, savedAddressId } from '../account.js';
import { startCheckout } from '../checkout.js';
import { sessionToken } from '../extension-host.js';
import { update } from '../preview/mock-engine.mjs';
import { FakeDocument, FakeEvent } from './fake-dom.mjs';
import { engineCheckout, init, pageState, plain, sandboxWindow, settle } from './harness.mjs';

const SANDBOX = 'http://localhost/__lsf/checkout/assets/sandbox.html';
const AVA = 'ava.chen@example.com';
const SAM = 'sam.okafor@example.com';
const publicState = ({ selection, ...rest }) => rest;

/** A page on a checkout the mock engine keeps. `start` is operations applied before the page
 * loads (a login made elsewhere). */
function app({ start = [], variant = 'standard' } = {}) {
  let engine = engineCheckout();
  engine.variant = variant;
  if (start.length) engine = update(engine, start).state;
  const document = new FakeDocument();
  const root = document.createElement('div');
  document.body.appendChild(root);
  const requests = [];
  const reply = (status, body) => ({ ok: status < 400, status, json: async () => body });
  const fetch = async (url, init) => {
    const body = JSON.parse(init.body);
    requests.push({ url, body });
    const result = update(engine, body.operations);
    if (result.error) return reply(400, { error: result.error });
    engine = result.state;
    return reply(200, publicState(engine));
  };
  const location = { href: 'http://localhost/checkouts/cn/preview', assign() {} };
  const window = { addEventListener() {}, removeEventListener() {}, open() {} };
  const checkout = startCheckout({ root, initial: publicState(engine), document, window, location, fetch, storage: undefined, sandboxUrl: SANDBOX });
  const query = (selector) => root.querySelector(selector);
  return {
    checkout,
    document,
    root,
    query,
    trigger: () => query('.lsf-account-trigger'),
    menu: () => query('.lsf-account-menu'),
    items: () => root.querySelectorAll('.lsf-account-menu [role="menuitemradio"]'),
    operations: () => requests.flatMap((request) => request.body.operations),
    key: (element, key) => element.dispatchEvent(new FakeEvent('keydown', { bubbles: true, cancelable: true, data: { key } })),
  };
}

const flush = async () => {
  for (let i = 0; i < 4; i++) await settle();
};

test('a guest has a Log in button that opens a menu of the customers of the test data', async () => {
  const page = app();
  const trigger = page.trigger();
  assert.equal(trigger.textContent, 'Log in');
  assert.equal(trigger.getAttribute('aria-haspopup'), 'menu');
  assert.equal(trigger.getAttribute('aria-expanded'), 'false');
  assert.equal(page.menu(), null);
  assert.ok(page.query('.lsf-field[data-field="email"]'), 'a guest types an email');

  trigger.click();
  assert.equal(page.trigger().getAttribute('aria-expanded'), 'true');
  const menu = page.menu();
  assert.equal(menu.getAttribute('role'), 'menu');
  assert.equal(page.trigger().getAttribute('aria-controls'), menu.getAttribute('id'));
  const items = page.items();
  assert.deepEqual(items.map((item) => item.getAttribute('data-customer')), [AVA, 'leo.martin@example.com', SAM, '']);
  assert.ok(items.every((item) => item.getAttribute('tabindex') === '-1'), 'the arrows move between the items, not Tab');
  assert.equal(items.at(-1).getAttribute('aria-checked'), 'true', 'nobody is logged in');
  assert.equal(page.document.activeElement, items[0], 'the focus goes to the first customer');
  // Name, email and what tells them apart.
  assert.match(items[0].textContent, /Ava Chen/);
  assert.match(items[0].textContent, /ava\.chen@example\.com/);
  const facts = (item) => item.querySelectorAll('.lsf-badge').map((badge) => badge.textContent);
  assert.deepEqual(facts(items[0]), ['vip', '3 orders', '2 addresses', 'FR']);
  assert.deepEqual(facts(items[1]), ['No orders']);
  assert.deepEqual(facts(items[2]), ['B2B · Harbor Cafés', '2 locations', 'wholesale', 'No orders', 'FR']);
  assert.equal(page.operations().length, 0, 'opening the menu sends nothing');
});

test('the menu follows the keyboard, and Escape gives the focus back to its button', () => {
  const page = app();
  page.key(page.trigger(), 'ArrowDown');
  const items = page.items();
  assert.equal(page.document.activeElement, items[0]);
  page.key(items[0], 'ArrowDown');
  assert.equal(page.document.activeElement, items[1]);
  page.key(items[1], 'ArrowUp');
  page.key(items[0], 'ArrowUp');
  assert.equal(page.document.activeElement, items.at(-1), 'up from the first wraps to the last');
  page.key(items.at(-1), 'ArrowDown');
  assert.equal(page.document.activeElement, items[0], 'down from the last wraps to the first');
  page.key(items[0], 'End');
  assert.equal(page.document.activeElement, items.at(-1));
  page.key(items.at(-1), 'Home');
  assert.equal(page.document.activeElement, items[0]);

  page.key(items[0], 'Escape');
  assert.equal(page.menu(), null);
  assert.equal(page.trigger().getAttribute('aria-expanded'), 'false');
  assert.equal(page.document.activeElement, page.trigger());

  // Up on the button opens it on the last item; Tab closes it.
  page.key(page.trigger(), 'ArrowUp');
  assert.equal(page.document.activeElement, page.items().at(-1));
  page.key(page.items().at(-1), 'Tab');
  assert.equal(page.menu(), null);
  assert.equal(page.operations().length, 0);
});

test('a click outside the menu closes it, a click on the button too', () => {
  const page = app();
  page.trigger().click();
  assert.ok(page.menu());
  page.query('.lsf-pay').click();
  assert.equal(page.menu(), null);
  page.trigger().click();
  page.trigger().click();
  assert.equal(page.menu(), null);
});

test('picking a customer logs the checkout in: the Contact section shows the account', async () => {
  const page = app();
  page.checkout.actions.draft('email', 'typed@example.com');
  page.trigger().click();
  page.items()[0].click();
  await flush();
  assert.deepEqual(page.operations(), [{ type: 'customer', customer: AVA }]);
  assert.equal(page.menu(), null);
  assert.equal(page.query('.lsf-field[data-field="email"]'), null, 'no email to type');
  const card = page.query('.lsf-account-card');
  assert.equal(card.getAttribute('data-account'), AVA);
  assert.match(card.textContent, /Ava Chen/);
  assert.match(card.textContent, /ava\.chen@example\.com/);
  assert.equal(page.query('.lsf-avatar').textContent, 'AC');
  assert.equal(page.trigger().textContent, 'Switch');
  assert.equal(page.trigger().getAttribute('aria-label'), 'Switch customer');
  assert.equal(page.document.activeElement, page.trigger(), 'the focus is back on the button');
  assert.equal(page.checkout.ui.draft.email, undefined, 'what the guest typed is gone');
  assert.equal(page.checkout.getState().buyer.email, AVA);

  // The menu says who is logged in, and opens on them.
  page.trigger().click();
  const items = page.items();
  assert.equal(items[0].getAttribute('aria-checked'), 'true');
  assert.equal(items.at(-1).textContent, 'Log out, continue as a guest');
  assert.equal(page.document.activeElement, items[0]);
});

test('the saved addresses of the customer are a select, the default one chosen', async () => {
  const page = app();
  await page.checkout.actions.logIn(AVA);
  const select = page.query('select[name="savedAddress"]');
  assert.ok(select, 'a select of saved addresses');
  const options = select.querySelectorAll('option');
  assert.deepEqual(options.map((option) => option.value), ['address-1', 'address-2', 'new']);
  assert.equal(options[0].textContent, '5 rue Oberkampf, Paris 75011, France');
  assert.equal(options.at(-1).textContent, 'Use a new address');
  assert.equal(select.value, 'address-1');
  assert.equal(page.query('input[name="address1"]').value, '5 rue Oberkampf', 'the fields hold the default address');

  // Another saved address.
  select.value = 'address-2';
  select.dispatchEvent(new FakeEvent('change', { bubbles: true }));
  await flush();
  assert.equal(page.operations().at(-1).type, 'shippingAddress');
  assert.equal(page.operations().at(-1).address.city, 'Lyon');
  assert.equal(page.query('select[name="savedAddress"]').value, 'address-2');
  assert.equal(page.query('input[name="city"]').value, 'Lyon');

  // A new one: empty fields, the same country.
  await page.checkout.actions.chooseSavedAddress('new');
  assert.deepEqual(page.operations().at(-1), { type: 'shippingAddress', address: { countryCode: 'FR' } });
  assert.equal(page.query('select[name="savedAddress"]').value, 'new');
  assert.equal(page.query('input[name="address1"]').value, '');
});

test('a customer without addresses has no select, a guest neither', async () => {
  const page = app();
  assert.equal(page.query('select[name="savedAddress"]'), null);
  await page.checkout.actions.logIn('leo.martin@example.com');
  assert.equal(page.query('select[name="savedAddress"]'), null);
  assert.ok(page.query('.lsf-account-card'));
});

test('logging out is a guest checkout again', async () => {
  const page = app({ start: [{ type: 'customer', customer: AVA }] });
  assert.ok(page.query('.lsf-account-card'), 'logged in when the page loads');
  page.query('.lsf-logout').click();
  await flush();
  assert.deepEqual(page.operations(), [{ type: 'customer', customer: null }]);
  assert.equal(page.query('.lsf-account-card'), null);
  assert.equal(page.query('input[name="email"]').value, '');
  assert.equal(page.query('select[name="savedAddress"]'), null);
  assert.equal(page.trigger().textContent, 'Log in');
  assert.equal(page.checkout.getState().shippingAddress, null, 'the address was the customer\'s');
});

test('a logged-in checkout with another contact email shows it, and can give it back to the account', async () => {
  // A scenario that logs in and enters its own contact email, or a buyer who typed one.
  const page = app({ start: [{ type: 'customer', customer: AVA }, { type: 'email', value: 'orders@example.com' }] });
  assert.equal(page.checkout.getState().buyer.email, 'orders@example.com', 'what the Functions and extensions read');
  assert.equal(page.query('.lsf-account-card').getAttribute('data-account'), AVA, 'the account stays');
  const contact = page.query('.lsf-contact-email');
  assert.equal(contact.getAttribute('data-contact-email'), 'orders@example.com');
  assert.equal(page.query('input[name="email"]').value, 'orders@example.com');
  assert.equal(page.query('label[for="lsf-email"]').textContent, 'Email for this order');
  assert.match(contact.textContent, /Not the email of the account/);

  // Corrected while logged in.
  const input = page.query('input[name="email"]');
  input.value = 'billing@example.com';
  input.dispatchEvent(new FakeEvent('change', { bubbles: true }));
  await flush();
  assert.deepEqual(page.operations().at(-1), { type: 'email', value: 'billing@example.com' });
  assert.equal(page.query('.lsf-contact-email').getAttribute('data-contact-email'), 'billing@example.com');

  // Given back to the account: the field goes, the account's email is the contact.
  page.query('.lsf-use-account-email').click();
  await flush();
  assert.deepEqual(page.operations().at(-1), { type: 'email', value: null });
  assert.equal(page.checkout.getState().buyer.email, AVA);
  assert.equal(page.query('.lsf-contact-email'), null);
  assert.equal(page.query('.lsf-field[data-field="email"]'), null);
  assert.equal(page.checkout.ui.draft.email, undefined);
});

test('emptying the email for this order gives it back to the account, and does not block paying', async () => {
  const start = [{ type: 'customer', customer: AVA }, { type: 'email', value: 'orders@example.com' }];

  // Emptied, then the field is left.
  const left = app({ start });
  const input = left.query('input[name="email"]');
  input.value = '';
  left.checkout.actions.draft('email', '');
  input.dispatchEvent(new FakeEvent('change', { bubbles: true }));
  await flush();
  assert.deepEqual(left.operations().at(-1), { type: 'email', value: null });
  assert.equal(left.checkout.getState().buyer.email, AVA);
  assert.equal(left.checkout.ui.draft.email, undefined);
  assert.equal(left.query('.lsf-contact-email'), null);

  // Emptied, and Pay right away: the email goes back to the account first, nothing is missing.
  const paid = app({ start });
  await paid.checkout.actions.chooseDelivery('shipping', 'standard');
  await paid.checkout.actions.choosePayment('cod');
  assert.deepEqual(paid.checkout.getState().buyer.email, 'orders@example.com');
  paid.checkout.actions.draft('email', ' ');
  await paid.checkout.actions.pay().catch(() => {});
  await flush();
  assert.deepEqual(paid.operations().find((operation) => operation.type === 'email'), { type: 'email', value: null });
  assert.equal(paid.checkout.getState().buyer.email, AVA);
  assert.equal(paid.checkout.ui.draft.email, undefined);
  assert.equal(paid.query('[data-field="email"] [aria-invalid="true"]'), null, 'no "Enter an email"');
});

test('typing a new email while the fallback to the account is on its way keeps the field', async () => {
  const start = [{ type: 'customer', customer: AVA }, { type: 'email', value: 'orders@example.com' }];
  const page = app({ start });
  const input = page.query('input[name="email"]');
  input.focus();
  input.dispatchEvent(new FakeEvent('focusin', { bubbles: true }));
  input.value = '';
  page.checkout.actions.draft('email', '');
  input.dispatchEvent(new FakeEvent('change', { bubbles: true }));
  // Before the server answers, the buyer types again in the same field.
  input.value = 're';
  page.checkout.actions.draft('email', 're');
  await flush();
  assert.equal(page.checkout.getState().buyer.email, AVA, 'the server fell back to the account');
  assert.equal(page.query('input[name="email"]'), input, 'the field the buyer types in stays');
  assert.equal(page.query('.lsf-contact-email').getAttribute('data-contact-email'), 're');
  input.value = 'replacement@example.com';
  page.checkout.actions.draft('email', 'replacement@example.com');
  input.dispatchEvent(new FakeEvent('change', { bubbles: true }));
  await flush();
  assert.deepEqual(page.operations().at(-1), { type: 'email', value: 'replacement@example.com' });
  assert.equal(page.checkout.getState().buyer.email, 'replacement@example.com');
});

test('the field stays while the buyer is in it, even when what they typed so far is the account\'s email', async () => {
  const start = [{ type: 'customer', customer: AVA }, { type: 'email', value: 'orders@example.com' }];
  const page = app({ start });
  const input = page.query('input[name="email"]');
  input.focus();
  input.dispatchEvent(new FakeEvent('focusin', { bubbles: true }));
  input.value = '';
  page.checkout.actions.draft('email', '');
  input.dispatchEvent(new FakeEvent('change', { bubbles: true }));
  // The answer lands when the buyer has typed exactly the account's email, and goes on typing.
  input.value = AVA;
  page.checkout.actions.draft('email', AVA);
  await flush();
  assert.equal(page.checkout.getState().buyer.email, AVA);
  assert.equal(page.query('input[name="email"]'), input, 'the field the buyer is in stays');
  input.value = `${AVA}.au`;
  page.checkout.actions.draft('email', `${AVA}.au`);
  input.dispatchEvent(new FakeEvent('change', { bubbles: true }));
  input.dispatchEvent(new FakeEvent('focusout', { bubbles: true }));
  await flush();
  assert.deepEqual(page.operations().at(-1), { type: 'email', value: `${AVA}.au` });
  assert.equal(page.checkout.getState().buyer.email, `${AVA}.au`);
  assert.ok(page.query('.lsf-contact-email'), 'not the account\'s email: the field stays');
});

test('an emptied field stays empty when the buyer leaves it before the server answers, and what they type next is all that is sent', async () => {
  const start = [{ type: 'customer', customer: AVA }, { type: 'email', value: 'orders@example.com' }];
  const page = app({ start });
  const input = page.query('input[name="email"]');
  input.focus();
  input.dispatchEvent(new FakeEvent('focusin', { bubbles: true }));
  input.value = '';
  page.checkout.actions.draft('email', '');
  // Leaves the field: the change goes to the server, and the page paints before it answers.
  input.dispatchEvent(new FakeEvent('change', { bubbles: true }));
  page.document.activeElement = null;
  input.dispatchEvent(new FakeEvent('focusout', { bubbles: true }));
  assert.equal(page.checkout.getState().buyer.email, 'orders@example.com', 'not answered yet');
  assert.equal(page.query('input[name="email"]').value, '', 'the old email does not come back');

  // Comes back and types the whole new email, the answer landing on the way.
  input.focus();
  input.dispatchEvent(new FakeEvent('focusin', { bubbles: true }));
  input.value = AVA;
  page.checkout.actions.draft('email', AVA);
  await flush();
  input.value = `${AVA}.au`;
  page.checkout.actions.draft('email', `${AVA}.au`);
  input.dispatchEvent(new FakeEvent('change', { bubbles: true }));
  page.document.activeElement = null;
  input.dispatchEvent(new FakeEvent('focusout', { bubbles: true }));
  await flush();
  assert.deepEqual(page.operations().filter((operation) => operation.type === 'email').slice(-2), [{ type: 'email', value: null }, { type: 'email', value: `${AVA}.au` }]);
  assert.equal(page.checkout.getState().buyer.email, `${AVA}.au`);
  assert.equal(page.query('input[name="email"]').value, `${AVA}.au`);
});

test('moving the focus inside the email block keeps it on the page', async () => {
  const start = [{ type: 'customer', customer: AVA }, { type: 'email', value: 'orders@example.com' }];
  const page = app({ start });
  const block = page.query('.lsf-contact-email');
  const input = page.query('input[name="email"]');
  const button = page.query('.lsf-use-account-email');
  input.dispatchEvent(new FakeEvent('focusin', { bubbles: true }));
  // From the field to the "Use …" button: focusout with the button as where the focus goes.
  input.dispatchEvent(new FakeEvent('focusout', { bubbles: true, data: { relatedTarget: button } }));
  assert.equal(page.checkout.ui.emailEditing, true);
  button.dispatchEvent(new FakeEvent('focusout', { bubbles: true, data: { relatedTarget: null } }));
  assert.equal(page.checkout.ui.emailEditing, false);
  assert.ok(block);
});

test('the field goes once the buyer leaves it with the account\'s email', async () => {
  const start = [{ type: 'customer', customer: AVA }, { type: 'email', value: 'orders@example.com' }];
  const page = app({ start });
  const input = page.query('input[name="email"]');
  input.focus();
  input.dispatchEvent(new FakeEvent('focusin', { bubbles: true }));
  input.value = AVA.toUpperCase();
  page.checkout.actions.draft('email', AVA.toUpperCase());
  input.dispatchEvent(new FakeEvent('change', { bubbles: true }));
  await flush();
  assert.ok(page.query('.lsf-contact-email'), 'still in the field');
  input.dispatchEvent(new FakeEvent('focusout', { bubbles: true }));
  await flush();
  assert.equal(page.query('.lsf-contact-email'), null);
});

test('a guest who empties the email still has to enter one', async () => {
  const page = app();
  page.checkout.actions.draft('email', '');
  await page.checkout.actions.pay();
  assert.equal(page.operations().some((operation) => operation.type === 'email'), false);
  assert.equal(page.checkout.ui.tried, true);
});

test('the account\'s email typed in another case is the account\'s', () => {
  const page = app({ start: [{ type: 'customer', customer: AVA }, { type: 'email', value: 'Ava.Chen@Example.com' }] });
  assert.equal(page.query('.lsf-contact-email'), null);
});

test('a B2B customer chooses the location they buy for, and ships there', async () => {
  const page = app();
  await page.checkout.actions.logIn(SAM);
  const company = page.query('.lsf-company');
  assert.equal(company.getAttribute('data-company'), 'Harbor Cafés');
  const select = page.query('select[name="companyLocation"]');
  assert.deepEqual(select.querySelectorAll('option').map((option) => option.textContent), ['Harbor Cafés Nantes', 'Harbor Cafés Bordeaux']);
  assert.equal(select.value, 'gid://shopify/CompanyLocation/11');
  assert.equal(page.query('select[name="savedAddress"]').querySelectorAll('option')[0].textContent, 'Harbor Cafés Nantes · 3 quai de la Fosse, Nantes 44000, France');
  assert.equal(page.query('label[for="lsf-savedAddress"]').textContent, 'Ship to the location');

  select.value = 'gid://shopify/CompanyLocation/12';
  select.dispatchEvent(new FakeEvent('change', { bubbles: true }));
  await flush();
  assert.deepEqual(page.operations().at(-1), { type: 'companyLocation', location: 'gid://shopify/CompanyLocation/12' });
  assert.equal(page.checkout.getState().buyer.purchasingCompany.location.name, 'Harbor Cafés Bordeaux');
  assert.equal(page.query('input[name="city"]').value, 'Bordeaux');
});

test('the Shop skin logs in the same way', async () => {
  const page = app({ variant: 'shop' });
  page.trigger().click();
  page.items()[0].click();
  await flush();
  assert.equal(page.document.body.getAttribute('data-variant'), 'shop');
  assert.ok(page.query('.lsf-account-card'));
  assert.ok(page.query('select[name="savedAddress"]'));
});

test('extensions do not see the list of customers', async () => {
  const page = app();
  const { stateForExtensions } = await import('../extension-host.js');
  assert.ok(page.checkout.getState().account.profiles.length > 0);
  assert.equal(stateForExtensions(page.checkout.getState()).account, undefined);
});

// --- What extensions read when the customer changes -------------------------------------

function runtime() {
  const win = sandboxWindow();
  return win.context.LsfSandbox.createRuntime({
    post: () => {},
    init: init({ handle: 'x', target: 'purchase.checkout.block.render' }),
  });
}

test('the Buyer Identity API tells its subscribers when the checkout logs in, switches and logs out', () => {
  const { shopify, refresh } = runtime();
  const customers = [];
  const emails = [];
  const companies = [];
  shopify.buyerIdentity.customer.subscribe((customer) => customers.push(customer?.email ?? null));
  shopify.buyerIdentity.email.subscribe((email) => emails.push(email ?? null));
  shopify.buyerIdentity.purchasingCompany.subscribe((company) => companies.push(company?.location?.name ?? null));

  let engine = engineCheckout();
  const step = (operation) => {
    engine = update(engine, [operation]).state;
    refresh(pageState(engine));
  };
  step({ type: 'customer', customer: AVA });
  assert.deepEqual(plain(shopify.buyerIdentity.customer.value), {
    id: 'gid://shopify/Customer/1', email: AVA, phone: '+33 6 12 34 56 78', fullName: 'Ava Chen', firstName: 'Ava', lastName: 'Chen',
    image: { url: '' }, acceptsMarketing: true, acceptsEmailMarketing: true, acceptsSmsMarketing: false, storeCreditAccounts: [], ordersCount: 3,
  });
  assert.equal(shopify.buyerIdentity.phone.value, '+33 6 12 34 56 78');
  assert.equal(shopify.shippingAddress.value.city, 'Paris', 'the default address');
  step({ type: 'customer', customer: SAM });
  step({ type: 'companyLocation', location: 'gid://shopify/CompanyLocation/12' });
  step({ type: 'customer', customer: null });

  assert.deepEqual(customers, [null, AVA, SAM, null]);
  assert.deepEqual(emails, [null, AVA, SAM, null]);
  assert.deepEqual(companies, [null, 'Harbor Cafés Nantes', 'Harbor Cafés Bordeaux', null]);
});

test('the session token names the customer logged in', () => {
  const ext = { handle: 'x' };
  const claims = (state) => JSON.parse(Buffer.from(sessionToken(state, ext).split('.')[1], 'base64url').toString());
  const engine = engineCheckout();
  assert.equal(claims(pageState(engine)).sub, undefined);
  const ava = update(engine, [{ type: 'customer', customer: AVA }]).state;
  assert.equal(claims(pageState(ava)).sub, 'gid://shopify/Customer/1');
});

// --- The helpers -------------------------------------------------------------------------

test('addresses are the same field by field, an empty field is a missing one', () => {
  assert.ok(sameAddress({ address1: '1 Main', countryCode: 'us', company: '' }, { address1: '1 Main ', countryCode: 'US' }));
  assert.ok(!sameAddress({ address1: '1 Main' }, { address1: '2 Main' }));
  assert.ok(!sameAddress(null, { address1: '1 Main' }));
  const state = { shippingAddress: { address1: '2 Main' }, account: { addresses: [{ id: 'a', address: { address1: '1 Main' } }, { id: 'b', address: { address1: '2 Main' } }] } };
  assert.equal(savedAddressId(state), 'b');
  assert.equal(savedAddressId({ ...state, shippingAddress: { address1: '3 Main' } }), 'new');
  assert.equal(addressLabel({ location: 'HQ', address: { address1: '1 Main', city: 'Ottawa', countryCode: 'CA' } }, [['CA', 'Canada']]), 'HQ · 1 Main, Ottawa, Canada');
});

test('initials and badges', () => {
  assert.equal(initials('Jane Doe'), 'JD');
  assert.equal(initials('jane@example.com'), 'JE');
  assert.equal(initials(''), '?');
  assert.deepEqual(badges({ tags: [], ordersCount: 1, addressesCount: 1 }).map(([, text]) => text), ['1 order', '1 address']);
});
