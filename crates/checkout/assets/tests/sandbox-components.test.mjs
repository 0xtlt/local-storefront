// The Polaris web components of the sandbox: every tag exists, properties mirror attributes,
// layout attributes become CSS.

import assert from 'node:assert/strict';
import { test } from 'node:test';
import { FakeEvent } from './fake-dom.mjs';
import { plain, sandboxWindow } from './harness.mjs';

/** `SUPPORTED_COMPONENTS` of `@shopify/ui-extensions` 2026.10 (checkout). */
const SUPPORTED = [
  'Abbreviation', 'Announcement', 'Badge', 'Banner', 'Box', 'Button', 'Chat', 'Checkbox', 'Chip', 'Choice', 'ChoiceList',
  'Clickable', 'ClickableChip', 'ClipboardItem', 'ConsentCheckbox', 'ConsentPhoneField', 'DateField', 'DatePicker', 'Details',
  'Divider', 'DropZone', 'EmailField', 'Form', 'Grid', 'GridItem', 'Heading', 'Icon', 'Image', 'Link', 'ListItem', 'Map',
  'MapMarker', 'Modal', 'MoneyField', 'NumberField', 'Option', 'OrderedList', 'Paragraph', 'PasswordField', 'PaymentIcon',
  'PhoneField', 'Popover', 'PressButton', 'ProductThumbnail', 'Progress', 'QueryContainer', 'QRCode', 'ScrollBox', 'Section',
  'Select', 'Sheet', 'SkeletonParagraph', 'Spinner', 'Stack', 'Summary', 'Switch', 'Text', 'TextArea', 'TextField', 'Time',
  'Tooltip', 'UnorderedList', 'UrlField',
].map((name) => (name === 'QRCode' ? 's-qr-code' : 's-' + name.replace(/(?<=.)([A-Z])/g, '-$1').toLowerCase()));

function components() {
  const defined = new Map();
  const customElements = { define: (tag, constructor) => defined.set(tag, constructor), get: (tag) => defined.get(tag) };
  const win = sandboxWindow({ scripts: ['sandbox-components.js'], customElements });
  const make = (tag) => {
    const element = new (defined.get(tag))();
    element.localName = tag;
    element.tagName = tag.toUpperCase();
    return element;
  };
  return { defined, win, make, Sandbox: win.context.LsfSandbox, document: win.document };
}

test('defines every component of the checkout UI extension API', () => {
  const { defined, Sandbox } = components();
  assert.equal(SUPPORTED.length, 63);
  assert.deepEqual(SUPPORTED.filter((tag) => !defined.has(tag)), []);
  assert.deepEqual(plain(Sandbox.components.TAGS).sort(), [...SUPPORTED].sort());
});

test('spaces with tokens or CSS', () => {
  const { spacing } = components().Sandbox.components;
  assert.equal(spacing('base'), '1rem');
  assert.equal(spacing('small-200 large'), '0.5rem 1.25rem');
  assert.equal(spacing('none'), '0');
  assert.equal(spacing('12px 3%'), '12px 3%');
});

test('turns layout attributes into CSS', () => {
  const { boxStyle } = components().Sandbox.components;
  assert.deepEqual(plain(boxStyle({ padding: 'base large', paddingblockstart: 'small', background: 'subdued', borderradius: 'base', inlinesize: 'fill', display: 'auto' })), {
    padding: '1rem 1.25rem',
    'padding-block-start': '0.875rem',
    background: '#f7f7f7',
    'border-radius': '8px',
    'inline-size': '100%',
  });
  assert.deepEqual(plain(boxStyle({ gap: 'small-200 base', gridtemplatecolumns: '1fr 2fr', alignitems: 'center', justifycontent: 'space-between' })), {
    gap: '0.5rem 1rem', 'grid-template-columns': '1fr 2fr', 'align-items': 'center', 'justify-content': 'space-between',
  });
  assert.equal(boxStyle({ border: 'large dashed strong' }).border, '2px dashed #8a8a8a');
  assert.equal(boxStyle({ border: 'base' }).border, '1px solid #d4d4d4');
  assert.equal(boxStyle({ borderwidth: 'large', borderstyle: 'dotted' }).border, '1.25rem dotted #d4d4d4');
  assert.deepEqual(plain(boxStyle({ padding: '' })), {});
});

test('reads the direction a stack is written for', () => {
  const { direction } = components().Sandbox.components;
  assert.equal(direction('inline'), 'inline');
  assert.equal(direction('@container (inline-size > 500px) inline, block'), 'block');
  assert.equal(direction(undefined), 'block');
});

test('a stack follows its attributes and its properties mirror them', () => {
  const { make } = components();
  const stack = make('s-stack');
  stack.gap = 'base';
  stack.direction = 'inline';
  stack.alignItems = 'center';
  stack.paddingInline = 'large';
  assert.equal(stack.getAttribute('gap'), 'base');
  assert.equal(stack.getAttribute('alignitems'), 'center');
  assert.equal(stack.getAttribute('paddinginline'), 'large');
  assert.equal(stack.style.cssText, 'padding-inline:1.25rem;gap:1rem;align-items:center;flex-direction:row');
  assert.equal(stack.gap, 'base');
  stack.gap = null;
  assert.equal(stack.hasAttribute('gap'), false);
  stack.accessibilityLabel = 'Things';
  assert.equal(stack.getAttribute('aria-label'), 'Things');
});

