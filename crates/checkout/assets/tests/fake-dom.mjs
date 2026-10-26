// A DOM just big enough to run the checkout page and the sandbox in Node: elements with
// attributes, children and listeners, events that bubble, and the few selectors the code uses.

export class FakeEvent {
  constructor(type, init = {}) {
    this.type = type;
    this.bubbles = Boolean(init.bubbles);
    this.cancelable = Boolean(init.cancelable);
    this.composed = Boolean(init.composed);
    this.defaultPrevented = false;
    this.target = null;
    this.currentTarget = null;
    this.stopped = false;
    Object.assign(this, init.data ?? {});
  }
  preventDefault() {
    this.defaultPrevented = true;
  }
  stopImmediatePropagation() {
    this.stopped = true;
  }
}

class Node {
  constructor(doc) {
    this.ownerDocument = doc;
    this.parentNode = null;
    this.childNodes = [];
  }
  get firstChild() {
    return this.childNodes[0] ?? null;
  }
  get nextSibling() {
    const siblings = this.parentNode?.childNodes ?? [];
    return siblings[siblings.indexOf(this) + 1] ?? null;
  }
  appendChild(child) {
    return this.insertBefore(child, null);
  }
  insertBefore(child, reference) {
    if (child.parentNode) child.parentNode.removeChild(child);
    const at = reference ? this.childNodes.indexOf(reference) : this.childNodes.length;
    if (reference && at < 0) throw new Error('insertBefore: the reference is not a child');
    this.childNodes.splice(at, 0, child);
    child.parentNode = this;
    return child;
  }
  removeChild(child) {
    const at = this.childNodes.indexOf(child);
    if (at < 0) throw new Error('removeChild: not a child');
    this.childNodes.splice(at, 1);
    child.parentNode = null;
    return child;
  }
  replaceChild(child, old) {
    this.insertBefore(child, old);
    this.removeChild(old);
    return old;
  }
  remove() {
    this.parentNode?.removeChild(this);
  }
  contains(other) {
    for (let node = other; node; node = node.parentNode) if (node === this) return true;
    return false;
  }
}

export class FakeText extends Node {
  constructor(doc, text) {
    super(doc);
    this.nodeValue = text;
    this.nodeType = 3;
  }
  get textContent() {
    return this.nodeValue;
  }
  // Preact changes a text through `data`.
  get data() {
    return this.nodeValue;
  }
  set data(value) {
    this.nodeValue = value;
  }
}

/** Does `el` match one simple selector: `tag`, `.class`, `[attr]`, `[attr="v"]`, combined. */
function matches(el, selector) {
  const parts = selector.match(/^([\w-]*)((?:\.[\w-]+|\[[^\]]+\])*)$/);
  if (!parts) throw new Error(`fake DOM: unsupported selector ${selector}`);
  const [, tag, rest] = parts;
  if (tag && el.localName !== tag) return false;
  for (const part of rest.match(/\.[\w-]+|\[[^\]]+\]/g) ?? []) {
    if (part[0] === '.') {
      if (!el.className.split(/\s+/).includes(part.slice(1))) return false;
    } else {
      const [, name, value] = part.match(/^\[([\w-]+)(?:=["']?([^\]"']*)["']?)?\]$/);
      if (!el.hasAttribute(name)) return false;
      if (value !== undefined && el.getAttribute(name) !== value) return false;
    }
  }
  return true;
}

/** Do the ancestors of `el`, up to `root`, match the descendant steps of a selector? */
function within(el, steps, root) {
  let node = el.parentNode;
  for (let i = steps.length - 1; i >= 0; i--) {
    while (node && node !== root && !matches(node, steps[i])) node = node.parentNode;
    if (!node || node === root) return false;
    node = node.parentNode;
  }
  return true;
}

export class FakeElement extends Node {
  constructor(doc, tag) {
    super(doc);
    this.localName = tag.toLowerCase();
    this.tagName = tag.toUpperCase();
    this.nodeType = 1;
    this.attributes = new Map();
    this.listeners = new Map();
    this.style = {
      setProperty(name, value) {
        this[name] = value;
      },
    };
    this.hidden = false;
  }

