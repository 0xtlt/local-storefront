import '@shopify/ui-extensions/preact';
import {render} from 'preact';
import {useSignal} from '@preact/signals';
import {useCartLines, useTotalAmount} from '@shopify/ui-extensions/checkout/preact';

export default async () => {
  render(<CartLines />, document.body);
};

interface Line {
  title: string;
  quantity: number;
}

function CartLines() {
  const lines = useCartLines();
  const total = useTotalAmount();
  const expanded = useSignal(false);
  const shown: Line[] = lines.map((line) => ({
    title: line.merchandise.title,
    quantity: line.quantity,
  }));
  const units = shown.reduce((sum, line) => sum + line.quantity, 0);

  return (
    <s-stack gap="small-200">
      <s-text>
        {`${shown.length} lines, ${units} items, ${shopify.i18n.formatCurrency(Number(total.amount))}`}
      </s-text>
      <s-button onClick={() => (expanded.value = !expanded.value)}>
        {expanded.value ? 'Hide the lines' : 'Show the lines'}
      </s-button>
      {expanded.value &&
        shown.map((line) => (
          <s-text key={line.title}>
            {line.quantity} × {line.title}
          </s-text>
        ))}
    </s-stack>
  );
}
