import '@shopify/ui-extensions/preact';
import {render} from 'preact';

export default function extension() {
  render(<DeliveryInstructions />, document.body);
}

function DeliveryInstructions() {
  const {deliveryGroups, note} = shopify;
  const options = deliveryGroups.value.reduce((sum, group) => sum + group.deliveryOptions.length, 0);

  return (
    <s-stack gap="small-200">
      <s-text color="subdued">
        {options} shipping option{options === 1 ? '' : 's'} for this order
      </s-text>
      <s-text-area
        label="Delivery instructions"
        rows={2}
        value={note.value ?? ''}
        onChange={(event) =>
          shopify.applyNoteChange(
            event.currentTarget.value
              ? {type: 'updateNote', note: event.currentTarget.value}
              : {type: 'removeNote'},
          )
        }
      />
    </s-stack>
  );
}
