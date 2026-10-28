// The pages of the fake checkout as virtual DOM: the one-page checkout, the thank-you page and
// the badge and dialog that show the Functions. Both skins (`standard`, `shop`) share the markup; the CSS
// tells them apart (`[data-variant]`), apart from a few words and the order of the summary.
//
// A view takes `{state, ui, slot, actions, refusals}`:
//   state    the checkout, amounts as `{amount, currencyCode}` (see `withCurrency`)
//   ui       what only the page knows: drafts of the fields, what is open, what failed
//   slot     `slot(target, instance)` → the node where extensions of that target render, or null
//   actions  what the buyer's gestures do
//   refusals the extensions the host does not load, `[{handle, reason}]` (see `api-versions.js`)

import { accountMenu, addressLabel, fullName, initials, otherContactEmail, savedAddressId } from './account.js';
import { countriesOf, countryName, price, priceOrFree, storefrontLink } from './format.js';
import { missingFields, placeErrors } from './operations.js';
import { h } from './vdom.js';

// Single-color outline icons that follow the text color (the tokens set it): 1.4px strokes,
// round joins.
const outline = (size, body) =>
  `<svg viewBox="0 0 20 20" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">${body}</svg>`;

const ICONS = {
  lock: outline(16, '<rect x="4" y="9" width="12" height="8" rx="2"/><path d="M7 9V6.5a3 3 0 016 0V9"/>'),
  chevron: outline(10, '<path d="M4 7.5l6 6 6-6"/>'),
  bag: outline(18, '<path d="M4.5 6.5h11l1 10.5h-13z"/><path d="M7.5 8.5V6a2.5 2.5 0 015 0v2.5"/>'),
  cart: outline(24, '<path d="M2 3.5h2.2l1.9 9.2a1 1 0 001 .8h7.6a1 1 0 001-.75L17.2 7H5"/><circle cx="8" cy="16.6" r="1.1"/><circle cx="14" cy="16.6" r="1.1"/>'),
  check: outline(28, '<circle cx="10" cy="10" r="8.5"/><path d="M6 10.5l3 3 5-6"/>').replace('stroke-width="1.4"', 'stroke-width="1.6"'),
  warning: outline(16, '<path d="M10 3l7.5 13.5h-15z"/><path d="M10 8.5v3.5M10 14.2v.1"/>'),
  close: outline(16, '<path d="M5 5l10 10M15 5L5 15"/>'),
};

const icon = (name) => h('span', { class: 'lsf-icon', 'aria-hidden': 'true', html: ICONS[name] });

const isShop = (state) => state.variant === 'shop';

// --- Fields ------------------------------------------------------------------------------

/** A text field with a floating label, the way Shopify's checkout draws them. */
function field({ id, label, value, type = 'text', autocomplete, error, actions }) {
  return h(
    'div',
    { class: `lsf-field${error ? ' has-error' : ''}`, key: id, 'data-field': id.replace(/^lsf-/, '') },
    h('input', {
      id,
      class: 'lsf-input',
      type,
      name: id.replace(/^lsf-/, ''),
      value: value ?? '',
      placeholder: ' ',
      autocomplete,
      'aria-invalid': error ? 'true' : null,
      'aria-describedby': error ? `${id}-error` : null,
      onInput: (event) => actions.draft(id.replace(/^lsf-/, ''), event.target.value),
      onChange: (event) => actions.commit(id.replace(/^lsf-/, ''), event.target.value),
    }),
    h('label', { for: id }, label),
    error && h('p', { class: 'lsf-field-error', id: `${id}-error` }, icon('warning'), error),
  );
}

/** A select with its label inside. `onChange` replaces committing the value as a field of the
 * address; `blank: false` leaves out the empty choice. */
function selectField({ id, label, value, options, error, actions, onChange, blank = true }) {
  return h(
    'div',
    { class: `lsf-field lsf-field-select${error ? ' has-error' : ''}`, key: id, 'data-field': id.replace(/^lsf-/, '') },
    h(
      'select',
      {
        id,
        class: 'lsf-input',
        name: id.replace(/^lsf-/, ''),
        value: value ?? '',
        onChange: (event) => (onChange ? onChange(event.target.value) : actions.commit(id.replace(/^lsf-/, ''), event.target.value)),
      },
      blank && h('option', { value: '' }, ''),
      options.map(([code, name]) => h('option', { value: code }, name)),
    ),
    h('label', { for: id }, label),
    icon('chevron'),
    error && h('p', { class: 'lsf-field-error', id: `${id}-error` }, icon('warning'), error),
  );
}

/** The countries of the store; the one already on the address stays, even if the store dropped it. */
function countryOptions(state, current) {
  const countries = countriesOf(state);
  return current && !countries.some(([code]) => code === current)
    ? [...countries, [current, countryName(current)]]
    : countries;
}

/** What the buyer sees in a field: what they typed, else what the checkout holds. */
function addressValue({ state, ui }, name) {
  return ui.draft?.address?.[name] ?? state.shippingAddress?.[name] ?? '';
}

/** The messages for each field: the checkout's own errors, the extensions' and, once the
 * buyer tried to pay, what is missing. */
function fieldErrors({ state, ui }) {
  const placed = placeErrors([...(state.errors ?? []), ...(ui.block?.errors ?? [])]);
  const errors = placed.fields;
  if (ui.tried) {
    const required = {
      email: 'Enter an email',
      lastName: 'Enter a last name',
      address1: 'Enter an address',
      city: 'Enter a city',
      countryCode: 'Select a country/region',
      delivery: 'Select a shipping method',
      payment: 'Select a payment method',
    };
    for (const name of missingFields(state, ui.draft)) {
      if (required[name] && !errors[name]) errors[name] = [required[name]];
    }
  }
  return errors;
}

const firstError = (errors, name) => errors[name]?.[0] ?? null;

// --- The steps ---------------------------------------------------------------------------

/** The customer the checkout is logged in as, where a guest has the email field: who they are,
 * a way out, and for a B2B customer the company and the location they buy for. */
