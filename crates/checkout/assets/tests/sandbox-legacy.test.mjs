// The host of legacy extensions: a remote-ui channel in, the same elements as the current API
// out. The messages are the ones `@remote-ui/core` 2.x sends.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { FakeEvent } from './fake-dom.mjs';
import { sandboxWindow } from './harness.mjs';

const MOUNT = 0;
const INSERT = 1;
const REMOVE = 2;
const TEXT = 3;
const PROPS = 4;

const component = (id, type, props = {}, children = []) => ({ id, kind: 1, type, props, children });
const text = (id, value) => ({ id, kind: 2, text: value });
const fragment = (id, children) => ({ id, kind: 3, children });

function host() {
  const win = sandboxWindow({ scripts: ['sandbox-legacy.js'] });
  const warnings = [];
  const legacy = win.context.LsfSandbox.createLegacyHost({
    document: win.document,
    container: win.document.body,
    warn: (message) => warnings.push(message),
  });
  return { ...legacy, body: win.document.body, document: win.document, warnings };
}

const html = (host) => host.body.childNodes.map((node) => node.outerHTML ?? node.nodeValue).join('');

test('mounts a tree as elements', () => {
  const { channel, body } = host();
  channel(MOUNT, [
    component('1', 'Banner', { status: 'success', title: 'Done' }, [
      component('2', 'BlockStack', { spacing: 'loose' }, [component('3', 'Text', { emphasis: 'bold', appearance: 'critical' }, [text('4', 'Hello')])]),
    ]),
  ]);
  assert.equal(
    body.firstChild.outerHTML,
    '<s-banner heading="Done" tone="success"><s-stack direction="block" gap="large-100"><s-text tone="critical" type="strong">Hello</s-text></s-stack></s-banner>',
  );
});

test('inserts, moves and removes children by index', () => {
  const h = host();
  h.channel(MOUNT, [component('1', 'BlockStack', {}, [text('2', 'a'), text('3', 'c')])]);
  h.channel(INSERT, '1', 1, text('4', 'b'), false);
  assert.equal(h.body.firstChild.textContent, 'abc');
  h.channel(INSERT, '1', 0, component('5', 'Text', {}, [text('6', 'x')]), false);
  assert.equal(h.body.firstChild.textContent, 'xabc');
  // A node that moves arrives with the id of the parent it leaves.
  h.channel(INSERT, '1', 3, text('2', 'a'), '1');
  assert.equal(h.body.firstChild.textContent, 'xbca');
  h.channel(REMOVE, '1', 0);
  assert.equal(h.body.firstChild.textContent, 'bca');
  assert.equal(h.nodes.has('5'), false, 'a removed node is forgotten');
  assert.equal(h.nodes.has('6'), false, 'and what was in it');
});

test('changes texts and props', () => {
  const h = host();
  h.channel(MOUNT, [component('1', 'Banner', { status: 'info', title: 'A' }, [text('2', 'one')])]);
  h.channel(TEXT, '2', 'two');
  h.channel(PROPS, '1', { status: 'critical', title: undefined });
  assert.equal(h.body.firstChild.outerHTML, '<s-banner tone="critical">two</s-banner>');
});

