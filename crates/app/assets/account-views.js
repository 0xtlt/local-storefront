// The pages of the local customer account as virtual DOM: the orders, the status of an order,
// the profile, and the full pages of extensions. They draw what Shopify's new customer accounts
// show, with a slot at each target of customer account UI extensions.
//
// A view takes `{state, ui, slot, has, actions, refusals, preview}` (see `views.js`):
//   state    the account (`customer-account.rs`): `customer`, `orders`, `order`, `company`, …
//   ui       what only the page knows: the dialog of an order action, the toasts
//   slot     `slot(target, instance)` → the node where extensions of that target render, or null

import { initials } from './account.js';
import { countriesOf, countryName, price } from './format.js';
import { h } from './vdom.js';

const FULL_PAGE = 'customer-account.page.render';
const ORDER_PAGE = 'customer-account.order.page.render';
const MENU_ITEM = 'customer-account.order.action.menu-item.render';
const ACTION = 'customer-account.order.action.render';

const fullName = (customer) =>
  [customer?.firstName, customer?.lastName].filter(Boolean).join(' ') || customer?.email || '';

/** A date in the words of the page, which are English. */
const date = (iso) => {
  if (!iso) return '';
  try {
    return new Intl.DateTimeFormat('en', { dateStyle: 'medium' }).format(new Date(iso));
  } catch {
    return iso.slice(0, 10);
  }
};

const plural = (count, word) => `${count} ${word}${count === 1 ? '' : 's'}`;

const loginLink = (email, back) =>
  `/__lsf/login?customer=${encodeURIComponent(email)}&return_to=${encodeURIComponent(back)}`;

/** Where the order's page is. */
const orderPath = (state, order) => `${state.root}/orders/${order.legacyId}`;

/** What the customer sees of where an order is. */
export function orderStatus(order) {
  if (order.cancelledAt) return { tone: 'critical', label: 'Cancelled' };
  if ((order.returns ?? []).length > 0) return { tone: 'info', label: 'Return in progress' };
  if (order.fulfillmentStatus === 'fulfilled') return { tone: 'success', label: 'Delivered' };
  if (order.fulfillmentStatus === 'partial') return { tone: 'info', label: 'Partially shipped' };
  return { tone: 'neutral', label: 'Confirmed' };
}

const badge = ({ tone, label }) => h('span', { class: `lsf-ca-badge is-${tone}` }, label);

const card = (props, ...children) => h('section', { class: 'lsf-ca-card', ...props }, children);

function addressLines(address, countries) {
  if (!address) return [];
  return [
    [address.firstName, address.lastName].filter(Boolean).join(' '),
    address.company,
    address.address1,
    address.address2,
    [address.zip, address.city, address.provinceCode].filter(Boolean).join(' '),
    countryName(address.countryCode, countries),
    address.phone,
  ].filter(Boolean);
}

const lines = (texts, none = null) =>
  texts.length > 0 ? texts.map((text) => h('p', null, text)) : none && h('p', { class: 'lsf-muted' }, none);

// --- The frame of every page ---------------------------------------------------------------

function header(ctx) {
  const { state, ui, actions } = ctx;
  const here = state.page;
  const link = (href, label, current) =>
    h('a', { class: `lsf-ca-nav-link${current ? ' is-current' : ''}`, href, 'aria-current': current ? 'page' : null }, label);
  const customer = state.customer;
  return h('header', { class: 'lsf-ca-header' },
    h('a', { class: 'lsf-brand', href: state.shop?.storefrontUrl ?? '/' }, state.shop?.name ?? 'Shop'),
    customer && h('nav', { class: 'lsf-ca-nav', 'aria-label': 'Account' },
      link(state.root, 'Orders', here === 'orders' || here === 'order' || here === 'order-page'),
      link(`${state.root}/profile`, 'Profile', here === 'profile')),
    h('div', { class: 'lsf-ca-account' },
      customer
        ? h('button', {
          type: 'button', class: 'lsf-ca-avatar-button', 'aria-haspopup': 'menu', 'aria-expanded': ui.menuOpen ? 'true' : 'false',
          'aria-label': `${fullName(customer)}: switch customer`, onClick: () => actions.toggleMenu(),
        }, h('span', { class: 'lsf-avatar', 'aria-hidden': 'true' }, initials(fullName(customer))))
        : null,
      customer && ui.menuOpen && h('div', { class: 'lsf-ca-menu', role: 'menu' },
        h('p', { class: 'lsf-muted' }, customer.email),
        (state.profiles ?? []).filter((profile) => profile.email !== customer.email).map((profile) =>
          h('a', { role: 'menuitem', class: 'lsf-ca-menu-item', href: loginLink(profile.email, ctx.here), 'data-customer': profile.email }, `Switch to ${profile.name}`)),
        h('a', { role: 'menuitem', class: 'lsf-ca-menu-item', href: loginLink('none', state.root), 'data-customer': '' }, 'Log out'))));
}