function accountCard(ctx, error) {
  const { state, actions } = ctx;
  const customer = state.buyer.customer;
  const name = fullName(customer);
  const purchasing = state.buyer.purchasingCompany;
  const locations = state.account?.locations ?? [];
  const current = locations.find((location) => location.current);
  return h(
    'div',
    { class: `lsf-account-card${error ? ' has-error' : ''}`, key: 'account-card', 'data-account': customer.email },
    h(
      'div',
      { class: 'lsf-account-row' },
      h('span', { class: 'lsf-avatar', 'aria-hidden': 'true' }, initials(name)),
      h('span', { class: 'lsf-account-text' },
        h('span', { class: 'lsf-account-name' }, name),
        name !== customer.email && h('span', { class: 'lsf-muted' }, customer.email)),
      h('button', { type: 'button', class: 'lsf-link lsf-logout', onClick: () => actions.logIn(null) }, 'Log out'),
    ),
    purchasing?.company &&
      h(
        'div',
        { class: 'lsf-company', 'data-company': purchasing.company.name },
        h('p', null, h('span', { class: 'lsf-muted' }, 'Company '), h('strong', null, purchasing.company.name)),
        locations.length > 1
          ? selectField({
              id: 'lsf-companyLocation',
              label: 'Buying for',
              value: current?.id ?? '',
              options: locations.map((location) => [location.id, location.name]),
              blank: false,
              onChange: (id) => actions.chooseLocation(id),
              actions,
            })
          : purchasing.location && h('p', null, h('span', { class: 'lsf-muted' }, 'Buying for '), purchasing.location.name),
      ),
    error && h('p', { class: 'lsf-field-error' }, icon('warning'), error),
  );
}

function contactSection(ctx) {
  const { state, ui, actions, slot } = ctx;
  const errors = fieldErrors(ctx);
  const emailField = (label) =>
    field({
      id: 'lsf-email',
      label,
      type: 'email',
      autocomplete: 'email',
      value: ui.draft?.email ?? state.buyer?.email ?? '',
      error: firstError(errors, 'email'),
      actions,
    });
  const customer = state.buyer?.customer;
  // Logged in with a contact email that is not the account's: the account, and the email the
  // order is for, which the buyer can still correct or give back to the account's.
  // The field the buyer is in stays, whatever the server answers meanwhile (its email may even
  // be what the buyer has typed so far): it goes, if the email is the account's, once they leave it.
  const editing = customer && ui.emailEditing;
  const other = customer && (otherContactEmail(state) ?? (editing ? ui.draft?.email ?? state.buyer?.email ?? '' : null));
  return h(
    'section',
    { class: 'lsf-section', 'data-section': 'contact' },
    h('div', { class: 'lsf-section-head' }, h('h2', null, 'Contact'), accountMenu(ctx)),
    customer ? accountCard(ctx, other ? null : firstError(errors, 'email')) : emailField('Email'),
    other &&
      h(
        'div',
        {
          class: 'lsf-contact-email',
          key: 'contact-email',
          'data-contact-email': editing ? ui.draft?.email ?? other : other,
          onFocusin: (event) => actions.editEmail(true, event),
          onFocusout: (event) => actions.editEmail(false, event),
        },
        emailField('Email for this order'),
        h(
          'p',
          { class: 'lsf-muted lsf-contact-note' },
          'Not the email of the account. ',
          h('button', { type: 'button', class: 'lsf-link lsf-use-account-email', onClick: () => actions.useAccountEmail() }, `Use ${customer.email}`),
        ),
      ),
    slot('purchase.checkout.contact.render-after', { key: 'contact' }),
  );
}

/** The addresses the logged-in customer saved, to ship to one without typing it. */
function savedAddresses(ctx) {
  const { state, actions } = ctx;
  const saved = state.buyer?.customer ? state.account?.addresses ?? [] : [];
  if (saved.length === 0) return null;
  const countries = countriesOf(state);
  return selectField({
    id: 'lsf-savedAddress',
    label: state.buyer.purchasingCompany ? 'Ship to the location' : 'Saved addresses',
    value: savedAddressId(state),
    options: [...saved.map((entry) => [entry.id, addressLabel(entry, countries)]), ['new', 'Use a new address']],
    blank: false,
    onChange: (id) => actions.chooseSavedAddress(id),
    actions,
  });
}

function addressSection(ctx) {
  const { state, actions, slot } = ctx;
  const errors = fieldErrors(ctx);
  const value = (name) => addressValue(ctx, name);
  const text = (name, label, autocomplete) =>
    field({ id: `lsf-${name}`, label, autocomplete, value: value(name), error: firstError(errors, name), actions });
  return h(
    'section',
    { class: 'lsf-section', 'data-section': 'delivery' },
    h('h2', null, isShop(state) ? 'Ship to' : 'Delivery'),
    slot('purchase.checkout.delivery-address.render-before', { key: 'address-before' }),
    savedAddresses(ctx),
    selectField({
      id: 'lsf-countryCode',
      label: 'Country/Region',
      value: value('countryCode'),
      options: countryOptions(state, value('countryCode')),
      error: firstError(errors, 'countryCode'),
      actions,
    }),
    h('div', { class: 'lsf-row' }, text('firstName', 'First name (optional)', 'given-name'), text('lastName', 'Last name', 'family-name')),
    text('company', 'Company (optional)', 'organization'),
    text('address1', 'Address', 'address-line1'),
    text('address2', 'Apartment, suite, etc. (optional)', 'address-line2'),
    h('div', { class: 'lsf-row' }, text('zip', 'Postal code', 'postal-code'), text('city', 'City', 'address-level2')),
    text('provinceCode', 'State / Province (optional)', 'address-level1'),
    text('phone', 'Phone (optional)', 'tel'),
    slot('purchase.checkout.delivery-address.render-after', { key: 'address-after' }),
  );
}

