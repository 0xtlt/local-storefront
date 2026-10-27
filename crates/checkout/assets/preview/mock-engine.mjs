// A stand-in for the checkout server, enough for a browser to try the page without building
// the Rust crate: it keeps one checkout in memory, applies the operations the page sends and
// answers with a state shaped like `CheckoutResult` (see `crates/checkout/src/types.rs`).
// Two discount codes (`SAVE10`: 10 % off the products, `FREESHIP`: free shipping) and one
// "Function" that refuses a note with the word "forbidden" stand in for the real Functions.

const cents = (money) => Math.round(money.amount * 100);
const money = (value) => ({ amount: value / 100 });

export const RATES = [
  { handle: 'standard', title: 'Standard', description: '3 to 5 business days', price: 490 },
  { handle: 'express', title: 'Express', description: 'Next business day', price: 1200 },
];

const CODES = { SAVE10: { class: 'PRODUCT', title: 'SAVE10' }, FREESHIP: { class: 'SHIPPING', title: 'FREESHIP' } };
const VARIANTS = {
  'gid://shopify/ProductVariant/101': { variantId: 101, productId: 11, title: 'Linen shirt', variantTitle: 'M / Sand', price: 4900, sku: 'LIN-SH-M-SAND', vendor: 'Northline', productType: 'Shirts', selectedOptions: [{ name: 'Size', value: 'M' }, { name: 'Color', value: 'Sand' }] },
  'gid://shopify/ProductVariant/202': { variantId: 202, productId: 22, title: 'Canvas tote', variantTitle: null, price: 1200, sku: 'TOTE-01', vendor: 'Northline', productType: 'Bags', selectedOptions: [{ name: 'Title', value: 'Default Title' }] },
};

// The customers of the preview's store, the ones the page can log in as: a returning customer
// with two addresses, a new one, and a B2B buyer with two locations. As the server says them in
// `buyer` and `account`.
export const CUSTOMERS = [
  {
    id: 'gid://shopify/Customer/1', email: 'ava.chen@example.com', firstName: 'Ava', lastName: 'Chen',
    phone: '+33 6 12 34 56 78', acceptsMarketing: true, ordersCount: 3, tags: ['vip'],
    addresses: [
      { firstName: 'Ava', lastName: 'Chen', address1: '5 rue Oberkampf', city: 'Paris', zip: '75011', countryCode: 'FR' },
      { firstName: 'Ava', lastName: 'Chen', company: 'Studio Chen', address1: '20 quai Perrache', city: 'Lyon', zip: '69002', countryCode: 'FR' },
    ],
  },
  { id: 'gid://shopify/Customer/2', email: 'leo.martin@example.com', firstName: 'Léo', lastName: 'Martin', ordersCount: 0, tags: [], addresses: [] },
  {
    id: 'gid://shopify/Customer/3', email: 'sam.okafor@example.com', firstName: 'Sam', lastName: 'Okafor', ordersCount: 0, tags: ['wholesale'],
    addresses: [],
    company: { id: 'gid://shopify/Company/1', name: 'Harbor Cafés', externalId: 'HC-1' },
    locations: [
      { id: 'gid://shopify/CompanyLocation/11', name: 'Harbor Cafés Nantes', address: { company: 'Harbor Cafés', address1: '3 quai de la Fosse', city: 'Nantes', zip: '44000', countryCode: 'FR' } },
      { id: 'gid://shopify/CompanyLocation/12', name: 'Harbor Cafés Bordeaux', address: { company: 'Harbor Cafés', address1: '9 cours du Médoc', city: 'Bordeaux', zip: '33300', countryCode: 'FR' } },
    ],
  },
];

const customerNamed = (email) => CUSTOMERS.find((customer) => customer.email === email) ?? null;

/** The addresses a customer ships to without typing them, the default one first: the location
 * a B2B customer buys for, otherwise the ones they saved. */
