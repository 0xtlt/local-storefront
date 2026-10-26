// The checkout page. The server renders a shell with the state of the checkout as JSON; this
// script draws the page from it, sends the buyer's choices as operations
// (`POST …/update`), and draws again from the state that comes back. The checkout UI
// extensions are hosted by `extension-host.js`.

import { createExtensionHost } from './extension-host.js';
import { withCurrency } from './format.js';
import { missingFields, op } from './operations.js';
import { renderPage } from './views.js';
import { h, render } from './vdom.js';

const ADDRESS_FIELDS = [
  'firstName', 'lastName', 'company', 'address1', 'address2', 'city', 'countryCode',
  'provinceCode', 'zip', 'phone',
];

const JSON_HEADERS = { 'content-type': 'application/json', accept: 'application/json' };

/**
 * Starts the page in `root` from `initial` (the state JSON). Everything the page touches comes
 * in as an argument so a test can run it without a browser.
 */
export function startCheckout({ root, initial, document, window, location, fetch, storage, sandboxUrl }) {
  let state = withCurrency(initial);
  const ui = {
    draft: { address: {} },
    discountInput: '',
    discountError: null,
    summaryOpen: false,
    functionsOpen: false,
    openRuns: new Set(),
    tried: false,
    busy: false,
    block: null,
    failure: null,
  };

  const endpoint = (path) => `/checkouts/cn/${encodeURIComponent(state.token)}${path}`;

  const host = createExtensionHost({
    document,
    window,
    sandboxUrl,
    storage,
    getState: () => state,
    dispatch: (operations) => send(operations),
  });

  function slot(target, instance = {}) {
    if (!host.hasTarget(target)) return null;
    return h('div', {
      class: 'lsf-slot',
      key: `slot:${target}:${instance.key ?? ''}`,
      opaque: true,
      'data-target': target,
      hooks: {
        insert: (el) => host.attach(el, target, instance),
        destroy: (el) => host.detach(el),
      },
    });
  }

  function paint() {
    document.body?.setAttribute?.('data-variant', state.variant);
    render(root, renderPage({ state, ui, slot, actions, location: location.href, refusals: host.refusals() }));
    host.update(state);
  }

  /** Takes a new state from the server. What the buyer typed and the server now agrees with is
   * no longer a draft. */
  function accept(next) {
    state = withCurrency(next);
    if (ui.draft.email === (state.buyer?.email ?? '')) delete ui.draft.email;
    for (const [name, value] of Object.entries(ui.draft.address)) {
      if (value === (state.shippingAddress?.[name] ?? '')) delete ui.draft.address[name];
    }
    paint();
  }

  // The server applies operations one after the other: so do we, whatever asks.
  let queue = Promise.resolve();
  function send(operations) {
    const request = queue.then(async () => {
      const response = await fetch(endpoint('/update'), {
        method: 'POST',
        headers: JSON_HEADERS,
        body: JSON.stringify({ operations }),
      });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error ?? `The checkout answered ${response.status}.`);
      accept(body);
      return body;
    });
    queue = request.catch(() => {});
    return request;
  }

  /** Sends operations for the buyer, showing a failure instead of throwing. */
  async function run(operations) {
    ui.failure = null;
    try {
      await send(operations);
    } catch (error) {
      ui.failure = error.message;
      paint();
    }
  }

  function addressToSend() {
    const address = {};
    for (const name of ADDRESS_FIELDS) {
      const value = ui.draft.address[name] ?? state.shippingAddress?.[name] ?? '';
      address[name] = value === '' ? null : value;
    }
    return address;
  }

  const actions = {
    draft(name, value) {
      if (name === 'email') ui.draft.email = value;
      else if (name === 'discount') ui.discountInput = value;
      else ui.draft.address[name] = value;
    },

    commit(name, value) {
      if (name === 'email') {
        ui.draft.email = value;
        if (value !== (state.buyer?.email ?? '')) return run([op.email(value)]);
        return undefined;
      }
      ui.draft.address[name] = value;
      const operations = [op.shippingAddress(addressToSend())];
      if (name === 'phone' && value !== (state.buyer?.phone ?? '')) operations.push(op.phone(value));
      if (value === (state.shippingAddress?.[name] ?? '')) return undefined;
      return run(operations);
    },

    chooseDelivery: (group, handle) => run([op.deliveryOption(group, handle)]),
    choosePayment: (handle) => run([op.paymentMethod(handle)]),

    async addDiscount() {
      const code = ui.discountInput.trim();
      if (!code) return;
      ui.discountError = null;
      await run([op.discountCode('add', code)]);
      const entry = state.discountCodes?.find((candidate) => candidate.code.toLowerCase() === code.toLowerCase());
      if (entry && !entry.applicable) {
        ui.discountError = 'Enter a valid discount code';
        await run([op.discountCode('remove', entry.code)]);
      } else {
        ui.discountInput = '';
      }
      paint();
    },

    removeDiscount: (code) => run([op.discountCode('remove', code)]),

    toggleSummary() {
      ui.summaryOpen = !ui.summaryOpen;
      paint();
    },

    toggleFunctions(open) {
      if (open === ui.functionsOpen) return;
      ui.functionsOpen = open;
      paint();
    },

    toggleRun(index, open) {
      if (open === ui.openRuns.has(index)) return;
      if (open) ui.openRuns.add(index);
      else ui.openRuns.delete(index);
      paint();
    },

    async pay() {
      if (ui.busy) return;
      ui.tried = true;
      ui.block = null;
      ui.failure = null;
      if (missingFields(state, ui.draft).length > 0) {
        paint();
        document.querySelector?.('[aria-invalid="true"]')?.focus?.();
        return;
      }
      ui.busy = true;
      paint();
      try {
        const { blocks } = await host.intercept();
        if (blocks.length > 0) {
          const reasons = blocks.map((block) => block.reason).filter(Boolean);
          ui.block = {
            reason: reasons.join(' ') || 'An extension is blocking this order.',
            errors: blocks.flatMap((block) => block.errors),
          };
          return;
        }
        const response = await fetch(endpoint('/complete'), { method: 'POST', headers: JSON_HEADERS, body: '{}' });
        const body = await response.json();
        if (response.ok) {
          location.assign(body.redirect);
        } else if (body.lines) {
          accept(body);
        } else {
          ui.failure = body.error ?? `The checkout answered ${response.status}.`;
        }
      } catch (error) {
        ui.failure = error.message;
      } finally {
        ui.busy = false;
        paint();
      }
    },
  };

  paint();
  return { actions, ui, host, send, getState: () => state, paint };
}

if (typeof document !== 'undefined' && document.getElementById?.('lsf-checkout-state')) {
  const initial = JSON.parse(document.getElementById('lsf-checkout-state').textContent);
  document.title = `Checkout · ${initial.shop?.name ?? ''}`.trim();
  startCheckout({
    root: document.getElementById('lsf-checkout'),
    initial,
    document,
    window,
    location,
    fetch: (...args) => window.fetch(...args),
    storage: (() => {
      try {
        return window.localStorage;
      } catch {
        return undefined;
      }
    })(),
    sandboxUrl: new URL('sandbox.html', import.meta.url).href,
  });
}
