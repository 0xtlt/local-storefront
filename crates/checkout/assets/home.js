// The home page of `lsf checkout`. The page is complete without this script; it adds:
// - the filter of the scenarios: a search on their title, file and extension handles, and
//   chips (all, with problems, uses Functions, uses UI extensions), with the count of the rows
//   shown. The scenarios are in groups (`data-group`) that open while a filter is on, and say
//   how many of their scenarios have problems;
// - a link to a part of the page (`#scenario-…`, `#function-…`) opens the groups and folds
//   around it;
// - the check of the Functions: when the server has not run the Functions of the scenarios yet,
//   the page asks `/__lsf/checkouts/check` once it is shown, and puts what it found in place
//   (the list of problems, the rows of the scenarios and Functions that fail, the order of the
//   scenarios). A check that fails or takes too long says so, the rest of the page stays;
// - the load of the checkout UI extensions: each bundle is loaded in a hidden frame of the
//   sandbox, as the checkout loads it, to see the targets it registers. A bundle that does not
//   load, throws while it loads, or does not register a target it declares, is listed with the
//   problems, and the scenarios that run it take its level. A bundle can itself call the
//   network as it loads. The page fetches the bundle and hands its text to the sandbox, which
//   runs it from a `blob:` URL of its own: what it throws reaches the page with its message,
//   though the frame has an opaque origin.

import { bundleSource } from './extension-host.js';

/** How long the page waits for the check, in milliseconds. */
export const CHECK_TIMEOUT = 30000;

/** Whether a scenario row (`{search, status, uses}`) is shown for the filter `{query, chip}`. */
export function rowMatches(row, { query = '', chip = 'all' } = {}) {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.every((word) => row.search.includes(word))) return false;
  switch (chip) {
    case 'problems':
      return row.status === 'error' || row.status === 'warning';
    case 'functions':
      return row.uses.includes('functions');
    case 'ui':
      return row.uses.includes('ui');
    default:
      return true;
  }
}

/** What the count under the filter says. */
export function countText(shown, total) {
  const noun = (n) => (n === 1 ? 'scenario' : 'scenarios');
  return shown === total ? `${total} ${noun(total)}` : `${shown} of ${total} ${noun(total)} shown`;
}

function rowOf(element) {
  const data = element.dataset;
  return { search: data.search ?? '', status: data.status ?? 'ok', uses: (data.uses ?? '').split(' ') };
}

/** Shows the rows of `list` that match `filter`, and says how many. */
export function applyFilter(list, filter) {
  const rows = [...list.querySelectorAll('[data-scenario]')];
  let shown = 0;
  for (const row of rows) {
    const match = rowMatches(rowOf(row), filter);
    row.hidden = !match;
    if (match) shown += 1;
  }
  return { shown, total: rows.length };
}

/** The rows of `list` with problems first (errors, then warnings), then by name, as the server orders them. */
export function sortRows(list) {
  const rank = (row) => ({ error: 0, warning: 1 })[row.dataset.status] ?? 2;
  const rows = [...list.querySelectorAll('[data-scenario]')];
  rows.sort((a, b) => rank(a) - rank(b) || (a.dataset.scenario < b.dataset.scenario ? -1 : a.dataset.scenario > b.dataset.scenario ? 1 : 0));
  reorder(list, rows.map((row) => row.id));
}

/** Puts the rows of `list` in the order of `ids` (the others stay after them); the ids of
 * rows of other lists are skipped. */
export function reorder(list, ids) {
  for (const id of ids) {
    const row = list.querySelector(`[id="${id}"]`);
    if (row) list.appendChild(row);
  }
}

/** The lists of scenarios: one, or one in each group. */
const scenarioLists = (document) => [...document.querySelectorAll('[data-list="scenarios"]')];

/**
 * Brings each group of scenarios in line with its rows: the worst status of its rows as its
 * dot, how many have problems, hidden when the filter shows none of them, and open while a
 * filter is on (closed again once it is off, unless the visitor opened it).
 */