function savedOf(customer, locationId) {
  if (!customer) return [];
  if (customer.company) {
    const location = customer.locations.find((entry) => entry.id === locationId) ?? customer.locations[0];
    return location ? [{ id: `location-${location.id.split('/').pop()}`, location: location.name, address: location.address }] : [];
  }
  return customer.addresses.map((address, index) => ({ id: `address-${index + 1}`, location: null, address }));
}

const same = (a, b) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

/** What logging in as `email` (or out, `null`) does to the checkout: the contact is theirs, the
 * address their default one (a typed one stays for a customer who has none). */
function logIn(state, email, locationId) {
  const before = customerNamed(state.selection.customer);
  const after = email === null ? null : customerNamed(email);
  if (email !== null && !after) throw new Error(`there is no customer with the email "${email}"`);
  const previous = savedOf(before, state.selection.location).map((entry) => entry.address);
  if (before?.email !== after?.email) state.selection.location = null;
  if (locationId !== undefined) {
    if (!after?.locations?.some((location) => location.id === locationId)) throw new Error(`cannot buy for the location ${locationId}`);
    state.selection.location = locationId;
  }
  state.selection.customer = after?.email ?? null;
  const theirs = previous.some((address) => same(address, state.shippingAddress));
  const fallback = savedOf(after, state.selection.location)[0]?.address ?? null;
  if (before?.email !== after?.email) {
    state.buyer.email = after?.email ?? null;
    state.buyer.phone = after?.phone ?? null;
    if (fallback || theirs) state.shippingAddress = fallback ? structuredClone(fallback) : null;
  } else if (fallback && (theirs || !state.shippingAddress)) {
    state.shippingAddress = structuredClone(fallback);
  }
}

/** `buyer.customer`, `buyer.purchasingCompany` and `account`, as the server writes them. */
function identify(state) {
  const customer = customerNamed(state.selection.customer);
  const location = customer?.locations?.find((entry) => entry.id === state.selection.location) ?? customer?.locations?.[0] ?? null;
  state.buyer.customer = customer && {
    id: customer.id, email: customer.email, phone: customer.phone ?? null, firstName: customer.firstName,
    lastName: customer.lastName, acceptsMarketing: Boolean(customer.acceptsMarketing), ordersCount: customer.ordersCount,
  };
  state.buyer.purchasingCompany = customer?.company
    ? { company: customer.company, location: location && { id: location.id, name: location.name, externalId: null } }
    : null;
  state.account = {
    profiles: CUSTOMERS.map((entry) => ({
      email: entry.email,
      name: `${entry.firstName} ${entry.lastName}`,
      tags: entry.tags,
      ordersCount: entry.ordersCount,
      addressesCount: entry.addresses.length,
      company: entry.company?.name ?? null,
      locationsCount: entry.locations?.length ?? 0,
      countryCode: savedOf(entry, null)[0]?.address.countryCode ?? null,
      current: entry.email === customer?.email,
    })),
    addresses: savedOf(customer, state.selection.location),
    locations: (customer?.locations ?? []).map((entry) => ({ id: entry.id, name: entry.name, current: entry === location })),
  };
}

/** The checkout the page starts with: `sample` (sample-state.json) plus what the server adds. */
export function createCheckout(sample, { origin, variant = 'standard' } = {}) {
  const state = structuredClone(sample);
  state.variant = variant;
  if (origin) state.shop.storefrontUrl = origin;
  state.selection = { delivery: {}, codes: [], customer: null, location: null };
  return recompute(state);
}

