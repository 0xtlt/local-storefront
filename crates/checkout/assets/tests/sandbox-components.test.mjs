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

/** Lets the timers set to 0 run: the action of a click comes after the click listeners. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 5));

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
    background: 'var(--lsf-color-bg-subdued)',
    'border-radius': 'var(--lsf-radius-small)',
    'inline-size': '100%',
  });
  assert.deepEqual(plain(boxStyle({ gap: 'small-200 base', gridtemplatecolumns: '1fr 2fr', alignitems: 'center', justifycontent: 'space-between' })), {
    gap: '0.5rem 1rem', 'grid-template-columns': '1fr 2fr', 'align-items': 'center', 'justify-content': 'space-between',
  });
  assert.equal(boxStyle({ border: 'large dashed strong' }).border, '2px dashed var(--lsf-color-border-emphasis)');
  assert.equal(boxStyle({ border: 'base' }).border, '1px solid var(--lsf-color-border)');
  assert.equal(boxStyle({ borderwidth: 'large', borderstyle: 'dotted' }).border, '1.25rem dotted var(--lsf-color-border)');
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

test('defaultValue fills a field until someone writes to it', () => {
  const { make } = components();
  const field = make('s-text-field');
  field.defaultValue = 'Happy birthday';
  assert.equal(field.value, 'Happy birthday');
  const control = field._root.querySelector('.control');
  control.value = 'Hi';
  control.dispatchEvent(new FakeEvent('input'));
  field.defaultValue = 'Happy new year';
  assert.equal(field.value, 'Hi', 'what the buyer typed stays');
  const set = make('s-text-area');
  set.value = 'Set';
  set.defaultValue = 'Default';
  assert.equal(set.value, 'Set', 'value wins over defaultValue');
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

test('a button opens the overlay it commands', async () => {
  const { make, document } = components();
  const modal = make('s-modal');
  modal.setAttribute('id', 'dialog');
  document.body.appendChild(modal);
  const button = make('s-button');
  button.setAttribute('command', '--show');
  button.setAttribute('commandfor', 'dialog');
  document.body.appendChild(button);
  button.click();
  await settle();
  assert.equal(modal.hasAttribute('open'), true);
  button.setAttribute('command', '--hide');
  button.click();
  await settle();
  assert.equal(modal.hasAttribute('open'), false);
});

test('without a command, commandFor means --auto: an overlay toggles, a clipboard item is copied', async () => {
  const { make, document, win } = components();
  const copied = [];
  win.context.navigator = { clipboard: { writeText: (text) => (copied.push(text), Promise.resolve()) } };
  const modal = make('s-modal');
  modal.setAttribute('id', 'dialog');
  const item = make('s-clipboard-item');
  item.setAttribute('id', 'code');
  item.setAttribute('text', 'SAVE10');
  document.body.append(modal, item);
  for (const tag of ['s-button', 's-link', 's-clickable']) {
    const action = make(tag);
    action.setAttribute('commandfor', 'dialog');
    document.body.appendChild(action);
    action.click();
    await settle();
    assert.equal(modal.hasAttribute('open'), true, `${tag} opens it`);
    action.click();
    await settle();
    assert.equal(modal.hasAttribute('open'), false, `${tag} closes it`);
  }
  const copy = make('s-button');
  copy.setAttribute('commandfor', 'code');
  document.body.appendChild(copy);
  copy.click();
  await settle();
  assert.deepEqual(copied, ['SAVE10']);
});

/** An action component of each kind, with the attributes of the case. */
const ACTIONS = ['s-button', 's-link', 's-clickable', 's-clickable-chip'];

