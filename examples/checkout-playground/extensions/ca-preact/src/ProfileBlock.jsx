import '@shopify/ui-extensions/preact';
import {render} from 'preact';
import {useAuthenticatedAccountCustomer, useTranslate} from '@shopify/ui-extensions/customer-account/preact';

export default async () => {
  render(<Extension />, document.body);
};

function Extension() {
  const translate = useTranslate();
  const customer = useAuthenticatedAccountCustomer();
  return (
    <s-section heading={translate('profileHeading')}>
      <s-stack gap="base">
        <s-text>{translate('customer', {id: customer?.id.split('/').pop() ?? ''})}</s-text>
        <s-button onClick={() => shopify.toast.show(translate('saved'))}>{translate('save')}</s-button>
      </s-stack>
    </s-section>
  );
}
