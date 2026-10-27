// The Polaris web components (`s-banner`, `s-stack`, `s-text-field`, …) that checkout UI
// extensions render, as custom elements. A classic script, like the other files of the
// sandbox (see `sandbox-api.js`).
//
// Layout and text components are styled from their attributes by a stylesheet of the
// document; the others have a shadow root. Properties mirror attributes the way Polaris
// does, so Preact's `el.gap = 'base'` and `setAttribute('gap', 'base')` end up the same.
(function (global) {
  'use strict';
  const Sandbox = (global.LsfSandbox = global.LsfSandbox || {});

  /** Every tag of `SUPPORTED_COMPONENTS` in `@shopify/ui-extensions` 2026.10, kebab-cased. */
  const TAGS = [
    'abbreviation', 'announcement', 'badge', 'banner', 'box', 'button', 'chat', 'checkbox', 'chip',
    'choice', 'choice-list', 'clickable', 'clickable-chip', 'clipboard-item', 'consent-checkbox',
    'consent-phone-field', 'date-field', 'date-picker', 'details', 'divider', 'drop-zone',
    'email-field', 'form', 'grid', 'grid-item', 'heading', 'icon', 'image', 'link', 'list-item',
    'map', 'map-marker', 'modal', 'money-field', 'number-field', 'option', 'ordered-list',
    'paragraph', 'password-field', 'payment-icon', 'phone-field', 'popover', 'press-button',
    'product-thumbnail', 'progress', 'query-container', 'qr-code', 'scroll-box', 'section',
    'select', 'sheet', 'skeleton-paragraph', 'spinner', 'stack', 'summary', 'switch', 'text',
    'text-area', 'text-field', 'time', 'tooltip', 'unordered-list', 'url-field',
  ].map((name) => 's-' + name);

  // --- Style from attributes --------------------------------------------------------------

  const SPACING = {
    none: '0', 'small-500': '0.125rem', 'small-400': '0.25rem', 'small-300': '0.375rem',
    'small-200': '0.5rem', 'small-100': '0.75rem', small: '0.875rem', base: '1rem', large: '1.25rem',
    'large-100': '1.5rem', 'large-200': '2rem', 'large-300': '2.5rem', 'large-400': '3rem',
    'large-500': '4rem',
  };

  /** `"base large"` → `"1rem 1.25rem"`: tokens become lengths, anything else is CSS already. */
  function spacing(value) {
    return String(value)
      .trim()
      .split(/\s+/)
      .map((token) => (token in SPACING ? SPACING[token] : token))
      .join(' ');
  }

  const BACKGROUNDS = { base: '#fff', subdued: '#f7f7f7', strong: '#ebebeb', transparent: 'transparent' };
  const BORDER_COLORS = { base: '#d4d4d4', subdued: '#e3e3e3', strong: '#8a8a8a' };
  const RADIUS = { none: '0', small: '4px', 'small-100': '4px', base: '8px', large: '12px', 'large-100': '16px', max: '999px' };

  function size(value) {
    return value === 'fill' ? '100%' : value;
  }

  /** `"large-100 dashed"` / `"base"` → `{width, style, color}`: the `border` shorthand. */
  function border(value) {
    const out = { width: '1px', style: 'solid', color: BORDER_COLORS.base };
    for (const token of String(value).trim().split(/\s+/)) {
      if (token === 'none') out.width = '0';
      else if (/^(solid|dashed|dotted|double)$/.test(token)) out.style = token;
      else if (token in BORDER_COLORS) out.color = BORDER_COLORS[token];
      else if (token === 'base') out.width = '1px';
      else if (token === 'large' || token === 'large-100') out.width = '2px';
      else if (token === 'large-200') out.width = '3px';
    }
    return out;
  }

  /** Attribute (lowercased by the HTML parser) → how it sets CSS. */
  const BOX = {
    padding: (v) => ({ padding: spacing(v) }),
    paddingblock: (v) => ({ 'padding-block': spacing(v) }),
    paddingblockstart: (v) => ({ 'padding-block-start': spacing(v) }),
    paddingblockend: (v) => ({ 'padding-block-end': spacing(v) }),
    paddinginline: (v) => ({ 'padding-inline': spacing(v) }),
    paddinginlinestart: (v) => ({ 'padding-inline-start': spacing(v) }),
    paddinginlineend: (v) => ({ 'padding-inline-end': spacing(v) }),
    background: (v) => ({ background: BACKGROUNDS[v] || v }),
    inlinesize: (v) => ({ 'inline-size': size(v) }),
    blocksize: (v) => ({ 'block-size': size(v) }),
    mininlinesize: (v) => ({ 'min-inline-size': size(v) }),
    minblocksize: (v) => ({ 'min-block-size': size(v) }),
    maxinlinesize: (v) => ({ 'max-inline-size': size(v) }),
    maxblocksize: (v) => ({ 'max-block-size': size(v) }),
    borderradius: (v) => ({ 'border-radius': String(v).split(/\s+/).map((t) => RADIUS[t] || t).join(' ') }),
    overflow: (v) => ({ overflow: v }),
    display: (v) => (v === 'auto' ? {} : { display: v }),
    gap: (v) => ({ gap: spacing(v) }),
    rowgap: (v) => ({ 'row-gap': spacing(v) }),
    columngap: (v) => ({ 'column-gap': spacing(v) }),
    alignitems: (v) => ({ 'align-items': v }),
    aligncontent: (v) => ({ 'align-content': v }),
    justifyitems: (v) => ({ 'justify-items': v }),
    justifycontent: (v) => ({ 'justify-content': v }),
    placeitems: (v) => ({ 'place-items': v }),
    placecontent: (v) => ({ 'place-content': v }),
    gridtemplatecolumns: (v) => ({ 'grid-template-columns': v }),
    gridtemplaterows: (v) => ({ 'grid-template-rows': v }),
    gridcolumn: (v) => ({ 'grid-column': v }),
    gridrow: (v) => ({ 'grid-row': v }),
    flex: (v) => ({ flex: v }),
  };

  /** The CSS declarations for the attributes of a layout element (`{name: value}`). */
  function boxStyle(attributes) {
    const css = {};
    const bordered = {};
    for (const name of Object.keys(attributes)) {
      const value = attributes[name];
      if (value === null || value === undefined || value === '') continue;
      if (name in BOX) Object.assign(css, BOX[name](value));
      else if (name === 'border') Object.assign(bordered, border(value));
      else if (name === 'borderwidth') bordered.width = spacing(value);
      else if (name === 'borderstyle') bordered.style = value;
      else if (name === 'bordercolor') bordered.color = BORDER_COLORS[value] || value;
    }
    if (attributes.border || attributes.borderwidth || attributes.borderstyle) {
      css.border = [bordered.width || '1px', bordered.style || 'solid', bordered.color || BORDER_COLORS.base].join(' ');
    }
    return css;
  }

  function cssText(css) {
    return Object.keys(css)
      .map((name) => name + ':' + css[name])
      .join(';');
  }

  /** `direction` may be written for container queries (`@container … inline, block`): the last
   * plain keyword wins. */
  function direction(value) {
    const words = String(value || '').split(/[\s,]+/).filter((word) => word === 'inline' || word === 'block');
    return words.length ? words[words.length - 1] : 'block';
  }

  // --- Definition helpers -----------------------------------------------------------------

  const BOOLEAN = new Set([
    'disabled', 'loading', 'checked', 'defaultchecked', 'required', 'readonly', 'open', 'selected',
    'dismissible', 'collapsible', 'removable', 'multiple', 'pressed', 'hidden', 'indeterminate',
  ]);

  const registry = (global.customElements && global.customElements) || null;

  /**
   * Defines `tag`. `spec.props` are the camelCase properties mirrored by attributes;
   * `spec.shadow` is the template of the shadow root (none: light DOM); `spec.sync(el)` brings
   * the DOM in line with the attributes; `spec.connected(el)` runs once the element is in the
   * document.
   *
   * A custom element made with `document.createElement` must come out of its constructor
   * without attributes, classes or children (Chromium turns it into an `HTMLUnknownElement`
   * otherwise): the constructor only attaches the shadow root, fills it and listens. What the
   * host itself looks like is set from `connected` and `sync`.
   */
  function define(tag, spec) {
    if (!registry || !global.HTMLElement) return;
    const props = spec.props || [];
    const observed = props.map((prop) => prop.toLowerCase());
    class LsfElement extends global.HTMLElement {
      static get observedAttributes() {
        return observed;
      }
      constructor() {
        super();
        if (spec.shadow) {
          this._root = this.attachShadow({ mode: 'open' });
          this._root.innerHTML = '<style>' + BASE_CSS + (spec.css || '') + '</style>' + spec.shadow;
        }
        if (spec.init) spec.init(this);
      }
      connectedCallback() {
        if (spec.connected) spec.connected(this);
        if (spec.sync) spec.sync(this);
      }
      disconnectedCallback() {
        if (spec.disconnected) spec.disconnected(this);
      }
      attributeChangedCallback() {
        if (spec.sync) spec.sync(this);
      }
      /** The attributes as `{lowercased: value}`. */
      get attrs() {
        const out = {};
        for (const name of observed) if (this.hasAttribute(name)) out[name] = this.getAttribute(name);
        return out;
      }
    }
    for (const prop of props) {
      const name = prop.toLowerCase();
      const flag = BOOLEAN.has(name);
      if (Object.getOwnPropertyDescriptor(LsfElement.prototype, prop)) continue;
      Object.defineProperty(LsfElement.prototype, prop, {
        configurable: true,
        get() {
          return flag ? this.hasAttribute(name) : this.getAttribute(name);
        },
        set(value) {
          // A property written before the element joins the page beats its `default…`.
          (this._written = this._written || new Set()).add(name);
          if (value === null || value === undefined || value === false) this.removeAttribute(name);
          else this.setAttribute(name, value === true ? '' : String(value));
        },
      });
    }
    if (spec.members) Object.defineProperties(LsfElement.prototype, spec.members);
    registry.define(tag, LsfElement);
  }

  const $ = (el, selector) => el._root.querySelector(selector);

  /**
   * `defaultChecked`, `defaultOpen`, … : the state an element starts with. Applied once, when
   * the element first joins the page (not at the first attribute that happens to be synced),
   * and only when nobody wrote the state itself; the buyer's changes stay after that.
   * `state` is the attribute that mirrors the state: the default turns it on.
   */
  function applyDefault(el, state, initial) {
    if (el._defaulted) return;
    el._defaulted = true;
    if (el.hasAttribute(initial) && !el.hasAttribute(state) && !(el._written && el._written.has(state))) {
      el.toggleAttribute(state, true);
    }
  }
  const fire = (el, type, init) => el.dispatchEvent(new global.Event(type, { bubbles: true, composed: true, ...init }));

  // --- Shared CSS (inside every shadow root) ----------------------------------------------

  const BASE_CSS = `
    :host { box-sizing: border-box; font: inherit; color: inherit; }
    *, *::before, *::after { box-sizing: inherit; }
    :host([hidden]) { display: none !important; }
    button { font: inherit; color: inherit; }
    slot[name="fragments"] { display: contents; }
  `;

  // --- Layout ------------------------------------------------------------------------------

  /** The properties of layout elements: `paddingblock` (as the attribute reads) → `paddingBlock`. */
  const CAMEL = {
    padding: 'padding', paddingblock: 'paddingBlock', paddingblockstart: 'paddingBlockStart',
    paddingblockend: 'paddingBlockEnd', paddinginline: 'paddingInline',
    paddinginlinestart: 'paddingInlineStart', paddinginlineend: 'paddingInlineEnd',
    background: 'background', inlinesize: 'inlineSize', blocksize: 'blockSize',
    mininlinesize: 'minInlineSize', minblocksize: 'minBlockSize', maxinlinesize: 'maxInlineSize',
    maxblocksize: 'maxBlockSize', borderradius: 'borderRadius', overflow: 'overflow',
    display: 'display', gap: 'gap', rowgap: 'rowGap', columngap: 'columnGap',
    alignitems: 'alignItems', aligncontent: 'alignContent', justifyitems: 'justifyItems',
    justifycontent: 'justifyContent', placeitems: 'placeItems', placecontent: 'placeContent',
    gridtemplatecolumns: 'gridTemplateColumns', gridtemplaterows: 'gridTemplateRows',
    gridcolumn: 'gridColumn', gridrow: 'gridRow', flex: 'flex',
  };

  function layout(tag, extra) {
    define(tag, {
      props: [
        ...Object.values(CAMEL), 'border', 'borderWidth', 'borderStyle', 'borderColor', 'direction',
        'accessibilityLabel', 'accessibilityRole', 'accessibilityVisibility', ...(extra || []),
      ],
      sync(el) {
        const attrs = el.attrs;
        const css = boxStyle(attrs);
        if (tag === 's-stack') css['flex-direction'] = direction(attrs.direction) === 'inline' ? 'row' : 'column';
        el.style.cssText = cssText(css);
        const role = attrs.accessibilityrole;
        if (role && role !== 'generic' && role !== 'none') el.setAttribute('role', role);
        if (attrs.accessibilitylabel) el.setAttribute('aria-label', attrs.accessibilitylabel);
        if (attrs.accessibilityvisibility === 'hidden') el.setAttribute('aria-hidden', 'true');
      },
    });
  }

  layout('s-box');
  layout('s-stack');
  layout('s-grid');
  layout('s-grid-item');
  layout('s-scroll-box');
  layout('s-query-container');
  layout('s-clickable', ['href', 'target', 'download', 'command', 'commandFor', 'interestFor', 'disabled', 'loading', 'type', 'lang']);

  // --- Text and media ----------------------------------------------------------------------

  const plain = (tag, props) => define(tag, { props: props || [] });
  plain('s-text', ['type', 'color', 'tone', 'display', 'accessibilityVisibility', 'interestFor', 'dir', 'lang']);
  plain('s-heading', ['accessibilityRole', 'accessibilityVisibility', 'lineClamp']);
  plain('s-paragraph', ['color', 'tone', 'fontVariantNumeric', 'lineClamp', 'accessibilityVisibility', 'dir']);
  plain('s-abbreviation', ['title']);
  plain('s-time', ['dateTime']);
  plain('s-badge', ['color', 'icon', 'iconPosition', 'size', 'tone']);
  plain('s-ordered-list');
  plain('s-unordered-list');
  plain('s-list-item');
  plain('s-divider', ['direction', 'color']);
  plain('s-spinner', ['size', 'accessibilityLabel']);
  plain('s-skeleton-paragraph', []);
  plain('s-option', ['value', 'selected', 'defaultSelected', 'disabled']);
  plain('s-chat', []);

  define('s-progress', {
    props: ['value', 'max', 'tone', 'accessibilityLabel'],
    sync(el) {
      const max = Number(el.getAttribute('max')) || 1;
      const value = Number(el.getAttribute('value')) || 0;
      el.style.setProperty('--lsf-progress', Math.max(0, Math.min(100, (value / max) * 100)) + '%');
      el.setAttribute('role', 'progressbar');
    },
  });

  define('s-summary', {
    props: [],
    connected(el) {
      el.setAttribute('slot', 'summary');
    },
  });

  define('s-payment-icon', {
    props: ['name', 'accessibilityLabel'],
    shadow: '<span class="icon"></span>',
    css: ':host{display:inline-block}.icon{display:inline-block;min-width:2.5rem;padding:.25rem .4rem;border:1px solid #d4d4d4;border-radius:4px;background:#fff;font-size:.65rem;font-weight:700;letter-spacing:.04em;text-align:center;text-transform:uppercase}',
    sync(el) {
      $(el, '.icon').textContent = (el.getAttribute('name') || 'card').replace(/-/g, ' ');
    },
  });

  define('s-qr-code', {
    props: ['content', 'size', 'border', 'logo', 'accessibilityLabel'],
    shadow: '<div class="code"><span>QR</span><small></small></div>',
    css: ':host{display:inline-block}.code{display:grid;place-items:center;width:8rem;height:8rem;border:1px dashed #8a8a8a;border-radius:8px;color:#616161;text-align:center}small{font-size:.65rem;padding:0 .5rem;overflow-wrap:anywhere}',
    sync(el) {
      $(el, 'small').textContent = el.getAttribute('content') || '';
    },
  });

  for (const tag of ['s-map', 's-map-marker']) {
    define(tag, {
      props: ['apiKey', 'latitude', 'longitude', 'zoom', 'accessibilityLabel'],
      shadow: '<div class="map">Maps are not available in the local checkout.</div><slot></slot>',
      css: '.map{display:grid;place-items:center;min-height:6rem;border:1px dashed #8a8a8a;border-radius:8px;color:#616161;font-size:.8rem}',
    });
  }

  define('s-image', {
    props: ['src', 'srcSet', 'sizes', 'alt', 'aspectRatio', 'objectFit', 'inlineSize', 'loading', 'accessibilityRole', 'border', 'borderWidth', 'borderRadius'],
    shadow: '<img part="image" alt="">',
    css: ':host{display:block;max-width:100%}img{display:block;width:100%;height:100%;border-radius:inherit}',
    sync(el) {
      const img = $(el, 'img');
      for (const [attribute, property] of [['src', 'src'], ['srcset', 'srcset'], ['sizes', 'sizes'], ['loading', 'loading']]) {
        if (el.hasAttribute(attribute)) img.setAttribute(property, el.getAttribute(attribute));
        else img.removeAttribute(property);
      }
      img.setAttribute('alt', el.getAttribute('accessibilityrole') === 'presentation' ? '' : el.getAttribute('alt') || '');
      const css = boxStyle(el.attrs);
      const ratio = el.getAttribute('aspectratio');
      if (ratio) css['aspect-ratio'] = ratio.replace(/\s*\/\s*/, ' / ');
      el.style.cssText = cssText(css);
      img.style.objectFit = el.getAttribute('objectfit') || 'fill';
    },
  });

  define('s-product-thumbnail', {
    props: ['src', 'alt', 'size', 'totalItems'],
    shadow: '<div class="thumb"><img alt=""><span class="count" hidden></span></div>',
    css: ':host{display:inline-block}.thumb{position:relative;width:3.5rem;height:3.5rem;border:1px solid #e3e3e3;border-radius:8px;background:#f7f7f7}img{width:100%;height:100%;border-radius:inherit;object-fit:cover}.count{position:absolute;top:-.5rem;right:-.5rem;min-width:1.25rem;padding:0 .3rem;border-radius:999px;background:#303030;color:#fff;font-size:.75rem;text-align:center}',
    sync(el) {
      const img = $(el, 'img');
      if (el.hasAttribute('src')) img.setAttribute('src', el.getAttribute('src'));
      img.setAttribute('alt', el.getAttribute('alt') || '');
      const count = $(el, '.count');
      count.hidden = !el.hasAttribute('totalitems');
      count.textContent = el.getAttribute('totalitems') || '';
    },
  });

  // Icons: a small set of strokes on a 20 × 20 grid. Any other name draws an empty circle.
  const ICONS = {
    check: 'M4 10.5l4 4 8-9', 'check-circle': 'M10 2a8 8 0 100 16 8 8 0 000-16zM6.5 10.5l2.5 2.5 4.5-5',
    x: 'M5 5l10 10M15 5L5 15', 'x-circle': 'M10 2a8 8 0 100 16 8 8 0 000-16zM7 7l6 6M13 7l-6 6',
    plus: 'M10 4v12M4 10h12', minus: 'M4 10h12', info: 'M10 2a8 8 0 100 16 8 8 0 000-16zM10 9v5M10 6.5v.5',
    alert: 'M10 3l8 14H2L10 3zM10 8v4M10 14.5v.5', 'alert-circle': 'M10 2a8 8 0 100 16 8 8 0 000-16zM10 6v5M10 13.5v.5',
    cart: 'M2 3h2l2 9h9l2-6H5M8 16.5v.5M14 16.5v.5', delivery: 'M2 5h10v8H2zM12 8h4l2 3v2h-6M5 15.5v.5M14 15.5v.5',
    lock: 'M5 9h10v8H5zM7 9V6a3 3 0 016 0v3', email: 'M2 4h16v12H2zM2 5l8 6 8-6',
    gift: 'M3 8h14v9H3zM2 5h16v3H2zM10 5v12M10 5C8 1 5 3 7 5M10 5c2-4 5-2 3 0', heart: 'M10 17S3 12.5 3 7.5A3.5 3.5 0 0110 6a3.5 3.5 0 017 1.5C17 12.5 10 17 10 17z',
    star: 'M10 2l2.4 5 5.6.7-4 3.9 1 5.4-5-2.7-5 2.7 1-5.4-4-3.9 5.6-.7L10 2z', search: 'M9 3a6 6 0 100 12A6 6 0 009 3zM14 14l4 4',
    'arrow-left': 'M17 10H3M8 5l-5 5 5 5', 'arrow-right': 'M3 10h14M12 5l5 5-5 5', 'arrow-up': 'M10 17V3M5 8l5-5 5 5', 'arrow-down': 'M10 3v14M5 12l5 5 5-5',
    'chevron-left': 'M12 4l-6 6 6 6', 'chevron-right': 'M8 4l6 6-6 6', 'chevron-up': 'M4 12l6-6 6 6', 'chevron-down': 'M4 8l6 6 6-6',
    clock: 'M10 2a8 8 0 100 16 8 8 0 000-16zM10 5v5l3 2', discount: 'M4 16L16 4M5.5 7a1.5 1.5 0 100-3 1.5 1.5 0 000 3zM14.5 16a1.5 1.5 0 100-3 1.5 1.5 0 000 3z',
    location: 'M10 18s6-5.5 6-10a6 6 0 10-12 0c0 4.5 6 10 6 10zM10 9.5a2 2 0 100-4 2 2 0 000 4z', question: 'M10 2a8 8 0 100 16 8 8 0 000-16zM8 8a2 2 0 114 0c0 1.5-2 1.5-2 3M10 13.5v.5',
  };
  define('s-icon', {
    props: ['type', 'tone', 'color', 'size', 'interestFor'],
    shadow: '<svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path></path></svg>',
    css: ':host{display:inline-flex;width:1.25rem;height:1.25rem;vertical-align:middle}svg{width:100%;height:100%}',
    sync(el) {
      $(el, 'path').setAttribute('d', ICONS[el.getAttribute('type')] || 'M10 2a8 8 0 100 16 8 8 0 000-16z');
    },
  });

  // --- Containers --------------------------------------------------------------------------

  define('s-banner', {
    props: ['heading', 'tone', 'dismissible', 'collapsible', 'hidden'],
    shadow: `<div class="banner" part="banner"><svg class="icon" viewBox="0 0 20 20" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path></path></svg>
      <div class="main"><strong class="heading" part="heading"></strong><div class="content"><slot></slot></div><div class="actions"><slot name="secondary-actions"></slot><slot name="primary-action"></slot></div></div>
      <button class="dismiss" type="button" aria-label="Dismiss" hidden>×</button></div>`,
    css: `:host{display:block}.banner{display:flex;gap:.625rem;padding:.75rem;border-radius:var(--lsf-radius,8px);border:1px solid var(--tone-border,#cce1ff);background:var(--tone-bg,#eaf4ff);color:var(--tone-fg,#00527c)}
      .icon{flex:none;margin-top:.1rem}.main{flex:1;min-width:0;display:flex;flex-direction:column;gap:.25rem}.heading:empty{display:none}.content{color:#303030}.actions{display:flex;gap:.5rem;flex-wrap:wrap}.actions:not(:has(*)){display:none}
      .dismiss{flex:none;border:0;background:none;cursor:pointer;font-size:1.25rem;line-height:1}
      :host([tone="success"]){--tone-bg:#e4f7ec;--tone-border:#b4e5c8;--tone-fg:#0c5132}:host([tone="warning"]),:host([tone="caution"]){--tone-bg:#fff5ea;--tone-border:#ffd9a3;--tone-fg:#5e4200}
      :host([tone="critical"]){--tone-bg:#fff4f4;--tone-border:#f6c3c3;--tone-fg:#8e1f0b}:host([tone="neutral"]){--tone-bg:#f7f7f7;--tone-border:#d4d4d4;--tone-fg:#303030}`,
    init(el) {
      $(el, '.dismiss').addEventListener('click', () => {
        el.setAttribute('hidden', '');
        fire(el, 'dismiss', { composed: false });
        fire(el, 'afterhide', { composed: false });
      });
    },
    sync(el) {
      const tone = el.getAttribute('tone') || 'info';
      $(el, '.heading').textContent = el.getAttribute('heading') || '';
      $(el, '.dismiss').hidden = !el.hasAttribute('dismissible');
      $(el, 'path').setAttribute('d', ICONS[{ success: 'check-circle', warning: 'alert', caution: 'alert', critical: 'alert-circle' }[tone] || 'info']);
    },
  });

  define('s-announcement', {
    props: [],
    shadow: '<div class="bar"><div class="content"><slot></slot></div><button type="button" class="dismiss" aria-label="Dismiss">×</button></div>',
    css: ':host{display:block}.bar{display:flex;align-items:center;gap:.75rem;padding:.75rem 1rem;border-radius:var(--lsf-radius,8px);background:var(--lsf-accent,#005bd3);color:#fff}.content{flex:1}.dismiss{border:0;background:none;color:inherit;font-size:1.25rem;cursor:pointer}',
    members: {
      dismiss: { value() { this.setAttribute('hidden', ''); fire(this, 'dismiss', { composed: false }); } },
    },
    init(el) {
      $(el, '.dismiss').addEventListener('click', () => el.dismiss());
    },
  });

  define('s-section', {
    props: ['heading', 'accessibilityLabel', 'padding'],
    shadow: '<section part="section"><h2 class="heading" part="heading"></h2><div class="body"><slot></slot></div><div class="actions"><slot name="secondary-actions"></slot><slot name="primary-action"></slot></div></section>',
    css: ':host{display:block}section{display:flex;flex-direction:column;gap:.75rem}.heading{margin:0;font-size:1rem;font-weight:650}.heading:empty{display:none}.actions{display:flex;gap:.5rem}',
    sync(el) {
      $(el, '.heading').textContent = el.getAttribute('heading') || '';
      const label = el.getAttribute('accessibilitylabel');
      if (label) $(el, 'section').setAttribute('aria-label', label);
    },
  });

  define('s-details', {
    props: ['open', 'defaultOpen'],
    shadow: '<details><summary><slot name="summary"></slot></summary><div class="body"><slot></slot></div></details>',
    css: ':host{display:block}summary{cursor:pointer;font-weight:600}.body{padding-top:.5rem}',
    init(el) {
      $(el, 'details').addEventListener('toggle', () => {
        el.toggleAttribute('open', $(el, 'details').open);
        fire(el, 'toggle', { composed: false });
      });
    },
    connected(el) {
      applyDefault(el, 'open', 'defaultopen');
    },
    sync(el) {
      const details = $(el, 'details');
      const open = el.hasAttribute('open');
      if (details.open !== open) details.open = open;
    },
  });

  // Overlays take over the whole iframe while open: the page grows the frame to the window,
  // and `Sandbox.overlays` tells it so.
  let openOverlays = 0;
  function overlayChange(delta) {
    openOverlays = Math.max(0, openOverlays + delta);
    if (Sandbox.onOverlay) Sandbox.onOverlay(openOverlays > 0);
  }

  function overlay(tag, extraCss) {
    define(tag, {
      props: ['heading', 'size', 'padding', 'accessibilityLabel', 'id', 'open'],
      shadow: `<div class="backdrop" hidden><div class="dialog" role="dialog" aria-modal="true"><header><h2 class="heading"></h2><button class="close" type="button" aria-label="Close">×</button></header>
        <div class="body"><slot></slot></div><footer><slot name="secondary-actions"></slot><slot name="primary-action"></slot></footer></div></div>`,
      css: `.backdrop{position:fixed;inset:0;display:flex;align-items:center;justify-content:center;background:rgba(0,0,0,.4);z-index:10;visibility:visible}.backdrop[hidden]{display:none}
        .dialog{display:flex;flex-direction:column;max-width:min(32rem,calc(100vw - 2rem));max-height:calc(100vh - 2rem);border-radius:var(--lsf-radius,12px);background:#fff;color:#303030;box-shadow:0 8px 32px rgba(0,0,0,.3)}
        header{display:flex;justify-content:space-between;align-items:center;padding:.75rem 1rem;border-bottom:1px solid #e3e3e3}.heading{margin:0;font-size:1rem}.heading:empty{display:none}.close{border:0;background:none;font-size:1.5rem;cursor:pointer}
        .body{padding:1rem;overflow:auto}footer{display:flex;justify-content:flex-end;gap:.5rem;padding:.75rem 1rem;border-top:1px solid #e3e3e3}footer:not(:has(*)){display:none}${extraCss || ''}`,
      members: {
        showOverlay: { value() { setOverlay(this, true); } },
        hideOverlay: { value() { setOverlay(this, false); } },
        toggleOverlay: { value() { setOverlay(this, $(this, '.backdrop').hidden); } },
      },
      init(el) {
        $(el, '.backdrop').hidden = true;
        // Actions slotted after the overlay opened show up too.
        $(el, 'footer').addEventListener('slotchange', () => footerVisibility(el));
        $(el, '.close').addEventListener('click', () => setOverlay(el, false));
        $(el, '.backdrop').addEventListener('click', (event) => {
          if (event.target === $(el, '.backdrop')) setOverlay(el, false);
        });
      },
      sync(el) {
        $(el, '.heading').textContent = el.getAttribute('heading') || '';
        // `open` written before the element joins the page takes effect when it does: an
        // overlay nobody can see must not grow the frame.
        if (el.isConnected === false) return;
        if (el.hasAttribute('open') === $(el, '.backdrop').hidden) setOverlay(el, el.hasAttribute('open'));
        footerVisibility(el);
      },
      disconnected(el) {
        if (!$(el, '.backdrop').hidden) setOverlay(el, false);
      },
    });
  }
  function footerVisibility(el) {
    $(el, 'footer').hidden = !el.querySelector('[slot="primary-action"], [slot="secondary-actions"]');
  }
  function setOverlay(el, open) {
    const backdrop = $(el, '.backdrop');
    if (backdrop.hidden !== open) return;
    backdrop.hidden = !open;
    footerVisibility(el);
    el.toggleAttribute('open', open);
    overlayChange(open ? 1 : -1);
    fire(el, open ? 'show' : 'hide', { composed: false });
    fire(el, open ? 'aftershow' : 'afterhide', { composed: false });
  }
  overlay('s-modal');
  overlay('s-popover');
  overlay('s-sheet', '.backdrop{align-items:flex-end}.dialog{width:100vw;max-width:none;border-radius:12px 12px 0 0}');

  define('s-tooltip', { props: ['id'], shadow: '<span hidden><slot></slot></span>' });

  // --- Actions -----------------------------------------------------------------------------

  /** `command="--show"` on a button opens the overlay `commandFor` names. */
  function runCommand(el) {
    const id = el.getAttribute('commandfor');
    const command = el.getAttribute('command');
    if (!id || !command) return;
    const target = global.document.getElementById(id);
    if (!target) return;
    if (command === '--show' && target.showOverlay) target.showOverlay();
    else if (command === '--hide' && target.hideOverlay) target.hideOverlay();
    else if (command === '--toggle' && target.toggleOverlay) target.toggleOverlay();
    else if (command === '--auto' && target.toggleOverlay) target.toggleOverlay();
    else if (command === '--copy' && target.text && global.navigator && global.navigator.clipboard) {
      global.navigator.clipboard.writeText(target.text).catch(() => {});
    }
  }

  function navigate(el) {
    const href = el.getAttribute('href');
    if (href && Sandbox.navigate) Sandbox.navigate(href, el.getAttribute('target'));
  }

  function submit(el) {
    const form = el.closest && el.closest('s-form');
    if (form) form.dispatchEvent(new global.Event('submit', { bubbles: true, cancelable: true }));
  }

  function clickable(el) {
    el.addEventListener('click', (event) => {
      if (el.hasAttribute('disabled') || el.hasAttribute('loading')) {
        event.stopImmediatePropagation();
        return;
      }
      runCommand(el);
      navigate(el);
      if (el.getAttribute('type') === 'submit') submit(el);
    });
  }

  const BUTTON_CSS = `:host{display:inline-flex}button{display:inline-flex;align-items:center;justify-content:center;gap:.4rem;width:100%;min-height:2.25rem;padding:.4rem .9rem;border:1px solid #c9c9c9;border-radius:var(--lsf-radius,8px);background:#fff;color:#303030;font-weight:550;cursor:pointer}
    button:hover{background:#f7f7f7}:host([variant="primary"]) button{border-color:transparent;background:var(--lsf-accent,#005bd3);color:#fff}:host([variant="primary"]) button:hover{filter:brightness(.92)}
    :host([variant="secondary"]) button{background:#f7f7f7}:host([tone="critical"]) button{color:#8e1f0b;border-color:#e22c38}:host([tone="critical"][variant="primary"]) button{background:#e22c38;color:#fff}
    :host([disabled]) button,:host([loading]) button{opacity:.6;cursor:default}:host([loading]) button::before{content:"";width:.9em;height:.9em;border:2px solid currentColor;border-right-color:transparent;border-radius:50%;animation:spin .7s linear infinite}
    @keyframes spin{to{transform:rotate(360deg)}}`;

  define('s-button', {
    props: ['variant', 'tone', 'disabled', 'loading', 'href', 'target', 'type', 'command', 'commandFor', 'interestFor', 'inlineSize', 'accessibilityLabel', 'download'],
    shadow: '<button part="button" type="button"><slot></slot></button><slot name="fragments"></slot>',
    css: BUTTON_CSS,
    init: clickable,
    sync(el) {
      const button = $(el, 'button');
      button.disabled = el.hasAttribute('disabled');
      button.setAttribute('aria-busy', el.hasAttribute('loading') ? 'true' : 'false');
      if (el.hasAttribute('accessibilitylabel')) button.setAttribute('aria-label', el.getAttribute('accessibilitylabel'));
      const inline = el.getAttribute('inlinesize');
      el.style.display = inline === 'fill' ? 'flex' : '';
    },
  });

  define('s-press-button', {
    props: ['pressed', 'defaultPressed', 'disabled', 'loading', 'variant', 'tone', 'accessibilityLabel'],
    shadow: '<button part="button" type="button" aria-pressed="false"><slot></slot></button>',
    css: BUTTON_CSS + ':host([pressed]) button{background:#303030;color:#fff}',
    init(el) {
      $(el, 'button').addEventListener('click', () => {
        el.toggleAttribute('pressed');
        fire(el, 'change', { composed: false });
      });
    },
    connected(el) {
      applyDefault(el, 'pressed', 'defaultpressed');
    },
    sync(el) {
      $(el, 'button').setAttribute('aria-pressed', el.hasAttribute('pressed') ? 'true' : 'false');
      $(el, 'button').disabled = el.hasAttribute('disabled');
    },
  });

  define('s-link', {
    props: ['href', 'target', 'command', 'commandFor', 'interestFor', 'tone', 'lang', 'accessibilityLabel', 'download'],
    shadow: '<a part="link" role="link" tabindex="0"><slot></slot></a><slot name="fragments"></slot>',
    css: ':host{display:inline}a{color:var(--lsf-link,#005bd3);text-decoration:underline;cursor:pointer}:host([tone="critical"]) a{color:#8e1f0b}',
    init: clickable,
  });

  define('s-clickable-chip', {
    props: ['removable', 'hidden', 'href', 'accessibilityLabel', 'color'],
    shadow: '<span class="chip"><slot name="graphic"></slot><slot></slot><button class="remove" type="button" aria-label="Remove" hidden>×</button></span>',
    css: ':host{display:inline-flex}.chip{display:inline-flex;align-items:center;gap:.35rem;padding:.2rem .6rem;border:1px solid #d4d4d4;border-radius:999px;background:#f7f7f7;font-size:.85rem}.remove{border:0;background:none;cursor:pointer}',
    init(el) {
      $(el, '.remove').addEventListener('click', () => fire(el, 'remove', { composed: false }));
    },
    sync(el) {
      $(el, '.remove').hidden = !el.hasAttribute('removable');
    },
  });
  define('s-chip', {
    props: ['accessibilityLabel'],
    shadow: '<span class="chip"><slot name="graphic"></slot><slot></slot></span>',
    css: ':host{display:inline-flex}.chip{display:inline-flex;align-items:center;gap:.35rem;padding:.2rem .6rem;border:1px solid #d4d4d4;border-radius:999px;background:#f7f7f7;font-size:.85rem}',
  });

  define('s-clipboard-item', {
    props: ['text', 'id'],
    members: { text: { get() { return this.getAttribute('text') || ''; }, set(v) { this.setAttribute('text', v); }, configurable: true } },
  });

  define('s-form', {
    props: [],
    init(el) {
      el.addEventListener('keydown', (event) => {
        const tag = event.target && event.target.localName;
        if (event.key === 'Enter' && tag && tag.endsWith('-field')) submit(event.target);
      });
    },
  });

  // --- Fields ------------------------------------------------------------------------------

  const FIELD_CSS = `:host{display:block}.field{position:relative;display:flex;align-items:center;gap:.25rem;min-height:2.75rem;padding:0 .75rem;border:1px solid #8a8a8a;border-radius:var(--lsf-radius,8px);background:#fff}
    .field:focus-within{outline:2px solid var(--lsf-accent,#005bd3);outline-offset:0}.lbl{position:absolute;top:50%;left:.75rem;transform:translateY(-50%);color:#616161;pointer-events:none;transition:all .12s;font-size:.9rem}
    .control{flex:1;min-width:0;border:0;outline:0;background:transparent;font:inherit;color:inherit;padding:1rem 0 .25rem}
    .control:not(:placeholder-shown)~.lbl,.control:focus~.lbl{top:.55rem;transform:none;font-size:.7rem}textarea.control{resize:vertical;min-height:4.5rem}
    :host([error]) .field{border-color:#e22c38}.message{margin-top:.25rem;font-size:.8rem;color:#616161}.message.error{color:#8e1f0b}.message:empty{display:none}
    :host([disabled]) .field{background:#f7f7f7;opacity:.7}`;

  const FIELD_PROPS = ['label', 'labelAccessibilityVisibility', 'name', 'value', 'defaultValue', 'placeholder', 'disabled', 'readOnly', 'required', 'error', 'details', 'autocomplete', 'minLength', 'maxLength', 'min', 'max', 'step', 'prefix', 'suffix', 'icon', 'rows', 'accessory'];

  /**
   * The attribute is the initial value; the property takes over once someone sets it, and the
   * buyer once they typed. `defaultValue` fills a field nobody has written to yet.
   */
  function initialValue(el, control) {
    if (el._valueSet || el._typed) return;
    const wanted = el.hasAttribute('value') ? el.getAttribute('value') : el.getAttribute('defaultvalue');
    if (wanted !== null && control.value !== wanted) control.value = wanted;
  }

  function fieldTag(tag, inputType, { area = false, extra = [] } = {}) {
    define(tag, {
      props: [...FIELD_PROPS, ...extra],
      shadow: `<div class="field" part="field"><slot name="prefix"></slot>${area ? '<textarea class="control" placeholder=" "></textarea>' : `<input class="control" type="${inputType}" placeholder=" ">`}<span class="lbl"></span><slot name="suffix"></slot><slot name="accessory"></slot></div><div class="message"></div><div class="message error"></div>`,
      css: FIELD_CSS,
      init(el) {
        const control = $(el, '.control');
        control.addEventListener('change', () => fire(el, 'change', { composed: false }));
        // Once the buyer typed, `defaultValue` no longer resets the field.
        control.addEventListener('input', () => { el._typed = true; });
        control.addEventListener('keydown', (event) => {
          if (event.key === 'Enter' && !area) submit(el);
        });
      },
      sync(el) {
        const control = $(el, '.control');
        $(el, '.lbl').textContent = el.getAttribute('label') || '';
        const map = { name: 'name', placeholder: 'placeholder', autocomplete: 'autocomplete', minlength: 'minlength', maxlength: 'maxlength', min: 'min', max: 'max', step: 'step', rows: 'rows' };
        for (const attribute of Object.keys(map)) {
          if (el.hasAttribute(attribute)) control.setAttribute(map[attribute], el.getAttribute(attribute));
          else if (attribute !== 'placeholder') control.removeAttribute(map[attribute]);
        }
        if (el.hasAttribute('placeholder') === false) control.setAttribute('placeholder', ' ');
        control.disabled = el.hasAttribute('disabled');
        control.readOnly = el.hasAttribute('readonly');
        control.required = el.hasAttribute('required');
        $(el, '.message:not(.error)').textContent = el.getAttribute('details') || '';
        $(el, '.message.error').textContent = el.getAttribute('error') || '';
        initialValue(el, control);
      },
      members: {
        value: {
          configurable: true,
          get() { return $(this, '.control').value; },
          set(value) { this._valueSet = true; $(this, '.control').value = value === null || value === undefined ? '' : String(value); },
        },
        defaultValue: { configurable: true, get() { return this.getAttribute('defaultvalue') || ''; }, set(v) { this.setAttribute('defaultvalue', v); } },
        focus: { value() { $(this, '.control').focus(); } },
        select: { value() { $(this, '.control').select(); } },
      },
    });
  }
  fieldTag('s-text-field', 'text');
  fieldTag('s-email-field', 'email');
  fieldTag('s-phone-field', 'tel');
  fieldTag('s-url-field', 'url');
  fieldTag('s-password-field', 'password');
  fieldTag('s-number-field', 'number', { extra: ['inputMode', 'controls'] });
  fieldTag('s-money-field', 'number');
  fieldTag('s-text-area', 'text', { area: true });
  fieldTag('s-date-field', 'date');
  fieldTag('s-consent-phone-field', 'tel', { extra: ['policy', 'type'] });
  define('s-date-picker', {
    props: ['value', 'defaultValue', 'disabled', 'name'],
    shadow: '<input class="control" type="date">',
    css: ':host{display:inline-block}.control{padding:.5rem;border:1px solid #8a8a8a;border-radius:var(--lsf-radius,8px);font:inherit}',
    init(el) {
      const control = $(el, '.control');
      control.addEventListener('change', () => fire(el, 'change', { composed: false }));
      control.addEventListener('input', () => { el._typed = true; });
    },
    sync(el) {
      const control = $(el, '.control');
      control.disabled = el.hasAttribute('disabled');
      if (el.hasAttribute('name')) control.name = el.getAttribute('name');
      initialValue(el, control);
    },
    members: {
      value: {
        configurable: true,
        get() { return $(this, '.control').value; },
        set(v) { this._valueSet = true; $(this, '.control').value = v || ''; },
      },
    },
  });

  const CHECK_CSS = `:host{display:block}label{display:flex;align-items:flex-start;gap:.6rem;cursor:pointer}input{position:absolute;opacity:0;pointer-events:none}.box{flex:none;display:grid;place-items:center;width:1.25rem;height:1.25rem;margin-top:.1rem;border:1px solid #8a8a8a;border-radius:4px;background:#fff;color:#fff}
    input:checked+.box{background:var(--lsf-accent,#005bd3);border-color:var(--lsf-accent,#005bd3)}input:focus-visible+.box{outline:2px solid var(--lsf-accent,#005bd3);outline-offset:2px}.box svg{width:.8rem;height:.8rem;visibility:hidden}input:checked+.box svg{visibility:visible}
    .text{display:flex;flex-direction:column}.message{font-size:.8rem;color:#616161}.message.error{color:#8e1f0b}.message:empty{display:none}:host([disabled]) label{opacity:.6;cursor:default}
    .switch .box{width:2.25rem;border-radius:999px;position:relative}.switch .box::after{content:"";position:absolute;left:.15rem;width:.9rem;height:.9rem;border-radius:50%;background:#8a8a8a;transition:left .12s}.switch input:checked+.box::after{left:1.1rem;background:#fff}.switch .box svg{display:none}`;

  function checkTag(tag, { kind = 'checkbox', switchy = false } = {}) {
    define(tag, {
      props: ['label', 'name', 'value', 'checked', 'defaultChecked', 'disabled', 'required', 'error', 'details', 'accessibilityLabel', 'command', 'commandFor', 'indeterminate', 'policy'],
      shadow: `<label${switchy ? ' class="switch"' : ''}><input type="${kind}"><span class="box"><svg viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><path d="M4 10.5l4 4 8-9"/></svg></span>
        <span class="text"><span class="lbl"><slot></slot></span><span class="message"></span><span class="message error"></span></span></label>`,
      css: CHECK_CSS,
      // The switch look lives in the shadow root (`label.switch`): a constructor must not
      // touch the host, and a class on it would not survive the extension's own `class`.
      init(el) {
        const input = $(el, 'input');
        input.addEventListener('change', () => {
          el.toggleAttribute('checked', input.checked);
          fire(el, 'change', { composed: false });
          runCommand(el);
        });
      },
      connected(el) {
        applyDefault(el, 'checked', 'defaultchecked');
      },
      sync(el) {
        const input = $(el, 'input');
        const label = el.getAttribute('label');
        const slot = $(el, 'slot');
        if (label !== null) {
          let text = $(el, '.lbl .own');
          if (!text) {
            text = global.document.createElement('span');
            text.className = 'own';
            $(el, '.lbl').insertBefore(text, slot);
          }
          text.textContent = label;
        }
        const checked = el.hasAttribute('checked');
        if (input.checked !== checked) input.checked = checked;
        input.indeterminate = el.hasAttribute('indeterminate');
        input.disabled = el.hasAttribute('disabled');
        input.required = el.hasAttribute('required');
        if (el.hasAttribute('name')) input.name = el.getAttribute('name');
        $(el, '.message:not(.error)').textContent = el.getAttribute('details') || '';
        $(el, '.message.error').textContent = el.getAttribute('error') || '';
      },
      members: {
        checked: {
          configurable: true,
          get() { return $(this, 'input').checked; },
          set(value) {
            (this._written = this._written || new Set()).add('checked');
            $(this, 'input').checked = Boolean(value);
            this.toggleAttribute('checked', Boolean(value));
          },
        },
      },
    });
  }
  checkTag('s-checkbox');
  checkTag('s-consent-checkbox');
  checkTag('s-switch', { switchy: true });

  define('s-choice', {
    props: ['value', 'selected', 'defaultSelected', 'disabled', 'error', 'accessibilityLabel', 'id'],
    shadow: '<label><input type="radio"><span class="mark"></span><span class="text"><slot></slot></span></label><div class="details"><slot name="details"></slot></div>',
    css: ':host{display:block}label{display:flex;gap:.6rem;align-items:flex-start;cursor:pointer}input{position:absolute;opacity:0}.mark{flex:none;width:1.1rem;height:1.1rem;margin-top:.15rem;border:1px solid #8a8a8a;border-radius:50%;background:#fff}input:checked+.mark{border:.35rem solid var(--lsf-accent,#005bd3)}.details{padding-left:1.7rem;color:#616161;font-size:.85rem}:host([disabled]) label{opacity:.6}',
    init(el) {
      $(el, 'input').addEventListener('change', () => {
        el.toggleAttribute('selected', $(el, 'input').checked);
        fire(el, 'change', { composed: false });
      });
    },
    connected(el) {
      // A list that was told its `values` decides, not the choices' defaults.
      const list = el.closest && el.closest('s-choice-list');
      if (list && list._values) el._defaulted = true;
      applyDefault(el, 'selected', 'defaultselected');
    },
    sync: syncChoice,
  });

  /** A choice follows its own attributes and those of the list it is in. */
  function syncChoice(el) {
    const input = $(el, 'input');
    const selected = el.hasAttribute('selected');
    if (input.checked !== selected) input.checked = selected;
    const list = el.closest && el.closest('s-choice-list');
    input.disabled = el.hasAttribute('disabled') || Boolean(list && list.hasAttribute('disabled'));
    input.type = list && list.hasAttribute('multiple') ? 'checkbox' : 'radio';
    if (list && list.getAttribute('name')) input.name = list.getAttribute('name');
  }

  /** `values` set on a list gives its choices the `selected` they should have. */
  function applyValues(el) {
    if (!el._values) return;
    const wanted = new Set(el._values);
    for (const choice of el.querySelectorAll('s-choice')) choice.toggleAttribute('selected', wanted.has(choice.getAttribute('value')));
  }

  define('s-choice-list', {
    props: ['label', 'labelAccessibilityVisibility', 'name', 'multiple', 'disabled', 'error', 'details'],
    shadow: '<fieldset><legend class="legend"></legend><div class="list"><slot></slot></div><div class="message error"></div></fieldset>',
    css: 'fieldset{border:0;margin:0;padding:0}.legend{padding:0;margin-bottom:.5rem;font-weight:600}.legend:empty{display:none}.list{display:flex;flex-direction:column;gap:.6rem}.message.error{color:#8e1f0b;font-size:.8rem}.message:empty{display:none}',
    init(el) {
      el.addEventListener('change', (event) => {
        const choice = event.target;
        if (!choice || choice.localName !== 's-choice') return;
        const choices = [...el.querySelectorAll('s-choice')];
        if (!el.hasAttribute('multiple')) {
          for (const other of choices) if (other !== choice) other.removeAttribute('selected');
        }
        el._values = null; // the buyer took over
      });
      // `values` is often written before the choices exist, and a choice's `value` after it.
      if (global.MutationObserver) {
        el._observer = new global.MutationObserver(() => applyValues(el));
        el._observer.observe(el, { childList: true, subtree: true, attributes: true, attributeFilter: ['value'] });
      }
    },
    connected: applyValues,
    sync(el) {
      $(el, '.legend').textContent = el.getAttribute('label') || '';
      $(el, '.message').textContent = el.getAttribute('error') || '';
      for (const choice of el.querySelectorAll('s-choice')) if (choice._root) syncChoice(choice);
    },
    members: {
      values: {
        configurable: true,
        get() {
          const choices = [...this.querySelectorAll('s-choice')];
          if (this._values && !choices.length) return [...this._values];
          return choices.filter((c) => c.hasAttribute('selected')).map((c) => c.getAttribute('value'));
        },
        set(values) {
          this._values = [...(values || [])];
          applyValues(this);
        },
      },
    },
  });

  define('s-select', {
    props: ['label', 'name', 'value', 'placeholder', 'disabled', 'required', 'error', 'details', 'icon', 'autocomplete'],
    shadow: '<div class="field"><select class="control"></select><span class="lbl"></span></div><div class="message error"></div>',
    css: FIELD_CSS + '.control{padding:1rem 0 .25rem;appearance:auto}.lbl{top:.55rem;transform:none;font-size:.7rem}',
    init(el) {
      $(el, '.control').addEventListener('change', () => {
        el._value = $(el, '.control').value; // what the buyer picked survives a rebuild
        el._valueSet = true;
        fire(el, 'change', { composed: false });
      });
      if (global.MutationObserver) {
        el._observer = new global.MutationObserver(() => rebuildOptions(el));
        el._observer.observe(el, { childList: true, subtree: true, characterData: true, attributes: true });
      }
    },
    connected: rebuildOptions,
    sync(el) {
      const control = $(el, '.control');
      $(el, '.lbl').textContent = el.getAttribute('label') || '';
      $(el, '.message').textContent = el.getAttribute('error') || '';
      control.disabled = el.hasAttribute('disabled');
      if (el.hasAttribute('name')) control.name = el.getAttribute('name');
      applySelectValue(el);
    },
    members: {
      value: {
        configurable: true,
        // Before its options exist a select has no value to give: it answers what was asked.
        get() { return this._valueSet && !this._built ? this._value : $(this, '.control').value; },
        set(value) {
          this._valueSet = true;
          this._value = value === null || value === undefined ? '' : String(value);
          applySelectValue(this);
        },
      },
    },
  });
  /** The value asked for: the property if written, else the attribute. `undefined`: none. */
  function applySelectValue(el) {
    const wanted = el._valueSet ? el._value : el.hasAttribute('value') ? el.getAttribute('value') : undefined;
    if (wanted !== undefined) $(el, '.control').value = wanted;
  }
  function rebuildOptions(el) {
    const control = $(el, '.control');
    control.textContent = '';
    el._built = false;
    for (const option of el.querySelectorAll('s-option')) {
      const native = global.document.createElement('option');
      native.value = option.getAttribute('value') || option.textContent.trim();
      native.textContent = option.textContent.trim();
      native.disabled = option.hasAttribute('disabled');
      native.selected = option.hasAttribute('selected') || option.hasAttribute('defaultselected');
      control.appendChild(native);
      el._built = true;
    }
    // The options are new: the value asked for (before they existed, or by the buyer) goes back.
    applySelectValue(el);
  }

  define('s-drop-zone', {
    props: ['label', 'accept', 'multiple', 'disabled', 'error', 'name', 'required'],
    shadow: '<label class="zone"><span class="lbl"></span><input type="file"></label>',
    css: '.zone{display:flex;flex-direction:column;align-items:center;gap:.5rem;padding:1.5rem;border:1px dashed #8a8a8a;border-radius:var(--lsf-radius,8px);color:#616161;cursor:pointer}',
    init(el) { $(el, 'input').addEventListener('change', () => fire(el, 'change', { composed: false })); },
    sync(el) {
      $(el, '.lbl').textContent = el.getAttribute('label') || 'Add file';
      const input = $(el, 'input');
      input.multiple = el.hasAttribute('multiple');
      input.disabled = el.hasAttribute('disabled');
      input.required = el.hasAttribute('required');
      if (el.hasAttribute('name')) input.name = el.getAttribute('name');
      if (el.hasAttribute('accept')) input.accept = el.getAttribute('accept');
      else input.removeAttribute('accept');
    },
    members: { files: { configurable: true, get() { return $(this, 'input').files; } } },
  });

  // --- Light DOM style (the document's stylesheet) -----------------------------------------

  const LIGHT_CSS = `
    s-stack { display: flex; flex-direction: column; }
    s-box, s-grid-item, s-scroll-box, s-query-container, s-section, s-clickable { display: block; }
    s-grid { display: grid; }
    s-scroll-box { overflow: auto; }
    s-query-container { container-type: inline-size; }
    s-clickable { cursor: pointer; }
    s-clickable[disabled] { opacity: .6; cursor: default; }
    s-text { display: inline; }
    s-text[type="strong"] { font-weight: 650; }
    s-text[type="emphasis"] { font-style: italic; }
    s-text[type="small"], s-text[type="offset"] { font-size: .85em; }
    s-text[type="address"] { font-style: normal; }
    s-text[type="redundant"] { text-decoration: line-through; color: var(--lsf-subdued, #616161); }
    s-text[color="subdued"], s-paragraph[color="subdued"] { color: var(--lsf-subdued, #616161); }
    s-text[color="strong"], s-paragraph[color="strong"] { font-weight: 650; }
    s-text[tone="critical"], s-paragraph[tone="critical"] { color: #8e1f0b; }
    s-text[tone="success"], s-paragraph[tone="success"] { color: #0c5132; }
    s-text[tone="warning"], s-paragraph[tone="warning"], s-text[tone="caution"] { color: #5e4200; }
    s-text[tone="info"], s-paragraph[tone="info"] { color: #00527c; }
    s-text[tone="neutral"] { color: var(--lsf-subdued, #616161); }
    s-heading { display: block; margin: 0; font-size: 1.25rem; font-weight: 650; line-height: 1.3; }
    s-section s-heading { font-size: 1rem; }
    s-section s-section s-heading { font-size: .9rem; }
    s-paragraph { display: block; margin: 0; }
    s-abbreviation { text-decoration: underline dotted; }
    s-badge { display: inline-flex; align-items: center; gap: .25rem; padding: .1rem .5rem; border-radius: 999px; background: #ebebeb; color: #303030; font-size: .8rem; font-weight: 550; line-height: 1.4; }
    s-badge[size="small"], s-badge[size="small-100"] { font-size: .7rem; padding: 0 .4rem; }
    s-badge[size="large"] { font-size: .9rem; padding: .2rem .65rem; }
    s-badge[tone="info"] { background: #d4eaff; color: #00527c; }
    s-badge[tone="success"] { background: #cdfee1; color: #0c5132; }
    s-badge[tone="warning"], s-badge[tone="caution"] { background: #ffd9a3; color: #5e4200; }
    s-badge[tone="critical"] { background: #fed1d7; color: #8e1f0b; }
    s-badge[tone="accent"] { background: var(--lsf-accent, #005bd3); color: #fff; }
    s-ordered-list, s-unordered-list { display: block; margin: 0; padding-inline-start: 1.25rem; }
    s-ordered-list { list-style: decimal; }
    s-unordered-list { list-style: disc; }
    s-ordered-list > s-list-item, s-unordered-list > s-list-item { display: list-item; }
    s-divider { display: block; border: 0; border-block-start: 1px solid #e3e3e3; margin: 0; }
    s-divider[direction="inline"] { display: inline-block; align-self: stretch; border-block-start: 0; border-inline-start: 1px solid #e3e3e3; }
    s-spinner { display: inline-block; width: 1.25rem; height: 1.25rem; border: 2px solid var(--lsf-accent, #005bd3); border-right-color: transparent; border-radius: 50%; animation: lsf-spin .7s linear infinite; }
    s-spinner[size="small"] { width: .9rem; height: .9rem; }
    s-spinner[size="large"], s-spinner[size="large-100"] { width: 2rem; height: 2rem; }
    @keyframes lsf-spin { to { transform: rotate(360deg); } }
    s-progress { display: block; height: .5rem; border-radius: 999px; background: #e3e3e3; overflow: hidden; }
    s-progress::before { content: ""; display: block; height: 100%; width: var(--lsf-progress, 0%); background: var(--lsf-accent, #005bd3); }
    s-progress[tone="critical"]::before { background: #e22c38; }
    s-skeleton-paragraph { display: block; height: 1em; border-radius: 4px; background: linear-gradient(90deg, #ebebeb, #f7f7f7, #ebebeb); }
    s-option, s-tooltip { display: none; }
    s-summary { display: block; }
    s-modal, s-popover, s-sheet { display: contents; }
  `;

  function install(doc) {
    const style = doc.createElement('style');
    style.setAttribute('data-lsf', 'components');
    style.textContent = LIGHT_CSS;
    doc.head.appendChild(style);
    // Hovering what a tooltip describes shows its text.
    doc.addEventListener('mouseover', (event) => {
      const target = event.target && event.target.closest && event.target.closest('[interestfor]');
      if (!target || target.title) return;
      const tip = doc.getElementById(target.getAttribute('interestfor'));
      if (tip) target.title = tip.textContent.trim();
    });
  }

  Sandbox.components = { TAGS, spacing, boxStyle, border, direction, cssText, install, LIGHT_CSS };
  if (global.document && global.document.head) install(global.document);
})(typeof globalThis !== 'undefined' ? globalThis : this);