export function updateGroups(document, filtering = false) {
  const rank = (status) => ({ error: 0, warning: 1 })[status] ?? 2;
  for (const group of document.querySelectorAll('[data-group]')) {
    const rows = [...group.querySelectorAll('[data-scenario]')];
    const statuses = rows.map((row) => row.getAttribute('data-status') ?? 'ok');
    group.hidden = rows.every((row) => row.hidden);
    if (filtering && !group.open) {
      group.open = true;
      group.setAttribute('data-opened-by-filter', '');
    } else if (!filtering && group.hasAttribute('data-opened-by-filter')) {
      group.open = false;
      group.removeAttribute('data-opened-by-filter');
    }
    const worst = [...statuses].sort((a, b) => rank(a) - rank(b))[0] ?? 'ok';
    group.setAttribute('data-worst', worst);
    const slot = group.querySelector('[data-group-dot]');
    if (slot && !slot.querySelector(`.lsf-dot.is-${worst}`)) {
      slot.replaceChildren(worst === 'ok' ? element(document, 'span', { class: 'lsf-dot is-ok', role: 'img', 'aria-label': 'OK' }) : dot(document, worst));
    }
    const troubled = statuses.filter((status) => rank(status) < 2).length;
    const problems = group.querySelector('[data-group-problems]');
    if (problems) problems.textContent = troubled ? `${troubled} with problems` : '';
  }
}

function setUpFilter(document) {
  const box = document.querySelector('[data-filter]');
  const lists = scenarioLists(document);
  if (!box || lists.length === 0) return () => {};
  const query = box.querySelector('[data-filter-query]');
  const count = box.querySelector('[data-filter-count]');
  const empty = document.querySelector('[data-filter-empty]');
  const chips = [...box.querySelectorAll('[data-filter-chip]')];
  const filter = { query: '', chip: 'all' };
  const apply = () => {
    let shown = 0;
    let total = 0;
    for (const list of lists) {
      const counted = applyFilter(list, filter);
      shown += counted.shown;
      total += counted.total;
    }
    count.textContent = countText(shown, total);
    if (empty) empty.hidden = shown > 0;
    updateGroups(document, filter.query.trim() !== '' || filter.chip !== 'all');
  };
  query.addEventListener('input', () => {
    filter.query = query.value;
    apply();
  });
  for (const chip of chips) {
    chip.addEventListener('click', () => {
      filter.chip = chip.dataset.filterChip;
      for (const other of chips) other.setAttribute('aria-pressed', String(other === chip));
      apply();
    });
  }
  // The filter is on the page from the start, waiting for this script: it only wakes up.
  query.disabled = false;
  for (const chip of chips) chip.disabled = false;
  apply();
  return apply;
}

function say(section, text) {
  const line = section.querySelector('[data-check-line]');
  if (!line) return;
  line.textContent = text;
  line.title = text;
  line.classList.add('is-error');
}

/** Asks for the check of the Functions, and puts what it found in the page. */
async function check(document, window, refilter) {
  const section = document.querySelector('[data-section="problems"]');
  if (!section || section.dataset.check !== 'pending') return;
  const url = section.dataset.checkUrl;
  const controller = new window.AbortController();
  const timer = window.setTimeout(() => controller.abort(), CHECK_TIMEOUT);
  let answer;
  try {
    const response = await window.fetch(url, { signal: controller.signal, headers: { accept: 'application/json' } });
    if (!response.ok) throw new Error(`the server answered ${response.status}`);
    answer = await response.json();
  } catch (error) {
    say(section, `Could not check the Functions of the scenarios: ${error.name === 'AbortError' ? 'it took too long' : error.message}.`);
    return;
  } finally {
    window.clearTimeout(timer);
  }
  if (answer.state === 'error' || typeof answer.problems !== 'string') {
    say(section, `Could not check the Functions of the scenarios: ${answer.error ?? 'no answer'}.`);
    return;
  }
  section.outerHTML = answer.problems;
  for (const [id, html] of Object.entries(answer.rows ?? {})) {
    const row = document.getElementById(id);
    if (row) row.outerHTML = html;
  }
  if (Array.isArray(answer.order)) for (const list of scenarioLists(document)) reorder(list, answer.order);
  refilter();
}

/** How long a bundle may take to load in its frame, in milliseconds. */
export const PROBE_TIMEOUT = 5000;

/** How many problems a group shows before the others are folded, as on the server. */
const SHOWN_PROBLEMS = 8;

/** How many bundles load at once. */
const PROBES_AT_ONCE = 4;

/**
 * What loading its bundle says of a checkout UI extension that declares the targets `declared`:
 * null when it registered them all, otherwise `{severity, message}`.
 * - `registered` is what it registered, or null when the frame did not answer in time;
 * - `errors` is what the frame reported while it loaded;
 * - `loaded` is false when the bundle could not be loaded at all (no file, a network error).
 * An error the frame reported is never dropped: only a frame that reported nothing and did not
 * answer leaves it open, as a warning.
 */
