// Who the checkout is logged in as. The customers of the store data are the login profiles of
// the local checkout (`account.profiles` of the state): the "Log in" button of the Contact
// section opens a menu of them, and picking one sends a `customer` operation. Once logged in,
// the Contact section shows the account instead of the email field, and the Delivery section
// offers the addresses the customer saved (`account.addresses`, the default one first).

import { countryName } from './format.js';
import { h } from './vdom.js';

export const MENU_ID = 'lsf-account-menu';
export const TRIGGER_ID = 'lsf-account-trigger';
/** The items of the menu, in the order of the page. */
export const MENU_ITEMS = '.lsf-account-menu [role="menuitemradio"]';

const ADDRESS_FIELDS = [
  'firstName', 'lastName', 'company', 'address1', 'address2', 'city', 'countryCode',
  'provinceCode', 'zip', 'phone',
];

const text = (value) => (typeof value === 'string' ? value.trim() : '');

/** Whether two addresses are the same, field by field: a field that is missing is empty. */
export function sameAddress(a, b) {
  if (!a || !b) return false;
  return ADDRESS_FIELDS.every((name) => {
    const left = text(a[name]);
    const right = text(b[name]);
    return name === 'countryCode' ? left.toUpperCase() === right.toUpperCase() : left === right;
  });
}

/** The saved address the checkout ships to (its `id`), or `new` for an address of its own. */
export function savedAddressId(state) {
  const current = state.shippingAddress;
  const saved = state.account?.addresses ?? [];
  return saved.find((entry) => sameAddress(entry.address, current))?.id ?? 'new';
}

/** One line for a saved address: `Northwind Portland · 815 Harbor Way, Portland, United States`. */
export function addressLabel(entry, countries) {
  const address = entry.address ?? {};
  const where = [
    address.address1,
    address.address2,
    [address.city, address.provinceCode, address.zip].filter(Boolean).join(' '),
    countryName(address.countryCode, countries),
  ].filter(Boolean).join(', ');
  return entry.location ? `${entry.location} · ${where}` : where;
}

/** `JD` for Jane Doe, `J` for jane@example.com. */
export function initials(name) {
  const words = text(name).split(/[\s@.]+/).filter(Boolean);
  return words.slice(0, 2).map((word) => word[0].toUpperCase()).join('') || '?';
}

const count = (n, one, many) => `${n} ${n === 1 ? one : many}`;

/** The short facts the menu shows under a customer: `[kind, text]`. */
export function badges(profile) {
  const out = [];
  if (profile.company) {
    out.push(['b2b', `B2B · ${profile.company}`]);
    if (profile.locationsCount > 1) out.push(['', count(profile.locationsCount, 'location', 'locations')]);
  }
  for (const tag of profile.tags ?? []) out.push(['tag', tag]);
  out.push(['', profile.ordersCount ? count(profile.ordersCount, 'order', 'orders') : 'No orders']);
  if (profile.addressesCount) out.push(['', count(profile.addressesCount, 'address', 'addresses')]);
  if (profile.countryCode) out.push(['', profile.countryCode]);
  return out;
}

/** The contact email of a logged-in checkout when it is not the account's (a scenario or the
 * buyer entered another one), otherwise `null`. It is the one the Functions and the extensions
 * read and the order confirmation goes to, so the page shows it. */
export function otherContactEmail(state) {
  const account = state.buyer?.customer?.email;
  const contact = state.buyer?.email;
  if (!account || !contact) return null;
  return contact.trim().toLowerCase() === account.trim().toLowerCase() ? null : contact;
}

export const fullName = (customer) =>
  [customer?.firstName, customer?.lastName].filter(Boolean).join(' ') || customer?.email || '';

/** The button that opens the menu, and the menu when it is open. */
export function accountMenu({ state, ui, actions }) {
  const loggedIn = Boolean(state.buyer?.customer);
  const current = state.buyer?.customer?.email ?? null;
  const profiles = state.account?.profiles ?? [];
  const item = ({ email, checked, label, extra }) =>
    h(
      'button',
      {
        type: 'button',
        role: 'menuitemradio',
        'aria-checked': checked ? 'true' : 'false',
        tabindex: '-1',
        class: `lsf-account-option${email === null ? ' is-guest' : ''}`,
        key: email ?? '(guest)',
        'data-customer': email ?? '',
        onClick: () => actions.logIn(email),
      },
      extra,
      label,
    );
  return h(
    'div',
    { class: 'lsf-account', key: 'account' },
    h(
      'button',
      {
        type: 'button',
        id: TRIGGER_ID,
        class: 'lsf-link lsf-account-trigger',
        'aria-haspopup': 'menu',
        'aria-expanded': ui.accountOpen ? 'true' : 'false',
        'aria-controls': MENU_ID,
        'aria-label': loggedIn ? 'Switch customer' : null,
        onClick: () => actions.toggleAccount(!ui.accountOpen, 'current'),
        onKeydown: (event) => actions.accountTriggerKey(event),
      },
      loggedIn ? 'Switch' : 'Log in',
    ),
    ui.accountOpen &&
      h(
        'div',
        {
          id: MENU_ID,
          class: 'lsf-account-menu',
          role: 'menu',
          'aria-labelledby': `${MENU_ID}-title`,
          onKeydown: (event) => actions.accountMenuKey(event),
        },
        h('p', { id: `${MENU_ID}-title`, class: 'lsf-account-menu-title', role: 'presentation' }, 'Log in as a customer of the test data'),
        profiles.length === 0 &&
          h('p', { class: 'lsf-muted lsf-account-empty', role: 'presentation' }, 'The store data has no customers: add some to customers.json.'),
        profiles.map((profile) =>
          item({
            email: profile.email,
            checked: profile.email === current,
            extra: h('span', { class: 'lsf-avatar', 'aria-hidden': 'true' }, initials(profile.name)),
            label: h(
              'span',
              { class: 'lsf-account-text' },
              h('span', { class: 'lsf-account-name' }, profile.name),
              h('span', { class: 'lsf-muted' }, profile.email),
              h('span', { class: 'lsf-badges' }, badges(profile).map(([kind, label]) =>
                h('span', { class: `lsf-badge${kind ? ` is-${kind}` : ''}` }, label))),
            ),
          }),
        ),
        h('div', { class: 'lsf-account-separator', role: 'separator' }),
        item({ email: null, checked: !loggedIn, label: loggedIn ? 'Log out, continue as a guest' : 'Continue as a guest' }),
      ),
  );
}