/** What is wrong with the app, as on the checkout page. */
function diagnostics({ state, refusals = [] }) {
  const messages = [
    ...(state.diagnostics ?? []),
    ...refusals.map((refusal) => `${refusal.handle}: not loaded. ${refusal.reason}`),
  ];
  return messages.length > 0
    ? h('div', { class: 'lsf-diagnostics' }, messages.map((message) => h('div', { class: 'lsf-banner is-warning', role: 'status' }, h('p', null, message))))
    : null;
}

/** Logged out: the customers of the store data to log in as. */
function logIn(ctx) {
  const { state } = ctx;
  const profiles = state.profiles ?? [];
  return card({ 'data-section': 'log-in' },
    h('h1', null, 'Log in'),
    h('p', { class: 'lsf-muted' }, 'The customer account of the test store: log in as one of the customers of the store data.'),
    profiles.length === 0
      ? h('p', { class: 'lsf-placeholder' }, 'The store data has no customers: add some to customers.json.')
      : h('ul', { class: 'lsf-ca-profiles' }, profiles.map((profile) =>
        h('li', { key: profile.email },
          h('a', { class: 'lsf-ca-profile', href: loginLink(profile.email, ctx.here), 'data-customer': profile.email },
            h('span', { class: 'lsf-avatar', 'aria-hidden': 'true' }, initials(profile.name)),
            h('span', { class: 'lsf-account-text' },
              h('span', { class: 'lsf-account-name' }, profile.name),
              h('span', { class: 'lsf-muted' }, profile.email)))))));
}

// --- Orders ----------------------------------------------------------------------------------

/** The actions of an order: the menu items of the extensions, and the dialog one opened. */
function orderActions(ctx, order) {
  const { slot } = ctx;
  return h('div', { class: 'lsf-ca-actions', key: `actions:${order.legacyId}` },
    slot(MENU_ITEM, { key: `menu:${order.legacyId}`, orderId: order.id }));
}

function thumbnails(order) {
  return h('div', { class: 'lsf-ca-thumbs' }, (order.lines ?? []).slice(0, 4).map((line) =>
    h('span', { class: 'lsf-thumb', key: line.id }, line.image ? h('img', { src: line.image, alt: '' }) : line.title.slice(0, 1))));
}

function orderCard(ctx, order) {
  const { state } = ctx;
  const count = (order.lines ?? []).reduce((sum, line) => sum + line.quantity, 0);
  return h('li', { class: 'lsf-ca-order', key: order.legacyId, 'data-order': order.legacyId },
    h('div', { class: 'lsf-ca-order-head' },
      badge(orderStatus(order)),
      h('span', { class: 'lsf-muted' }, date(order.processedAt))),
    thumbnails(order),
    h('div', { class: 'lsf-ca-order-facts' },
      h('a', { class: 'lsf-ca-order-name', href: orderPath(state, order) }, `Order ${order.name}`),
      h('span', { class: 'lsf-muted' }, `${plural(count, 'item')} · ${price(order.cost?.total, state.shop)}`)),
    orderActions(ctx, order));
}

function ordersPage(ctx) {
  const { state, slot } = ctx;
  const orders = state.orders ?? [];
  return h('main', { class: 'lsf-ca-main', 'data-page': 'orders' },
    slot('customer-account.order-index.announcement.render', { key: 'index-announcement' }),
    h('h1', null, 'Orders'),
    slot('customer-account.order-index.block.render', { key: 'index-block' }),
    orders.length === 0
      ? h('p', { class: 'lsf-placeholder' }, 'No orders yet. Orders in the store data and the one placed in this checkout show here.')
      : h('ul', { class: 'lsf-ca-orders' }, orders.map((order) => orderCard(ctx, order))));
}

// --- The status of an order -----------------------------------------------------------------

function lineTitle(line) {
  return line.variantTitle ? `${line.title} · ${line.variantTitle}` : line.title;
}

function lineList(order, ids) {
  const byId = new Map((order.lines ?? []).map((line) => [line.id, line]));
  return h('ul', { class: 'lsf-ca-items' }, ids.map((entry) => {
    const id = typeof entry === 'string' ? entry : entry.lineId;
    const line = byId.get(id);
    const quantity = typeof entry === 'string' ? line?.quantity : entry.quantity;
    return line && h('li', { key: id }, `${quantity} × ${lineTitle(line)}`);
  }));
}

