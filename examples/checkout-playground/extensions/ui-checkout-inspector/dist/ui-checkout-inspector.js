// ui-checkout-inspector · API 2026-10 · purchase.checkout.block.render
// Shows what the `shopify` global exposes, one foldable section per API, each value on a line
// as `path = value`, redrawn when any of them changes: buyer identity, localization, cost,
// discount codes and allocations, delivery groups and the selected option, payment options,
// lines with their attributes and allocations, note, attributes, metafields, the address,
// shop, extension, buyer journey, instructions and settings.
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

  // [title, () => value]: what each section shows. Signals are read with `.value`.
  const SECTIONS = [
    ['buyerIdentity', () => ({
      email: shopify.buyerIdentity.email.value,
      phone: shopify.buyerIdentity.phone.value,
      customer: shopify.buyerIdentity.customer.value,
      purchasingCompany: shopify.buyerIdentity.purchasingCompany.value,
    })],
    ['localization', () => ({
      language: shopify.localization.language.value,
      extensionLanguage: shopify.localization.extensionLanguage.value,
      country: shopify.localization.country.value,
      currency: shopify.localization.currency.value,
      market: shopify.localization.market.value,
      timezone: shopify.localization.timezone.value,
    })],
    ['cost', () => ({
      subtotalAmount: shopify.cost.subtotalAmount.value,
      totalShippingAmount: shopify.cost.totalShippingAmount.value,
      totalTaxAmount: shopify.cost.totalTaxAmount.value,
      totalAmount: shopify.cost.totalAmount.value,
    })],
    ['discounts', () => ({
      discountCodes: shopify.discountCodes.value,
      discountAllocations: shopify.discountAllocations.value,
      appliedGiftCards: shopify.appliedGiftCards.value,
    })],
    ['delivery', () => {
      const groups = shopify.deliveryGroups.value;
      const selected = groups.map((group) => {
        const handle = group.selectedDeliveryOption && group.selectedDeliveryOption.handle;
        return group.deliveryOptions.find((option) => option.handle === handle) || null;
      });
      return { selectedDeliveryOption: selected, deliveryGroups: groups, shippingAddress: shopify.shippingAddress.value };
    }],
    ['payment', () => ({
      availablePaymentOptions: shopify.availablePaymentOptions.value,
      selectedPaymentOptions: shopify.selectedPaymentOptions.value,
    })],
    ['lines', () => shopify.lines.value.map((line) => ({
      title: line.merchandise.title,
      sku: line.merchandise.sku,
      quantity: line.quantity,
      cost: line.cost.totalAmount,
      attributes: line.attributes,
      discountAllocations: line.discountAllocations,
      sellingPlan: line.merchandise.sellingPlan,
      lineComponents: line.lineComponents.length,
    }))],
    ['note and attributes', () => ({ note: shopify.note.value, attributes: shopify.attributes.value })],
    ['metafields', () => ({ appMetafields: shopify.appMetafields.value, metafields: shopify.metafields.value })],
    ['shop and extension', () => ({
      shop: shopify.shop,
      extension: { apiVersion: shopify.extension.apiVersion, target: shopify.extension.target, capabilities: shopify.extension.capabilities.value },
      checkoutToken: shopify.checkoutToken.value,
      settings: shopify.settings.value,
    })],
    ['buyerJourney and instructions', () => ({
      completed: shopify.buyerJourney.completed.value,
      activeStep: shopify.buyerJourney.activeStep.value,
      steps: shopify.buyerJourney.steps.value,
      instructions: shopify.instructions.value,
      checkoutSettings: shopify.checkoutSettings.value,
    })],
  ];

  /** `{a: {b: 1}}` → `["a.b = 1"]`. */
  const flatten = (value, path = '') => {
    if (value === null || value === undefined) return [`${path || '(value)'} = ${value}`];
    if (Array.isArray(value)) {
      if (value.length === 0) return [`${path} = []`];
      return value.flatMap((item, index) => flatten(item, `${path}[${index}]`));
    }
    if (typeof value === 'object') {
      const keys = Object.keys(value);
      if (keys.length === 0) return [`${path} = {}`];
      return keys.flatMap((key) => flatten(value[key], path ? `${path}.${key}` : key));
    }
    return [`${path} = ${JSON.stringify(value)}`];
  };

  shopify.extend('purchase.checkout.block.render', () => {
    const open = new Set(['buyerIdentity', 'localization', 'cost']);
    const body = h('s-stack', { gap: 'small-200' });
    let scheduled = false;
    const draw = () => {
      scheduled = false;
      body.replaceChildren(...SECTIONS.map(([title, read]) => {
        const details = h('s-details', { open: open.has(title) },
          h('s-summary', {}, title),
          h('s-stack', { gap: 'none', padding: 'small-300' },
            flatten(read()).map((row) => h('s-text', { color: 'subdued' }, row))));
        details.addEventListener('toggle', () => {
          if (details.hasAttribute('open')) open.add(title);
          else open.delete(title);
        });
        return details;
      }));
    };
    const schedule = () => {
      if (scheduled) return;
      scheduled = true;
      queueMicrotask(draw);
    };
    for (const signal of [
      shopify.buyerIdentity.email, shopify.buyerIdentity.phone, shopify.buyerIdentity.customer,
      shopify.buyerIdentity.purchasingCompany, shopify.localization.language, shopify.localization.country,
      shopify.localization.currency, shopify.localization.market, shopify.cost.subtotalAmount,
      shopify.cost.totalShippingAmount, shopify.cost.totalTaxAmount, shopify.cost.totalAmount,
      shopify.discountCodes, shopify.discountAllocations, shopify.deliveryGroups, shopify.shippingAddress,
      shopify.availablePaymentOptions, shopify.selectedPaymentOptions, shopify.lines, shopify.note,
      shopify.attributes, shopify.appMetafields, shopify.metafields, shopify.buyerJourney.completed,
    ]) signal.subscribe(schedule);
    mount(h('s-stack', { gap: 'small-200' }, h('s-heading', {}, 'What the shopify global exposes'), body));
  });
})();