test('callbacks run when the element says so, with what the callback expects', () => {
  const h = host();
  const calls = [];
  h.channel(MOUNT, [
    component('1', 'BlockStack', {}, [
      component('2', 'Button', { onPress: () => calls.push('press') }, [text('3', 'Go')]),
      component('4', 'TextField', { label: 'Name', value: 'Ada', onChange: (value) => calls.push(['change', value]), onInput: (value) => calls.push(['input', value]) }),
      component('5', 'Checkbox', { checked: false, onChange: (value) => calls.push(['checked', value]) }, [text('6', 'Agree')]),
    ]),
  ]);
  const [button, field, box] = h.body.firstChild.childNodes;
  assert.equal(button.localName, 's-button');
  assert.equal(field.localName, 's-text-field');
  assert.equal(field.value, 'Ada');
  button.click();
  field.value = 'Grace';
  field.dispatchEvent(new FakeEvent('input'));
  field.dispatchEvent(new FakeEvent('change'));
  box.checked = true;
  box.dispatchEvent(new FakeEvent('change'));
  assert.deepEqual(calls, ['press', ['input', 'Grace'], ['change', 'Grace'], ['checked', true]]);
  // A new callback replaces the old one: no listener is added.
  h.channel(PROPS, '2', { onPress: () => calls.push('second') });
  button.click();
  assert.deepEqual(calls.at(-1), 'second');
  assert.equal(calls.filter((call) => call === 'press').length, 1);
  assert.equal(button.listeners.get('click').length, 1);
});

test('events of what is inside are not the component\'s', () => {
  const h = host();
  const calls = [];
  h.channel(MOUNT, [component('1', 'Modal', { title: 'M', onClose: () => calls.push('close') }, [component('2', 'TextField', { label: 'x' })])]);
  const modal = h.body.firstChild;
  modal.dispatchEvent(new FakeEvent('hide', { bubbles: true }));
  modal.firstChild.dispatchEvent(new FakeEvent('hide', { bubbles: true }));
  assert.deepEqual(calls, ['close']);
});

test('fragments hold their own tree, in the slot of the prop', () => {
  const h = host();
  h.channel(MOUNT, [
    component('1', 'Modal', { title: 'Terms', primaryAction: fragment('2', [component('3', 'Button', {}, [text('4', 'Accept')])]) }, [text('5', 'Read this')]),
  ]);
  const modal = h.body.firstChild;
  assert.equal(modal.getAttribute('heading'), 'Terms');
  assert.equal(modal.childNodes[0].nodeValue, 'Read this');
  const slot = modal.childNodes[1];
  assert.equal(slot.getAttribute('slot'), 'primary-action');
  assert.equal(slot.firstChild.outerHTML, '<s-button>Accept</s-button>');
  h.channel(INSERT, '2', 1, component('6', 'Button', {}, [text('7', 'Decline')]), false);
  assert.equal(slot.textContent, 'AcceptDecline');
  // Children added after a fragment stay before it.
  h.channel(INSERT, '1', 1, text('8', 'More'), false);
  assert.deepEqual(modal.childNodes.map((node) => node.nodeValue ?? node.localName), ['Read this', 'More', 'span']);
});

test('a button with an overlay opens it', () => {
  const h = host();
  h.channel(MOUNT, [component('1', 'Button', { overlay: fragment('2', [component('3', 'Modal', { id: 'm', title: 'T' })]) }, [text('4', 'Open')])]);
  const [button] = h.body.childNodes;
  const modal = button.querySelector('s-modal');
  const opened = [];
  modal.showOverlay = () => opened.push(true);
  button.click();
  assert.deepEqual(opened, [true]);
  modal.dispatchEvent(new FakeEvent('click', { bubbles: true }));
  assert.deepEqual(opened, [true], 'a click inside the modal does not open it again');
});

test('Select takes its options as a prop', () => {
  const h = host();
  h.channel(MOUNT, [component('1', 'Select', { label: 'Size', value: 'm', options: [{ value: 's', label: 'Small' }, { value: 'm', label: 'Medium' }], onChange: () => {} })]);
  const select = h.body.firstChild;
  assert.deepEqual(select.querySelectorAll('s-option').map((option) => [option.getAttribute('value'), option.textContent]), [['s', 'Small'], ['m', 'Medium']]);
  assert.equal(select.value, 'm');
  h.channel(PROPS, '1', { options: [{ value: 'x', label: 'Only' }], value: 'x' });
  assert.equal(select.querySelectorAll('s-option').length, 1);
});