function summary(ctx, order) {
  const { state, slot } = ctx;
  const cost = order.cost ?? {};
  return h('aside', { class: 'lsf-ca-summary', 'aria-label': 'Order summary' },
    h('ul', { class: 'lsf-lines' }, (order.lines ?? []).map((line) =>
      h('li', { class: 'lsf-line', key: line.id, 'data-line': line.id },
        h('div', { class: 'lsf-line-main' },
          h('div', { class: 'lsf-thumb' },
            line.image ? h('img', { src: line.image, alt: '' }) : null,
            h('span', { class: 'lsf-qty', 'aria-label': `Quantity ${line.quantity}` }, line.quantity)),
          h('div', { class: 'lsf-line-info' },
            h('span', { class: 'lsf-line-title' }, line.title),
            line.variantTitle && h('span', { class: 'lsf-muted' }, line.variantTitle)),
          h('div', { class: 'lsf-line-price' }, price(line.total, state.shop))),
        slot('customer-account.order-status.cart-line-item.render-after', { key: line.id, lineId: line.id, orderId: order.id })))),
    slot('customer-account.order-status.cart-line-list.render-after', { key: 'lines-after', orderId: order.id }),
    h('dl', { class: 'lsf-costs' },
      h('div', null, h('dt', null, 'Subtotal'), h('dd', null, price(cost.subtotal, state.shop))),
      h('div', null, h('dt', null, 'Shipping'), h('dd', null, price(cost.shipping, state.shop))),
      cost.tax?.amount > 0 && h('div', null, h('dt', null, 'Taxes'), h('dd', null, price(cost.tax, state.shop))),
      h('div', { class: 'lsf-total' }, h('dt', null, 'Total'),
        h('dd', null, h('span', { class: 'lsf-muted lsf-currency' }, cost.currencyCode), h('strong', null, price(cost.total, state.shop))))));
}

function orderPage(ctx) {
  const { state, slot } = ctx;
  const order = state.order;
  const countries = countriesOf(state);
  const where = { orderId: order.id };
  return h('div', { class: 'lsf-ca-layout' },
    h('main', { class: 'lsf-ca-main', 'data-page': 'order' },
      h('a', { class: 'lsf-link lsf-ca-back', href: state.root }, '‹ Orders'),
      slot('customer-account.order-status.announcement.render', { key: 'announcement', ...where }),
      h('div', { class: 'lsf-ca-title' },
        h('h1', null, `Order ${order.name}`),
        h('p', { class: 'lsf-muted' }, `Confirmation ${order.confirmationNumber} · ${date(order.processedAt)}`)),
      orderActions(ctx, order),
      card({ 'data-section': 'status' },
        badge(orderStatus(order)),
        h('p', null, order.cancelledAt
          ? 'This order was cancelled.'
          : order.fulfillmentStatus === 'fulfilled'
            ? 'Every item of the order was delivered.'
            : 'We received your order. You will get an email when it ships.')),
      slot('customer-account.order-status.block.render', { key: 'block', ...where }),
      (order.fulfillments ?? []).map((fulfillment, index) =>
        card({ key: fulfillment.id, 'data-section': 'fulfillment' },
          h('h2', null, `Shipment ${index + 1}`),
          lineList(order, fulfillment.lineIds ?? []),
          slot('customer-account.order-status.fulfillment-details.render-after', { key: `fulfillment:${fulfillment.id}`, fulfillmentId: fulfillment.id, ...where }))),
      (order.unfulfilledLineIds ?? []).length > 0 && card({ 'data-section': 'unfulfilled' },
        h('h2', null, 'Not shipped yet'),
        lineList(order, order.unfulfilledLineIds),
        slot('customer-account.order-status.unfulfilled-items.render-after', { key: 'unfulfilled', ...where })),
      (order.returns ?? []).map((sent, index) =>
        card({ key: sent.id, 'data-section': 'return' },
          h('h2', null, `Return ${index + 1}`, ' ', badge({ tone: 'info', label: sent.status.toLowerCase() })),
          lineList(order, sent.lines ?? []),
          slot('customer-account.order-status.return-details.render-after', { key: `return:${sent.id}`, returnId: sent.id, ...where }))),
      card({ 'data-section': 'details' },
        h('h2', null, 'Order details'),
        h('div', { class: 'lsf-details' },
          h('div', { class: 'lsf-detail' }, h('h3', null, 'Contact information'), lines([state.customer?.email, state.customer?.phone].filter(Boolean))),
          h('div', { class: 'lsf-detail' }, h('h3', null, 'Shipping address'), lines(addressLines(order.shippingAddress, countries), 'None')),
          h('div', { class: 'lsf-detail' }, h('h3', null, 'Billing address'), lines(addressLines(order.billingAddress ?? order.shippingAddress, countries), 'None'))),
        slot('customer-account.order-status.customer-information.render-after', { key: 'customer', ...where })),
      card({ 'data-section': 'payment' },
        h('h2', null, 'Payment'),
        h('p', null, `${order.financialStatus === 'paid' ? 'Paid' : order.financialStatus} · ${price(order.cost?.total, state.shop)}`),
        slot('customer-account.order-status.payment-details.render-after', { key: 'payment', ...where }))),
    summary(ctx, order));
}

