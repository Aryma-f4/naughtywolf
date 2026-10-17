const { test } = require('node:test');
const assert = require('node:assert/strict');

const { createWizard, summarizePayload } = require('../static/payload_wizard.js');

function interactiveElement() {
  const listeners = {};
  return {
    hidden: false,
    disabled: false,
    attrs: {},
    dataset: {},
    addEventListener(type, fn) { listeners[type] = fn; },
    setAttribute(name, value) { this.attrs[name] = String(value); },
    removeAttribute(name) { delete this.attrs[name]; },
    click() { listeners.click?.({ preventDefault() {} }); },
    fire(type) { listeners[type]?.({ target: this }); },
  };
}

function wizardHarness() {
  const steps = Array.from({ length: 4 }, interactiveElement);
  const tabs = Array.from({ length: 4 }, interactiveElement);
  tabs.forEach((tab, index) => { tab.dataset = { wizardTab: String(index) }; });
  const back = interactiveElement();
  const next = interactiveElement();
  const build = interactiveElement();
  const form = {
    querySelectorAll(selector) {
      if (selector === '[data-payload-step]') return steps;
      if (selector === '[data-wizard-tab]') return tabs;
      return [];
    },
    querySelector(selector) {
      return {
        '[data-wizard-back]': back,
        '[data-wizard-next]': next,
        '[data-wizard-build]': build,
      }[selector] || null;
    },
  };
  return { form, steps, tabs, back, next, build };
}

function chatHarness() {
  const base = wizardHarness();
  const lhost = Object.assign(interactiveElement(), { value: 'c2.lab.example', readOnly: false, attrs: { required: '' } });
  const lport = Object.assign(interactiveElement(), { value: '8443', readOnly: false, attrs: { required: '' }, hasRequired: true });
  const protocol = Object.assign(interactiveElement(), { value: 'https' });
  const chatGroup = Object.assign(interactiveElement(), { hidden: true });
  const chatInputs = [interactiveElement(), interactiveElement()];
  chatGroup.querySelectorAll = () => chatInputs;
  chatInputs[0].getAttribute = (n) => (n === 'required' ? '' : null);

  const nodes = {
    '#payload-lhost': lhost,
    '#payload-lport': lport,
    '#payload-proto': protocol,
    '[data-chat-fields]': chatGroup,
  };
  const form = {
    querySelectorAll(sel) { return base.form.querySelectorAll(sel); },
    querySelector(sel) {
      if (sel in nodes) return nodes[sel];
      return base.form.querySelector(sel);
    },
  };
  // `required` is toggled through set/removeAttribute in the source.
  Object.defineProperty(lport, 'hasRequired', {
    get() { return 'required' in this.attrs; },
  });
  return { ...base, form, lhost, lport, protocol, chatGroup, chatInputs };
}

test('payload wizard exposes one step at a time and keeps build for the final step', () => {
  const h = wizardHarness();
  const wizard = createWizard(h.form);

  assert.deepEqual(h.steps.map(step => step.hidden), [false, true, true, true]);
  assert.equal(h.back.disabled, true);
  assert.equal(h.build.hidden, true);

  wizard.goTo(3);
  assert.deepEqual(h.steps.map(step => step.hidden), [true, true, true, false]);
  assert.equal(h.next.hidden, true);
  assert.equal(h.build.hidden, false);
  assert.equal(h.tabs[3].attrs['aria-current'], 'step');
});

test('progress tabs cannot skip required intermediate steps', () => {
  const h = wizardHarness();
  const wizard = createWizard(h.form);

  h.tabs[3].click();

  assert.equal(wizard.current, 1);
  assert.deepEqual(h.steps.map(step => step.hidden), [true, false, true, true]);
});

test('payload summary derives a review card from submitted values', () => {
  assert.deepEqual(
    summarizePayload({
      name: 'field-kit', os: 'linux', arch: 'amd64', protocol: 'https',
      lhost: '10.0.0.8', lport: '8443', interval_ms: '1500', jitter_ms: '300',
    }),
    [
      ['Identity', 'field-kit'],
      ['Target', 'linux / amd64'],
      ['Callback', 'HTTPS · 10.0.0.8:8443'],
      ['Timing', '1500 ms · ±300 ms'],
    ],
  );
});

test('gsocket summary describes the local forward without exposing its secret', () => {
  const summary = summarizePayload({
    name: 'gs-kit', os: 'linux', arch: 'amd64', protocol: 'gs',
    lhost: '127.0.0.1', lport: '4630', gsocket_secret: 'never-render-me',
    gsocket_local_port: '4630', interval_ms: '1500', jitter_ms: '300',
  });
  assert.deepEqual(summary[2], ['Callback', 'GSOCKET · local forward :4630']);
  assert.equal(JSON.stringify(summary).includes('never-render-me'), false);
});

test('chat protocol reveals token/address fields and pins host/port', () => {
  const h = chatHarness();
  createWizard(h.form);

  h.protocol.value = 'discord';
  h.protocol.fire('change');

  assert.equal(h.chatGroup.hidden, false, 'chat fields must be visible');
  h.chatInputs.forEach(input => {
    assert.equal(input.disabled, false, 'chat inputs must be enabled');
    assert.equal(input.required, true, 'chat inputs must be required');
  });
  assert.equal(h.lhost.readOnly, true, 'LHost is meaningless for chat');
  assert.equal(h.lport.readOnly, true);
  assert.equal(h.lhost.value, '—');
  assert.equal(h.lport.hasRequired, false, 'a chat payload has no port');
});

test('switching away from a chat protocol restores host and port', () => {
  const h = chatHarness();
  createWizard(h.form);

  h.protocol.value = 'discord';
  h.protocol.fire('change');
  h.protocol.value = 'https';
  h.protocol.fire('change');

  assert.equal(h.chatGroup.hidden, true);
  h.chatInputs.forEach(input => assert.equal(input.disabled, true));
  assert.equal(h.lhost.readOnly, false, 'LHost must be editable again');
  assert.equal(h.lhost.value, 'c2.lab.example', 'previous host is restored');
  assert.equal(h.lport.readOnly, false);
  assert.equal(h.lport.value, '8443', 'previous port is restored');
  assert.equal(h.lport.hasRequired, true, 'host protocols still need a port');
});