test('boolean properties are flags', () => {
  const { make } = components();
  const button = make('s-button');
  assert.equal(button.disabled, false);
  button.disabled = true;
  assert.equal(button.hasAttribute('disabled'), true);
  assert.equal(button.disabled, true);
  button.disabled = false;
  assert.equal(button.hasAttribute('disabled'), false);
  button.variant = 'primary';
  assert.equal(button.getAttribute('variant'), 'primary');
});

test('a text field has a label, a value and says when it changes', () => {
  const { make } = components();
  const field = make('s-text-field');
  field.label = 'Name';
  assert.equal(field._root.querySelector('.lbl').textContent, 'Name');
  field.value = 'Ada';
  assert.equal(field.value, 'Ada');
  assert.equal(field._root.querySelector('.control').value, 'Ada');
  field.error = 'Too short';
  assert.equal(field._root.querySelector('.message.error').textContent, 'Too short');
  field.disabled = true;
  assert.equal(field._root.querySelector('.control').disabled, true);
  const changes = [];
  field.addEventListener('change', (event) => changes.push(event.target === field));
  field._root.querySelector('.control').dispatchEvent(new FakeEvent('change'));
  assert.deepEqual(changes, [true], 'the change of the inner input is the field\'s');
});

test('a checkbox mirrors checked and says when it changes', () => {
  const { make } = components();
  const box = make('s-checkbox');
  box.label = 'Gift';
  assert.equal(box.checked, false);
  box.checked = true;
  assert.equal(box.hasAttribute('checked'), true);
  const changes = [];
  box.addEventListener('change', () => changes.push(box.checked));
  const input = box._root.querySelector('input');
  input.checked = false;
  input.dispatchEvent(new FakeEvent('change'));
  assert.deepEqual(changes, [false]);
  assert.equal(box.hasAttribute('checked'), false);
});

test('a choice list selects one choice, or many', () => {
  const { make } = components();
  const list = make('s-choice-list');
  const [a, b] = ['a', 'b'].map((value) => {
    const choice = make('s-choice');
    choice.setAttribute('value', value);
    list.appendChild(choice);
    return choice;
  });
  list.values = ['a'];
  assert.deepEqual(plain(list.values), ['a']);
  const changes = [];
  list.addEventListener('change', () => changes.push(plain(list.values)));
  b._root.querySelector('input').checked = true;
  b._root.querySelector('input').dispatchEvent(new FakeEvent('change'));
  assert.deepEqual(changes, [['b']], 'one choice at a time');
  assert.equal(a.hasAttribute('selected'), false);
  list.setAttribute('multiple', '');
  a._root.querySelector('input').checked = true;
  a._root.querySelector('input').dispatchEvent(new FakeEvent('change'));
  assert.deepEqual(changes.at(-1), ['a', 'b']);
});

test('an open overlay takes over the frame', () => {
  const { make, Sandbox } = components();
  const states = [];
  Sandbox.onOverlay = (open) => states.push(open);
  const modal = make('s-modal');
  const events = [];
  modal.addEventListener('show', () => events.push('show'));
  modal.addEventListener('hide', () => events.push('hide'));
  modal.heading = 'Title';
  assert.equal(modal._root.querySelector('.heading').textContent, 'Title');
  modal.showOverlay();
  modal.showOverlay();
  assert.equal(modal.hasAttribute('open'), true);
  const other = make('s-popover');
  other.showOverlay();
  modal.hideOverlay();
  other.hideOverlay();
  assert.deepEqual(states, [true, true, true, false], 'the frame stays big until the last one closes');
  assert.deepEqual(events, ['show', 'hide']);
});

test('a button opens the overlay it commands', () => {
  const { make, document } = components();
  const modal = make('s-modal');
  modal.setAttribute('id', 'dialog');
  document.body.appendChild(modal);
  const button = make('s-button');
  button.setAttribute('command', '--show');
  button.setAttribute('commandfor', 'dialog');
  document.body.appendChild(button);
  button.click();
  assert.equal(modal.hasAttribute('open'), true);
  button.setAttribute('command', '--hide');
  button.click();
  assert.equal(modal.hasAttribute('open'), false);
});

test('a button that is disabled or loading does not click', () => {
  const { make } = components();
  const button = make('s-button');
  const clicks = [];
  button.addEventListener('click', () => clicks.push('late'));
  button.setAttribute('disabled', '');
  button.click();
  assert.deepEqual(clicks, [], 'the guard runs before what the extension listens to');
});

test('a form submits when a submit button is pressed', () => {
  const { make } = components();
  const form = make('s-form');
  const button = make('s-button');
  button.setAttribute('type', 'submit');
  form.appendChild(button);
  const submits = [];
  form.addEventListener('submit', () => submits.push(true));
  button.click();
  assert.deepEqual(submits, [true]);
});