function deliverySection(ctx) {
  const { state, actions, slot } = ctx;
  const errors = fieldErrors(ctx);
  const known = Boolean(state.shippingAddress?.countryCode);
  const titles = new Map(state.lines.map((line) => [line.id, line.title]));
  const groups = state.deliveryGroups ?? [];
  return h(
    'section',
    { class: 'lsf-section', 'data-section': 'shipping' },
    h('h2', null, isShop(state) ? 'Delivery' : 'Shipping method'),
    slot('purchase.checkout.shipping-option-list.render-before', { key: 'shipping-before' }),
    !known
      ? h('p', { class: 'lsf-placeholder' }, 'Enter your shipping address to view available shipping methods.')
      : groups.length === 0
        ? h('p', { class: 'lsf-placeholder' }, 'There are no shipping methods available for your address.')
        : groups.map((group, index) =>
            h(
              'div',
              { class: 'lsf-group', key: group.handle, 'data-group': group.handle },
              groups.length > 1 &&
                h('h3', null, `Shipment ${index + 1} of ${groups.length}`, h('span', { class: 'lsf-muted' }, ` · ${group.lineIds.map((id) => titles.get(id)).filter(Boolean).join(', ')}`)),
              h(
                'div',
                { class: 'lsf-radio-list', role: 'radiogroup' },
                group.options.map((option) => {
                  const selected = group.selected === option.handle;
                  return h(
                    'div',
                    { class: `lsf-radio-card${selected ? ' is-selected' : ''}`, key: option.handle },
                    h(
                      'label',
                      { class: 'lsf-radio' },
                      h('input', {
                        type: 'radio',
                        name: `delivery-${group.handle}`,
                        value: option.handle,
                        checked: selected,
                        onChange: () => actions.chooseDelivery(group.handle, option.handle),
                      }),
                      h('span', { class: 'lsf-radio-text' },
                        h('span', { class: 'lsf-radio-title' }, option.title),
                        option.description && h('span', { class: 'lsf-muted' }, option.description)),
                      h('span', { class: 'lsf-radio-price' },
                        option.cost.amount !== option.originalCost.amount &&
                          h('s', { class: 'lsf-muted' }, price(option.originalCost, state.shop)),
                        priceOrFree(option.cost, state.shop)),
                    ),
                    slot('purchase.checkout.shipping-option-item.render-after', {
                      key: `${group.handle}:${option.handle}`, group: group.handle, option: option.handle,
                    }),
                    selected && slot('purchase.checkout.shipping-option-item.details.render', {
                      key: `${group.handle}:${option.handle}:details`, group: group.handle, option: option.handle,
                    }),
                  );
                }),
              ),
            ),
          ),
    firstError(errors, 'delivery') && h('p', { class: 'lsf-field-error' }, icon('warning'), firstError(errors, 'delivery')),
    slot('purchase.checkout.shipping-option-list.render-after', { key: 'shipping-after' }),
  );
}

const PAYMENT_NOTES = {
  cod: 'You pay when the order is delivered.',
  bank: 'Our bank details are sent to you after you complete your order. Your order ships once the payment arrives.',
};

function paymentNote(method) {
  const key = Object.keys(PAYMENT_NOTES).find((name) => method.handle.toLowerCase().includes(name));
  return key ? PAYMENT_NOTES[key] : `After clicking “Complete order”, you pay with ${method.name}.`;
}

function cardForm() {
  const input = (name, label, autocomplete) =>
    h('div', { class: 'lsf-field' },
      h('input', { id: `lsf-card-${name}`, class: 'lsf-input', placeholder: ' ', autocomplete, inputmode: 'numeric' }),
      h('label', { for: `lsf-card-${name}` }, label));
  return h('div', { class: 'lsf-card-form' },
    input('number', 'Card number', 'cc-number'),
    h('div', { class: 'lsf-row' }, input('expiry', 'Expiration date (MM / YY)', 'cc-exp'), input('cvc', 'Security code', 'cc-csc')),
    input('name', 'Name on card', 'cc-name'),
    h('p', { class: 'lsf-muted lsf-demo-note' }, 'Demo checkout: card details are not sent anywhere.'));
}

function paymentSection(ctx) {
  const { state, actions, slot } = ctx;
  const errors = fieldErrors(ctx);
  return h(
    'section',
    { class: 'lsf-section', 'data-section': 'payment' },
    h('h2', null, 'Payment'),
    h('p', { class: 'lsf-muted lsf-subtitle' }, 'All transactions are secure and encrypted.'),
    slot('purchase.checkout.payment-method-list.render-before', { key: 'payment-before' }),
    h(
      'div',
      { class: 'lsf-radio-list', role: 'radiogroup' },
      (state.paymentMethods ?? []).map((method) => {
        const selected = state.selectedPayment === method.handle;
        return h(
          'div',
          { class: `lsf-radio-card${selected ? ' is-selected' : ''}`, key: method.handle },
          h('label', { class: 'lsf-radio' },
            h('input', { type: 'radio', name: 'payment', value: method.handle, checked: selected, onChange: () => actions.choosePayment(method.handle) }),
            h('span', { class: 'lsf-radio-text' }, h('span', { class: 'lsf-radio-title' }, method.name)),
            method.handle === 'card' && h('span', { class: 'lsf-card-brands', 'aria-hidden': 'true' }, ['VISA', 'MC', 'AMEX'].map((brand) => h('span', { class: 'lsf-card-brand', key: brand }, brand)))),
          selected && h('div', { class: 'lsf-radio-panel' }, method.handle === 'card' ? cardForm() : h('p', null, paymentNote(method))),
        );
      }),
    ),
    firstError(errors, 'payment') && h('p', { class: 'lsf-field-error' }, icon('warning'), firstError(errors, 'payment')),
    slot('purchase.checkout.payment-method-list.render-after', { key: 'payment-after' }),
  );
}

function banner(tone, children, extra = {}) {
  return h('div', { class: `lsf-banner is-${tone}`, role: tone === 'critical' ? 'alert' : 'status', ...extra }, icon('warning'), h('div', null, children));
}

/** What the buyer pays now. A state without the split (an older one) has all of it due now. */
const dueNow = (state) => state.cost.dueNow ?? state.cost.total;

/** What a selling plan leaves to pay later (a pre-order's balance), or null when nothing. */
const dueLater = (state) => (state.cost.dueLater?.amount > 0 ? state.cost.dueLater : null);

function actionsSection(ctx) {
  const { state, ui, actions, slot } = ctx;
  const placed = placeErrors(state.errors);
  const blockErrors = placeErrors(ui.block?.errors).general;
  const total = price(dueNow(state), state.shop);
  const label = state.selectedPayment && state.selectedPayment !== 'card' ? 'Complete order' : 'Pay now';
  return h(
    'div',
    { class: 'lsf-actions' },
    placed.general.length > 0 &&
      banner('critical', placed.general.map((message) => h('p', null, message)), { 'data-banner': 'errors' }),
    ui.block && banner('critical', [h('p', null, ui.block.reason), blockErrors.map((message) => h('p', null, message))], { 'data-banner': 'blocked' }),
    ui.failure && banner('critical', h('p', null, ui.failure), { 'data-banner': 'failure' }),
    slot('purchase.checkout.actions.render-before', { key: 'actions' }),
    h('div', { class: 'lsf-actions-row' },
      h('button', {
        class: 'lsf-pay', type: 'button', disabled: ui.busy ? true : null, onClick: actions.pay,
      }, ui.busy ? 'Processing…' : isShop(state) ? `${label} · ${total}` : label),
      h('a', { class: 'lsf-link lsf-return', href: storefrontLink(state, '/cart') }, '‹ Return to cart')));
}

// --- The summary -------------------------------------------------------------------------