test('every action follows the same contract: click first, then command or href, then type', async () => {
  for (const tag of ACTIONS) {
    const { make, document, Sandbox } = components();
    const modal = make('s-modal');
    modal.setAttribute('id', 'dialog');
    document.body.appendChild(modal);
    const form = make('s-form');
    document.body.appendChild(form);
    const visited = [];
    Sandbox.navigate = (url, target) => visited.push([url, target]);
    const submits = [];
    form.addEventListener('submit', () => submits.push(true));

    const action = make(tag);
    form.appendChild(action);
    const order = [];
    action.addEventListener('click', () => order.push(modal.hasAttribute('open') ? 'late' : 'click'));
    action.setAttribute('href', '/cart');
    action.setAttribute('target', '_blank');
    action.setAttribute('type', 'submit');
    action.click();
    assert.deepEqual(visited, [], `${tag}: the extension hears the click before the page moves`);
    await settle();
    assert.deepEqual(visited, [['/cart', '_blank']], tag);
    assert.deepEqual(submits, [], `${tag}: href makes type moot`);

    action.setAttribute('commandfor', 'dialog');
    action.setAttribute('command', '--show');
    action.click();
    await settle();
    assert.equal(modal.hasAttribute('open'), true, `${tag}: the command runs`);
    assert.equal(visited.length, 1, `${tag}: commandFor beats href`);
    assert.deepEqual(order, ['click', 'click'], tag);

    action.removeAttribute('commandfor');
    action.removeAttribute('href');
    action.click();
    await settle();
    assert.deepEqual(submits, [], `${tag}: type is ignored when a command is set`);
    action.removeAttribute('command');
    action.click();
    await settle();
    // Only a button and a clickable have a `type`.
    const typed = tag === 's-button' || tag === 's-clickable';
    assert.deepEqual(submits, typed ? [true] : [], `${tag}: type=submit submits the form`);
  }
});

test('an extension that cancels the click keeps the action from happening', async () => {
  for (const tag of ACTIONS) {
    const { make, Sandbox } = components();
    const visited = [];
    Sandbox.navigate = (url) => visited.push(url);
    const action = make(tag);
    action.setAttribute('href', '/cart');
    action.addEventListener('click', (event) => event.preventDefault());
    action.click();
    await settle();
    assert.deepEqual(visited, [], tag);
  }
});

test('an action that is disabled or loading does not click, nor act', async () => {
  for (const tag of [...ACTIONS, 's-press-button']) {
    for (const state of ['disabled', 'loading']) {
      const { make, Sandbox } = components();
      const visited = [];
      Sandbox.navigate = (url) => visited.push(url);
      const action = make(tag);
      action.setAttribute('href', '/cart');
      const clicks = [];
      action.addEventListener('click', () => clicks.push('late'));
      action.setAttribute(state, '');
      action.click();
      await settle();
      assert.deepEqual(clicks, [], `${tag} ${state}: the guard runs before what the extension listens to`);
      assert.deepEqual(visited, [], `${tag} ${state}`);
    }
  }
});

test('a form submits when a submit button is pressed', async () => {
  const { make } = components();
  const form = make('s-form');
  const button = make('s-button');
  button.setAttribute('type', 'submit');
  form.appendChild(button);
  const submits = [];
  form.addEventListener('submit', () => submits.push(true));
  button.click();
  await settle();
  assert.deepEqual(submits, [true]);
});

test('a clickable is a button for the keyboard and for assistive technology', async () => {
  const { make, document } = components();
  const clickable = make('s-clickable');
  document.body.appendChild(clickable);
  connect(clickable);
  assert.equal(clickable.getAttribute('role'), 'button');
  assert.equal(clickable.getAttribute('tabindex'), '0');
  clickable.setAttribute('href', '/cart');
  assert.equal(clickable.getAttribute('role'), 'link');
  clickable.setAttribute('accessibilityrole', 'listitem');
  assert.equal(clickable.getAttribute('role'), 'listitem', 'a role the author chose stays');
  clickable.removeAttribute('accessibilityrole');

  const clicks = [];
  clickable.addEventListener('click', () => clicks.push('click'));
  const press = (type, key, target = clickable) => {
    const event = new FakeEvent(type, { bubbles: true, cancelable: true, data: { key, target } });
    clickable.dispatchEvent(event);
    return event;
  };
  assert.equal(press('keydown', 'Enter').defaultPrevented, true);
  assert.deepEqual(clicks, ['click'], 'Enter clicks');
  assert.equal(press('keydown', ' ').defaultPrevented, true, 'Space does not scroll the page');
  assert.deepEqual(clicks, ['click'], 'Space waits for the release');
  press('keyup', ' ');
  assert.deepEqual(clicks, ['click', 'click']);
  const inner = make('s-text-field');
  clickable.appendChild(inner);
  press('keydown', 'Enter', inner);
  assert.deepEqual(clicks, ['click', 'click'], 'a key typed in a field inside is the field\'s');

  clickable.setAttribute('disabled', '');
  assert.equal(clickable.hasAttribute('tabindex'), false, 'a disabled clickable cannot be focused');
  assert.equal(clickable.getAttribute('aria-disabled'), 'true');
  press('keydown', 'Enter');
  assert.deepEqual(clicks, ['click', 'click'], 'nor pressed');
  clickable.removeAttribute('disabled');
  clickable.setAttribute('loading', '');
  assert.equal(clickable.getAttribute('aria-busy'), 'true');
  assert.equal(clickable.hasAttribute('tabindex'), false);
  clickable.removeAttribute('loading');
  assert.equal(clickable.getAttribute('tabindex'), '0');
  assert.equal(clickable.hasAttribute('aria-disabled'), false);
});

