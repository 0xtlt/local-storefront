// `@shopify/partial-rendering`, for the local server.
//
// `{% partial 'name' %}` renders its content between two comments, `<!--shopify-partial
// name="name"-->` and `<!--/shopify-partial-->`. This module finds those regions, asks the
// server for the page again and swaps the regions in, as Shopify's package does with its own
// markup (which Shopify has not published). The page is requested whole: the regions are
// taken out of it here.

const START = /^shopify-partial name="(.*)"$/s;
const END = '/shopify-partial';

const unescapeName = (name) => name.replaceAll('&quot;', '"');

/** Every partial under `scope`, outermost first: `{ name, start, end }` (the two comments). */
function regions(scope) {
  const root = scope ?? document;
  const owner = root.ownerDocument ?? root;
  const walker = owner.createTreeWalker(root, NodeFilter.SHOW_COMMENT);
  const found = [];
  const open = [];
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const start = START.exec(node.data);
    if (start) {
      const region = { name: unescapeName(start[1]), start: node, end: null };
      found.push(region);
      open.push(region);
    } else if (node.data === END && open.length > 0) {
      open.pop().end = node;
    }
  }
  return found.filter((region) => region.end && region.start.parentNode === region.end.parentNode);
}

const rangeOf = (region) => {
  const range = document.createRange();
  range.setStartAfter(region.start);
  range.setEndBefore(region.end);
  return range;
};

/** The partials on the page, or under an element: `{ name, range }`. */
function getAll(scope) {
  return regions(scope).map((region) => ({ name: region.name, range: rangeOf(region) }));
}

/** The first partial of a name, on the page or under an element. */
function get(name, scope) {
  const region = regions(scope).find((candidate) => candidate.name === name);
  return region ? { name: region.name, range: rangeOf(region) } : null;
}

/** `fetch('a', 'b', { url, signal })`: the names, then the options when there are some. */
function namesAndOptions(args) {
  const last = args[args.length - 1];
  const hasOptions = last !== null && typeof last === 'object' && !(last instanceof Node);
  return {
    names: (hasOptions ? args.slice(0, -1) : args).map(String),
    options: hasOptions ? last : {},
  };
}

/** The HTML of each named partial on the page at `options.url` (the current page by default). */
async function fetchPartials(...args) {
  const { names, options } = namesAndOptions(args);
  const response = await window.fetch(options.url ?? window.location.href, {
    method: options.method ?? 'GET',
    body: options.body,
    headers: options.headers,
    signal: options.signal,
    credentials: 'same-origin',
  });
  if (!response.ok) {
    throw new Error(`Partials request failed: ${response.status} ${response.statusText}`);
  }
  const html = await response.text();
  const fresh = new DOMParser().parseFromString(html, 'text/html');
  const wanted = names.length > 0 ? new Set(names) : null;
  const partials = [];
  const seen = new Set();
  for (const region of regions(fresh)) {
    if ((wanted && !wanted.has(region.name)) || seen.has(region.name)) continue;
    seen.add(region.name);
    const range = fresh.createRange();
    range.setStartAfter(region.start);
    range.setEndBefore(region.end);
    const holder = fresh.createElement('template');
    holder.content.append(range.cloneContents());
    partials.push({ name: region.name, html: holder.innerHTML });
  }
  return { url: response.url, partials };
}

/** The nodes of a region, between its two comments. */
function nodesBetween(region) {
  const nodes = [];
  for (let node = region.start.nextSibling; node && node !== region.end; node = node.nextSibling) {
    nodes.push(node);
  }
  return nodes;
}

/**
 * The form controls among `nodes` and their descendants, by a key that finds a control again
 * after a swap: its id, else its name and its position among the controls of that name.
 */
function controlsIn(nodes) {
  const keys = new Map();
  const counts = new Map();
  const visit = (control) => {
    let key = control.id ? `#${control.id}` : null;
    if (!key && control.name) {
      const index = counts.get(control.name) ?? 0;
      counts.set(control.name, index + 1);
      key = `${control.name}[${index}]`;
    }
    if (key && !keys.has(key)) keys.set(key, control);
  };
  const selector = 'input, textarea, select';
  for (const node of nodes) {
    if (node.nodeType !== Node.ELEMENT_NODE) continue;
    if (node.matches(selector)) visit(node);
    node.querySelectorAll(selector).forEach(visit);
  }
  return keys;
}

/** Swaps in the partials of an update, keeping focus, selection, form values and scroll. */
function apply(update, scope) {
  const scrollX = window.scrollX;
  const scrollY = window.scrollY;
  for (const partial of update?.partials ?? []) {
    const region = regions(scope).find((candidate) => candidate.name === partial.name);
    if (!region) continue;
    const old = nodesBetween(region);
    const before = controlsIn(old);
    const active = document.activeElement;
    const focused = [...before].find(([, control]) => control === active)?.[0];
    const selection =
      focused && typeof active.selectionStart === 'number'
        ? [active.selectionStart, active.selectionEnd, active.selectionDirection]
        : null;

    const holder = document.createElement('template');
    holder.innerHTML = partial.html;
    const fresh = holder.content;
    const after = controlsIn([...fresh.childNodes]);
    for (const [key, control] of before) {
      const replacement = after.get(key);
      if (!replacement || replacement.type !== control.type) continue;
      if (control.type === 'checkbox' || control.type === 'radio') {
        replacement.checked = control.checked;
      } else if (control.type !== 'file') {
        replacement.value = control.value;
      }
    }
    for (const node of old) node.remove();
    region.end.before(fresh);

    if (focused) {
      const replacement = controlsIn(nodesBetween(region)).get(focused);
      replacement?.focus({ preventScroll: true });
      if (replacement && selection) {
        try {
          replacement.setSelectionRange(...selection);
        } catch {
          // Some input types have no selection.
        }
      }
    }
  }
  window.scrollTo(scrollX, scrollY);
}

/**
 * Fetches the current page and applies the named partials: every partial of the page when
 * none is named, or the partials inside an element when one is given.
 */
async function refresh(...args) {
  const first = args[0];
  if (first instanceof Element || first instanceof Document) {
    const names = [...new Set(regions(first).map((region) => region.name))];
    const update = await fetchPartials(...names, { ...(args[1] ?? {}), url: window.location.href });
    apply(update, first);
    return update;
  }
  const { names, options } = namesAndOptions(args);
  const update = await fetchPartials(...names, { ...options, url: window.location.href });
  apply(update);
  return update;
}

export const partials = { fetch: fetchPartials, apply, refresh, get, getAll };
export default partials;