export function verdict(declared, registered, errors, loaded = true) {
  const thrown = errors.find((message) => message && message !== 'Script error.');
  if (registered === null) {
    if (errors.length === 0) {
      return { severity: 'warning', message: `could not tell whether its bundle loads: the frame did not answer in ${PROBE_TIMEOUT / 1000} s` };
    }
    return {
      severity: 'error',
      message: thrown
        ? `its bundle fails while it loads (${thrown}), and does not finish loading`
        : 'throws while its bundle loads, and does not finish loading. Open a checkout with it to read the error.',
    };
  }
  if (!loaded) return { severity: 'error', message: thrown ? `its bundle does not load: ${thrown}` : 'its bundle does not load' };
  const missing = declared.filter((target) => !registered.includes(target));
  const what = thrown ? `throws while its bundle loads (${thrown})` : 'throws while its bundle loads';
  if (missing.length === 0) {
    return errors.length > 0 ? { severity: 'warning', message: `${what}, after it registered its targets` } : null;
  }
  if (registered.length === 0) {
    return errors.length > 0
      ? { severity: 'error', message: thrown ? `${what}: it registers no target` : `${what}: it registers no target. Open a checkout with it to read the error.` }
      : { severity: 'error', message: 'its bundle registers no target: `shopify.extend` is never called' };
  }
  return {
    severity: 'error',
    message: `its bundle does not register ${missing.join(', ')} (it registers ${registered.join(', ')})`,
  };
}

/**
 * Loads the bundle of one extension in a hidden frame of the sandbox at `sandboxUrl`. Resolves
 * `{row, targets, registered, loaded, errors}`: `registered` is null when the frame did not
 * answer within `timeout` ms. The frame and the listener go away whatever happens.
 */
export function probe(document, window, sandboxUrl, row, timeout = PROBE_TIMEOUT) {
  const data = row.dataset;
  const targets = (data.targets ?? '').split(' ').filter(Boolean);
  const script = new URL(data.probe, document.baseURI).href;
  // Fetched while the frame loads.
  const source = bundleSource(window, script);
  return new Promise((resolve) => {
    const frame = document.createElement('iframe');
    frame.setAttribute('sandbox', 'allow-scripts');
    frame.setAttribute('aria-hidden', 'true');
    frame.setAttribute('tabindex', '-1');
    frame.style.cssText = 'position:absolute;width:0;height:0;border:0;visibility:hidden';
    const errors = [];
    let timer = null;
    let done = false;
    const finish = (registered, loaded = registered !== null) => {
      if (done) return;
      done = true;
      window.removeEventListener('message', listen);
      window.clearTimeout(timer);
      frame.remove();
      resolve({ row, targets, registered, loaded, errors });
    };
    const listen = (event) => {
      const message = event.data;
      if (done || event.source !== frame.contentWindow || !message || message.lsf !== true) return;
      if (message.type === 'ready') {
        source.then((text) => {
          if (done) return;
          frame.contentWindow.postMessage({
            lsf: true,
            type: 'init',
            probe: true,
            handle: data.uiExtension,
            target: targets[0],
            script,
            source: text,
            apiVersion: data.api,
            ctx: {},
            ext: { settings: {}, capabilities: {}, translations: {}, defaultLocale: null },
            state: {},
          }, '*');
        });
      } else if (message.type === 'error') {
        errors.push(String(message.message));
      } else if (message.type === 'probed') {
        finish(Array.isArray(message.targets) ? message.targets : [], message.loaded !== false);
      }
    };
    timer = window.setTimeout(() => finish(null), timeout);
    window.addEventListener('message', listen);
    try {
      frame.setAttribute('src', sandboxUrl);
      document.body.appendChild(frame);
    } catch (error) {
      errors.push(String(error && error.message ? error.message : error));
      finish([], false);
    }
  });
}

function element(document, tag, attributes = {}, ...children) {
  const node = document.createElement(tag);
  for (const [name, value] of Object.entries(attributes)) node.setAttribute(name, value);
  for (const child of children) node.append(child);
  return node;
}

/**
 * A message as nodes: what is between backticks as `<code>`, the rest as text. Never HTML: the
 * message is only ever text content.
 */
export function richText(document, text) {
  const parts = String(text).split('`');
  const nodes = [];
  parts.forEach((part, index) => {
    if (index % 2 === 0) nodes.push(part);
    else if (index + 1 < parts.length) nodes.push(element(document, 'code', {}, part));
    else nodes.push(`\`${part}`);
  });
  return nodes.filter((node) => node !== '');
}

