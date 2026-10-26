// Written by hand to be what `shopify app build` leaves in dist/ for an extension on API
// 2025-07, before checkout UI extensions moved to Preact and web components: one IIFE that
// carries the part of `@remote-ui/core` it needs and registers each target with
// `shopify.extend`. The function it registers gets `{channel, components}` and the API, and
// describes the interface as a tree of remote components whose changes go down the channel.
(() => {
  // --- @remote-ui/core, as much of it as this extension uses ---------------------------------
  const ACTION_MOUNT = 0;
  const ACTION_INSERT_CHILD = 1;
  const ACTION_REMOVE_CHILD = 2;
  const ACTION_UPDATE_TEXT = 3;
  const ACTION_UPDATE_PROPS = 4;
  const KIND_COMPONENT = 1;
  const KIND_TEXT = 2;

  function createRemoteRoot(channel, { components } = {}) {
    let nextId = 0;
    let mounted = false;
    const send = (...message) => {
      if (mounted) channel(...message);
    };
    const serialize = (node) =>
      node.kind === KIND_TEXT
        ? { id: node.id, kind: node.kind, text: node.text }
        : { id: node.id, kind: node.kind, type: node.type, props: node.props, children: node.children.map(serialize) };
    const connected = (node) => (node.parent ? connected(node.parent) : node === root);
    const insert = (parent, child) => {
      parent.children.push(child);
      child.parent = parent;
      if (connected(parent)) send(ACTION_INSERT_CHILD, parent.id, parent.children.length - 1, serialize(child), false);
    };
    const remove = (parent, child) => {
      const index = parent.children.indexOf(child);
      if (index < 0) return;
      const wasConnected = connected(parent);
      parent.children.splice(index, 1);
      child.parent = null;
      if (wasConnected) send(ACTION_REMOVE_CHILD, parent.id, index);
    };

    const root = {
      id: "~",
      children: [],
      parent: null,
      appendChild: (child) => insert(root, child),
      removeChild: (child) => remove(root, child),
      createComponent(type, props = {}, ...children) {
        if (components && !components.includes(type)) throw new Error(`Unsupported component: ${type}`);
        const { children: _children, ...own } = props;
        const node = {
          id: String(nextId++),
          kind: KIND_COMPONENT,
          type,
          props: own,
          children: [],
          parent: null,
          appendChild: (child) => insert(node, child),
          removeChild: (child) => remove(node, child),
          updateProps(next) {
            Object.assign(node.props, next);
            if (connected(node)) send(ACTION_UPDATE_PROPS, node.id, next);
          },
        };
        for (const child of children.flat()) {
          node.children.push(child);
          child.parent = node;
        }
        return node;
      },
      createText(text = "") {
        const node = {
          id: String(nextId++),
          kind: KIND_TEXT,
          text,
          parent: null,
          update(next) {
            node.text = next;
            if (connected(node)) send(ACTION_UPDATE_TEXT, node.id, next);
          },
        };
        return node;
      },
      mount() {
        if (mounted) return Promise.resolve();
        mounted = true;
        return Promise.resolve(channel(ACTION_MOUNT, root.children.map(serialize)));
      },
    };
    return root;
  }

  // --- purchase.checkout.reductions.render-after ---------------------------------------------

  shopify.extend("purchase.checkout.reductions.render-after", async ({ channel, components }, api) => {
    const root = createRemoteRoot(channel, { components });
    const settings = api.settings.current;
    const threshold = Number(settings.threshold) || 50;
    const code = settings.code || "FREESHIP";
    const money = (amount) => api.i18n.formatCurrency(amount);

    const sentence = root.createText("");
    const progress = root.createComponent("Progress", { value: 0, max: threshold });
    const banner = root.createComponent("Banner", { status: "info", title: `Free shipping from ${money(threshold)}` });
    const button = root.createComponent(
      "Button",
      {
        kind: "secondary",
        onPress: async () => {
          const result = await api.applyDiscountCodeChange({ type: "addDiscountCode", code });
          if (result.type === "error") banner.updateProps({ status: "critical", title: result.message });
        },
      },
      root.createText(`Apply ${code}`),
    );
    const stack = root.createComponent("BlockStack", { spacing: "tight" }, root.createComponent("Text", { appearance: "subdued" }, sentence), progress, button);
    banner.appendChild(stack);

    const update = (total) => {
      const missing = Math.max(0, threshold - total.amount);
      sentence.update(missing > 0 ? `You are ${money(missing)} away from free shipping.` : "Shipping is free on this order.");
      progress.updateProps({ value: Math.min(total.amount, threshold) });
      banner.updateProps({ status: missing > 0 ? "info" : "success" });
    };
    update(api.cost.totalAmount.current);
    api.cost.totalAmount.subscribe(update);

    root.appendChild(banner);
    await root.mount();
  });

  // --- purchase.checkout.shipping-option-list.render-after -----------------------------------

  shopify.extend("purchase.checkout.shipping-option-list.render-after", async ({ channel, components }, api) => {
    const root = createRemoteRoot(channel, { components });
    const count = root.createText("");
    const field = root.createComponent("TextField", {
      label: "Delivery instructions",
      value: api.note.current ?? "",
      multiline: 2,
      onChange: (value) => api.applyNoteChange(value ? { type: "updateNote", note: value } : { type: "removeNote" }),
    });
    const stack = root.createComponent(
      "BlockStack",
      { spacing: "tight" },
      root.createComponent("Text", { size: "small", appearance: "subdued" }, count),
      field,
    );
    const show = (groups) => {
      const total = groups.reduce((sum, group) => sum + group.deliveryOptions.length, 0);
      count.update(`${total} shipping option${total === 1 ? "" : "s"} for this order`);
    };
    show(api.deliveryGroups.current);
    api.deliveryGroups.subscribe(show);
    root.appendChild(stack);
    await root.mount();
  });
})();
