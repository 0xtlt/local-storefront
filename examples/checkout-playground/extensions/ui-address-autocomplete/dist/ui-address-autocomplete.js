// ui-address-autocomplete · API 2026-10 · the two targets of the address autocomplete, which
// render nothing: the checkout runs them as the buyer types the address field.
//
// - `purchase.address-autocomplete.suggest` answers the addresses of ADDRESSES whose street
//   contains what was typed, in the country selected first, with the part that matches;
// - the one of Seattle comes without its address: picking it runs
//   `purchase.address-autocomplete.format-suggestion`, which makes it from its id.
(() => {
  const ADDRESSES = [
    { id: 'pdx', address1: '815 Harbor Way', city: 'Portland', provinceCode: 'OR', zip: '97209', countryCode: 'US' },
    { id: 'sea', address1: '1201 Harbor Avenue', city: 'Seattle', provinceCode: 'WA', zip: '98101', countryCode: 'US' },
    { id: 'nyc', address1: '350 Fifth Avenue', city: 'New York', provinceCode: 'NY', zip: '10118', countryCode: 'US' },
    { id: 'lyon', address1: '12 Rue de la République', city: 'Lyon', zip: '69002', countryCode: 'FR' },
    { id: 'paris', address1: '8 Avenue des Champs-Élysées', city: 'Paris', zip: '75008', countryCode: 'FR' },
  ];
  // Seattle's address comes from format-suggestion.
  const WITHOUT_ADDRESS = new Set(['sea']);
  const label = (entry) => [entry.address1, entry.city, entry.zip, entry.countryCode].join(', ');

  shopify.extend('purchase.address-autocomplete.suggest', async () => {
    const { value, selectedCountryCode } = shopify.target;
    const typed = String(value || '').trim().toLowerCase();
    const found = ADDRESSES
      .filter((entry) => entry.address1.toLowerCase().includes(typed))
      .sort((a, b) => (b.countryCode === selectedCountryCode) - (a.countryCode === selectedCountryCode));
    return {
      suggestions: found.map((entry) => {
        const { id, ...address } = entry;
        return {
          id,
          label: label(entry),
          matchedSubstrings: [{ offset: entry.address1.toLowerCase().indexOf(typed), length: typed.length }],
          formattedAddress: WITHOUT_ADDRESS.has(id) ? undefined : address,
        };
      }),
    };
  });

  shopify.extend('purchase.address-autocomplete.format-suggestion', async () => {
    const entry = ADDRESSES.find((candidate) => candidate.id === shopify.target.selectedSuggestion.id);
    const { id, ...address } = entry || {};
    return { formattedAddress: address };
  });
})();