/** The dot of a status, as the server draws it. */
function dot(document, status) {
  const label = status === 'error' ? 'Error' : 'Warning';
  return element(document, 'span', { class: `lsf-dot is-${status}`, role: 'img', 'aria-label': label });
}

/**
 * The link on the right of a problem found in the browser: a fresh checkout of the scenario
 * that runs the extension, as the server picks it (`data-open`, `data-open-in`), or of the
 * store's cart, where the extension loads and shows what goes wrong; without one, its row on
 * this page.
 */
export function openLink(document, row) {
  const href = row.dataset.open;
  if (!href) return element(document, 'a', { class: 'lsf-hp-where', href: `#${row.id}` }, 'details');
  // An extension that only renders in the customer account shows its problem there.
  if ('openAccount' in row.dataset) {
    return element(document, 'a', { class: 'lsf-hp-where lsf-hp-open', href, title: 'Opens the customer account, where it renders' }, 'Open the customer account');
  }
  const scenario = row.dataset.openIn || '';
  return element(document, 'a', {
    class: 'lsf-hp-where lsf-hp-open',
    href,
    title: `Opens a fresh checkout of ${scenario ? `the scenario ${scenario}` : "the store's cart"}`,
  }, 'Open in checkout ', element(document, 'span', { class: 'lsf-hp-in' }, scenario || "store's cart"));
}

/** Adds a problem found in the browser to the list, and says it on the row of the extension. */
function report(document, { row, problem }) {
  const handle = row.dataset.uiExtension;
  const section = document.querySelector('[data-section="problems"]');
  if (section) {
    const { severity, message } = problem;
    const groups = section.querySelector('[data-problem-groups]') ?? section;
    let group = section.querySelector(`.lsf-hp-group.is-${severity}`);
    if (!group) {
      group = element(document, 'div', { class: `lsf-hp-group is-${severity}` },
        element(document, 'h3', {}, severity === 'error' ? 'Errors ' : 'Warnings ', element(document, 'span', { class: 'lsf-h-n' }, '0')),
        element(document, 'ul', { class: 'lsf-hp-rows' }));
      const warnings = groups.querySelector('.lsf-hp-group.is-warning');
      if (severity === 'error' && warnings) groups.insertBefore(group, warnings);
      else groups.appendChild(group);
      // The fold takes the place of "No problems", on the same line.
      section.querySelector('.lsf-hp-clear')?.remove();
      const fold = section.querySelector('.lsf-hp-all');
      if (fold) fold.hidden = false;
    }
    const line = element(document, 'li', { class: `lsf-hp-row is-${severity}`, 'data-problem': 'load', 'data-subject': handle },
      element(document, 'details', {},
        element(document, 'summary', {}, dot(document, severity),
          element(document, 'span', { class: 'lsf-hp-kind' }, 'UI extension'),
          element(document, 'code', {}, handle),
          element(document, 'span', { class: 'lsf-hp-msg', title: message }, ...richText(document, message))),
        element(document, 'div', { class: 'lsf-hp-full' },
          element(document, 'pre', {}, ...richText(document, message)),
          element(document, 'p', {}, 'Found by loading its bundle in this page, as the checkout does.'),
          element(document, 'p', { class: 'lsf-hp-rowlinks' }, 'On this page: ',
            element(document, 'a', { href: `#${row.id}` }, handle), 'openAccount' in row.dataset ? ' in the customer account UI extensions' : ' in the checkout UI extensions'))),
      openLink(document, row));
    // In the order of the server (scenarios last), with the rows past the first few folded.
    const list = group.querySelector('.lsf-hp-rows');
    list.insertBefore(line, list.querySelector('[data-problem="scenario"]'));
    while (list.children.length > SHOWN_PROBLEMS) {
      let more = group.querySelector('.lsf-hp-more');
      if (!more) {
        more = group.appendChild(element(document, 'details', { class: 'lsf-hp-more' },
          element(document, 'summary', {}), element(document, 'ul', { class: 'lsf-hp-rows' })));
      }
      const rest = more.querySelector('.lsf-hp-rows');
      rest.insertBefore(list.lastElementChild, rest.firstElementChild);
      more.querySelector('summary').textContent = `Show ${rest.children.length} more`;
    }
    group.querySelector('.lsf-h-n').textContent = String(section.querySelectorAll(`.lsf-hp-row.is-${severity}`).length);
    const errors = section.querySelectorAll('.lsf-hp-row.is-error').length;
    const warnings = section.querySelectorAll('.lsf-hp-row.is-warning').length;
    const counts = [];
    if (errors) counts.push(`${errors} error${errors === 1 ? '' : 's'}`);
    if (warnings) counts.push(`${warnings} warning${warnings === 1 ? '' : 's'}`);
    const head = section.querySelector('.lsf-h-head');
    let count = head.querySelector('.lsf-h-count');
    if (!count) count = head.appendChild(element(document, 'p', { class: 'lsf-h-count' }));
    count.textContent = counts.join(' · ');
    const fold = section.querySelector('[data-problem-summary]');
    if (fold) fold.textContent = `Show the ${counts.join(' and ')}`;
    section.classList.remove('is-clear', 'has-warnings', 'has-errors');
    section.classList.add(errors ? 'has-errors' : 'has-warnings');
  }
  // The row of the extension.
  const { severity, message } = problem;
  if (row.dataset.status !== 'error' && row.dataset.status !== severity) {
    row.dataset.status = severity;
    row.querySelector('.lsf-dot')?.replaceWith(dot(document, severity));
  }
  // On the line the row keeps for it: the row does not grow.
  const issue = row.querySelector('[data-issue]');
  if (issue && issue.childNodes.length === 0) {
    issue.setAttribute('class', `lsf-h-meta is-${severity}`);
    issue.setAttribute('title', message);
    issue.append(...richText(document, message));
  } else if (issue) {
    issue.append(' · ', ...richText(document, message));
  } else {
    row.querySelector('.lsf-h-main')?.appendChild(
      element(document, 'span', { class: `lsf-h-meta is-${severity}`, title: message }, ...richText(document, message)));
  }
  markScenarios(document, handle, severity);
}