test('a link answers Enter even without an href, and labels its anchor', () => {
  const { make } = components();
  const link = make('s-link');
  const anchor = link._root.querySelector('a');
  link.setAttribute('accessibilitylabel', 'Open the terms');
  link.setAttribute('lang', 'fr');
  assert.equal(anchor.getAttribute('aria-label'), 'Open the terms');
  assert.equal(anchor.getAttribute('lang'), 'fr');
  const clicks = [];
  anchor.addEventListener('click', () => clicks.push('click'));
  const key = (name) => anchor.dispatchEvent(new FakeEvent('keydown', { bubbles: true, cancelable: true, data: { key: name } }));
  key('Enter');
  key(' ');
  assert.deepEqual(clicks, ['click'], 'Enter only, as for a link');
});

test('a press button flips when clicked unless it is disabled or loading, and says so', async () => {
  const { make } = components();
  const button = make('s-press-button');
  const changes = [];
  button.addEventListener('change', () => changes.push(button.hasAttribute('pressed')));
  button.click();
  button.click();
  assert.deepEqual(changes, [true, false]);
  button.setAttribute('loading', '');
  button.click();
  assert.deepEqual(changes, [true, false], 'a loading button does not flip');
  assert.equal(button._root.querySelector('button').disabled, true, 'loading disables it');
  assert.equal(button._root.querySelector('button').getAttribute('aria-busy'), 'true');
  button.removeAttribute('loading');
  button.setAttribute('accessibilitylabel', 'Gift wrap');
  assert.equal(button._root.querySelector('button').getAttribute('aria-label'), 'Gift wrap');
});

test('a clickable chip acts as a whole, and removing it is not clicking it', async () => {
  const { make, Sandbox } = components();
  const visited = [];
  Sandbox.navigate = (url) => visited.push(url);
  const chip = make('s-clickable-chip');
  chip.setAttribute('href', '/collections/all');
  chip.setAttribute('removable', '');
  const clicks = [];
  const removed = [];
  chip.addEventListener('click', () => clicks.push('click'));
  chip.addEventListener('remove', () => removed.push('remove'));
  const remove = chip._root.querySelector('.remove');
  remove.dispatchEvent(new FakeEvent('click', { bubbles: true }));
  assert.deepEqual(removed, ['remove']);
  chip.dispatchEvent(new FakeEvent('click', { bubbles: true }));
  await settle();
  assert.deepEqual(visited, ['/collections/all']);
  chip.setAttribute('disabled', '');
  assert.equal(remove.disabled, true);
  assert.equal(chip._root.querySelector('.body').disabled, true);
  remove.dispatchEvent(new FakeEvent('click', { bubbles: true }));
  assert.deepEqual(removed, ['remove'], 'a disabled chip cannot be removed');
});

test('Enter in a field submits its form once, however the keydown travels', async () => {
  const { make } = components();
  const form = make('s-form');
  const field = make('s-text-field');
  form.appendChild(field);
  const submits = [];
  form.addEventListener('submit', () => submits.push(true));
  connect(form);
  connect(field);
  const control = field._root.querySelector('.control');
  const keydown = (target, data = {}) => {
    const event = new FakeEvent('keydown', { bubbles: true, cancelable: true, composed: true, data: { key: 'Enter', ...data } });
    target.dispatchEvent(event);
  };
  keydown(control);
  // In a browser the same keydown goes on, composed, through the field and the form.
  keydown(field);
  assert.deepEqual(submits, [true], 'the form does not submit again for the key that reached it');
  keydown(control, { isComposing: true });
  assert.deepEqual(submits, [true], 'Enter that picks an IME candidate submits nothing');
  const area = make('s-text-area');
  form.appendChild(area);
  keydown(area._root.querySelector('.control'));
  assert.deepEqual(submits, [true], 'Enter in a text area adds a line');
});