// --- Profile ---------------------------------------------------------------------------------

function profilePage(ctx) {
  const { state, slot } = ctx;
  const customer = state.customer;
  const countries = countriesOf(state);
  const company = state.company;
  return h('main', { class: 'lsf-ca-main', 'data-page': 'profile' },
    slot('customer-account.profile.announcement.render', { key: 'announcement' }),
    h('h1', null, 'Profile'),
    card({ 'data-section': 'name' },
      h('h2', null, fullName(customer)),
      lines([customer.email, customer.phone].filter(Boolean))),
    slot('customer-account.profile.block.render', { key: 'block' }),
    card({ 'data-section': 'addresses' },
      h('h2', null, 'Addresses'),
      (customer.addresses ?? []).length === 0
        ? h('p', { class: 'lsf-muted' }, 'No addresses saved.')
        : h('ul', { class: 'lsf-ca-addresses' }, customer.addresses.map((entry) =>
          h('li', { key: entry.id }, entry.default && badge({ tone: 'neutral', label: 'Default' }), lines(addressLines(entry.address, countries))))),
      slot('customer-account.profile.addresses.render-after', { key: 'addresses' })),
    card({ 'data-section': 'payment' },
      h('h2', null, 'Payment methods'),
      h('p', { class: 'lsf-muted' }, 'No saved payment methods.'),
      slot('customer-account.profile.payment.render-after', { key: 'payment' })),
    company && card({ 'data-section': 'company' },
      h('h2', null, company.name),
      company.externalId && h('p', { class: 'lsf-muted' }, `External ID ${company.externalId}`),
      slot('customer-account.profile.company-details.render-after', { key: 'company' })),
    company && (company.locations ?? []).map((location) => {
      const where = { locationId: location.id };
      return card({ key: location.id, 'data-section': 'location', 'data-location': location.name },
        h('h2', null, location.name, location.current && [' ', badge({ tone: 'success', label: 'Buying for' })]),
        h('h3', null, 'Addresses'),
        location.address ? lines(addressLines(location.address, countries)) : h('p', { class: 'lsf-muted' }, 'No address.'),
        slot('customer-account.profile.company-location-addresses.render-after', { key: `addresses:${location.id}`, ...where }),
        h('h3', null, 'Payment'),
        h('p', { class: 'lsf-muted' }, location.taxRegistrationId ? `Tax ID ${location.taxRegistrationId}` : 'No payment terms.'),
        slot('customer-account.profile.company-location-payment.render-after', { key: `payment:${location.id}`, ...where }),
        h('h3', null, 'Staff'),
        h('ul', { class: 'lsf-ca-items' }, (location.staff ?? []).map((member) =>
          h('li', { key: member.email }, member.name, member.title && h('span', { class: 'lsf-muted' }, ` · ${member.title}`)))),
        slot('customer-account.profile.company-location-staff.render-after', { key: `staff:${location.id}`, ...where }));
    }));
}

// --- Full pages ------------------------------------------------------------------------------

function fullPage(ctx) {
  const { state, slot } = ctx;
  const page = state.extensionPage;
  const forOrder = state.page === 'order-page';
  const target = forOrder ? ORDER_PAGE : FULL_PAGE;
  return h('main', { class: 'lsf-ca-main lsf-ca-full', 'data-page': state.page, 'data-extension': page.handle },
    forOrder && h('a', { class: 'lsf-link lsf-ca-back', href: orderPath(state, state.order) }, `‹ Order ${state.order.name}`),
    slot(target, { key: `page:${page.handle}:${page.path}`, handle: page.handle, path: page.path, orderId: state.order?.id, order: forOrder ? state.order.legacyId : null })
      ?? h('p', { class: 'lsf-placeholder' }, `${page.handle} is not loaded: the page of the Functions says why.`));
}