function lineItem(ctx, line, { thankYou = false } = {}) {
  const { state, slot } = ctx;
  const discounted = line.total.amount !== line.subtotal.amount;
  return h(
    'li',
    { class: 'lsf-line', key: line.id, 'data-line': line.id },
    h('div', { class: 'lsf-line-main' },
      h('div', { class: 'lsf-thumb' },
        line.image ? h('img', { src: line.image, alt: '' }) : icon('bag'),
        h('span', { class: 'lsf-qty', 'aria-label': `Quantity ${line.quantity}` }, line.quantity)),
      h('div', { class: 'lsf-line-info' },
        h('span', { class: 'lsf-line-title' }, line.title),
        line.variantTitle && h('span', { class: 'lsf-muted' }, line.variantTitle),
        line.sellingPlan?.name && h('span', { class: 'lsf-muted lsf-plan' }, line.sellingPlan.name),
        line.dueLater?.amount > 0 && h('span', { class: 'lsf-muted lsf-line-later' }, `Due later ${price(line.dueLater, state.shop)}`),
        (line.attributes ?? []).map((attribute) => h('span', { class: 'lsf-muted' }, `${attribute.key}: ${attribute.value}`)),
        (line.discountAllocations ?? []).map((allocation) =>
          h('span', { class: 'lsf-tag' }, allocation.code ?? allocation.title, ` (−${price(allocation.amount, state.shop)})`)),
        line.transformedBy && h('span', { class: 'lsf-muted' }, `Changed by a cart transform (${line.transformedBy})`)),
      h('div', { class: 'lsf-line-price' },
        discounted && h('s', { class: 'lsf-muted' }, price(line.subtotal, state.shop)),
        price(line.total, state.shop))),
    slot(thankYou ? 'purchase.thank-you.cart-line-item.render-after' : 'purchase.checkout.cart-line-item.render-after', { key: line.id, lineId: line.id }),
  );
}

function costRows(ctx) {
  const { state } = ctx;
  const { cost } = state;
  const later = dueLater(state);
  const count = state.lines.reduce((sum, line) => sum + line.quantity, 0);
  const hasShipping = state.deliveryGroups?.some((group) => group.selected);
  const shipping = state.status === 'completed' || hasShipping
    ? priceOrFree(cost.shipping, state.shop)
    : h('span', { class: 'lsf-muted' }, state.lines.some((line) => line.requiresShipping) ? 'Enter shipping address' : 'Free');
  return h('dl', { class: 'lsf-costs' },
    h('div', null, h('dt', null, `Subtotal · ${count} item${count === 1 ? '' : 's'}`), h('dd', null, price(cost.subtotal, state.shop))),
    (state.discounts ?? []).map((discount, index) =>
      h('div', { class: 'lsf-cost-discount', key: `${discount.function}:${index}` },
        h('dt', null, h('span', { class: 'lsf-tag' }, discount.code ?? discount.title), discount.class === 'SHIPPING' && h('span', { class: 'lsf-muted' }, ' shipping')),
        h('dd', null, `−${price(discount.amount, state.shop)}`))),
    h('div', null, h('dt', null, 'Shipping'), h('dd', null, shipping)),
    cost.tax.amount > 0 && h('div', null, h('dt', null, 'Estimated taxes'), h('dd', null, price(cost.tax, state.shop))),
    // A selling plan that defers a part of the price: the order costs its total, and the buyer
    // pays what is due today.
    later
      ? h('div', { class: 'lsf-cost-order-total' }, h('dt', null, 'Total'), h('dd', null, price(cost.total, state.shop)))
      : null,
    h('div', { class: 'lsf-total', ...(later ? { 'data-due': 'now' } : {}) },
      h('dt', null, later ? 'Due today' : 'Total'),
      h('dd', null, h('span', { class: 'lsf-muted lsf-currency' }, cost.currencyCode), h('strong', null, price(dueNow(state), state.shop)))),
    later && h('div', { class: 'lsf-cost-later', 'data-due': 'later' }, h('dt', null, 'Due later'), h('dd', null, price(later, state.shop))),
  );
}

function discountForm(ctx) {
  const { state, ui, actions, slot } = ctx;
  const codes = state.discountCodes ?? [];
  return h('div', { class: 'lsf-reductions' },
    slot('purchase.checkout.reductions.render-before', { key: 'reductions-before' }),
    h('form', { class: 'lsf-discount-form', onSubmit: (event) => { event.preventDefault(); actions.addDiscount(); } },
      h('div', { class: `lsf-field${ui.discountError ? ' has-error' : ''}` },
        h('input', {
          id: 'lsf-discount', class: 'lsf-input', name: 'discount', placeholder: ' ', value: ui.discountInput ?? '', autocomplete: 'off',
          onInput: (event) => {
            actions.draft('discount', event.target.value);
            // Typing does not render the page again: the button follows the field by hand.
            const apply = event.target.parentElement?.querySelector('.lsf-apply');
            if (apply) apply.disabled = ui.busy || event.target.value.trim() === '';
          },
        }),
        h('label', { for: 'lsf-discount' }, 'Discount code'),
        // Waits for a code: disabled, for the mouse and the keyboard, while the field is empty.
        h('button', { class: 'lsf-apply', type: 'submit', disabled: ui.busy || !(ui.discountInput ?? '').trim() ? true : null }, 'Apply'),
        ui.discountError && h('p', { class: 'lsf-field-error' }, icon('warning'), ui.discountError))),
    codes.length > 0 && h('ul', { class: 'lsf-codes' },
      codes.map((entry) =>
        h('li', { key: entry.code, class: `lsf-code${entry.applicable ? '' : ' is-ignored'}` },
          h('span', { class: 'lsf-tag' }, entry.code),
          !entry.applicable && h('span', { class: 'lsf-muted' }, 'Not applied'),
          h('button', { type: 'button', class: 'lsf-code-remove', 'aria-label': `Remove ${entry.code}`, onClick: () => actions.removeDiscount(entry.code) }, '×')))),
    slot('purchase.checkout.reductions.render-after', { key: 'reductions-after' }),
  );
}

function summary(ctx, { thankYou = false } = {}) {
  const { state, ui, actions, slot } = ctx;
  const total = price(state.cost.total, state.shop);
  return h(
    'aside',
    { class: 'lsf-summary', 'data-open': ui.summaryOpen ? 'true' : 'false', 'aria-label': 'Order summary' },
    h('button', { class: 'lsf-summary-toggle', type: 'button', 'aria-expanded': ui.summaryOpen ? 'true' : 'false', onClick: actions.toggleSummary },
      icon('bag'), h('span', null, ui.summaryOpen ? 'Hide order summary' : 'Show order summary'), icon('chevron'), h('strong', null, total)),
    h('div', { class: 'lsf-summary-body' },
      h('ul', { class: 'lsf-lines' }, state.lines.map((line) => lineItem(ctx, line, { thankYou }))),
      slot(thankYou ? 'purchase.thank-you.cart-line-list.render-after' : 'purchase.checkout.cart-line-list.render-after', { key: 'lines-after' }),
      !thankYou && discountForm(ctx),
      costRows(ctx)),
  );
}