test('hidden hides every component, whatever display rule its tag has', () => {
  const { Sandbox } = components();
  const { LIGHT_CSS, TAGS } = Sandbox.components;
  const rule = LIGHT_CSS.match(/([^{}]*\[hidden\][^{}]*)\{\s*display:\s*none\s*!important/);
  assert.ok(rule, 'one rule hides them');
  const hidden = rule[1].split(',').map((entry) => entry.trim());
  for (const tag of TAGS) assert.ok(hidden.includes(tag + '[hidden]'), tag);
  assert.ok(LIGHT_CSS.indexOf('s-text { display: inline; }') < LIGHT_CSS.indexOf(rule[0]), 'after the tags it overrides');
});

// --- Custom element conformance and state written before the element is in the page ---------

/** Plays the page putting an element in the document: connected, then synced. */
const connect = (element) => element.connectedCallback();

/** Makes the stand-in `<select>` of the fake shadow root choose like a real one: a value that
 * matches no option selects nothing, and replacing the options selects the first again. */
function nativeSelect(control) {
  let chosen = null;
  let none = false;
  const options = () => control.childNodes;
  Object.defineProperty(control, 'value', {
    configurable: true,
    get() {
      if (none) return '';
      return (chosen && options().includes(chosen) ? chosen : options()[0])?.value ?? '';
    },
    set(value) {
      chosen = options().find((option) => option.value === value) ?? null;
      none = chosen === null;
    },
  });
  // Changing the options starts over, as for a browser.
  const replace = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(control), 'textContent').set;
  Object.defineProperty(control, 'textContent', {
    configurable: true,
    get: () => '',
    set(value) {
      replace.call(control, value);
      chosen = null;
      none = false;
    },
  });
  return control;
}

test('no constructor touches its element: no attribute, no class, no child', () => {
  const { make, Sandbox } = components();
  for (const tag of Sandbox.components.TAGS) {
    const element = make(tag);
    assert.deepEqual([...element.attributes], [], `${tag} has attributes`);
    assert.deepEqual(element.childNodes, [], `${tag} has children`);
  }
});

test('a switch looks like one from its shadow root, not from a class of its own', () => {
  const { make } = components();
  const toggle = make('s-switch');
  assert.equal(toggle.className, '');
  assert.match(toggle._root.html, /<label class="switch">/);
  assert.match(toggle._root.html, /\.switch \.box::after/);
  assert.doesNotMatch(toggle._root.html, /:host\(\.switch\)/);
  assert.doesNotMatch(make('s-checkbox')._root.html, /class="switch"/);
  connect(toggle);
  assert.equal(toggle.className, '');
});

test('a select keeps the value it was given before it had options', () => {
  const { make } = components();
  const select = make('s-select');
  const control = nativeSelect(select._root.querySelector('.control'));
  select.value = 'b';
  assert.equal(select.value, 'b', 'it answers what it was asked');
  for (const value of ['a', 'b', 'c']) {
    const option = make('s-option');
    option.setAttribute('value', value);
    select.appendChild(option);
  }
  connect(select);
  assert.equal(control.value, 'b', 'applied once the options are built');
  assert.equal(select.value, 'b');
  // The options are rebuilt: the value comes back.
  const extra = make('s-option');
  extra.setAttribute('value', 'd');
  select.appendChild(extra);
  connect(select);
  assert.equal(control.value, 'b');
  // What the buyer picks is the value from then on.
  control.value = 'c';
  control.dispatchEvent(new FakeEvent('change'));
  connect(select);
  assert.equal(select.value, 'c');
  select.value = 'a';
  assert.equal(control.value, 'a');
});

test('a select applies its value attribute when the options come', () => {
  const { make } = components();
  const select = make('s-select');
  const control = nativeSelect(select._root.querySelector('.control'));
  select.setAttribute('value', 'b');
  for (const value of ['a', 'b']) {
    const option = make('s-option');
    option.setAttribute('value', value);
    select.appendChild(option);
  }
  connect(select);
  assert.equal(control.value, 'b');
});

test('defaultChecked starts a checkbox checked, even after other attributes synced', () => {
  const { make } = components();
  const box = make('s-checkbox');
  box.label = 'Gift';
  box.defaultChecked = true;
  assert.equal(box.checked, false, 'nothing happens before the element is in the page');
  connect(box);
  assert.equal(box.checked, true);
  assert.equal(box.hasAttribute('checked'), true);
  // The buyer unchecks it: neither another attribute nor reconnecting checks it again.
  const input = box._root.querySelector('input');
  input.checked = false;
  input.dispatchEvent(new FakeEvent('change'));
  box.label = 'Gift wrap';
  connect(box);
  assert.equal(box.checked, false);
  assert.equal(box.hasAttribute('checked'), false);
});

