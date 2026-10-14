// `@shopify/partial-rendering`, for the local server.
//
// `{% partial 'name' %}` renders its content between two comments, `<!--shopify-partial
// name="name"-->` and `<!--/shopify-partial-->`. This module finds those regions, asks the
// server for the page again and swaps the regions in, as Shopify's package does with its own
// markup (which Shopify has not published). The page is requested whole: the regions are
// taken out of it here.

const START = /^shopify-partial name="(.*)"$/s;
const END = '/shopify-partial';

// The server writes `&`, `"` and the `-` of a `--` (which a comment cannot hold) as entities.
const unescapeName = (name) =>
  name.replaceAll('&#45;', '-').replaceAll('&quot;', '"').replaceAll('&amp;', '&');

/**
 * Every partial under `scope`, in the order of the page (outermost first): `{ name, start,
 * end }`, the two comments. They can have different parents: an HTML parser moves the end of a
 * region of table rows into the `<tbody>` it adds.
 */
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
  return found.filter((region) => region.end);
}

const rangeOf = (region) => {
  const range = (region.start.ownerDocument ?? document).createRange();
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
  // Every region of a wanted name, in the order of the page: a partial rendered in a loop is
  // there once per turn.
  const partials = [];
  for (const region of regions(fresh)) {
    if (wanted && !wanted.has(region.name)) continue;
    const holder = fresh.createElement('template');
    holder.content.append(rangeOf(region).cloneContents());
    partials.push({ name: region.name, html: holder.innerHTML });
  }
  return { url: response.url, partials };
}

/**
 * The form controls of a region (a range of the page) or of a fragment, by a key that finds a
 * control again after a swap: its id, else its name and its position among the controls of
 * that name.
 */
function controlsIn(where) {
  const selector = 'input, textarea, select';
  let candidates;
  if (where instanceof Range) {
    const ancestor = where.commonAncestorContainer;
    const root = ancestor.nodeType === Node.ELEMENT_NODE ? ancestor : ancestor.parentNode;
    candidates = [...(root?.querySelectorAll(selector) ?? [])].filter((control) =>
      where.intersectsNode(control),
    );
  } else {
    candidates = [...where.querySelectorAll(selector)];
  }
  const keys = new Map();
  const counts = new Map();
  for (const control of candidates) {
    let key = control.id ? `#${control.id}` : null;
    if (!key && control.name) {
      const index = counts.get(control.name) ?? 0;
      counts.set(control.name, index + 1);
      key = `${control.name}[${index}]`;
    }
    if (key && !keys.has(key)) keys.set(key, control);
  }
  return keys;
}

/** Swaps one region for new HTML, keeping focus, selection and form values. */
function swap(region, html) {
  const range = rangeOf(region);
  const before = controlsIn(range);
  const active = document.activeElement;
  const focused = [...before].find(([, control]) => control === active)?.[0];
  const selection =
    focused && typeof active.selectionStart === 'number'
      ? [active.selectionStart, active.selectionEnd, active.selectionDirection]
      : null;

  const holder = document.createElement('template');
  holder.innerHTML = html;
  const fresh = holder.content;
  // The values of the page win over the markup, as Shopify documents for `apply()`: a theme
  // updates what the server changes itself.
  const after = controlsIn(fresh);
  for (const [key, control] of before) {
    const replacement = after.get(key);
    if (!replacement || replacement.type !== control.type || control.type === 'file') continue;
    if (control.type === 'checkbox' || control.type === 'radio') {
      replacement.checked = control.checked;
    } else {
      replacement.value = control.value;
    }
  }
  // Inserted where the region starts: when the end of a region of table rows sits in a
  // `<tbody>`, the new rows come in their own `<tbody>` beside it, not inside it.
  range.deleteContents();
  range.insertNode(fresh);

  if (focused) {
    const replacement = controlsIn(rangeOf(region)).get(focused);
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

/**
 * Swaps in the partials of an update, keeping focus, selection, form values and scroll. The
 * n-th partial of a name in the update replaces the n-th one of that name on the page, counted
 * over the whole page; with a `scope`, only those inside it are replaced. A name the update
 * has fewer of than the page (a line removed from the cart) leaves the extra regions empty.
 */
function apply(update, scope) {
  const scrollX = window.scrollX;
  const scrollY = window.scrollY;
  const inScope = (region) => !scope || scope === document || scope.contains(region.start);
  const byName = new Map();
  for (const partial of update?.partials ?? []) {
    if (!byName.has(partial.name)) byName.set(partial.name, []);
    byName.get(partial.name).push(partial.html);
  }
  for (const [name, htmls] of byName) {
    const count = regions().filter((region) => region.name === name).length;
    for (let index = 0; index < Math.max(count, htmls.length); index++) {
      // Found again each time: a swap can replace regions inside the one swapped.
      const region = regions().filter((candidate) => candidate.name === name)[index];
      if (!region || !inScope(region)) continue;
      swap(region, htmls[index] ?? '');
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