// --- The pages ---------------------------------------------------------------------------

function header(ctx) {
  const { state } = ctx;
  return h('header', { class: 'lsf-header' },
    h('a', { class: 'lsf-brand', href: storefrontLink(state) }, isShop(state) ? h('span', { class: 'lsf-shop-logo' }, 'shop') : state.shop.name),
    isShop(state) && h('span', { class: 'lsf-brand-sub' }, state.shop.name),
    state.scenario && h('a', { class: 'lsf-chip lsf-scenario', href: state.home ?? '/', title: 'All checkouts and extensions', 'data-scenario': state.scenario.name }, `Scenario: ${state.scenario.title}`),
    isShop(state)
      ? h('span', { class: 'lsf-secure' }, icon('lock'), 'Shop Pay')
      : h('a', { class: 'lsf-cart-link', href: storefrontLink(state, '/cart'), 'aria-label': 'Return to cart' }, icon('cart')));
}

/** A build of `--watch` that failed, with the end of its output: the Function runs the last
 * module that built, when one did (`built`). */
function buildFailure(build) {
  const meanwhile = build.built === false ? 'No module of it built yet.' : 'The checkout runs the last module that built.';
  return [
    h('p', null, h('strong', null, build.handle), `: ${build.reason}. ${meanwhile}`),
    build.output && h('pre', { class: 'lsf-build-output' }, build.output),
  ];
}

/** What is wrong with the app: the builds that failed, the server's diagnostics and the
 * extensions the host does not load. They open the main column, on its panel, so the order
 * summary's panel starts under the header. */
function diagnostics({ state, refusals = [] }) {
  const builds = state.builds ?? [];
  const messages = [
    ...(state.diagnostics ?? []),
    ...refusals.map((refusal) => `${refusal.handle}: not loaded. ${refusal.reason}`),
  ];
  return messages.length > 0 || builds.length > 0
    ? h('div', { class: 'lsf-diagnostics' },
      builds.map((build) => banner('critical', buildFailure(build), { 'data-banner': 'build', 'data-build': build.handle })),
      messages.map((message) => banner('warning', h('p', null, message))))
    : null;
}

/** An empty cart has nothing to check out: Shopify sends the buyer back to the cart. The page
 * says so and offers the scenarios, which come with a cart. */
function emptyCart(ctx) {
  const { state } = ctx;
  return h('section', { class: 'lsf-section lsf-empty-cart', 'data-section': 'empty-cart' },
    h('h2', null, 'Your cart is empty'),
    h('p', { class: 'lsf-placeholder' },
      'This checkout has no line to pay for. ',
      state.home ? 'Open a checkout scenario, which comes with its cart, or add products from the store.' : 'Add products from the store.'),
    h('div', { class: 'lsf-empty-cart-links' },
      state.home && h('a', { class: 'lsf-link', href: state.home, 'data-link': 'scenarios' }, 'Checkout scenarios'),
      h('a', { class: 'lsf-link lsf-return', href: storefrontLink(state, '/cart') }, '‹ Return to cart')));
}

function checkoutPage(ctx) {
  const { state, slot } = ctx;
  if (state.lines.length === 0 && !ctx.preview) {
    return h('div', { class: 'lsf-layout' },
      h('main', { class: 'lsf-main' }, diagnostics(ctx), contactSection(ctx), emptyCart(ctx)),
      summary(ctx));
  }
  // Showing every target: the steps of a cart that ships are there, whatever the cart.
  const needsShipping = state.lines.some((line) => line.requiresShipping) || Boolean(ctx.preview);
  return h('div', { class: 'lsf-layout' },
    h('main', { class: 'lsf-main' },
      diagnostics(ctx),
      slot('purchase.checkout.header.render-after', { key: 'header' }),
      contactSection(ctx),
      needsShipping && addressSection(ctx),
      needsShipping && deliverySection(ctx),
      slot('purchase.checkout.block.render', { key: 'block' }),
      paymentSection(ctx),
      actionsSection(ctx),
      slot('purchase.checkout.footer.render-after', { key: 'footer' })),
    summary(ctx));
}

function addressLines(address, countries) {
  if (!address) return [];
  return [
    [address.firstName, address.lastName].filter(Boolean).join(' '),
    address.company,
    address.address1,
    address.address2,
    [address.zip, address.city].filter(Boolean).join(' '),
    countryName(address.countryCode, countries),
  ].filter(Boolean);
}

function thankYouPage(ctx) {
  const { state, slot, actions } = ctx;
  const order = state.order ?? {};
  const name = state.shippingAddress?.firstName || state.buyer?.customer?.firstName;
  const selected = (state.deliveryGroups ?? []).flatMap((group) => group.options.filter((option) => option.handle === group.selected));
  const method = (state.paymentMethods ?? []).find((entry) => entry.handle === state.selectedPayment);
  // What the order still owes: the one it recorded, the balance of its cost otherwise.
  const owed = order.totalOutstanding ?? state.cost.dueLater;
  const balance = owed?.amount > 0 ? owed : null;
  const detail = (title, lines) =>
    h('div', { class: 'lsf-detail' }, h('h3', null, title), lines.map((line) => h('p', null, line)));
  return h('div', { class: 'lsf-layout lsf-thanks' },
    h('main', { class: 'lsf-main' },
      // Shown once: the thank-you page under a checkout is not the page.
      !ctx.underCheckout && diagnostics(ctx),
      slot('purchase.thank-you.announcement.render', { key: 'announcement' }),
      h('div', { class: 'lsf-confirmation' }, icon('check'),
        h('div', null,
          h('p', { class: 'lsf-muted', 'data-test': 'confirmation' }, `Confirmation ${order.number ?? '#1001'}`),
          h('h1', null, name ? `Thank you, ${name}!` : 'Thank you!'))),
      slot('purchase.thank-you.header.render-after', { key: 'header' }),
      h('section', { class: 'lsf-section lsf-confirmed' },
        h('h2', null, 'Your order is confirmed'),
        h('p', null, `You will receive a confirmation email at ${state.buyer?.email ?? 'your email address'}.`),
        balance && h('p', { 'data-test': 'balance' }, `The remaining balance of ${price(balance, state.shop)} is due later.`)),
      slot('purchase.thank-you.block.render', { key: 'block' }),
      h('section', { class: 'lsf-section' },
        h('h2', null, 'Order details'),
        h('div', { class: 'lsf-details' },
          detail('Contact information', [state.buyer?.email, state.buyer?.phone].filter(Boolean)),
          detail('Shipping address', addressLines(state.shippingAddress, countriesOf(state))),
          detail('Shipping method', selected.map((option) => `${option.title} · ${priceOrFree(option.cost, state.shop)}`)),
          detail('Payment method', method ? [`${method.name} · ${price(order.totalReceived ?? dueNow(state), state.shop)}`] : []),
          balance && detail('Remaining balance', [`${price(balance, state.shop)} due later`]),
          detail('Billing address', state.shippingAddress ? ['Same as shipping address'] : [])),
        slot('purchase.thank-you.customer-information.render-after', { key: 'customer' })),
      h('div', { class: 'lsf-actions-row' },
        h('a', { class: 'lsf-pay lsf-continue', href: storefrontLink(state), onClick: actions.continueShopping }, 'Continue shopping')),
      slot('purchase.thank-you.footer.render-after', { key: 'footer' })),
    summary(ctx, { thankYou: true }));
}

