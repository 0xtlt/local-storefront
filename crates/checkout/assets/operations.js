// The operations the page sends to `POST …/update`, and how the calls of checkout UI
// extensions (`applyCartLinesChange`, …) become operations.

export const op = {
  email: (value) => ({ type: 'email', value }),
  phone: (value) => ({ type: 'phone', value }),
  shippingAddress: (address) => ({ type: 'shippingAddress', address }),
  deliveryOption: (group, handle) => ({ type: 'deliveryOption', group, handle }),
  paymentMethod: (handle) => ({ type: 'paymentMethod', handle }),
  discountCode: (action, code) => ({ type: 'discountCode', action, code }),
};

const fail = (message) => ({ error: message });
const present = (value) => typeof value === 'string' && value.trim() !== '';

/**
 * What a call of the UI extension API means for the checkout: `{operations}` to send, or
 * `{error}` for a call the API refuses or this checkout cannot do. `state` is the checkout as
 * the extension last saw it.
 */
export function operationsForChange(method, change, state) {
  if (change === null || typeof change !== 'object') return fail(`${method}: no change given`);
  switch (method) {
    case 'applyCartLinesChange':
      return cartLines(change);
    case 'applyAttributeChange':
      return attribute(change);
    case 'applyNoteChange':
      return note(change);
    case 'applyDiscountCodeChange':
      return discountCode(change);
    case 'applyMetafieldChange':
      return metafield(change);
    case 'applyShippingAddressChange':
      return shippingAddress(change, state);
    case 'applyGiftCardChange':
    case 'applyRedeemableChange':
      return fail('Gift cards are not supported by the local checkout.');
    case 'applyPaymentMethodAttributesChange':
      return fail('Payment method attributes are not supported by the local checkout.');
    default:
      return fail(`${method} is not part of the checkout UI extension API.`);
  }
}

function cartLines(change) {
  switch (change.type) {
    case 'addCartLine':
      if (!present(change.merchandiseId)) return fail('addCartLine needs a merchandiseId.');
      if (!Number.isInteger(change.quantity) || change.quantity < 1) {
        return fail('addCartLine needs a quantity of at least 1.');
      }
      break;
    case 'removeCartLine':
      if (!present(change.id)) return fail('removeCartLine needs the id of a line.');
      if (!Number.isInteger(change.quantity) || change.quantity < 1) {
        return fail('removeCartLine needs a quantity of at least 1.');
      }
      break;
    case 'updateCartLine':
      if (!present(change.id)) return fail('updateCartLine needs the id of a line.');
      if (change.quantity !== undefined && (!Number.isInteger(change.quantity) || change.quantity < 0)) {
        return fail('updateCartLine needs a whole quantity.');
      }
      break;
    default:
      return fail(`Unknown cart line change "${change.type}".`);
  }
  return { operations: [{ type: 'cartLines', change }] };
}

function attribute(change) {
  if (change.type === 'updateAttribute') {
    if (!present(change.key)) return fail('updateAttribute needs a key.');
    return { operations: [{ type: 'attribute', change: { ...change, value: String(change.value ?? '') } }] };
  }
  if (change.type === 'removeAttribute') {
    if (!present(change.key)) return fail('removeAttribute needs a key.');
    return { operations: [{ type: 'attribute', change }] };
  }
  return fail(`Unknown attribute change "${change.type}".`);
}

function note(change) {
  if (change.type === 'updateNote') {
    return { operations: [{ type: 'note', change: { ...change, note: String(change.note ?? '') } }] };
  }
  if (change.type === 'removeNote') return { operations: [{ type: 'note', change }] };
  return fail(`Unknown note change "${change.type}".`);
}

function discountCode(change) {
  if (change.type !== 'addDiscountCode' && change.type !== 'removeDiscountCode') {
    return fail(`Unknown discount code change "${change.type}".`);
  }
  if (!present(change.code)) return fail(`${change.type} needs a code.`);
  const action = change.type === 'addDiscountCode' ? 'add' : 'remove';
  return { operations: [op.discountCode(action, change.code.trim())] };
}

