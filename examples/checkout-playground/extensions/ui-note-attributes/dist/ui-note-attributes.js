// ui-note-attributes · API 2026-10 · purchase.checkout.block.render
// Edits the cart note (`applyNoteChange` updateNote / removeNote) and two cart attributes, a
// delivery window from a select and a PO number from a field (`applyAttributeChange`). Lists
// every attribute of the cart (`shopify.attributes`), each with a Remove button.
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

  const valueOf = (key) => (shopify.attributes.value.find((entry) => entry.key === key) || {}).value;
  const setAttribute = (key, value) => shopify.applyAttributeChange(
    value ? { type: 'updateAttribute', key, value } : { type: 'removeAttribute', key });

  shopify.extend('purchase.checkout.block.render', () => {
    const result = h('s-text', { color: 'subdued' });
    const report = (what) => (outcome) => {
      result.textContent = `${what} → ${outcome.type}${outcome.message ? `: ${outcome.message}` : ''}`;
    };

    const note = h('s-text-area', { label: 'Cart note', rows: 2, value: shopify.note.value || '' });
    note.addEventListener('change', (event) => {
      const text = event.currentTarget.value;
      shopify.applyNoteChange(text ? { type: 'updateNote', note: text } : { type: 'removeNote' }).then(report('applyNoteChange'));
    });
    shopify.note.subscribe((value) => {
      if (note.value !== (value || '')) note.value = value || '';
    });

    const windowSelect = h('s-select', { label: 'Delivery window (attribute delivery_window)', value: valueOf('delivery_window') || '' },
      h('s-option', { value: '' }, 'Any time'),
      h('s-option', { value: 'morning' }, 'Morning'),
      h('s-option', { value: 'evening' }, 'Evening'));
    windowSelect.addEventListener('change', (event) =>
      setAttribute('delivery_window', event.currentTarget.value).then(report('applyAttributeChange delivery_window')));

    const po = h('s-text-field', { label: 'PO number (attribute po_number)', value: valueOf('po_number') || '' });
    po.addEventListener('change', (event) =>
      setAttribute('po_number', event.currentTarget.value.trim()).then(report('applyAttributeChange po_number')));

    const list = h('s-stack', { gap: 'small-300' });
    shopify.attributes.subscribe((attributes) => {
      list.replaceChildren(...(attributes.length ? attributes.map(({ key, value }) =>
        h('s-stack', { direction: 'inline', gap: 'small-200', alignItems: 'center' },
          h('s-text', {}, `${key} = ${value}`),
          h('s-button', { variant: 'secondary', onClick: () => setAttribute(key, '').then(report(`removeAttribute ${key}`)) }, 'Remove')))
        : [h('s-text', { color: 'subdued' }, 'The cart has no attribute.')]));
    });

    mount(h('s-stack', { gap: 'base' },
      h('s-heading', {}, 'Note and attributes'),
      note, windowSelect, po,
      h('s-text', { type: 'strong' }, 'shopify.attributes'), list, result));
  });
})();