// --- The Functions: a badge on every page, and a dialog with their runs ------------------

const json = (value) => JSON.stringify(value, null, 2);

// A string with its colon when it is a key, a number, or a literal of JSON text.
const JSON_TOKEN = /("(?:[^"\\]|\\.)*")(\s*:)?|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|\b(?:true|false|null)\b/g;
// Past this size, the text is shown plain: thousands of spans would slow the page down.
const HIGHLIGHT_LIMIT = 200_000;

/** `value` as indented JSON, its keys, strings, numbers and literals in their colors. */
export function jsonCode(value) {
  const text = json(value);
  if (text === undefined || text.length > HIGHLIGHT_LIMIT) return h('pre', null, text ?? String(value));
  const parts = [];
  let last = 0;
  for (const match of text.matchAll(JSON_TOKEN)) {
    const [token, string, colon] = match;
    if (match.index > last) parts.push(text.slice(last, match.index));
    const kind = string ? (colon ? 'key' : 'string') : token === 'null' ? 'null' : token === 'true' || token === 'false' ? 'boolean' : 'number';
    parts.push(h('span', { class: `lsf-json-${kind}` }, string ?? token));
    if (colon) parts.push(colon);
    last = match.index + token.length;
  }
  if (last < text.length) parts.push(text.slice(last));
  return h('pre', { class: 'lsf-json' }, parts);
}

const plural = (count, word) => `${count} ${word}${count === 1 ? '' : 's'}`;

/** A run of the dialog, as the page remembers it across updates: its handle, its target, and
 * which of the runs with both it is (a Function may run twice for the same target). */
export function runView(runs, index) {
  const { handle, target } = runs[index];
  const occurrence = runs.slice(0, index).filter((entry) => entry.handle === handle && entry.target === target).length;
  return { handle, target, occurrence };
}

/** What the dialog shows when it opens: the first run that failed, else the first run, else
 * the overview. */
export function defaultRunView(runs = []) {
  const failed = runs.findIndex((entry) => entry.error);
  if (failed >= 0) return runView(runs, failed);
  return runs.length > 0 ? runView(runs, 0) : 'overview';
}

/**
 * What `?functions=` of the URL `href` asks the dialog of the Functions to open on: null when
 * the URL does not have it; otherwise `{handle, target}`, from `<handle>` or
 * `<handle>:<target>`, both null for an empty value (the overview).
 */
export function functionsRequest(href) {
  let value;
  try {
    value = new URL(href ?? '', 'http://localhost/').searchParams.get('functions');
  } catch {
    return null;
  }
  if (value === null) return null;
  value = value.trim();
  if (value === '') return { handle: null, target: null };
  const colon = value.indexOf(':');
  if (colon < 0) return { handle: value, target: null };
  return { handle: value.slice(0, colon), target: value.slice(colon + 1) || null };
}

/** What the dialog opens on for `request` (see `functionsRequest`): the runs of its handle (and
 * target), the first that failed, else the first; the overview when the checkout has none. */
export function requestedView(runs = [], request) {
  if (!request?.handle) return 'overview';
  const matches = (entry) => entry.handle === request.handle && (!request.target || entry.target === request.target);
  const failed = runs.findIndex((entry) => matches(entry) && entry.error);
  const index = failed >= 0 ? failed : runs.findIndex(matches);
  return index >= 0 ? runView(runs, index) : 'overview';
}

/** The index of the run `view` names, `-1` for the overview. A run that is gone gives way to
 * the default. */
export function selectedRun(runs = [], view) {
  if (view === 'overview') return -1;
  if (view && typeof view === 'object') {
    let seen = 0;
    for (const [index, entry] of runs.entries()) {
      if (entry.handle !== view.handle || entry.target !== view.target) continue;
      if (seen === view.occurrence) return index;
      seen += 1;
    }
  }
  const fallback = defaultRunView(runs);
  return fallback === 'overview' ? -1 : selectedRun(runs, fallback);
}

/** How the Functions went: the runs, those that failed, the errors that block the checkout, the
 * builds of `--watch` that failed. */
function functionsSummary(state) {
  const runs = state.runs ?? [];
  const failed = runs.filter((entry) => entry.error).length;
  const errors = (state.errors ?? []).length;
  const builds = (state.builds ?? []).length;
  return { runs, failed, errors, builds, ok: failed === 0 && errors === 0 && builds === 0 };
}

/** The chips of that summary, after the number of runs. */
function summaryChips({ failed, errors, builds }) {
  return [
    failed > 0 && h('span', { class: 'lsf-chip is-error' }, `${failed} failed`),
    errors > 0 && h('span', { class: 'lsf-chip is-error' }, plural(errors, 'checkout error')),
    builds > 0 && h('span', { class: 'lsf-chip is-error' }, plural(builds, 'build error')),
  ];
}

/** The badge in the bottom left corner of every page: how the Functions went, and the way into
 * the dialog. It holds two faces and the CSS shows one: the short one (the dot, ƒ and the number
 * of runs) wherever the full one (its name, the runs, the chips) could reach the checkout. Its
 * name always says it all. */