function recompute(state) {
  identify(state);
  const lines = state.lines;
  for (const line of lines) {
    line.subtotal = money(cents(line.unitPrice) * line.quantity);
    line.discountAllocations = [];
    line.total = line.subtotal;
  }
  const runs = [];
  const discounts = [];
  const shippingFree = state.selection.codes.some((code) => CODES[code]?.class === 'SHIPPING');

  const productCodes = state.selection.codes.filter((code) => CODES[code]?.class === 'PRODUCT');
  for (const code of productCodes) {
    let total = 0;
    for (const line of lines) {
      const off = Math.round(cents(line.subtotal) * 0.1);
      line.discountAllocations.push({ title: code, code, class: 'PRODUCT', amount: money(off) });
      line.total = money(cents(line.total) - off);
      total += off;
    }
    discounts.push({ title: code, code, class: 'PRODUCT', amount: money(total), function: 'volume-discount' });
  }

  const hasAddress = Boolean(state.shippingAddress?.countryCode);
  state.deliveryGroups = hasAddress
    ? [
        {
          handle: 'shipping',
          lineIds: lines.filter((line) => line.requiresShipping).map((line) => line.id),
          options: RATES.map((rate) => ({
            handle: rate.handle,
            title: rate.title,
            description: rate.description,
            kind: 'SHIPPING',
            originalCost: money(rate.price),
            cost: money(shippingFree ? 0 : rate.price),
          })),
          selected: state.selection.delivery.shipping ?? null,
        },
      ]
    : [];
  const chosen = state.deliveryGroups[0]?.options.find((option) => option.handle === state.deliveryGroups[0].selected);
  if (shippingFree && chosen) {
    discounts.push({
      title: 'FREESHIP', code: 'FREESHIP', class: 'SHIPPING',
      amount: money(cents(chosen.originalCost) - cents(chosen.cost)), function: 'free-shipping',
    });
  }
  state.discountCodes = state.selection.codes.map((code) => ({
    code,
    applicable: code in CODES,
  }));
  state.discounts = discounts;

  const subtotal = lines.reduce((sum, line) => sum + cents(line.subtotal), 0);
  const off = discounts.filter((discount) => discount.class !== 'SHIPPING').reduce((sum, discount) => sum + cents(discount.amount), 0);
  const shipping = chosen ? cents(chosen.cost) : 0;
  state.cost = {
    subtotal: money(subtotal),
    discounts: money(off),
    shipping: money(shipping),
    tax: money(0),
    total: money(subtotal - off + shipping),
    // The preview has no selling plans: everything is due now.
    dueNow: money(subtotal - off + shipping),
    dueLater: money(0),
    checkoutChargeAmount: money(subtotal),
    currencyCode: state.shop.currencyCode,
  };

  state.errors = [];
  if (/forbidden/i.test(state.note ?? '')) {
    state.errors.push({ message: 'Delivery instructions cannot contain the word “forbidden”.', target: '$.cart', function: 'order-rules' });
  }
  runs.push({
    handle: 'volume-discount',
    target: 'cart.lines.discounts.generate.run',
    export: 'cart_lines_discounts_generate_run',
    input: { cart: { lines: lines.map((line) => ({ id: line.id, quantity: line.quantity })) }, discount: { discountClasses: ['PRODUCT'] }, enteredDiscountCodes: state.selection.codes.map((code) => ({ code, rejectable: true })) },
    output: { operations: productCodes.map((code) => ({ productDiscountsAdd: { candidates: [{ message: code, value: { percentage: { value: 10 } } }] } })) },
    logs: productCodes.length ? `applied ${productCodes.join(', ')}\n` : '',
    error: null,
    instructions: 41876,
  });
  runs.push({
    handle: 'order-rules',
    target: 'cart.validations.generate.run',
    export: 'cart_validations_generate_run',
    input: { cart: { attribute: state.note ? { value: state.note } : null } },
    output: { operations: [{ validationAdd: { errors: state.errors.map((error) => ({ message: error.message, target: error.target })) } }] },
    logs: '',
    error: null,
    instructions: 12033,
  });
  state.runs = runs;
  return state;
}

