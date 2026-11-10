// `Shopify.loadFeatures`, for the local server.
//
// On Shopify this script downloads the features a theme asks for. Locally, the Customer
// Privacy API (`consent-tracking-api`) is provided by this file; every other feature is
// hosted by Shopify and reported as unavailable, which themes are written to tolerate.
(() => {
  const Shopify = (window.Shopify ??= {});

  // --- The Customer Privacy API -------------------------------------------------------

  // Where consent has to be collected before tracking: the countries of the EEA, the United
  // Kingdom and Switzerland. A real store decides this in its privacy settings.
  const CONSENT_REQUIRED = new Set([
    'AT', 'BE', 'BG', 'HR', 'CY', 'CZ', 'DK', 'EE', 'FI', 'FR', 'DE', 'GR', 'HU', 'IS', 'IE',
    'IT', 'LV', 'LI', 'LT', 'LU', 'MT', 'NL', 'NO', 'PL', 'PT', 'RO', 'SK', 'SI', 'ES', 'SE',
    'GB', 'CH',
  ]);
  const PURPOSES = ['analytics', 'marketing', 'preferences', 'sale_of_data'];
  const STORAGE_KEY = '_tracking_consent';

  let memory = null;
  const stored = () => {
    try {
      return JSON.parse(window.localStorage.getItem(STORAGE_KEY)) ?? memory;
    } catch {
      return memory;
    }
  };
  const store = (consent) => {
    memory = consent;
    try {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(consent));
    } catch {
      // Storage may be disabled: the choice then lasts for the page.
    }
  };

  const consentRequired = () => CONSENT_REQUIRED.has(String(Shopify.country ?? '').toUpperCase());

  /** `'yes'`, `'no'`, or `''` when the visitor has not answered. */
  const answer = (purpose) => stored()?.[purpose] ?? '';

  const allowed = (purpose) => {
    const given = answer(purpose);
    if (given) return given === 'yes';
    return !consentRequired();
  };

  const customerPrivacy = {
    currentVisitorConsent: () => Object.fromEntries(PURPOSES.map((purpose) => [purpose, answer(purpose)])),
    analyticsProcessingAllowed: () => allowed('analytics'),
    marketingAllowed: () => allowed('marketing'),
    preferencesProcessingAllowed: () => allowed('preferences'),
    saleOfDataAllowed: () => allowed('sale_of_data'),
    firstPartyMarketingAllowed: () => allowed('marketing'),
    thirdPartyMarketingAllowed: () => allowed('marketing') && allowed('sale_of_data'),
    userCanBeTracked: () => allowed('analytics') && allowed('marketing'),
    userDataCanBeSold: () => allowed('sale_of_data'),
    getTrackingConsent: () => {
      if (!stored()) return 'no_interaction';
      return allowed('analytics') && allowed('marketing') ? 'yes' : 'no';
    },
    getRegion: () => String(Shopify.country ?? ''),
    isRegulationEnforced: () => consentRequired(),
    doesMerchantSupportGranularConsent: () => true,
    saleOfDataRegion: () => false,
    shouldShowBanner: () => consentRequired() && !stored(),
    shouldShowGDPRBanner: () => consentRequired() && !stored(),
    shouldShowCCPABanner: () => false,
    setTrackingConsent(consent, callback) {
      // Shopify's older signature takes a boolean for everything at once.
      const choices =
        typeof consent === 'boolean'
          ? Object.fromEntries(PURPOSES.map((purpose) => [purpose, consent]))
          : (consent ?? {});
      const next = { ...stored() };
      for (const purpose of PURPOSES) {
        if (typeof choices[purpose] === 'boolean') next[purpose] = choices[purpose] ? 'yes' : 'no';
      }
      store(next);
      document.dispatchEvent(
        new CustomEvent('visitorConsentCollected', {
          detail: {
            analyticsAllowed: allowed('analytics'),
            marketingAllowed: allowed('marketing'),
            preferencesAllowed: allowed('preferences'),
            saleOfDataAllowed: allowed('sale_of_data'),
            firstPartyMarketingAllowed: allowed('marketing'),
            thirdPartyMarketingAllowed: allowed('marketing') && allowed('sale_of_data'),
          },
        }),
      );
      if (typeof callback === 'function') callback();
    },
  };

  // --- Loading features ---------------------------------------------------------------

  const FEATURES = {
    'consent-tracking-api': () => {
      Shopify.customerPrivacy ??= customerPrivacy;
    },
  };

  /** The value Shopify passes to `onLoad` and to the callback when features failed. */
  const unavailable = (name) => [
    { message: `The feature "${name}" is hosted by Shopify and is not available locally.` },
  ];

  const loadFeatures = (features, callback) => {
    const errors = [];
    for (const feature of Array.isArray(features) ? features : []) {
      const install = FEATURES[feature?.name];
      const error = install ? undefined : unavailable(feature?.name);
      if (install) install();
      else errors.push(...error);
      if (typeof feature?.onLoad === 'function') {
        // Asynchronously, as a download would be.
        queueMicrotask(() => feature.onLoad(error));
      }
    }
    if (typeof callback === 'function') {
      queueMicrotask(() => callback(errors.length ? errors : undefined));
    }
  };

  // What was asked for before this script ran sits in the queues of the inline bootstrap.
  const queued = [...(Shopify.loadFeatures?.q ?? []), ...(Shopify.autoloadFeatures?.q ?? [])];
  Shopify.loadFeatures = loadFeatures;
  Shopify.autoloadFeatures = loadFeatures;
  for (const args of queued) loadFeatures(...args);
})();
