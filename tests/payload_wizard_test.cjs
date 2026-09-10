const { test } = require('node:test');
const assert = require('node:assert/strict');

const { createWizard, summarizePayload } = require('../static/payload_wizard.js');

function interactiveElement() {
  const listeners = {};
  return {
    hidden: false,
    disabled: false,
    attrs: {},
    addEventListener(type, fn) { listeners[type] = fn; },
    setAttribute(name, value) { this.attrs[name] = String(value); },
    removeAttribute(name) { delete this.attrs[name]; },
    click() { listeners.click?.({ preventDefault() {} }); },
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