  // Like a browser: only the observed attributes call back, and only when they change.
  changed(name, before) {
    if (this.constructor.observedAttributes?.includes(name) && before !== this.getAttribute(name)) {
      this.attributeChangedCallback?.(name);
    }
  }
  setAttribute(name, value) {
    const before = this.getAttribute(name);
    this.attributes.set(name.toLowerCase(), String(value));
    this.changed(name.toLowerCase(), before);
  }
  getAttribute(name) {
    return this.attributes.get(name.toLowerCase()) ?? null;
  }
  hasAttribute(name) {
    return this.attributes.has(name.toLowerCase());
  }
  removeAttribute(name) {
    const before = this.getAttribute(name);
    if (this.attributes.delete(name.toLowerCase())) this.changed(name.toLowerCase(), before);
  }
  toggleAttribute(name, force) {
    const on = force ?? !this.hasAttribute(name);
    if (on) this.setAttribute(name, '');
    else this.removeAttribute(name);
    return on;
  }
  get className() {
    return this.getAttribute('class') ?? '';
  }
  set className(value) {
    this.setAttribute('class', value);
  }
  get classList() {
    return {
      add: (name) => this.setAttribute('class', [...new Set([...this.className.split(/\s+/).filter(Boolean), name])].join(' ')),
      toggle: (name, force) => {
        const names = this.className.split(/\s+/).filter(Boolean);
        const on = force ?? !names.includes(name);
        this.setAttribute('class', (on ? [...new Set([...names, name])] : names.filter((entry) => entry !== name)).join(' '));
      },
      contains: (name) => this.className.split(/\s+/).includes(name),
    };
  }
  get id() {
    return this.getAttribute('id') ?? '';
  }
  get title() {
    return this.getAttribute('title') ?? '';
  }
  set title(value) {
    this.setAttribute('title', value);
  }
  get textContent() {
    return this.childNodes.map((child) => child.textContent ?? '').join('') + (this._text ?? '');
  }
  set textContent(value) {
    this.childNodes.slice().forEach((child) => this.removeChild(child));
    if (value !== '') this.appendChild(new FakeText(this.ownerDocument, String(value)));
  }
  set innerHTML(value) {
    this._html = value;
    this.childNodes.slice().forEach((child) => this.removeChild(child));
  }
  get innerHTML() {
    return this._html ?? '';
  }
  append(...nodes) {
    for (const node of nodes) this.appendChild(typeof node === 'string' ? new FakeText(this.ownerDocument, node) : node);
  }
  replaceChildren(...nodes) {
    this.childNodes.slice().forEach((child) => this.removeChild(child));
    this.append(...nodes);
  }
  focus() {
    this.ownerDocument.activeElement = this;
  }
  getBoundingClientRect() {
    return { height: this.heightForTests ?? 0, width: 0 };
  }

  addEventListener(type, listener) {
    if (!this.listeners.has(type)) this.listeners.set(type, []);
    this.listeners.get(type).push(listener);
  }
  removeEventListener(type, listener) {
    const list = this.listeners.get(type) ?? [];
    list.splice(list.indexOf(listener), 1);
  }
  dispatchEvent(event) {
    event.target ??= this;
    for (let node = this; node && !event.stopped; node = node.parentNode) {
      event.currentTarget = node;
      for (const listener of [...(node.listeners?.get(event.type) ?? [])]) {
        listener.call(node, event);
        if (event.stopped) break;
      }
      if (!event.bubbles) break;
    }
    return !event.defaultPrevented;
  }
  click() {
    return this.dispatchEvent(new FakeEvent('click', { bubbles: true }));
  }

  closest(selector) {
    for (let node = this; node?.nodeType === 1; node = node.parentNode) if (matches(node, selector)) return node;
    return null;
  }
  querySelectorAll(selector) {
    const found = [];
    for (const alternative of selector.split(',').map((entry) => entry.trim())) {
      const direct = alternative.startsWith(':scope >');
      const steps = alternative.replace(/^:scope >\s*/, '').split(/\s+/);
      const last = steps.pop();
      const walk = (node) => {
        for (const child of node.childNodes) {
          if (child.nodeType !== 1) continue;
          if (matches(child, last) && within(child, steps, this) && !found.includes(child)) found.push(child);
          if (!direct) walk(child);
        }
      };
      walk(this);
    }
    return found;
  }
  querySelector(selector) {
    return this.querySelectorAll(selector)[0] ?? null;
  }

  /** Markup of the element, to compare in tests. */
  get outerHTML() {
    const attributes = [...this.attributes].map(([name, value]) => (value === '' ? ` ${name}` : ` ${name}="${value}"`)).join('');
    const inner = this.childNodes.map((child) => (child.nodeType === 3 ? child.nodeValue : child.outerHTML)).join('');
    return `<${this.localName}${attributes}>${inner}</${this.localName}>`;
  }
}

export class FakeDocument extends FakeElement {
  constructor() {
    super(null, '#document');
    this.ownerDocument = this;
    this.head = new FakeElement(this, 'head');
    this.body = new FakeElement(this, 'body');
    this.documentElement = new FakeElement(this, 'html');
    this.documentElement.append(this.head, this.body);
    this.appendChild(this.documentElement);
    this.activeElement = null;
    this.baseURI = 'http://localhost/checkouts/cn/token';
  }
  createElement(tag) {
    const factory = this.elementFactory?.(tag.toLowerCase());
    return factory ? factory(this, tag) : new FakeElement(this, tag);
  }
  // Preact makes every element with it; the namespace does not matter here.
  createElementNS(_namespace, tag) {
    return this.createElement(tag);
  }
  createTextNode(text) {
    return new FakeText(this, text);
  }
  getElementById(id) {
    const walk = (node) => {
      for (const child of node.childNodes) {
        if (child.nodeType !== 1) continue;
        if (child.getAttribute('id') === id) return child;
        const inner = walk(child);
        if (inner) return inner;
      }
      return null;
    };
    return walk(this);
  }
}

/** Waits for the promises (and timers set to 0) the code under test started. */
export const settle = () => new Promise((resolve) => setImmediate(resolve));
