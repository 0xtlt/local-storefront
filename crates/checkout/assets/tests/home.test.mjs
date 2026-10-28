// The script of the home page of `lsf checkout`: the filter of the scenarios, and the check of
// the Functions it asks for.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { applyFilter, countText, reorder, rowMatches, start } from '../home.js';

const row = (search, status = 'ok', uses = 'functions ui') => ({ search, status, uses: uses.split(' ') });

test('a search keeps the rows that have every word', () => {
  const tees = row('three tees in french tees tees.json volume-discount gift-message');
  assert.equal(rowMatches(tees, { query: '' }), true);
  assert.equal(rowMatches(tees, { query: 'Tees' }), true);
  assert.equal(rowMatches(tees, { query: 'french volume' }), true);
  assert.equal(rowMatches(tees, { query: 'french  shop' }), false);
  // A handle is a word like the others.
  assert.equal(rowMatches(tees, { query: 'gift-message' }), true);
});

test('the chips keep the rows with problems, or that use Functions or UI extensions', () => {
  const ok = row('a', 'ok', 'functions');
  const warning = row('b', 'warning', 'ui');
  const error = row('c', 'error', '');
  const problems = { chip: 'problems' };
  assert.deepEqual([ok, warning, error].map((r) => rowMatches(r, problems)), [false, true, true]);
  assert.deepEqual([ok, warning, error].map((r) => rowMatches(r, { chip: 'functions' })), [true, false, false]);
  assert.deepEqual([ok, warning, error].map((r) => rowMatches(r, { chip: 'ui' })), [false, true, false]);
  assert.deepEqual([ok, warning, error].map((r) => rowMatches(r, { chip: 'all' })), [true, true, true]);
  // A search and a chip together.
  assert.equal(rowMatches(warning, { query: 'b', chip: 'problems' }), true);
  assert.equal(rowMatches(warning, { query: 'a', chip: 'problems' }), false);
});

test('the count says how many rows are shown', () => {
  assert.equal(countText(82, 82), '82 scenarios');
  assert.equal(countText(3, 82), '3 of 82 scenarios shown');
  assert.equal(countText(1, 1), '1 scenario');
});

/** A list of rows as the page has them: elements with `data-*` and an id. */
function fakeList(rows) {
  const children = rows.map(([id, search, status, uses]) => ({ id, hidden: false, dataset: { scenario: id, search, status, uses } }));
  return {
    children,
    querySelectorAll: () => [...children],
    querySelector: (selector) => children.find((child) => selector === `[id="${child.id}"]`) ?? null,
    appendChild(child) {
      children.splice(children.indexOf(child), 1);
      children.push(child);
    },
  };
}

test('the filter hides the rows that do not match and counts the others', () => {
  const list = fakeList([
    ['a', 'guest tees', 'ok', 'functions'],
    ['b', 'b2b tees', 'error', 'ui'],
    ['c', 'abroad', 'warning', 'functions ui'],
  ]);
  assert.deepEqual(applyFilter(list, { query: 'tees', chip: 'all' }), { shown: 2, total: 3 });
  assert.deepEqual(list.children.map((child) => child.hidden), [false, false, true]);
  assert.deepEqual(applyFilter(list, { query: '', chip: 'problems' }), { shown: 2, total: 3 });
  assert.deepEqual(list.children.map((child) => child.hidden), [true, false, false]);
  assert.deepEqual(applyFilter(list, { query: '', chip: 'all' }), { shown: 3, total: 3 });
});

test('the rows take the order the check gives them', () => {
  const list = fakeList([['scenario-a'], ['scenario-b'], ['scenario-c']]);
  reorder(list, ['scenario-c', 'scenario-a', 'scenario-gone']);
  assert.deepEqual(list.children.map((child) => child.id), ['scenario-b', 'scenario-c', 'scenario-a']);
});

/** A page with the problems still to check, and the window that answers the check. */
function fakePage(answer) {
  const line = { textContent: 'Running…', classList: new Set() };
  line.classList.add = line.classList.add.bind(line.classList);
  const section = {
    dataset: { check: 'pending', checkUrl: '/__lsf/checkouts/check' },
    outerHTML: '<section>pending</section>',
    querySelector: (selector) => (selector === '[data-check-line]' ? line : null),
  };
  const rows = { 'scenario-crash': { outerHTML: '<li>before</li>' } };
  const document = {
    querySelector: (selector) => (selector === '[data-section="problems"]' ? section : null),
    getElementById: (id) => rows[id] ?? null,
    querySelectorAll: () => [],
  };
  const asked = [];
  const window = {
    AbortController,
    setTimeout,
    clearTimeout,
    fetch: async (url) => {
      asked.push(url);
      if (answer instanceof Error) throw answer;
      return { ok: true, json: async () => answer };
    },
  };
  return { document, window, section, line, rows, asked };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 10));

test('what the check found takes the place of the problems and of the rows that fail', async () => {
  const page = fakePage({
    state: 'done',
    problems: '<section data-check="done">found</section>',
    rows: { 'scenario-crash': '<li data-status="error">after</li>', 'scenario-gone': '<li></li>' },
    order: ['scenario-crash'],
  });
  start(page.document, page.window);
  await settle();
  assert.deepEqual(page.asked, ['/__lsf/checkouts/check']);
  assert.equal(page.section.outerHTML, '<section data-check="done">found</section>');
  assert.equal(page.rows['scenario-crash'].outerHTML, '<li data-status="error">after</li>');
});

test('a check that fails says so, and leaves the page as it is', async () => {
  const page = fakePage(new Error('connection refused'));
  start(page.document, page.window);
  await settle();
  assert.equal(page.section.outerHTML, '<section>pending</section>');
  assert.match(page.line.textContent, /Could not check the Functions of the scenarios: connection refused/);
  assert.ok(page.line.classList.has('is-error'));

  const refused = fakePage({ state: 'error', error: 'the app cannot be read' });
  start(refused.document, refused.window);
  await settle();
  assert.match(refused.line.textContent, /the app cannot be read/);
});

test('a page whose check already ran asks for nothing', async () => {
  const page = fakePage({});
  page.section.dataset.check = 'done';
  start(page.document, page.window);
  await settle();
  assert.deepEqual(page.asked, []);
});
