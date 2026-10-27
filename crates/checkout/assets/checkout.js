// The checkout page. The server renders a shell with the state of the checkout as JSON; this
// script draws the page from it, sends the buyer's choices as operations
// (`POST …/update`), and draws again from the state that comes back. The checkout UI
// extensions are hosted by `extension-host.js`.

import { MENU_ITEMS } from './account.js';
import { RENDERED_TARGETS, createExtensionHost } from './extension-host.js';
import { languageOf, withCurrency } from './format.js';
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
    accountOpen: false,
    functionsOpen: false,
    openRuns: new Set(),
    tried: false,
    busy: false,
    block: null,
    failure: null,
  };

  // Under the locale root of the buyer's language (`/fr`), so that every request of the page
  // is made in that language too.
  const endpoint = (path) =>
    `${state.localization?.rootUrl ?? ''}/checkouts/cn/${encodeURIComponent(state.token)}${path}`;

  // `?targets=all` asks for a page where every extension renders at every target it declares;
  // without it there is no preview at all.
  const showAll = new URL(location.href ?? 'http://localhost/', 'http://localhost/').searchParams.get('targets') === 'all';
  const preview = showAll ? { all: true, rendered: RENDERED_TARGETS } : null;

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
    if (document.documentElement) document.documentElement.lang = languageOf(state);
    render(root, renderPage({ state, ui, slot, actions, location: location.href, refusals: host.refusals(), preview }));
    host.update(state);
  }

  /** Moves the focus to the button of the customer menu (`trigger`) or to one of its items:
   * `first`, `last`, or `current` (the customer logged in, else the first). */
  function focusAccount(which) {
    if (which === 'trigger') {
      root.querySelector?.('.lsf-account-trigger')?.focus?.();
      return;
    }
    const items = [...(root.querySelectorAll?.(MENU_ITEMS) ?? [])];
    if (items.length === 0) return;
    const checked = items.findIndex((item) => item.getAttribute('aria-checked') === 'true' && item.getAttribute('data-customer'));
    const index = which === 'last' ? items.length - 1 : which === 'current' ? Math.max(0, checked) : 0;
    items[index].focus?.();
  }

  // A click anywhere but on the customer menu closes it, and leaves the focus where it went.
  document.addEventListener?.('click', (event) => {
    if (!ui.accountOpen || event.target?.closest?.('.lsf-account')) return;
    ui.accountOpen = false;
    paint();
  });

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

  // The server applies operations one after the other: so do we, whatever asks. Everything that
  // talks to the server about this checkout, the payment included, goes through this queue.
  let queue = Promise.resolve();
  function enqueue(task) {
    const request = queue.then(task);
    queue = request.catch(() => {});
    return request;
  }

  /** `operations` is a list, or a function that makes it when its turn in the queue comes, from
   * the state the changes before it left. Nothing is sent for an empty list. */
  function send(operations) {
    return enqueue(async () => {
      if (typeof operations === 'function') operations = operations();
      if (operations.length === 0) return state;
      const response = await fetch(endpoint('/update'), {
        method: 'POST',
        headers: JSON_HEADERS,
        body: JSON.stringify({ operations }),
      });
      const body = await response.json();
      if (!response.ok) {
        // The checkout is not the one the operations were made for (another tab changed it, or
        // the order was placed): the reply is the checkout as it is now, and the page shows it
        // before it says the operations were not applied.
        if (response.status === 409 && body.lines) {
          accept(body);
          throw new Error(body.error ?? 'The checkout changed: that change was not applied.');
        }
        throw new Error(body.error ?? `The checkout answered ${response.status}.`);
      }
      accept(body);
      return body;
    });
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

  /** The operations for what the buyer typed and the server does not know yet. */
  function draftOperations() {
    const operations = [];
    if (ui.draft.email !== undefined && ui.draft.email !== (state.buyer?.email ?? '')) {
      operations.push(op.email(ui.draft.email));
    }
    const typedAddress = Object.entries(ui.draft.address).some(
      ([name, value]) => value !== (state.shippingAddress?.[name] ?? ''),
    );
    if (typedAddress) {
      operations.push(op.shippingAddress(addressToSend()));
      const phone = ui.draft.address.phone;
      if (phone !== undefined && phone !== (state.buyer?.phone ?? '')) operations.push(op.phone(phone));
    }
    return operations;
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
      if (value === (state.shippingAddress?.[name] ?? '')) return undefined;
      // The rest of the address is read when the turn comes: an extension may have changed it.
      return run(() => {
        const operations = [op.shippingAddress(addressToSend())];
        if (name === 'phone' && value !== (state.buyer?.phone ?? '')) operations.push(op.phone(value));
        return operations;
      });
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
      // The page does not write into the field that has the focus (the caret would jump), and
      // Enter keeps the focus there: the code that was applied is cleared from it by hand.
      if (!ui.discountInput) {
        const field = root.querySelector?.('.lsf-discount-form .lsf-input');
        if (field && field.value !== '') field.value = '';
      }
    },

    removeDiscount: (code) => run([op.discountCode('remove', code)]),

    /** Opens or closes the menu of the customers to log in as, and moves the focus (`focus`:
     * where, see `focusAccount`; the button when it closes). */
    toggleAccount(open, focus = open ? 'current' : 'trigger') {
      if (open === ui.accountOpen) return;
      ui.accountOpen = open;
      paint();
      if (focus) focusAccount(focus);
    },

    /** The arrows on the closed button open the menu on its first or its last item. */
    accountTriggerKey(event) {
      if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
      event.preventDefault?.();
      ui.accountOpen = true;
      paint();
      focusAccount(event.key === 'ArrowUp' ? 'last' : 'first');
    },

    /** The keys of a menu: the arrows, Home and End move between the items, Escape closes it
     * and gives the focus back to its button, Tab closes it and moves on. */
    accountMenuKey(event) {
      const items = [...(root.querySelectorAll?.(MENU_ITEMS) ?? [])];
      const index = items.indexOf(document.activeElement);
      const move = (to) => {
        event.preventDefault?.();
        items[(to + items.length) % items.length]?.focus?.();
      };
      switch (event.key) {
        case 'ArrowDown':
          return move(index + 1);
        case 'ArrowUp':
          return move(index < 0 ? items.length - 1 : index - 1);
        case 'Home':
          return move(0);
        case 'End':
          return move(items.length - 1);
        case 'Escape':
          event.preventDefault?.();
          return actions.toggleAccount(false, 'trigger');
        case 'Tab':
          // From the button, the browser moves the focus on to what follows it.
          return actions.toggleAccount(false, 'trigger');
        default:
          return undefined;
      }
    },

    /** Logs the checkout in as the customer with this email, or out (`null`). What was typed
     * belonged to the buyer before. */
    async logIn(email) {
      ui.accountOpen = false;
      ui.draft = { address: {} };
      ui.tried = false;
      ui.block = null;
      await run([op.customer(email)]);
      focusAccount('trigger');
    },

    chooseLocation: (id) => run([op.companyLocation(id)]),

    /** A logged-in checkout's contact email goes back to the account's: the checkout falls back
     * to it when none is entered. */
    useAccountEmail() {
      delete ui.draft.email;
      return run([op.email(null)]);
    },

    /** Ships to a saved address, or (`new`) to one the buyer types, in the same country. */
    chooseSavedAddress(id) {
      ui.draft.address = {};
      if (id === 'new') {
        return run(() => [op.shippingAddress({ countryCode: state.shippingAddress?.countryCode ?? null })]);
      }
      const entry = state.account?.addresses?.find((candidate) => candidate.id === id);
      return entry ? run([op.shippingAddress(entry.address)]) : undefined;
    },

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
        // What the buyer typed and left no field for yet goes first, after what is already on
        // its way: the order is paid for the state the server holds, not the one on screen.
        await queue;
        await send(draftOperations);
        if (missingFields(state, ui.draft).length > 0) {
          ui.tried = true;
          return;
        }
        const { blocks } = await host.intercept();
        if (blocks.length > 0) {
          const reasons = blocks.map((block) => block.reason).filter(Boolean);
          ui.block = {
            reason: reasons.join(' ') || 'An extension is blocking this order.',
            errors: blocks.flatMap((block) => block.errors),
          };
          return;
        }
        // Behind whatever the extensions changed while they answered.
        const { response, body } = await enqueue(async () => {
          const reply = await fetch(endpoint('/complete'), { method: 'POST', headers: JSON_HEADERS, body: '{}' });
          return { response: reply, body: await reply.json() };
        });
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
