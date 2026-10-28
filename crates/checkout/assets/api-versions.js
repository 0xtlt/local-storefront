// The Shopify API releases of checkout UI extensions the local checkout implements: exactly
// these, nothing older (2025-07 was the last one built on React and remote-ui). They are in
// `api.json`, which lsf-checkout reads too. To support another release, add it there and, if it
// changes something, read the change in `refusalOf` or in the sandbox;
// `crates/checkout/docs/ui-api-versions.md` says what each release changed. 2027-01 is a release
// candidate until 2027-01-01: the API of 2026-10, but declaring a metafield without `owner_type`
// blocks the deployment.

import api from './api.json' with { type: 'json' };

/** Each release, with what it requires: `{'2026-10': {metafieldOwnerType: 'optional'}, ...}`. */
export const API_VERSIONS = Object.fromEntries(api.releases.map(({ version, ...rules }) => [version, rules]));

/** The resources a metafield declaration (`owner_type`) can name. */
export const OWNER_TYPES = api.ownerTypes;

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