/** Applies one operation; throws an `Error` for one the page should not have sent. */
function apply(state, operation) {
  const need = (condition, message) => {
    if (!condition) throw new Error(message);
  };
  switch (operation.type) {
    case 'email':
      state.buyer.email = operation.value || null;
      break;
    case 'phone':
      state.buyer.phone = operation.value || null;
      break;
    case 'shippingAddress':
      state.shippingAddress = operation.address;
      break;
    case 'deliveryOption':
      need(RATES.some((rate) => rate.handle === operation.handle), `No delivery option ${operation.handle}`);
      state.selection.delivery[operation.group] = operation.handle;
      break;
    case 'paymentMethod':
      need(state.paymentMethods.some((method) => method.handle === operation.handle), `No payment method ${operation.handle}`);
      state.selectedPayment = operation.handle;
      break;
    case 'discountCode': {
      const code = String(operation.code ?? '').toUpperCase();
      need(code, 'A discount code is needed');
      const codes = state.selection.codes;
      if (operation.action === 'add') {
        if (!codes.includes(code)) codes.push(code);
      } else {
        state.selection.codes = codes.filter((entry) => entry !== code);
      }
      break;
    }
    case 'cartLines': {
      const change = operation.change;
      if (change.type === 'addCartLine') {
        const variant = VARIANTS[change.merchandiseId];
        need(variant, `No variant ${change.merchandiseId}`);
        const existing = state.lines.find((line) => line.variantId === variant.variantId);
        if (existing) existing.quantity += change.quantity;
        else {
          state.lines.push({
            id: `gid://shopify/CartLine/${state.lines.length + 1}`, variantId: variant.variantId, productId: variant.productId,
            title: variant.title, variantTitle: variant.variantTitle, sku: variant.sku, image: null,
            vendor: variant.vendor, productType: variant.productType, selectedOptions: variant.selectedOptions, sellingPlan: null, quantity: change.quantity,
            unitPrice: money(variant.price), subtotal: money(0), total: money(0), discountAllocations: [], attributes: [],
            components: [], transformedBy: null, requiresShipping: true,
          });
        }
      } else {
        const line = state.lines.find((entry) => entry.id === change.id);
        need(line, `No cart line ${change.id}`);
        if (change.type === 'removeCartLine') line.quantity -= change.quantity;
        else if (change.type === 'updateCartLine' && change.quantity !== undefined) line.quantity = change.quantity;
        else need(change.type === 'updateCartLine', `Unknown cart line change ${change.type}`);
        if (line.quantity <= 0) state.lines.splice(state.lines.indexOf(line), 1);
      }
      break;
    }
    case 'attribute': {
      const change = operation.change;
      state.attributes = state.attributes.filter((entry) => entry.key !== change.key);
      if (change.type === 'updateAttribute') state.attributes.push({ key: change.key, value: change.value });
      break;
    }
    case 'note':
      state.note = operation.change.type === 'removeNote' ? null : operation.change.note;
      break;
    case 'customer':
      logIn(state, operation.customer ?? null);
      break;
    case 'companyLocation':
      need(customerNamed(state.selection.customer)?.company, 'a company location needs a logged-in B2B customer');
      logIn(state, state.selection.customer, operation.location);
      break;
    case 'metafield': {
      const change = operation.change;
      state.metafields = state.metafields.filter((entry) => !(entry.namespace === change.namespace && entry.key === change.key));
      if (/^update/.test(change.type)) {
        state.metafields.push({ namespace: change.namespace, key: change.key, type: change.valueType, value: change.value });
      }
      break;
    }
    default:
      throw new Error(`Unknown operation type ${operation.type}`);
  }
}

/** `{state}` once every operation applied, `{error}` for the first one that did not (nothing
 * is changed then). */
export function update(state, operations) {
  const draft = structuredClone(state);
  try {
    for (const operation of operations) apply(draft, operation);
  } catch (error) {
    return { error: error.message };
  }
  return { state: recompute(draft) };
}

/** `{state}` of the finished checkout, or `{errors}`: the state with what blocks it. */
export function complete(state) {
  const draft = structuredClone(state);
  if (!draft.buyer.email) draft.errors.push({ message: 'Enter an email', target: '$.cart.buyerIdentity.email', function: null });
  if (!draft.selectedPayment) draft.errors.push({ message: 'Select a payment method', target: '$.cart.paymentMethods', function: null });
  if (draft.errors.length > 0) return { errors: draft };
  draft.status = 'completed';
  draft.order = { id: 'gid://shopify/Order/1001', number: '#1001' };
  return { state: draft };
}