/** Both the current change (`updateCartMetafield`) and the older one (`updateMetafield`). */
function metafield(change) {
  switch (change.type) {
    case 'updateCartMetafield':
    case 'updateMetafield': {
      const source = change.metafield ?? change;
      if (!present(source.key)) return fail(`${change.type} needs a key.`);
      return {
        operations: [
          {
            type: 'metafield',
            change: {
              type: change.type,
              namespace: source.namespace ?? '$app:default',
              key: source.key,
              // The current change names it `type`; on the older one `type` is the change itself.
              valueType: (change.metafield ? source.type : source.valueType) ?? 'single_line_text_field',
              value: String(source.value ?? ''),
            },
          },
        ],
      };
    }
    case 'removeCartMetafield':
    case 'removeMetafield':
      if (!present(change.key)) return fail(`${change.type} needs a key.`);
      return {
        operations: [
          {
            type: 'metafield',
            change: { type: change.type, namespace: change.namespace ?? '$app:default', key: change.key },
          },
        ],
      };
    default:
      return fail(`Unknown metafield change "${change.type}".`);
  }
}

function shippingAddress(change, state) {
  if (change.type !== 'updateShippingAddress') {
    return fail(`Unknown shipping address change "${change.type}".`);
  }
  const { name, oneTimeUse, addressLineComponents, addressCode, ...address } = change.address ?? {};
  if (present(name) && !address.firstName && !address.lastName) {
    const [first, ...rest] = name.trim().split(/\s+/);
    address.firstName = first;
    address.lastName = rest.join(' ') || undefined;
  }
  return { operations: [op.shippingAddress({ ...(state?.shippingAddress ?? {}), ...address })] };
}

// --- Where an error belongs ------------------------------------------------------------

const ADDRESS_FIELDS = [
  'firstName', 'lastName', 'company', 'address1', 'address2', 'city', 'countryCode',
  'provinceCode', 'zip', 'phone',
];

/**
 * The field of the page a validation target points at (`$.cart.buyerIdentity.email`,
 * `$.cart.deliveryGroups[0].deliveryAddress.zip`, …), or `null` for an error that belongs to
 * the checkout as a whole (`$.cart`).
 */
export function fieldForTarget(target) {
  if (typeof target !== 'string') return null;
  if (/buyerIdentity\.email(Address)?$/.test(target)) return 'email';
  if (/buyerIdentity\.phone$/.test(target)) return 'phone';
  const last = target.split('.').pop();
  if (/deliveryAddress|shippingAddress/.test(target) && ADDRESS_FIELDS.includes(last)) return last;
  if (/paymentMethods?|selectedPayment/.test(target)) return 'payment';
  if (/deliveryOptions?|deliveryGroups\[\d+\]$/.test(target)) return 'delivery';
  return null;
}

/** Splits errors into `{fields: {email: [...]}, general: [...]}` for the page. */
export function placeErrors(errors) {
  const placed = { fields: {}, general: [] };
  for (const error of errors ?? []) {
    const field = fieldForTarget(error.target);
    if (field) (placed.fields[field] ??= []).push(error.message);
    else placed.general.push(error.message);
  }
  return placed;
}

/** The fields the buyer has to fill before paying, by the order of the page. */
export function missingFields(state, draft = {}) {
  const missing = [];
  const email = draft.email ?? state.buyer?.email;
  if (!present(email)) missing.push('email');
  const needsShipping = state.lines?.some((line) => line.requiresShipping) ?? true;
  if (needsShipping) {
    const address = { ...(state.shippingAddress ?? {}), ...(draft.address ?? {}) };
    for (const field of ['lastName', 'address1', 'city', 'countryCode']) {
      if (!present(address[field])) missing.push(field);
    }
    if (state.deliveryGroups?.some((group) => !group.selected)) missing.push('delivery');
  }
  if (state.paymentMethods?.length && !state.selectedPayment) missing.push('payment');
  return missing;
}
