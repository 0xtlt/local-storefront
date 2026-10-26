import '@shopify/ui-extensions/preact';
import {render} from 'preact';
import {useState} from 'preact/hooks';

export default function extension() {
  render(<FreeShipping />, document.body);
}

// The metafield `[[extensions.metafields]]` declares in shopify.extension.toml: with API
// 2027-01 its `owner_type` is required, and it says the value is read from the cart.
const DISMISSED = {namespace: '$app:notice', key: 'dismissed'};

function FreeShipping() {
  const {appMetafields, cost, i18n, settings} = shopify;
  const [error, setError] = useState(null);

  const threshold = Number(settings.value.threshold) || 50;
  const code = settings.value.code || 'FREESHIP';
  const money = (amount) => i18n.formatCurrency(amount);

  const dismissed = appMetafields.value.some(
    ({target, metafield}) =>
      target.type === 'cart' &&
      metafield.namespace === DISMISSED.namespace &&
      metafield.key === DISMISSED.key &&
      metafield.value === 'true',
  );
  if (dismissed) return null;

  const missing = Math.max(0, threshold - cost.totalAmount.value.amount);

  async function apply() {
    const result = await shopify.applyDiscountCodeChange({type: 'addDiscountCode', code});
    setError(result.type === 'error' ? result.message : null);
  }

  function dismiss() {
    return shopify.applyMetafieldChange({
      type: 'updateCartMetafield',
      metafield: {...DISMISSED, type: 'boolean', value: 'true'},
    });
  }

  return (
    <s-banner
      heading={error ?? `Free shipping from ${money(threshold)}`}
      tone={error ? 'critical' : missing > 0 ? 'info' : 'success'}
    >
      <s-stack gap="small-200">
        <s-text color="subdued">
          {missing > 0
            ? `You are ${money(missing)} away from free shipping.`
            : 'Shipping is free on this order.'}
        </s-text>
        <s-progress value={Math.min(cost.totalAmount.value.amount, threshold)} max={threshold} />
        <s-stack direction="inline" gap="small-200">
          <s-button variant="secondary" onClick={apply}>
            Apply {code}
          </s-button>
          <s-button onClick={dismiss}>
            Dismiss
          </s-button>
        </s-stack>
      </s-stack>
    </s-banner>
  );
}