function functionsBadge(ctx) {
  const { state, actions } = ctx;
  const summary = functionsSummary(state);
  const count = summary.runs.length === 0 ? 'No runs' : plural(summary.runs.length, 'run');
  const label = ['Functions', count, summary.failed > 0 && `${summary.failed} failed`, summary.errors > 0 && plural(summary.errors, 'checkout error'),
    summary.builds > 0 && plural(summary.builds, 'build error')]
    .filter(Boolean).join(', ');
  return h('button', {
      type: 'button', class: 'lsf-fn-badge', key: 'badge', 'aria-haspopup': 'dialog', 'aria-label': label, title: label,
      'data-status': summary.ok ? 'ok' : 'failed',
      onClick: () => actions.toggleFunctions(true),
    },
    h('span', { class: 'lsf-fn-dot', 'aria-hidden': 'true' }),
    h('span', { class: 'lsf-fn-badge-short', 'aria-hidden': 'true' },
      h('span', { class: 'lsf-fn-badge-mark' }, 'ƒ'), String(summary.runs.length)),
    h('span', { class: 'lsf-fn-badge-full', 'aria-hidden': 'true' },
      h('span', { class: 'lsf-fn-badge-title' }, 'Functions'),
      h('span', { class: 'lsf-fn-badge-count' }, count),
      summary.failed > 0 && h('span', { class: 'lsf-chip is-error' }, `${summary.failed} failed`),
      summary.errors > 0 && h('span', { class: 'lsf-chip is-error' }, plural(summary.errors, 'error')),
      summary.builds > 0 && h('span', { class: 'lsf-chip is-error' }, plural(summary.builds, 'build error'))));
}

const okChip = (entry) => h('span', { class: `lsf-chip ${entry.error ? 'is-error' : 'is-ok'}` }, entry.error ? 'error' : 'ok');
const instructions = (entry) => {
  const count = entry.instructions ?? 0;
  return `${count.toLocaleString('en-US')} instruction${count === 1 ? '' : 's'}`;
};

/** The list on the left of the dialog: the overview, then each run. */
function functionsRail(ctx, runs, selected) {
  const { actions } = ctx;
  const entry = (key, current, props, ...children) =>
    h('li', { key }, h('button', { type: 'button', class: 'lsf-fn-entry', 'aria-current': current ? 'true' : null, ...props }, children));
  return h('nav', { class: 'lsf-fn-rail', 'aria-label': 'Functions' },
    h('ul', null,
      entry('overview', selected === -1, { 'data-view': 'overview', onClick: () => actions.showRun('overview') },
        h('strong', null, 'Overview')),
      runs.map((run, index) => entry(`${index}:${run.handle}:${run.target}`, selected === index, {
          'data-run': run.handle,
          onClick: () => actions.showRun(runView(runs, index)),
        },
        h('span', { class: 'lsf-fn-entry-head' }, h('strong', null, run.handle), okChip(run)),
        h('code', null, run.target),
        h('span', { class: 'lsf-muted' }, instructions(run))))));
}

/** One JSON of a run, with its title and a button that copies it. */
function jsonPane(ctx, id, title, value) {
  const { ui, actions } = ctx;
  const none = value === null || value === undefined;
  const copied = ui.copied === id;
  return h('section', { class: 'lsf-fn-pane', 'data-pane': id.split(':').pop() },
    h('div', { class: 'lsf-fn-pane-head' },
      h('h4', null, title),
      !none && h('button', {
        type: 'button', class: `lsf-fn-copy${copied ? ' is-copied' : ''}`, 'aria-live': 'polite',
        onClick: () => actions.copyJson(id, json(value)),
      }, copied ? 'Copied' : 'Copy')),
    none ? h('pre', null, '(none)') : jsonCode(value));
}

/** A run: what it is, its error and logs, its input and output side by side and, for a
 * `.fetch` target, the HTTP request it asked for and the response its `.run` target reads. */
function runDetail(ctx, runs, index) {
  const entry = runs[index];
  const key = `${index}:${entry.handle}:${entry.target}`;
  return h('div', { class: 'lsf-fn-run', key: `run:${key}`, 'data-run': entry.handle },
    h('div', { class: 'lsf-fn-run-head' },
      h('h3', null, entry.handle), h('code', null, entry.target), okChip(entry),
      h('span', { class: 'lsf-muted' }, instructions(entry))),
    entry.error && h('div', { class: 'lsf-fn-note' }, h('h4', null, 'Error'), h('pre', { class: 'is-error' }, entry.error)),
    entry.logs && h('div', { class: 'lsf-fn-note' }, h('h4', null, 'Logs'), h('pre', null, entry.logs)),
    h('div', { class: 'lsf-fn-panes', 'data-panes': 'run' },
      jsonPane(ctx, `${key}:input`, 'Input', entry.input),
      jsonPane(ctx, `${key}:output`, 'Output', entry.output)),
    entry.exchange && h('div', { class: 'lsf-fn-panes is-exchange', 'data-panes': 'exchange' },
      jsonPane(ctx, `${key}:request`, 'HTTP request', entry.exchange.request),
      jsonPane(ctx, `${key}:response`, 'HTTP response', entry.exchange.response)));
}

/** What the page and the app hold besides the runs: the other skin, the extensions, the errors
 * that block the checkout and the discounts that were not applied. */
function functionsOverview(ctx) {
  const { state } = ctx;
  const refused = new Set((ctx.refusals ?? []).map((refusal) => refusal.handle));
  // What `?functions=` asked for and has no run here: its entries stand out, and a note says
  // so when none of them names it.
  const focus = ctx.ui?.functionsFocus?.handle ?? null;
  const focused = (handle) => (focus !== null && handle === focus ? 'is-focus' : null);
  const named = focus !== null && ((state.errors ?? []).some((error) => error.function === focus)
    || (state.extensions ?? []).some((ext) => ext.handle === focus));
  const ran = focus !== null && (state.runs ?? []).some((entry) => entry.handle === focus);
  const target = ctx.ui?.functionsFocus?.target;
  const other = isShop(state) ? 'standard' : 'shop';
  const link = new URL(ctx.location ?? 'http://localhost/');
  link.searchParams.set('variant', other);
  const extensions = state.extensions ?? [];
  return h('div', { class: 'lsf-fn-overview', key: 'overview' },
    h('p', { class: 'lsf-muted' }, h('a', { class: 'lsf-link', href: link.pathname + link.search }, `Switch to the ${other} skin`),
      ` · ${plural(extensions.length, 'checkout UI extension')}`,
      state.scenario && ` · scenario ${state.scenario.title}`,
      state.home && ' · ', state.home && h('a', { class: 'lsf-link', href: state.home }, 'All checkouts and extensions')),
    focus !== null && !named && h('p', { class: 'lsf-fn-focus', 'data-focus': focus },
      h('code', null, focus),
      ran && target ? [' has no run of ', h('code', null, target), ' in this checkout.'] : ' did not run or load in this checkout.',
      state.home && [' ', h('a', { class: 'lsf-link', href: state.home }, 'The home page says why.')]),
    (state.builds ?? []).length > 0 && h('div', { 'data-test': 'build-errors' }, h('h4', null, 'Builds that failed'),
      h('ul', null, state.builds.map((build) => h('li', { key: build.handle, class: focused(build.handle) }, buildFailure(build))))),
    (state.runs ?? []).length === 0 && h('p', { class: 'lsf-muted' }, 'No Function ran.'),
    (state.errors ?? []).length > 0 && h('div', null, h('h4', null, 'Errors that block the checkout'),
      h('ul', null, state.errors.map((error) => h('li', { class: focused(error.function) }, error.message, error.function && h('code', null, error.function), error.target && h('code', null, error.target))))),
    (state.skippedDiscounts ?? []).length > 0 && h('div', { 'data-test': 'skipped-discounts' }, h('h4', null, 'Discounts not applied'),
      h('ul', null, state.skippedDiscounts.map((skipped) => h('li', null, h('strong', null, skipped.title), skipped.code && h('code', null, skipped.code), ` ${skipped.reason}`, h('code', null, skipped.function))))),
    extensions.length > 0 && h('div', null, h('h4', null, 'Extensions'),
      h('ul', null, extensions.map((ext) => h('li', { key: ext.handle, class: focused(ext.handle) }, h('strong', null, ext.handle), h('span', { class: 'lsf-muted' }, ` ${ext.apiVersion} · `), refused.has(ext.handle) && h('span', { class: 'lsf-chip is-error' }, 'not loaded'), (ext.targets ?? []).map((target) => h('code', null, target)))))));
}