/**
 * The scenarios that run the extension `handle`, once its bundle showed a problem of
 * `severity`: as on the server, a scenario takes the worst of its own status and of the
 * problems of the extensions it runs (an error stays one, a warning becomes an error only when
 * the problem is an error).
 */
export function markScenarios(document, handle, severity = 'warning') {
  const rank = (status) => ({ error: 0, warning: 1 })[status] ?? 2;
  for (const row of document.querySelectorAll('[data-scenario]')) {
    const uses = (row.dataset.uiExtensions ?? '').split(' ');
    if (!uses.includes(handle) || rank(row.dataset.status) <= rank(severity)) continue;
    row.dataset.status = severity;
    row.querySelector('.lsf-dot')?.replaceWith(dot(document, severity));
  }
}

/** Loads the bundle of every checkout UI extension the checkout loads, a few at a time. */
async function probeAll(document, window, refilter) {
  const rows = [...document.querySelectorAll('[data-probe]')];
  if (rows.length === 0) return;
  const sandboxUrl = new URL('sandbox.html', import.meta.url).href;
  let next = 0;
  const worker = async () => {
    while (next < rows.length) {
      const result = await probe(document, window, sandboxUrl, rows[next++]);
      const problem = verdict(result.targets, result.registered, result.errors, result.loaded);
      if (problem) report(document, { row: result.row, problem });
    }
  };
  await Promise.all(Array.from({ length: Math.min(PROBES_AT_ONCE, rows.length) }, worker));
  // The scenarios that now have problems move up with the others.
  for (const list of scenarioLists(document)) sortRows(list);
  refilter();
}

/** Opens what holds the element `#id` of the page (its group, its row, a fold), then shows it. */
export function reveal(document, id) {
  const target = id && document.getElementById(id);
  if (!target) return false;
  for (let node = target.parentNode; node; node = node.parentNode) {
    if (node.localName === 'details') node.open = true;
  }
  // A row opens too: its details are what the link is about.
  const details = (target.childNodes ?? []).find((child) => child.localName === 'details');
  if (details) details.open = true;
  target.scrollIntoView?.({ block: 'start' });
  return true;
}

function setUpLinks(document, window) {
  const follow = () => reveal(document, decodeURIComponent((window.location?.hash ?? '').slice(1)));
  window.addEventListener?.('hashchange', follow);
  follow();
}

export async function start(document, window) {
  setUpLinks(document, window);
  const refilter = setUpFilter(document);
  await check(document, window, refilter);
  // After the check: its answer replaces the list of problems.
  await probeAll(document, window, refilter);
}

if (typeof document !== 'undefined' && typeof window !== 'undefined') {
  start(document, window);
}
