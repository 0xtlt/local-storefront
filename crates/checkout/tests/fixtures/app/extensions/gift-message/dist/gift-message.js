// Written by hand to be what `shopify app build` leaves in dist/ for an extension on API
// 2026-10: one IIFE that registers each target with `shopify.extend` and draws Polaris web
// components into `document.body`. A real build carries Preact and its signals; this one does
// the same with the DOM, so the fixture stays small and readable.
(() => {
  const el = (tag, props = {}, ...children) => {
    const node = document.createElement(tag);
    for (const [name, value] of Object.entries(props)) {
      if (name.startsWith("on")) node.addEventListener(name.slice(2).toLowerCase(), value);
      else if (name in node) node[name] = value;
      else node.setAttribute(name, value);
    }
    node.append(...children);
    return node;
  };
  const mount = (node) => {
    document.body.replaceChildren(node);
  };

  const attribute = (attributes, key) => attributes.find((entry) => entry.key === key)?.value;

  // --- purchase.checkout.block.render -------------------------------------------------------

  shopify.extend("purchase.checkout.block.render", async () => {
    const { attributes, cost, i18n, settings } = shopify;
    const maxLength = Number(settings.value.max_length) || 140;

    const gift = el("s-checkbox", { label: "This order is a gift", checked: attribute(attributes.value, "gift") === "true" });
    const message = el("s-text-area", {
      label: "Message for the recipient",
      maxLength,
      rows: 3,
      value: attribute(attributes.value, "gift_message") ?? "",
    });
    const total = el("s-text", { color: "subdued" });
    const banner = el("s-banner", { tone: "critical", hidden: true });

    const save = (key, value) =>
      value === ""
        ? shopify.applyAttributeChange({ type: "removeAttribute", key })
        : shopify.applyAttributeChange({ type: "updateAttribute", key, value });

    gift.addEventListener("change", async () => {
      message.hidden = !gift.checked;
      if (gift.checked) {
        await save("gift", "true");
        await shopify.applyMetafieldChange({
          type: "updateCartMetafield",
          metafield: { namespace: "$app:gift", key: "wrapped", type: "boolean", value: "true" },
        });
      } else {
        await save("gift", "");
        await save("gift_message", "");
        await shopify.applyMetafieldChange({ type: "removeCartMetafield", namespace: "$app:gift", key: "wrapped" });
      }
    });
    message.addEventListener("change", () => {
      banner.hidden = true;
      return save("gift_message", message.value);
    });

    // The attributes can change under us (another extension, the page): follow them.
    attributes.subscribe((list) => {
      const isGift = attribute(list, "gift") === "true";
      if (gift.checked !== isGift) gift.checked = isGift;
      message.hidden = !isGift;
    });
    cost.totalAmount.subscribe(({ amount }) => {
      const money = i18n.formatCurrency(amount);
      const text = i18n.translate("total", { amount: money });
      // No locale file reaches a local checkout unless the page is given one.
      total.textContent = text === "total" ? `Order total: ${money}` : text;
    });

    await shopify.buyerJourney.intercept(({ canBlockProgress }) => {
      const isGift = attribute(shopify.attributes.value, "gift") === "true";
      if (canBlockProgress && isGift && !message.value.trim()) {
        banner.textContent = "Write a message for the recipient or untick “This order is a gift”.";
        banner.hidden = false;
        return {
          behavior: "block",
          reason: "The gift message is empty.",
          errors: [{ message: "Write a message for the recipient." }],
        };
      }
      return { behavior: "allow" };
    });

    mount(
      el("s-stack", { gap: "small-200" }, el("s-heading", {}, "Gift options"), gift, message, banner, total),
    );
  });

  // --- purchase.checkout.cart-line-item.render-after ------------------------------------------

  shopify.extend("purchase.checkout.cart-line-item.render-after", async () => {
    const line = el("s-text", { color: "subdued" });
    const badge = el("s-badge", { tone: "success" }, "Gift wrap available");
    shopify.target.subscribe((current) => {
      const { merchandise, quantity } = current;
      line.textContent = `${merchandise.title} × ${quantity}${merchandise.sku ? ` · SKU ${merchandise.sku}` : ""}`;
      badge.hidden = !merchandise.requiresShipping;
    });
    mount(el("s-stack", { direction: "inline", gap: "small-200", alignItems: "center" }, badge, line));
  });

  // --- purchase.thank-you.block.render -------------------------------------------------------

  shopify.extend("purchase.thank-you.block.render", async () => {
    const banner = el("s-banner", { tone: "success", heading: "Your gift message" });
    const text = el("s-text", {});
    banner.append(text);
    const show = () => {
      const message = attribute(shopify.attributes.value, "gift_message");
      banner.hidden = !message;
      text.textContent = message ? `“${message}” — order ${shopify.orderConfirmation.value.number ?? ""}`.trim() : "";
    };
    shopify.attributes.subscribe(show);
    mount(banner);
  });
})();
