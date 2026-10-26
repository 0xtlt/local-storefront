// The host of legacy checkout UI extensions (API versions before 2025-10, React bundles built
// with `@shopify/ui-extensions-react`). Such a bundle calls `shopify.extend(target, fn)`; the
// function is given `({channel, components}, api)` and builds a remote-ui tree whose changes
// it sends down `channel`:
//
//   channel(0, children)                            mount the first tree
//   channel(1, parentId, index, child, oldParentId) insert a node, or move it (oldParentId)
//   channel(2, parentId, index)                     remove the node at index
//   channel(3, id, text)                            change a text
//   channel(4, id, props)                           change props (functions arrive as they are)
//
// A node is `{id, kind: 1, type, props, children}` (a component) or `{id, kind: 2, text}`.
// A prop holding `{id, kind: 3, children}` is a fragment: a tree in a prop (`overlay`,
// `primaryAction`, …) that later changes come to as inserts into the fragment's id.
//
// This file turns the tree into the same `s-*` elements the current API uses. A classic script.
(function (global) {
  'use strict';
  const Sandbox = (global.LsfSandbox = global.LsfSandbox || {});

  const ACTION_MOUNT = 0;
  const ACTION_INSERT_CHILD = 1;
  const ACTION_REMOVE_CHILD = 2;
  const ACTION_UPDATE_TEXT = 3;
  const ACTION_UPDATE_PROPS = 4;
  const KIND_TEXT = 2;
  const KIND_FRAGMENT = 3;
  const ROOT_ID = '~';

  // --- Props of legacy components → attributes of the elements -----------------------------

  const SPACING = { none: 'none', extraTight: 'small-300', tight: 'small-200', base: 'base', loose: 'large-100', extraLoose: 'large-300' };
  const SIZES = { extraSmall: 'small-200', small: 'small', base: 'base', medium: 'base', large: 'large', extraLarge: 'large-200', fill: 'fill' };
  const ALIGN = { start: 'flex-start', center: 'center', end: 'flex-end', baseline: 'baseline', stretch: 'stretch', spaceBetween: 'space-between', spaceAround: 'space-around', spaceEvenly: 'space-evenly' };

  /** A legacy value may be conditional (`{conditions, defaultValue}`): take the default. */
  const plain = (value) => (value !== null && typeof value === 'object' && !Array.isArray(value) && 'defaultValue' in value ? value.defaultValue : value);
  const space = (value) => {
    const v = plain(value);
    if (v === undefined || v === null) return undefined;
    return (Array.isArray(v) ? v : [v]).map((token) => SPACING[token] || token).join(' ');
  };
  const align = (value) => (plain(value) === undefined ? undefined : ALIGN[plain(value)] || plain(value));
  const sizeOf = (value) => {
    const v = plain(value);
    return typeof v === 'number' ? v + 'px' : v === undefined ? undefined : SIZES[v] || v;
  };
  const columns = (value) => {
    const v = plain(value);
    if (v === undefined) return undefined;
    return (Array.isArray(v) ? v : [v])
      .map((column) => (column === 'fill' ? '1fr' : column === 'auto' ? 'auto' : typeof column === 'number' ? column + 'px' : column))
      .join(' ');
  };
  const TONES = { critical: 'critical', success: 'success', warning: 'warning', info: 'info', accent: 'accent' };
  const kebab = (name) => name.replace(/[A-Z]/g, (letter) => '-' + letter.toLowerCase());

  /** The padding, border, size and background that views and pressables share. */
  function box(p) {
    const border = plain(p.border);
    return {
      padding: space(p.padding),
      background: p.background === 'transparent' ? undefined : plain(p.background),
      border: border === undefined ? undefined : Array.isArray(border) ? border.join(' ') : border,
      borderRadius: p.cornerRadius === undefined ? undefined : plain(p.cornerRadius) === 'base' ? 'base' : plain(p.cornerRadius),
      inlineSize: sizeOf(p.inlineSize),
      minInlineSize: sizeOf(p.minInlineSize),
      maxInlineSize: sizeOf(p.maxInlineSize),
      blockSize: sizeOf(p.blockSize),
      minBlockSize: sizeOf(p.minBlockSize),
      maxBlockSize: sizeOf(p.maxBlockSize),
      overflow: p.overflow,
      display: plain(p.display),
    };
  }

  const textTone = (p) => ({
    color: p.appearance === 'subdued' ? 'subdued' : undefined,
    tone: TONES[p.appearance],
    type: p.emphasis === 'bold' ? 'strong' : p.emphasis === 'italic' ? 'emphasis' : undefined,
    size: p.size,
  });

  /**
   * One entry per legacy component. `tag` is the element (or a function of the first props, for
   * the components that become different elements); `map` turns the props into attributes
   * (`undefined` removes one), `style` into inline CSS, `props` into element properties;
   * `events` names the DOM event behind each callback prop and what the callback receives;
   * `slots` sends a fragment prop to a named slot of the element.
   */
  const LEGACY = {
    BlockStack: {
      tag: 's-stack',
      map: (p) => ({ direction: 'block', gap: space(p.spacing), alignItems: align(p.inlineAlignment), justifyContent: align(p.blockAlignment), ...box(p) }),
    },
    InlineStack: {
      tag: 's-stack',
      map: (p) => ({ direction: 'inline', gap: space(p.spacing), alignItems: align(p.blockAlignment), justifyContent: align(p.inlineAlignment), ...box(p) }),
      style: () => ({ flexWrap: 'wrap' }),
    },
    View: { tag: 's-box', map: (p) => box(p), style: (p) => ({ textAlign: p.inlineAlignment }) },
    ScrollView: { tag: 's-scroll-box', map: (p) => ({ ...box(p), overflow: p.direction === 'inline' ? 'auto hidden' : 'hidden auto' }) },
    Grid: {
      tag: 's-grid',
      map: (p) => ({ gridTemplateColumns: columns(p.columns), gridTemplateRows: columns(p.rows), gap: space(p.spacing), alignItems: align(p.blockAlignment), justifyItems: align(p.inlineAlignment), ...box(p) }),
    },
    GridItem: { tag: 's-grid-item', map: (p) => ({ gridColumn: p.columnSpan ? 'span ' + plain(p.columnSpan) : undefined, gridRow: p.rowSpan ? 'span ' + plain(p.rowSpan) : undefined, ...box(p) }) },
    InlineLayout: { tag: 's-grid', map: (p) => ({ gridTemplateColumns: columns(p.columns), gap: space(p.spacing), alignItems: align(p.blockAlignment), ...box(p) }) },
    BlockLayout: { tag: 's-grid', map: (p) => ({ gridTemplateRows: columns(p.rows), gap: space(p.spacing), ...box(p) }) },
    BlockSpacer: { tag: 's-box', map: (p) => ({ blockSize: space(p.spacing) === 'none' ? '0' : space(p.spacing) || 'base' }) },
    InlineSpacer: { tag: 's-box', map: (p) => ({ inlineSize: space(p.spacing) === 'none' ? '0' : space(p.spacing) || 'base' }), style: () => ({ display: 'inline-block' }) },
    Text: { tag: 's-text', map: textTone },
    TextBlock: { tag: 's-paragraph', map: textTone, style: (p) => ({ textAlign: p.inlineAlignment === 'center' ? 'center' : p.inlineAlignment === 'end' ? 'end' : undefined }) },
    Heading: { tag: 's-heading', map: (p) => ({ level: p.level }), style: (p) => ({ textAlign: p.inlineAlignment === 'center' ? 'center' : undefined }) },
    HeadingGroup: { tag: 's-box', map: () => ({}) },
    Banner: {
      tag: 's-banner',
      map: (p) => ({ heading: p.title, tone: { info: 'info', success: 'success', warning: 'warning', critical: 'critical' }[p.status] || 'info', collapsible: p.collapsible ? '' : undefined, dismissible: p.onDismiss ? '' : undefined }),
      events: { onDismiss: ['dismiss'] },
    },
    Announcement: { tag: 's-announcement', map: () => ({}), events: { onDismiss: ['dismiss'] } },
    Badge: { tag: 's-badge', map: (p) => ({ tone: { critical: 'critical', subdued: 'neutral' }[p.tone], size: p.size }) },
    Tag: { tag: 's-clickable-chip', map: (p) => ({ removable: p.onRemove ? '' : undefined }), events: { onRemove: ['remove'] } },
    Button: {
      tag: 's-button',
      map: (p) => ({
        variant: p.kind === 'primary' ? 'primary' : p.kind === 'secondary' ? 'secondary' : undefined,
        plain: p.kind === 'plain' ? '' : undefined,
        tone: p.appearance === 'critical' ? 'critical' : undefined,
        type: p.submit ? 'submit' : undefined,
        href: p.to,
        loading: p.loading ? '' : undefined,
        disabled: p.disabled ? '' : undefined,
        accessibilityLabel: p.accessibilityLabel,
        inlineSize: p.inlineAlignment === undefined ? undefined : 'fill',
      }),
      events: { onPress: ['click'] },
    },
    Link: {
      tag: 's-link',
      map: (p) => ({ href: p.to, target: p.external ? '_blank' : undefined, accessibilityLabel: p.accessibilityLabel }),
      events: { onPress: ['click'] },
    },
    Pressable: {
      tag: 's-clickable',
      map: (p) => ({ ...box(p), href: p.to, disabled: p.disabled ? '' : undefined, loading: p.loading ? '' : undefined, accessibilityLabel: p.accessibilityLabel, 'data-toggles': p.toggles }),
      events: { onPress: ['click'], onFocus: ['focus'], onBlur: ['blur'], onPointerEnter: ['pointerenter'], onPointerLeave: ['pointerleave'], onPointerDown: ['pointerdown'], onPointerUp: ['pointerup'] },
    },
    Image: {
      tag: 's-image',
      map: (p) => ({ src: typeof plain(p.source) === 'string' ? plain(p.source) : (p.source && p.source.default) || undefined, alt: p.accessibilityDescription, aspectRatio: p.aspectRatio, objectFit: plain(p.fit), loading: p.loading, accessibilityRole: p.accessibilityRole === 'decorative' ? 'presentation' : undefined }),
    },
    Icon: {
      tag: 's-icon',
      map: (p) => ({ type: { checkmark: 'check', close: 'x', critical: 'alert-circle', error: 'alert-circle', errorFill: 'alert-circle', infoFill: 'info', giftFill: 'gift', magnify: 'search' }[p.source] || kebab(String(p.source || '')), tone: TONES[p.appearance] }),
    },
    Divider: { tag: 's-divider', map: (p) => ({ direction: p.direction === 'block' ? 'inline' : undefined }) },
    Spinner: { tag: 's-spinner', map: (p) => ({ size: p.size === 'extraSmall' ? 'small' : p.size, accessibilityLabel: p.accessibilityLabel }) },
    Progress: { tag: 's-progress', map: (p) => ({ value: p.value, max: p.max, tone: p.tone, accessibilityLabel: p.accessibilityLabel }) },
    SkeletonText: { tag: 's-skeleton-paragraph', map: () => ({}), style: (p) => ({ width: p.inlineSize === 'small' ? '30%' : p.inlineSize === 'large' ? '100%' : '60%', display: 'inline-block' }) },
    SkeletonTextBlock: { tag: 's-skeleton-paragraph', map: () => ({}), style: (p) => ({ height: (p.lines || 3) * 1.25 + 'em' }) },
    SkeletonImage: { tag: 's-box', map: (p) => ({ background: 'strong', inlineSize: sizeOf(p.inlineSize) || 'fill', blockSize: sizeOf(p.blockSize), borderRadius: 'base' }), style: (p) => ({ aspectRatio: p.aspectRatio }) },
    List: {
      tag: (p) => (p.marker === 'number' ? 's-ordered-list' : 's-unordered-list'),
      map: (p) => ({ gap: space(p.spacing) }),
      style: (p) => ({ listStyle: p.marker === 'none' ? 'none' : undefined, paddingInlineStart: p.marker === 'none' ? '0' : undefined }),
    },
    ListItem: { tag: 's-list-item', map: () => ({}) },
    Form: { tag: 's-form', map: () => ({}), events: { onSubmit: ['submit'] } },
    TextField: {
      tag: (p) => (p.multiline ? 's-text-area' : { email: 's-email-field', telephone: 's-phone-field', number: 's-number-field' }[p.type] || 's-text-field'),
      map: fieldAttributes,
      props: (p) => ({ value: p.controlledValue !== undefined ? p.controlledValue : p.value }),
      events: fieldEvents(),
      slots: { accessory: 'accessory' },
    },
    PhoneField: { tag: 's-phone-field', map: fieldAttributes, props: (p) => ({ value: p.value }), events: fieldEvents() },
    ConsentPhoneField: { tag: 's-consent-phone-field', map: fieldAttributes, props: (p) => ({ value: p.value }), events: fieldEvents() },
    DateField: { tag: 's-date-field', map: fieldAttributes, props: (p) => ({ value: p.value }), events: fieldEvents() },
    Stepper: {
      tag: 's-number-field',
      map: (p) => ({ ...fieldAttributes(p), min: p.min, max: p.max, step: p.step }),
      props: (p) => ({ value: p.value === undefined ? '' : p.value }),
      events: { onChange: ['change', (el) => (el.value === '' ? undefined : Number(el.value))], onInput: ['input', (el) => (el.value === '' ? undefined : Number(el.value))], onFocus: ['focus'], onBlur: ['blur'] },
    },
    DatePicker: { tag: 's-date-picker', map: (p) => ({ disabled: p.disabled ? '' : undefined }), props: (p) => ({ value: p.selected || p.value }), events: { onChange: ['change', (el) => el.value] } },
    Checkbox: {
      tag: 's-checkbox',
      map: (p) => ({ name: p.name, disabled: p.disabled ? '' : undefined, error: p.error, accessibilityLabel: p.accessibilityLabel, id: p.id }),
      props: (p) => ({ checked: Boolean(p.checked !== undefined ? p.checked : p.value) }),
      events: { onChange: ['change', (el) => el.checked] },
    },
    ConsentCheckbox: {
      tag: 's-consent-checkbox',
      map: (p) => ({ name: p.name, disabled: p.disabled ? '' : undefined, error: p.error }),
      props: (p) => ({ checked: Boolean(p.checked !== undefined ? p.checked : p.value) }),
      events: { onChange: ['change', (el) => el.checked] },
    },
    Switch: {
      tag: 's-switch',
      map: (p) => ({ name: p.name, label: p.label, disabled: p.disabled ? '' : undefined, accessibilityLabel: p.accessibilityLabel }),
      props: (p) => ({ checked: Boolean(p.checked !== undefined ? p.checked : p.value) }),
      events: { onChange: ['change', (el) => el.checked] },
    },
    ToggleButton: { tag: 's-press-button', map: (p) => ({ pressed: p.pressed ? '' : undefined, disabled: p.disabled ? '' : undefined }), events: { onChange: ['change'] } },
    ToggleButtonGroup: { tag: 's-stack', map: () => ({ direction: 'inline', gap: 'small-200' }) },
    Select: {
      tag: 's-select',
      map: (p) => ({ label: p.label, name: p.name, placeholder: p.placeholder, disabled: p.disabled ? '' : undefined, required: p.required ? '' : undefined, error: p.error, id: p.id }),
      props: (p) => ({ value: p.value }),
      events: { onChange: ['change', (el) => el.value], onFocus: ['focus'], onBlur: ['blur'] },
      after: selectOptions,
    },
    ChoiceList: {
      tag: 's-choice-list',
      map: (p) => ({ name: p.name, multiple: Array.isArray(p.value) ? '' : undefined, label: p.title }),
      events: { onChange: ['change', (el, node) => (Array.isArray(node.props.value) ? el.values : el.values[0]), true] },
      after: (node) => {
        const value = node.props.value;
        node.el.values = Array.isArray(value) ? value : value === undefined ? [] : [value];
      },
    },
    Choice: {
      tag: 's-choice',
      map: (p) => ({ value: p.id, disabled: p.disabled ? '' : undefined, error: p.error, accessibilityLabel: p.accessibilityLabel }),
      slots: { details: 'details' },
    },
    Disclosure: {
      tag: 's-box',
      map: () => ({}),
      events: {},
      after: (node, host) => {
        disclosure(node, host);
        node.refresh();
      },
    },
    Modal: {
      tag: 's-modal',
      map: (p) => ({ heading: p.title, size: p.size, padding: p.padding === false ? 'none' : undefined, accessibilityLabel: p.accessibilityLabel, id: p.id }),
      events: { onClose: ['hide'], onOpen: ['show'] },
      slots: { primaryAction: 'primary-action', secondaryActions: 'secondary-actions' },
    },
    Popover: { tag: 's-popover', map: (p) => ({ id: p.id }), events: { onClose: ['hide'], onOpen: ['show'] } },
    Sheet: { tag: 's-sheet', map: (p) => ({ heading: p.accessibilityLabel, id: p.id }), events: { onClose: ['hide'] } },
    Tooltip: { tag: 's-tooltip', map: (p) => ({ id: p.id }) },
    PaymentIcon: { tag: 's-payment-icon', map: (p) => ({ name: p.name }) },
    ProductThumbnail: { tag: 's-product-thumbnail', map: (p) => ({ src: p.source, alt: p.accessibilityLabel, size: p.size, totalItems: p.badge }) },
    QRCode: { tag: 's-qr-code', map: (p) => ({ content: p.content, size: p.size }) },
    ClipboardItem: { tag: 's-clipboard-item', map: (p) => ({ text: p.text, id: p.id }) },
    DropZone: { tag: 's-drop-zone', map: (p) => ({ label: p.label, accept: p.accept, multiple: p.allowMultiple ? '' : undefined, disabled: p.disabled ? '' : undefined }), events: { onInput: ['change', (el) => el.files] } },
    Map: { tag: 's-map', map: () => ({}) },
    MapMarker: { tag: 's-map-marker', map: () => ({}) },
    MapPopover: { tag: 's-box', map: () => ({}) },
    Chat: { tag: 's-chat', map: () => ({}) },
    LoginWithShop: { tag: 's-box', map: () => ({}) },
  };

  function fieldAttributes(p) {
    return {
      label: p.label,
      name: p.name,
      required: p.required ? '' : undefined,
      disabled: p.disabled ? '' : undefined,
      readonly: p.readonly ? '' : undefined,
      error: p.error,
      details: p.accessibilityDescription,
      maxLength: p.maxLength,
      rows: typeof p.multiline === 'number' ? p.multiline : undefined,
      autocomplete: typeof p.autocomplete === 'string' ? p.autocomplete : p.autocomplete === false ? 'off' : undefined,
      id: p.id,
    };
  }

  function fieldEvents() {
    return {
      onChange: ['change', (el) => el.value],
      onInput: ['input', (el) => el.value],
      onFocus: ['focus'],
      onBlur: ['blur'],
    };
  }

  /** `Select` takes its options as a prop: they become `s-option` children. */
  function selectOptions(node, host) {
    const options = (node.props.options || []).slice();
    if (node.props.placeholder) options.unshift({ value: '', label: node.props.placeholder, disabled: true });
    for (const child of [...node.el.querySelectorAll(':scope > s-option')]) child.remove();
    for (const option of options) {
      const element = host.document.createElement('s-option');
      element.setAttribute('value', option.value);
      if (option.disabled) element.setAttribute('disabled', '');
      element.textContent = option.label;
      node.el.appendChild(element);
    }
    node.el.value = node.props.value === undefined ? '' : node.props.value;
  }

  /** `Disclosure`: the pressables with `toggles="id"` show and hide the element with that id. */
  function disclosure(node, host) {
    if (node.refresh) return;
    const open = () => {
      const listed = node.props.open !== undefined ? node.props.open : node.props.defaultOpen;
      return [].concat(listed === undefined ? [] : listed);
    };
    node.refresh = () => {
      const ids = new Set(open());
      for (const toggle of node.el.querySelectorAll('[data-toggles]')) {
        const id = toggle.getAttribute('data-toggles');
        const target = node.el.querySelector('[id="' + id + '"]');
        if (target) target.hidden = !ids.has(id);
      }
    };
    host.refreshers.add(node.refresh);
    node.el.addEventListener('click', (event) => {
      const toggle = event.target.closest && event.target.closest('[data-toggles]');
      if (!toggle || !node.el.contains(toggle)) return;
      const id = toggle.getAttribute('data-toggles');
      const ids = new Set(open());
      if (ids.has(id)) ids.delete(id);
      else ids.add(id);
      // A controlled disclosure waits for its owner to say so; the other one keeps count.
      if (node.props.open === undefined) node.props.defaultOpen = [...ids];
      if (node.props.onToggle) node.props.onToggle([...ids]);
      node.refresh();
    });
  }

  const UNSUPPORTED = { tag: 's-box', map: () => ({}) };

  // --- The host --------------------------------------------------------------------------------

  /**
   * @param {object} options
   * @param {Document} options.document
   * @param {Element} options.container     where the tree goes
   * @param {(message: string) => void} options.warn
   * @returns {{channel: Function, components: string[]}} what `shopify.extend` hands to the
   *   extension as its connection
   */
  function createLegacyHost({ document, container, warn }) {
    const nodes = new Map();
    const root = { id: ROOT_ID, kind: 'root', el: container, children: [] };
    nodes.set(ROOT_ID, root);
    const host = { document, refreshers: new Set() };

    const nodeOf = (id) => {
      const node = nodes.get(id);
      if (!node) throw new Error('remote-ui: no node ' + id);
      return node;
    };

    /** Where a child goes in the DOM of `parent`: before the element of the next child, else
     * before the first fragment container (they stay last), else at the end. */
    function reference(parent, index) {
      const next = parent.children[index + 1];
      if (next !== undefined) return nodes.get(next).el;
      if (parent.fragments) {
        for (const key of Object.keys(parent.fragments)) return parent.fragments[key].el;
      }
      return null;
    }

    function build(data) {
      if (data.kind === KIND_TEXT) {
        const node = { id: data.id, kind: 'text', el: document.createTextNode(data.text) };
        nodes.set(data.id, node);
        return node;
      }
      const spec = LEGACY[data.type] || UNSUPPORTED;
      if (spec === UNSUPPORTED) warn('The component ' + data.type + ' is not supported by the local checkout: it renders as a box.');
      const props = data.props || {};
      const tag = typeof spec.tag === 'function' ? spec.tag(props) : spec.tag;
      const node = {
        id: data.id, kind: 'component', type: data.type, spec, el: document.createElement(tag), children: [],
        props: {}, attributes: {}, styles: {}, listeners: {}, installed: {}, fragments: {},
      };
      nodes.set(data.id, node);
      if (spec === UNSUPPORTED) node.el.setAttribute('data-unsupported', data.type);
      change(node, props);
      for (const child of data.children || []) {
        const built = build(child);
        node.children.push(child.id);
        node.el.insertBefore(built.el, reference(node, node.children.length - 1));
      }
      settle(node);
      return node;
    }

    function fragmentRoot(node, key, fragment) {
      const old = node.fragments[key];
      if (old && old.id === fragment.id) return old;
      if (old) dropFragment(node, key);
      const el = document.createElement('span');
      el.style.display = 'contents';
      el.setAttribute('slot', (node.spec.slots && node.spec.slots[key]) || 'fragments');
      const made = { id: fragment.id, kind: 'fragment', el, children: [], key };
      nodes.set(fragment.id, made);
      node.fragments[key] = made;
      node.el.appendChild(el);
      for (const child of fragment.children || []) {
        const built = build(child);
        made.children.push(child.id);
        el.appendChild(built.el);
      }
      if (key === 'overlay') overlayTrigger(node, made);
      return made;
    }

    function dropFragment(node, key) {
      const made = node.fragments[key];
      if (!made) return;
      made.el.remove();
      forget(made);
      delete node.fragments[key];
    }

    /** A button with an `overlay` opens the modal or popover in it; a tooltip shows on hover. */
    function overlayTrigger(node, fragment) {
      if (node.overlayInstalled) return;
      node.overlayInstalled = true;
      node.el.addEventListener('click', (event) => {
        if (fragment.el.contains(event.target)) return;
        const overlay = fragment.el.querySelector('s-modal, s-popover, s-sheet');
        if (overlay && overlay.showOverlay) overlay.showOverlay();
      });
      node.el.addEventListener('mouseenter', () => {
        const tip = fragment.el.querySelector('s-tooltip');
        if (tip && !node.el.title) node.el.title = tip.textContent.trim();
      });
    }

    /** Applies props: fragments first, then what the element shows, properties and callbacks. */
    function change(node, props) {
      const spec = node.spec;
      for (const key of Object.keys(props)) {
        const value = props[key];
        if (value !== null && typeof value === 'object' && value.kind === KIND_FRAGMENT) {
          fragmentRoot(node, key, value);
        } else {
          if (node.fragments[key]) dropFragment(node, key);
          node.props[key] = value;
          if (typeof value === 'string' && spec.slots && spec.slots[key] && !node.fragments[key]) textSlot(node, key, value);
        }
      }
      const attributes = { id: node.props.id, ...spec.map(node.props, node) };
      for (const name of new Set([...Object.keys(node.attributes), ...Object.keys(attributes)])) {
        const value = attributes[name];
        const attribute = name.toLowerCase();
        if (value === undefined || value === null || value === false) node.el.removeAttribute(attribute);
        else node.el.setAttribute(attribute, value === true ? '' : String(value));
      }
      node.attributes = attributes;
      const styles = spec.style ? spec.style(node.props, node) : {};
      for (const name of new Set([...Object.keys(node.styles), ...Object.keys(styles)])) {
        node.el.style[name] = styles[name] === undefined ? '' : styles[name];
      }
      node.styles = styles;
      if (spec.props) {
        const properties = spec.props(node.props);
        for (const name of Object.keys(properties)) {
          if (node.el[name] !== properties[name]) node.el[name] = properties[name];
        }
      }
      for (const prop of Object.keys(spec.events || {})) {
        if (typeof node.props[prop] === 'function') node.listeners[prop] = node.props[prop];
        else delete node.listeners[prop];
        if (node.installed[prop]) continue;
        node.installed[prop] = true;
        const [type, adapt, fromDescendants] = spec.events[prop];
        node.el.addEventListener(type, (event) => {
          const callback = node.listeners[prop];
          if (!callback) return;
          // Events of what is inside (a field in a modal) are not this component's.
          if (!fromDescendants && ['change', 'input', 'hide', 'show', 'dismiss', 'remove'].includes(type) && event.target !== node.el) return;
          try {
            callback(adapt ? adapt(node.el, node, event) : undefined);
          } catch (error) {
            warn('A callback of ' + node.type + ' threw: ' + (error && error.message));
          }
        });
      }
    }

    /** A string for a slot prop (`details="…"`) is text in that slot. */
    function textSlot(node, key, text) {
      const name = node.spec.slots[key];
      let element = node.el.querySelector(':scope > [data-text-slot="' + name + '"]');
      if (!element) {
        element = document.createElement('span');
        element.setAttribute('slot', name);
        element.setAttribute('data-text-slot', name);
        node.el.appendChild(element);
      }
      element.textContent = text;
    }

    function settle(node) {
      if (node.spec.after) node.spec.after(node, host);
    }

    /** Drops a node and everything below it. */
    function forget(node) {
      nodes.delete(node.id);
      if (node.refresh) host.refreshers.delete(node.refresh);
      for (const id of node.children || []) if (nodes.has(id)) forget(nodes.get(id));
      for (const key of Object.keys(node.fragments || {})) forget(node.fragments[key]);
    }

    function detach(parent, id) {
      const at = parent.children.indexOf(id);
      if (at >= 0) parent.children.splice(at, 1);
      return at;
    }

    function channel(action, ...args) {
      switch (action) {
        case ACTION_MOUNT: {
          const [children] = args;
          root.children = [];
          for (const data of children) {
            const built = build(data);
            root.children.push(data.id);
            container.appendChild(built.el);
          }
          break;
        }
        case ACTION_INSERT_CHILD: {
          const [parentId, index, data, oldParentId] = args;
          const parent = nodeOf(parentId);
          let built;
          if (oldParentId !== false && oldParentId !== undefined && nodes.has(data.id)) {
            built = nodes.get(data.id);
            detach(nodeOf(oldParentId), data.id);
            if (built.el.parentNode) built.el.parentNode.removeChild(built.el);
          } else {
            built = build(data);
          }
          parent.children.splice(index, 0, data.id);
          parent.el.insertBefore(built.el, reference(parent, index));
          if (parent.kind === 'component') settle(parent);
          break;
        }
        case ACTION_REMOVE_CHILD: {
          const [parentId, index] = args;
          const parent = nodeOf(parentId);
          const id = parent.children[index];
          const child = nodes.get(id);
          parent.children.splice(index, 1);
          if (child) {
            if (child.el.parentNode) child.el.parentNode.removeChild(child.el);
            forget(child);
          }
          if (parent.kind === 'component') settle(parent);
          break;
        }
        case ACTION_UPDATE_TEXT: {
          const [id, text] = args;
          nodeOf(id).el.nodeValue = text;
          break;
        }
        case ACTION_UPDATE_PROPS: {
          const [id, props] = args;
          const node = nodeOf(id);
          change(node, props);
          settle(node);
          break;
        }
        default:
          throw new Error('remote-ui: unknown action ' + action);
      }
      // A `Disclosure` looks at its descendants once a batch landed.
      for (const refresh of host.refreshers) refresh();
      return undefined;
    }

    return { channel, components: Object.keys(LEGACY), nodes };
  }

  Sandbox.createLegacyHost = createLegacyHost;
  Sandbox.legacyComponents = LEGACY;
})(typeof globalThis !== 'undefined' ? globalThis : this);
