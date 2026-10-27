// The pages of the fake checkout as virtual DOM: the one-page checkout, the thank-you page and
// the panel that shows the Functions. Both skins (`standard`, `shop`) share the markup; the CSS
// tells them apart (`[data-variant]`), apart from a few words and the order of the summary.
//
// A view takes `{state, ui, slot, actions, refusals}`:
//   state    the checkout, amounts as `{amount, currencyCode}` (see `withCurrency`)
//   ui       what only the page knows: drafts of the fields, what is open, what failed
//   slot     `slot(target, instance)` → the node where extensions of that target render, or null
//   actions  what the buyer's gestures do
//   refusals the extensions the host does not load, `[{handle, reason}]` (see `api-versions.js`)

import { countriesOf, countryName, price, priceOrFree, storefrontLink } from './format.js';
import { missingFields, placeErrors } from './operations.js';
import { h } from './vdom.js';

const ICONS = {
  lock: '<svg viewBox="0 0 20 20" width="16" height="16" fill="currentColor"><path d="M6 8V6a4 4 0 118 0v2h1a1 1 0 011 1v8a1 1 0 01-1 1H5a1 1 0 01-1-1V9a1 1 0 011-1h1zm2 0h4V6a2 2 0 10-4 0v2z"/></svg>',
  chevron: '<svg viewBox="0 0 20 20" width="14" height="14" fill="currentColor"><path d="M5.3 7.3a1 1 0 011.4 0L10 10.6l3.3-3.3a1 1 0 111.4 1.4l-4 4a1 1 0 01-1.4 0l-4-4a1 1 0 010-1.4z"/></svg>',
  bag: '<svg viewBox="0 0 20 20" width="18" height="18" fill="currentColor"><path d="M6 6V5a4 4 0 118 0v1h2a1 1 0 011 1l1 10a1 1 0 01-1 1H3a1 1 0 01-1-1L3 7a1 1 0 011-1h2zm2 0h4V5a2 2 0 10-4 0v1z"/></svg>',
  check: '<svg viewBox="0 0 20 20" width="28" height="28" fill="none" stroke="currentColor" stroke-width="2"><circle cx="10" cy="10" r="8.5"/><path d="M6 10.5l3 3 5-6" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  warning: '<svg viewBox="0 0 20 20" width="16" height="16" fill="currentColor"><path d="M10 2l8.5 15h-17L10 2zm-1 6v4h2V8H9zm0 5v2h2v-2H9z"/></svg>',
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

function selectField({ id, label, value, options, error, actions }) {
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
        onChange: (event) => actions.commit(id.replace(/^lsf-/, ''), event.target.value),
      },
      h('option', { value: '' }, ''),
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

function contactSection(ctx) {
  const { state, ui, actions, slot } = ctx;
  const errors = fieldErrors(ctx);
  return h(
    'section',
    { class: 'lsf-section', 'data-section': 'contact' },
    h('div', { class: 'lsf-section-head' }, h('h2', null, 'Contact'),
      state.buyer?.customer ? h('span', { class: 'lsf-muted' }, state.buyer.customer.email) : h('a', { class: 'lsf-link', href: storefrontLink(state, '/account/login') }, 'Log in')),
    field({
      id: 'lsf-email',
      label: 'Email',
      type: 'email',
      autocomplete: 'email',
      value: ui.draft?.email ?? state.buyer?.email ?? '',
      error: firstError(errors, 'email'),
      actions,
    }),
    slot('purchase.checkout.contact.render-after', { key: 'contact' }),
  );
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
    h('p', { class: 'lsf-muted' }, 'All transactions are secure and encrypted.'),
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
            method.handle === 'card' && h('span', { class: 'lsf-card-brands', 'aria-hidden': 'true' }, 'VISA · MC · AMEX')),
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

function actionsSection(ctx) {
  const { state, ui, actions, slot } = ctx;
  const placed = placeErrors(state.errors);
  const blockErrors = placeErrors(ui.block?.errors).general;
  const total = price(state.cost.total, state.shop);
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
      h('a', { class: 'lsf-link lsf-return', href: storefrontLink(state, '/cart') }, '‹ Return to cart'),
      h('button', {
        class: 'lsf-pay', type: 'button', disabled: ui.busy ? true : null, onClick: actions.pay,
      }, ui.busy ? 'Processing…' : isShop(state) ? `${label} · ${total}` : label)),
  );
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
    h('div', { class: 'lsf-total' },
      h('dt', null, 'Total'),
      h('dd', null, h('span', { class: 'lsf-muted lsf-currency' }, cost.currencyCode), h('strong', null, price(cost.total, state.shop)))),
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
          onInput: (event) => actions.draft('discount', event.target.value),
        }),
        h('label', { for: 'lsf-discount' }, 'Discount code'),
        ui.discountError && h('p', { class: 'lsf-field-error' }, icon('warning'), ui.discountError)),
      h('button', { class: 'lsf-apply', type: 'submit', disabled: ui.busy ? true : null }, 'Apply')),
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
    h('span', { class: 'lsf-secure' }, icon('lock'), isShop(state) ? 'Shop Pay' : 'Secure checkout'));
}

