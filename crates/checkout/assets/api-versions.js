// The Shopify API releases of checkout UI extensions the local checkout implements: exactly
// these, nothing older (2025-07 was the last one built on React and remote-ui). To support
// another release, add it here and, if it changes something, read the change in `refusalOf`
// or in the sandbox; `crates/checkout/docs/ui-api-versions.md` says what each release changed.

export const API_VERSIONS = {
  '2026-10': { metafieldOwnerType: 'optional' },
  // A release candidate until 2027-01-01: the API of 2026-10, but declaring a metafield
  // without `owner_type` blocks the deployment.
  '2027-01': { metafieldOwnerType: 'required', candidate: true },
};

/** The resources a metafield declaration (`owner_type`) can name. */
export const OWNER_TYPES = [
  'CART', 'COMPANY', 'COMPANY_LOCATION', 'CUSTOMER', 'PRODUCT', 'PRODUCTVARIANT', 'SHOP',
];

const named = (list) => list.map((entry) => `${entry.namespace}.${entry.key}`).join(', ');

/**
 * Why the local checkout does not load an extension, or null when it does. `ext` is an entry of
 * the state's `extensions`: `apiVersion` and the metafields it declares (`ownerType` is the
 * `owner_type` of the toml).
 */
export function refusalOf(ext) {
  const api = API_VERSIONS[ext.apiVersion];
  if (!api) {
    return `API version ${ext.apiVersion ?? '(none)'} is not supported: the local checkout runs ${Object.keys(API_VERSIONS).join(' and ')}.`;
  }
  const declared = ext.metafields ?? [];
  const unknown = declared.filter((entry) => entry.ownerType && !OWNER_TYPES.includes(entry.ownerType));
  if (unknown.length > 0) {
    return `owner_type of ${named(unknown)} must be one of ${OWNER_TYPES.join(', ')}.`;
  }
  const missing = declared.filter((entry) => !entry.ownerType);
  if (api.metafieldOwnerType === 'required' && missing.length > 0) {
    return `API version ${ext.apiVersion} requires owner_type on every metafield declaration: ${named(missing)} has none.`;
  }
  return null;
}