test('ChoiceList keeps its value and answers with the value', () => {
  const h = host();
  const answers = [];
  h.channel(MOUNT, [
    component('1', 'ChoiceList', { name: 'c', value: 'b', onChange: (value) => answers.push(value) }, [component('2', 'Choice', { id: 'a' }, [text('3', 'A')]), component('4', 'Choice', { id: 'b' }, [text('5', 'B')])]),
  ]);
  const list = h.body.firstChild;
  assert.equal(list.childNodes[0].getAttribute('value'), 'a');
  list.values = ['b'];
  list.dispatchEvent(new FakeEvent('change'));
  assert.deepEqual(answers, ['b'], 'one value is a string');
  h.channel(PROPS, '1', { value: ['a', 'b'] });
  assert.equal(list.getAttribute('multiple'), '');
  list.dispatchEvent(new FakeEvent('change'));
  assert.deepEqual(answers.at(-1), list.values, 'many values are a list');
});

test('Disclosure shows what its pressables toggle', () => {
  const h = host();
  h.channel(MOUNT, [
    component('1', 'Disclosure', { defaultOpen: [] }, [
      component('2', 'Pressable', { toggles: 'more' }, [text('3', 'Show')]),
      component('4', 'View', { id: 'more' }, [text('5', 'Hidden')]),
    ]),
  ]);
  const more = h.body.querySelector('[id="more"]');
  assert.equal(more.hidden, true);
  h.body.querySelector('s-clickable').click();
  assert.equal(more.hidden, false);
  h.body.querySelector('s-clickable').click();
  assert.equal(more.hidden, true);
});

test('some components change the element they become', () => {
  const h = host();
  h.channel(MOUNT, [
    component('1', 'BlockStack', {}, [
      component('2', 'TextField', { label: 'a', type: 'email' }),
      component('3', 'TextField', { label: 'b', multiline: 3 }),
      component('4', 'List', { marker: 'number' }, [component('5', 'ListItem', {}, [text('6', 'one')])]),
      component('7', 'InlineStack', { spacing: 'tight', blockAlignment: 'center' }),
      component('8', 'Icon', { source: 'checkmark' }),
      component('9', 'Image', { source: 'http://x/y.png', accessibilityDescription: 'Why' }),
    ]),
  ]);
  const tags = h.body.firstChild.childNodes.map((node) => node.localName);
  assert.deepEqual(tags, ['s-email-field', 's-text-area', 's-ordered-list', 's-stack', 's-icon', 's-image']);
  const [, , , inline, icon, image] = h.body.firstChild.childNodes;
  assert.equal(inline.getAttribute('direction'), 'inline');
  assert.equal(inline.getAttribute('gap'), 'small-200');
  assert.equal(inline.getAttribute('alignitems'), 'center');
  assert.equal(icon.getAttribute('type'), 'check');
  assert.equal(image.getAttribute('alt'), 'Why');
});

test('a component it does not know renders as a box and says so', () => {
  const h = host();
  h.channel(MOUNT, [component('1', 'Gizmo', {}, [text('2', 'inside')])]);
  assert.equal(h.body.firstChild.outerHTML, '<s-box data-unsupported="Gizmo">inside</s-box>');
  assert.match(h.warnings[0], /Gizmo is not supported/);
});

test('offers the components remote-ui checks against', () => {
  const h = host();
  for (const name of ['Banner', 'BlockStack', 'InlineStack', 'Text', 'TextBlock', 'Heading', 'Button', 'TextField', 'Checkbox', 'Select', 'ChoiceList', 'Image', 'Link', 'Pressable', 'Modal', 'Disclosure', 'View', 'Grid', 'Divider', 'Tag', 'Badge', 'Spinner']) {
    assert.ok(h.components.includes(name), name);
  }
});

test('refuses a message it does not know', () => {
  assert.throws(() => host().channel(99), /unknown action/);
  assert.throws(() => host().channel(TEXT, 'nope', 'x'), /no node/);
});