/** What is wrong with the app: the server's diagnostics and the extensions the host does not load. */
function diagnostics({ state, refusals = [] }) {
  const messages = [
    ...(state.diagnostics ?? []),
    ...refusals.map((refusal) => `${refusal.handle}: not loaded. ${refusal.reason}`),
  ];
  return messages.length > 0
    ? h('div', { class: 'lsf-diagnostics' }, messages.map((message) => banner('warning', h('p', null, message))))
    : null;
}

function checkoutPage(ctx) {
  const { state, slot } = ctx;
  const needsShipping = state.lines.some((line) => line.requiresShipping);
  return h('div', { class: 'lsf-layout' },
    h('main', { class: 'lsf-main' },
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
  const detail = (title, lines) =>
    h('div', { class: 'lsf-detail' }, h('h3', null, title), lines.map((line) => h('p', null, line)));
  return h('div', { class: 'lsf-layout lsf-thanks' },
    h('main', { class: 'lsf-main' },
      slot('purchase.thank-you.announcement.render', { key: 'announcement' }),
      h('div', { class: 'lsf-confirmation' }, icon('check'),
        h('div', null,
          h('p', { class: 'lsf-muted', 'data-test': 'confirmation' }, `Confirmation ${order.number ?? '#1001'}`),
          h('h1', null, name ? `Thank you, ${name}!` : 'Thank you!'))),
      slot('purchase.thank-you.header.render-after', { key: 'header' }),
      h('section', { class: 'lsf-section lsf-confirmed' },
        h('h2', null, 'Your order is confirmed'),
        h('p', null, `You will receive a confirmation email at ${state.buyer?.email ?? 'your email address'}.`)),
      slot('purchase.thank-you.block.render', { key: 'block' }),
      h('section', { class: 'lsf-section' },
        h('h2', null, 'Order details'),
        h('div', { class: 'lsf-details' },
          detail('Contact information', [state.buyer?.email, state.buyer?.phone].filter(Boolean)),
          detail('Shipping address', addressLines(state.shippingAddress, countriesOf(state))),
          detail('Shipping method', selected.map((option) => `${option.title} · ${priceOrFree(option.cost, state.shop)}`)),
          detail('Payment method', method ? [`${method.name} · ${price(state.cost.total, state.shop)}`] : []),
          detail('Billing address', state.shippingAddress ? ['Same as shipping address'] : [])),
        slot('purchase.thank-you.customer-information.render-after', { key: 'customer' })),
      h('div', { class: 'lsf-actions-row' },
        h('a', { class: 'lsf-pay lsf-continue', href: storefrontLink(state), onClick: actions.continueShopping }, 'Continue shopping')),
      slot('purchase.thank-you.footer.render-after', { key: 'footer' })),
    summary(ctx, { thankYou: true }));
}

// --- The Functions panel -----------------------------------------------------------------

const json = (value) => JSON.stringify(value, null, 2);

function run(ctx, entry, index) {
  const { ui, actions } = ctx;
  const open = ui.openRuns?.has(index) ?? false;
  const failed = Boolean(entry.error);
  return h('details', {
      class: `lsf-run${failed ? ' is-failed' : ''}`, key: `${index}:${entry.handle}:${entry.target}`, open, 'data-run': entry.handle,
      onToggle: (event) => actions.toggleRun(index, event.target.open),
    },
    h('summary', null,
      h('strong', null, entry.handle), h('code', null, entry.target),
      h('span', { class: `lsf-chip ${failed ? 'is-error' : 'is-ok'}` }, failed ? 'error' : 'ok'),
      h('span', { class: 'lsf-muted' }, `${entry.instructions.toLocaleString('en-US')} instructions`)),
    // The inputs are large: build them only for the runs that are open.
    open && h('div', { class: 'lsf-run-body' },
      failed && h('div', null, h('h4', null, 'Error'), h('pre', { class: 'is-error' }, entry.error)),
      entry.logs && h('div', null, h('h4', null, 'Logs'), h('pre', null, entry.logs)),
      h('div', null, h('h4', null, 'Input'), h('pre', null, json(entry.input))),
      h('div', null, h('h4', null, 'Output'), h('pre', null, entry.output === null || entry.output === undefined ? '(none)' : json(entry.output)))));
}

function functionsPanel(ctx) {
  const { state, ui, actions } = ctx;
  const refused = new Set((ctx.refusals ?? []).map((refusal) => refusal.handle));
  const runs = state.runs ?? [];
  const failures = runs.filter((entry) => entry.error).length;
  const other = isShop(state) ? 'standard' : 'shop';
  const link = new URL(ctx.location ?? 'http://localhost/');
  link.searchParams.set('variant', other);
  return h('details', { class: 'lsf-functions', open: ui.functionsOpen, onToggle: (event) => actions.toggleFunctions(event.target.open) },
    h('summary', null, h('strong', null, 'Functions'),
      h('span', { class: 'lsf-muted' }, `${runs.length} run${runs.length === 1 ? '' : 's'}`),
      failures > 0 && h('span', { class: 'lsf-chip is-error' }, `${failures} failed`),
      (state.errors ?? []).length > 0 && h('span', { class: 'lsf-chip is-error' }, `${state.errors.length} checkout error${state.errors.length === 1 ? '' : 's'}`)),
    ui.functionsOpen && h('div', { class: 'lsf-functions-body' },
      h('p', { class: 'lsf-muted' }, h('a', { class: 'lsf-link', href: link.pathname + link.search }, `Switch to the ${other} skin`),
        ` · ${state.extensions?.length ?? 0} checkout UI extension${state.extensions?.length === 1 ? '' : 's'}`),
      (state.errors ?? []).length > 0 && h('div', null, h('h4', null, 'Errors that block the checkout'),
        h('ul', null, state.errors.map((error) => h('li', null, error.message, error.function && h('code', null, error.function), error.target && h('code', null, error.target))))),
      (state.extensions ?? []).length > 0 && h('div', null, h('h4', null, 'Extensions'),
        h('ul', null, state.extensions.map((ext) => h('li', { key: ext.handle }, h('strong', null, ext.handle), h('span', { class: 'lsf-muted' }, ` ${ext.apiVersion} · `), refused.has(ext.handle) && h('span', { class: 'lsf-chip is-error' }, 'not loaded'), (ext.targets ?? []).map((target) => h('code', null, target)))))),
      runs.length === 0 ? h('p', { class: 'lsf-muted' }, 'No Function ran.') : runs.map((entry, index) => run(ctx, entry, index))));
}

/** The whole page. */
export function renderPage(ctx) {
  const { state } = ctx;
  return h('div', { class: `lsf-app lsf-${state.status}`, 'data-variant': state.variant, 'data-status': state.status },
    header(ctx),
    diagnostics(ctx),
    state.status === 'completed' ? thankYouPage(ctx) : checkoutPage(ctx),
    functionsPanel(ctx));
}
