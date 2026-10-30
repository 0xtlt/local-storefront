// The virtual DOM: what it keeps when the page is drawn again.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { h, render } from '../vdom.js';
import { FakeDocument, FakeEvent } from './fake-dom.mjs';

function page() {
  const document = new FakeDocument();
  const root = document.createElement('div');
  document.body.appendChild(root);
  return { document, root };
}

test('builds the elements, attributes and text of a tree', () => {
  const { root } = page();
  render(root, h('p', { class: 'a', 'data-x': '1', hidden: false }, 'Hello ', h('b', null, 'world'), null, false, 3));
  assert.equal(root.firstChild.outerHTML, '<p class="a" data-x="1">Hello <b>world</b>3</p>');
});

test('updates in place: the same elements survive a second render', () => {
  const { root } = page();
  render(root, h('div', null, h('input', { id: 'a', value: 'x' }), h('span', null, 'one')));
  const [input, span] = root.firstChild.childNodes;
  render(root, h('div', { class: 'now' }, h('input', { id: 'a', value: 'y' }), h('span', null, 'two')));
  assert.equal(root.firstChild.childNodes[0], input);
  assert.equal(root.firstChild.childNodes[1], span);
  assert.equal(input.value, 'y');
  assert.equal(span.textContent, 'two');
  assert.equal(root.firstChild.className, 'now');
});

test('removes attributes that are no longer there', () => {
  const { root } = page();
  render(root, h('div', { title: 't', class: 'c' }));
  render(root, h('div', { class: 'c' }));
  assert.equal(root.firstChild.hasAttribute('title'), false);
  assert.equal(root.firstChild.className, 'c');
});

test('keyed children keep their element when they move', () => {
  const { root } = page();
  const items = (keys) => h('ul', null, keys.map((key) => h('li', { key }, key)));
  render(root, items(['a', 'b', 'c']));
  const [a, b, c] = root.firstChild.childNodes;
  render(root, items(['c', 'a', 'b']));
  assert.deepEqual(root.firstChild.childNodes, [c, a, b]);
  render(root, items(['b']));
  assert.deepEqual(root.firstChild.childNodes, [b]);
  assert.equal(c.parentNode, null);
});

test('does not move the caret of a field being typed in', () => {
  const { document, root } = page();
  render(root, h('input', { value: '' }));
  const input = root.firstChild;
  input.value = 'abc';
  document.activeElement = input;
  render(root, h('input', { value: '' }));
  assert.equal(input.value, 'abc');
  document.activeElement = null;
  render(root, h('input', { value: 'server' }));
  assert.equal(input.value, 'server');
});

test('handlers change without adding listeners', () => {
  const { root } = page();
  const calls = [];
  render(root, h('button', { onClick: () => calls.push('first') }));
  const button = root.firstChild;
  render(root, h('button', { onClick: () => calls.push('second') }));
  button.dispatchEvent(new FakeEvent('click'));
  assert.deepEqual(calls, ['second']);
  assert.equal(button.listeners.get('click').length, 1);
  render(root, h('button', null));
  button.dispatchEvent(new FakeEvent('click'));
  assert.deepEqual(calls, ['second']);
});

test('an opaque node keeps what its owner put inside', () => {
  const { document, root } = page();
  const calls = [];
  const slot = () => h('div', { opaque: true, class: 'slot', hooks: { insert: (el) => calls.push(['insert', el.className]), destroy: () => calls.push(['destroy']) } });
  render(root, h('main', null, slot()));
  const el = root.firstChild.firstChild;
  el.appendChild(document.createElement('iframe'));
  render(root, h('main', { class: 'again' }, slot()));
  assert.equal(root.firstChild.firstChild, el);
  assert.equal(el.childNodes.length, 1, 'the iframe is still there');
  assert.deepEqual(calls, [['insert', 'slot']], 'insert ran once');
  render(root, h('main', null));
  assert.deepEqual(calls, [['insert', 'slot'], ['destroy']]);
});

test('hooks of nested nodes run when an ancestor goes', () => {
  const { root } = page();
  const calls = [];
  const slot = h('i', { hooks: { destroy: () => calls.push('destroyed') } });
  render(root, h('div', null, h('section', null, h('p', null, slot))));
  render(root, h('div', null));
  assert.deepEqual(calls, ['destroyed']);
});

test('the root may change tag', () => {
  const { root } = page();
  render(root, h('p', null, 'a'));
  render(root, h('section', null, 'b'));
  assert.equal(root.firstChild.outerHTML, '<section>b</section>');
});

test('style objects become a style attribute', () => {
  const { root } = page();
  render(root, h('div', { style: { fontSize: '12px', color: null } }));
  assert.equal(root.firstChild.getAttribute('style'), 'font-size:12px');
});
