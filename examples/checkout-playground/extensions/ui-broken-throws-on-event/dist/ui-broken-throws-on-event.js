// ui-broken-throws-on-event · API 2027-01 · purchase.checkout.block.render · BROKEN ON PURPOSE
// Renders fine, then fails when used: a click handler that throws, one whose promise rejects
// with nobody to catch it, and a signal subscriber that throws when the cart changes (press
// "Change the note"). Each error shows above the frame.
(() => {
  // h(tag, attributes, ...children) makes a Polaris web component: attributes are set as
  // attributes, functions become listeners (`onClick` listens to `click`), children are
  // appended. Each bundle carries its own copy, as a real build carries its runtime.
  const h = (tag, attributes, ...children) => {
    const node = document.createElement(tag);
    for (const [name, value] of Object.entries(attributes || {})) {
      if (value === undefined || value === null || value === false) continue;
      if (typeof value === 'function') node.addEventListener(name.slice(2).toLowerCase(), value);
      else node.setAttribute(name, value === true ? '' : String(value));
    }
    for (const child of children.flat(Infinity)) {
      if (child === undefined || child === null || child === false) continue;
      node.append(typeof child === 'object' ? child : String(child));
    }
    return node;
  };
  const mount = (...nodes) => document.body.replaceChildren(...nodes);
  const money = (value) => (value ? shopify.i18n.formatCurrency(value.amount) : '—');

  shopify.extend('purchase.checkout.block.render', () => {
    let armed = false;
    shopify.note.subscribe(() => {
      if (armed) throw new Error('ui-broken-throws-on-event: a note subscriber threw.');
    });
    mount(h('s-stack', { gap: 'small-200' },
      h('s-text', {}, 'Each button fails in its own way.'),
      h('s-stack', { direction: 'inline', gap: 'small-200' },
        h('s-button', {
          variant: 'secondary',
          onClick: () => { throw new Error('ui-broken-throws-on-event: a click handler threw.'); },
        }, 'Throw in a handler'),
        h('s-button', {
          variant: 'secondary',
          onClick: () => { Promise.reject(new Error('ui-broken-throws-on-event: nobody caught this promise.')); },
        }, 'Reject a promise'),
        h('s-button', {
          variant: 'secondary',
          onClick: () => {
            armed = true;
            shopify.applyNoteChange({ type: 'updateNote', note: `Changed at ${new Date().toLocaleTimeString()}` });
          },
        }, 'Change the note'))));
  });
})();