/**
 * The dialog of the Functions. The element is always there, at the same place, so that the page
 * can open it with `showModal()` and no update replaces it while it is open; what is in it is
 * built only while it is open, and the JSON only for the run it shows.
 */
function functionsDialog(ctx) {
  const { state, ui, actions } = ctx;
  const summary = functionsSummary(state);
  const selected = selectedRun(summary.runs, ui.functionsView);
  return h('dialog', {
      class: 'lsf-fn-dialog', key: 'dialog', 'aria-labelledby': 'lsf-fn-title', closedby: 'any',
      onClose: () => actions.functionsClosed(),
      onPointerdown: (event) => actions.functionsPointer(event),
      onClick: (event) => actions.functionsClick(event),
    },
    ui.functionsOpen && h('div', { class: 'lsf-fn-frame' },
      h('header', { class: 'lsf-fn-head' },
        h('h2', { id: 'lsf-fn-title' }, 'Functions'),
        h('span', { class: 'lsf-muted' }, plural(summary.runs.length, 'run')),
        summaryChips(summary),
        h('button', { type: 'button', class: 'lsf-fn-close', 'aria-label': 'Close', onClick: () => actions.toggleFunctions(false) }, icon('close'))),
      h('div', { class: 'lsf-fn-body' },
        functionsRail(ctx, summary.runs, selected),
        h('div', { class: 'lsf-fn-main' },
          selected === -1 ? functionsOverview(ctx) : runDetail(ctx, summary.runs, selected)))));
}

/** The badge and the dialog, last on every page. */
function functions(ctx) {
  return h('div', { class: 'lsf-fn', key: 'functions' }, functionsBadge(ctx), functionsDialog(ctx));
}

// --- Every extension target --------------------------------------------------------------

/** Why a target that has a place on the page is not on it right now. */
function whyNotShown(target, state) {
  if (target.includes('shipping-option-item.details')) return 'needs a selected shipping method: enter a delivery address and choose one';
  if (target.includes('shipping-option-item')) return 'needs shipping methods: enter a delivery address';
  if (target.includes('cart-line-item')) return 'needs a line in the cart';
  return state.lines.length === 0 ? 'needs a cart with lines' : 'not on the page for this checkout';
}

/**
 * The page asked to show every target (`?targets=all`): what each extension declares and
 * whether it is on the page. `shown` is the targets the page placed a slot at.
 */
function targetsPanel({ state, preview, refusals = [] }) {
  const refused = new Map(refusals.map((refusal) => [refusal.handle, refusal.reason]));
  const extensions = state.extensions ?? [];
  const rows = extensions.flatMap((ext) => (ext.targets ?? []).map((target) => {
    let status;
    if (refused.has(ext.handle)) status = h('span', { class: 'lsf-chip is-error' }, 'not loaded');
    else if (preview.shown.has(target)) status = h('span', { class: 'lsf-chip is-ok' }, 'shown');
    else if (!preview.rendered.includes(target)) status = h('span', { class: 'lsf-chip is-error' }, 'no place on the page');
    else status = h('span', { class: 'lsf-muted' }, whyNotShown(target, state));
    return h('li', { key: `${ext.handle}:${target}`, 'data-for': ext.handle, 'data-for-target': target }, h('strong', null, ext.handle), h('code', null, target), status);
  }));
  return h('section', { class: 'lsf-preview', 'data-section': 'targets' },
    h('h2', null, 'Every extension target'),
    h('p', { class: 'lsf-muted' },
      'Each extension renders at every target it declares. The steps of a cart that ships are shown whatever the cart, and the thank-you page is shown under the checkout with the same cart: no order is placed. ',
      state.home && h('a', { class: 'lsf-link', href: state.home }, 'All checkouts and extensions')),
    rows.length === 0 ? h('p', { class: 'lsf-muted' }, 'No checkout UI extension is loaded.') : h('ul', { class: 'lsf-preview-targets' }, rows));
}

/** The thank-you page of the checkout as it is, for the targets it has. */
function thankYouPreview(ctx) {
  return h('section', { class: 'lsf-preview-thanks', 'data-section': 'thank-you-preview' },
    h('h2', { class: 'lsf-preview-title' }, 'Thank-you page'),
    thankYouPage({ ...ctx, underCheckout: true }));
}

/** The whole page. */
export function renderPage(ctx) {
  const { state } = ctx;
  // With `preview`, every target is asked for: note the slots that exist, which the panel lists.
  const preview = ctx.preview?.all ? { shown: new Set(), rendered: ctx.preview.rendered ?? [] } : null;
  const page = preview
    ? {
      ...ctx,
      preview,
      slot: (target, instance) => {
        const node = ctx.slot(target, instance);
        if (node) preview.shown.add(target);
        return node;
      },
    }
    : { ...ctx, preview: null };
  const body = state.status === 'completed'
    ? thankYouPage(page)
    : [checkoutPage(page), preview && thankYouPreview(page)];
  return h('div', { class: `lsf-app lsf-${state.status}`, 'data-variant': state.variant, 'data-status': state.status },
    header(ctx),
    preview && state.status !== 'completed' && targetsPanel(page),
    body,
    functions(ctx));
}
