// A small virtual DOM: views describe the page with `h()`, `render()` makes the DOM match.
//
// The checkout page is re-rendered after every operation, so it must not rebuild what the
// buyer is typing in, nor the iframes of the checkout UI extensions (moving an iframe reloads
// it). Two things follow: nodes are matched by key or position and updated in place, and a
// node marked `opaque` keeps whatever its owner put inside it.

const TEXT = '#text';

/** Builds a node. Children may be nested arrays; `null`, `false` and `true` are skipped. */
export function h(tag, props, ...children) {
  const flat = [];
  const push = (child) => {
    if (Array.isArray(child)) child.forEach(push);
    else if (child === null || child === undefined || typeof child === 'boolean') return;
    else if (typeof child === 'object') flat.push(child);
    else flat.push({ tag: TEXT, text: String(child), props: {}, children: [], key: null, el: null });
  };
  children.forEach(push);
  const own = props ?? {};
  return { tag, props: own, children: flat, key: own.key ?? null, el: null };
}

const RESERVED = new Set(['key', 'opaque', 'hooks', 'html']);
// Properties the browser keeps apart from the attribute once the user has touched the field.
const PROPERTIES = new Set(['value', 'checked']);

/**
 * Makes `container` show `tree`. The first call builds the DOM; later calls update it.
 * `hooks.insert(el)` runs once the new nodes are in the document, `hooks.destroy(el)` when a
 * node leaves it.
 */
export function render(container, tree) {
  const doc = container.ownerDocument ?? container;
  const after = [];
  const previous = container.__lsfTree;
  if (!previous) {
    container.appendChild(create(tree, doc, after));
  } else if (previous.tag === tree.tag) {
    patch(previous, tree, doc, after);
  } else {
    container.replaceChild(create(tree, doc, after), previous.el);
    destroy(previous);
  }
  container.__lsfTree = tree;
  after.forEach((run) => run());
}

function create(node, doc, after) {
  if (node.tag === TEXT) {
    node.el = doc.createTextNode(node.text);
    return node.el;
  }
  const el = doc.createElement(node.tag);
  node.el = el;
  applyProps(el, {}, node.props, doc, false);
  if (node.props.html !== undefined) el.innerHTML = node.props.html;
  for (const child of node.children) el.appendChild(create(child, doc, after));
  // The `value` of a <select> only sticks once its options exist.
  applyProps(el, {}, pick(node.props), doc, true);
  if (node.props.hooks?.insert) after.push(() => node.props.hooks.insert(el));
  return el;
}

function pick(props) {
  const out = {};
  for (const name of PROPERTIES) if (name in props) out[name] = props[name];
  return out;
}

function patch(old, node, doc, after) {
  const el = old.el;
  node.el = el;
  if (node.tag === TEXT) {
    if (old.text !== node.text) el.nodeValue = node.text;
    return;
  }
  applyProps(el, old.props, node.props, doc, false);
  if (node.props.html !== undefined && node.props.html !== old.props.html) {
    el.innerHTML = node.props.html;
  }
  if (node.props.opaque) node.children = old.children;
  else patchChildren(el, old.children, node.children, doc, after);
  applyProps(el, pick(old.props), pick(node.props), doc, true);
}

function patchChildren(parent, oldChildren, newChildren, doc, after) {
  const keyed = new Map();
  for (const child of oldChildren) if (child.key !== null) keyed.set(child.key, child);
  const reused = new Set();

  for (const [index, child] of newChildren.entries()) {
    let match;
    if (child.key !== null) {
      match = keyed.get(child.key);
    } else {
      const candidate = oldChildren[index];
      if (candidate !== undefined && candidate.key === null) match = candidate;
    }
    if (match !== undefined && !reused.has(match) && match.tag === child.tag) {
      reused.add(match);
      patch(match, child, doc, after);
    } else {
      create(child, doc, after);
    }
  }

  for (const child of oldChildren) {
    if (reused.has(child)) continue;
    parent.removeChild(child.el);
    destroy(child);
  }
  newChildren.forEach((child, index) => {
    const current = parent.childNodes[index];
    if (current !== child.el) parent.insertBefore(child.el, current ?? null);
  });
}

function destroy(node) {
  node.children.forEach(destroy);
  node.props.hooks?.destroy?.(node.el);
}

function applyProps(el, old, next, doc, propertiesOnly) {
  if (!propertiesOnly) {
    for (const name of Object.keys(old)) {
      if (!(name in next) && !RESERVED.has(name) && !PROPERTIES.has(name)) unset(el, name);
    }
  }
  for (const name of Object.keys(next)) {
    if (RESERVED.has(name) || PROPERTIES.has(name) !== propertiesOnly) continue;
    if (propertiesOnly || old[name] !== next[name]) set(el, name, next[name], doc);
  }
}

function unset(el, name) {
  if (isEvent(name)) listen(el, name, null);
  else el.removeAttribute(attribute(name));
}

function set(el, name, value, doc) {
  if (isEvent(name)) return listen(el, name, value);
  if (PROPERTIES.has(name)) {
    const wanted = name === 'value' ? String(value ?? '') : Boolean(value);
    // Do not move the caret of a field the buyer is typing in.
    const typing = name === 'value' && doc.activeElement === el;
    if (el[name] !== wanted && !typing) el[name] = wanted;
    return;
  }
  if (name === 'style' && value && typeof value === 'object') value = styleText(value);
  if (value === null || value === undefined || value === false) el.removeAttribute(attribute(name));
  else el.setAttribute(attribute(name), value === true ? '' : String(value));
}

function styleText(style) {
  return Object.entries(style)
    .filter(([, value]) => value !== null && value !== undefined && value !== false)
    .map(([name, value]) => `${name.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`)}:${value}`)
    .join(';');
}

const attribute = (name) => (name === 'className' ? 'class' : name);
const isEvent = (name) => /^on[A-Z]/.test(name);

function listen(el, name, handler) {
  const type = name.slice(2).toLowerCase();
  const handlers = (el.__lsfHandlers ??= {});
  if (!(type in handlers)) {
    // One listener per event type for the life of the element: the handlers change on every
    // render.
    el.addEventListener(type, (event) => handlers[type]?.(event));
  }
  handlers[type] = handler;
}