test('a checked written before connecting beats defaultChecked', () => {
  const { make } = components();
  const box = make('s-switch');
  box.defaultChecked = true;
  box.checked = false;
  connect(box);
  assert.equal(box.checked, false);
  const attribute = make('s-checkbox');
  attribute.setAttribute('checked', '');
  attribute.defaultChecked = true;
  connect(attribute);
  assert.equal(attribute.checked, true);
});

test('defaultSelected, defaultPressed and defaultOpen start once', () => {
  const { make } = components();
  const choice = make('s-choice');
  choice.setAttribute('value', 'x');
  choice.defaultSelected = true;
  connect(choice);
  assert.equal(choice.selected, true);
  assert.equal(choice._root.querySelector('input').checked, true);
  choice._root.querySelector('input').checked = false;
  choice._root.querySelector('input').dispatchEvent(new FakeEvent('change'));
  choice.disabled = true;
  assert.equal(choice.selected, false, 'a syncing attribute does not select the choice again');

  const press = make('s-press-button');
  press.defaultPressed = true;
  connect(press);
  assert.equal(press.hasAttribute('pressed'), true);
  assert.equal(press._root.querySelector('button').getAttribute('aria-pressed'), 'true');

  const details = make('s-details');
  details.defaultOpen = true;
  connect(details);
  const inner = details._root.querySelector('details');
  assert.equal(inner.open, true);
  inner.open = false;
  inner.dispatchEvent(new FakeEvent('toggle'));
  assert.equal(inner.open, false, 'what the buyer closed stays closed');
  assert.equal(details.hasAttribute('open'), false);
});

test('values written to a choice list before its choices exist are applied to them', () => {
  const { make } = components();
  const list = make('s-choice-list');
  list.values = ['b'];
  assert.deepEqual(plain(list.values), ['b']);
  const choices = ['a', 'b'].map((value) => {
    const choice = make('s-choice');
    choice.setAttribute('value', value);
    list.appendChild(choice);
    return choice;
  });
  connect(list);
  assert.deepEqual(plain(choices.map((choice) => choice.hasAttribute('selected'))), [false, true]);
  assert.deepEqual(plain(list.values), ['b']);
  // The buyer picks the other one: the values asked for are not applied again.
  choices[0]._root.querySelector('input').checked = true;
  choices[0]._root.querySelector('input').dispatchEvent(new FakeEvent('change'));
  connect(list);
  assert.deepEqual(plain(list.values), ['a']);
});

test('a choice follows the multiple and disabled of its list when they change', () => {
  const { make } = components();
  const list = make('s-choice-list');
  const choice = make('s-choice');
  list.appendChild(choice);
  connect(choice);
  const input = choice._root.querySelector('input');
  assert.equal(input.type, 'radio');
  list.setAttribute('multiple', '');
  list.setAttribute('disabled', '');
  assert.equal(input.type, 'checkbox');
  assert.equal(input.disabled, true);
});

test('an overlay opened before it is in the page opens when it joins it', () => {
  const { make, Sandbox } = components();
  const states = [];
  Sandbox.onOverlay = (open) => states.push(open);
  const modal = make('s-modal');
  let connected = false;
  Object.defineProperty(modal, 'isConnected', { get: () => connected });
  modal.open = true;
  assert.deepEqual(states, [], 'an element nobody sees does not grow the frame');
  assert.equal(modal._root.querySelector('.backdrop').hidden, true);
  connected = true;
  connect(modal);
  assert.deepEqual(states, [true]);
  assert.equal(modal._root.querySelector('.backdrop').hidden, false);
});

test('a date picker starts from its defaultValue until someone picks', () => {
  const { make } = components();
  const picker = make('s-date-picker');
  picker.defaultValue = '2026-01-02';
  const control = picker._root.querySelector('.control');
  assert.equal(picker.value, '2026-01-02');
  control.value = '2026-03-04';
  control.dispatchEvent(new FakeEvent('input'));
  picker.defaultValue = '2026-05-06';
  assert.equal(picker.value, '2026-03-04');
  picker.disabled = true;
  assert.equal(control.disabled, true);
});

test('a field keeps what the buyer typed when its other attributes change', () => {
  const { make } = components();
  const field = make('s-text-field');
  field.setAttribute('value', 'initial');
  const control = field._root.querySelector('.control');
  assert.equal(control.value, 'initial');
  control.value = 'typed';
  control.dispatchEvent(new FakeEvent('input'));
  field.label = 'Name';
  assert.equal(control.value, 'typed');
});