// --- Every page ------------------------------------------------------------------------------

/** The dialog an order action opened: the `customer-account.order.action.render` of the
 * extension whose menu item was pressed. */
function actionDialog(ctx) {
  const { ui, slot, actions } = ctx;
  if (!ui.action) return null;
  return h('div', { class: 'lsf-modal', key: 'order-action', role: 'dialog', 'aria-modal': 'true', 'aria-label': 'Order action' },
    h('div', { class: 'lsf-modal-card' },
      h('button', { type: 'button', class: 'lsf-fn-close lsf-modal-close', 'aria-label': 'Close', onClick: () => actions.closeAction() }, '×'),
      slot(ACTION, { key: `action:${ui.action.handle}:${ui.action.orderId}`, handle: ui.action.handle, orderId: ui.action.orderId })
        ?? h('p', { class: 'lsf-muted' }, `${ui.action.handle} has no ${ACTION}.`)));
}

function toasts(ctx) {
  const list = ctx.ui.toasts ?? [];
  return list.length > 0
    ? h('div', { class: 'lsf-ca-toasts', role: 'status', 'aria-live': 'polite' }, list.map((toast) => h('p', { class: 'lsf-ca-toast', key: toast.id }, toast.content)))
    : null;
}

/** What each extension declares, and where its customer account targets render. */
function targetsPanel(ctx) {
  const { state, preview } = ctx;
  const pages = {
    'order-index': state.root,
    'order-status': state.orders?.[0] ? orderPath(state, state.orders[0]) : null,
    profile: `${state.root}/profile`,
  };
  const rows = (state.extensions ?? []).flatMap((ext) => (ext.targets ?? []).filter((target) => target.startsWith('customer-account.')).map((target) => {
    let where;
    if (preview.shown.has(target)) where = h('span', { class: 'lsf-chip is-ok' }, 'shown');
    else if (target === FULL_PAGE) where = h('a', { class: 'lsf-link', href: `${state.root}/pages/${ext.handle}` }, 'its full page');
    else if (target === ORDER_PAGE) {
      where = pages['order-status']
        ? h('a', { class: 'lsf-link', href: `${pages['order-status']}/pages/${ext.handle}` }, 'its full page for an order')
        : h('span', { class: 'lsf-muted' }, 'needs an order');
    } else if (target === ACTION) where = h('span', { class: 'lsf-muted' }, 'opens from its menu item');
    else if (target === MENU_ITEM || target.includes('footer')) where = h('span', { class: 'lsf-muted' }, 'on every page with an order');
    else {
      const page = Object.keys(pages).find((name) => target.includes(`.${name}.`)) ?? (target.includes('.order-status.') ? 'order-status' : null);
      const href = page && pages[page];
      where = href ? h('a', { class: 'lsf-link', href: `${href}?targets=all` }, `on the ${page.replace('-', ' ')} page`) : h('span', { class: 'lsf-muted' }, 'needs an order');
    }
    return h('li', { key: `${ext.handle}:${target}`, 'data-for': ext.handle, 'data-for-target': target }, h('strong', null, ext.handle), h('code', null, target), where);
  }));
  return h('section', { class: 'lsf-preview', 'data-section': 'targets' },
    h('h2', null, 'Every customer account target'),
    rows.length === 0 ? h('p', { class: 'lsf-muted' }, 'No customer account UI extension is loaded.') : h('ul', { class: 'lsf-preview-targets' }, rows));
}

/** The whole page. */
export function renderAccount(ctx) {
  const { state } = ctx;
  const preview = ctx.preview?.all ? { shown: new Set() } : null;
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
    : ctx;
  let body;
  if (!state.customer) body = h('main', { class: 'lsf-ca-main' }, logIn(page));
  else if (state.page === 'order') body = orderPage(page);
  else if (state.page === 'profile') body = profilePage(page);
  else if (state.page === 'page' || state.page === 'order-page') body = fullPage(page);
  else body = ordersPage(page);
  const footer = state.customer ? page.slot('customer-account.footer.render-after', { key: 'footer' }) : null;
  return h('div', { class: 'lsf-app lsf-ca', 'data-page': state.page },
    header(page),
    h('div', { class: 'lsf-ca-body' },
      diagnostics(page),
      body,
      footer && h('footer', { class: 'lsf-ca-footer' }, footer),
      preview && targetsPanel(page)),
    actionDialog(page),
    toasts(page));
}
