import '@shopify/ui-extensions/preact';
import {render} from 'preact';
import {
  useAppMetafields,
  useCartLines,
  useOrder,
  useTotalAmount,
  useTranslate,
} from '@shopify/ui-extensions/customer-account/preact';

// The filter of the order's copy of the cart metafield `ui-preact` sets: `appMetafields` has it
// once the order is placed, typed (`true`, not `"true"`).
const FREE_GIFT = {type: 'order', namespace: '$app', key: 'requestedFreeGift'};

export default async () => {
  render(<Extension />, document.body);
};

function Extension() {
  const translate = useTranslate();
  const order = useOrder();
  const lines = useCartLines();
  const total = useTotalAmount();
  const units = lines.reduce((sum, line) => sum + line.quantity, 0);
  const [freeGift] = useAppMetafields(FREE_GIFT);
  return (
    <s-banner heading={translate('orderHeading', {name: order?.name ?? ''})}>
      <s-stack gap="small-200">
        <s-text>{translate('units', {count: units})}</s-text>
        <s-text color="subdued">
          {translate('total', {total: shopify.i18n.formatCurrency(total.amount, {currency: total.currencyCode})})}
        </s-text>
        <s-text color="subdued">{translate('confirmation', {number: order?.confirmationNumber ?? ''})}</s-text>
        {freeGift?.metafield.value === true && <s-text>{translate('freeGift')}</s-text>}
      </s-stack>
    </s-banner>
  );
}
