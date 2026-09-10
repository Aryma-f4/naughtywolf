((root, factory) => {
  const api = factory(root?.document);
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (root) root.NWPayloadWizard = api;
})(typeof window !== 'undefined' ? window : globalThis, (documentRef) => {
  function summarizePayload(values) {
    const callback = values.protocol === 'gs'
      ? `GSOCKET · local forward :${values.gsocket_local_port || values.lport || '—'}`
      : `${String(values.protocol || 'http').toUpperCase()} · ${values.lhost || 'unset'}:${values.lport || '—'}`;
    return [
      ['Identity', values.name || 'Unnamed payload'],
      ['Target', `${values.os || 'unknown'} / ${values.arch || 'unknown'}`],
      ['Callback', callback],
      ['Timing', `${values.interval_ms || '—'} ms · ±${values.jitter_ms || '0'} ms`],
    ];
  }

  function renderReview(form) {
    const container = form.querySelector('[data-payload-review]');
    if (!container || typeof FormData === 'undefined' || !documentRef) return;
    const summary = summarizePayload(Object.fromEntries(new FormData(form)));
    const fragment = documentRef.createDocumentFragment();
    for (const [label, value] of summary) {
      const item = documentRef.createElement('div');
      const term = documentRef.createElement('span');
      const detail = documentRef.createElement('strong');
      term.textContent = label;
      detail.textContent = value;
      item.append(term, detail);
      fragment.append(item);
    }
    container.replaceChildren(fragment);
  }

  function createWizard(form) {
    const steps = [...form.querySelectorAll('[data-payload-step]')];
    const tabs = [...form.querySelectorAll('[data-wizard-tab]')];
    const back = form.querySelector('[data-wizard-back]');
    const next = form.querySelector('[data-wizard-next]');
    const build = form.querySelector('[data-wizard-build]');
    let current = 0;

    function syncGSocketFields() {
      const protocol = form.querySelector('#payload-proto');
      const group = form.querySelector('[data-gsocket-fields]');
      if (!protocol || !group) return;
      const enabled = protocol.value === 'gs';
      const host = form.querySelector('#payload-lhost');
      const port = form.querySelector('#payload-lport');
      const localPort = form.querySelector('#payload-gsocket-port');
      group.hidden = !enabled;
      group.querySelectorAll?.('input').forEach(input => {
        input.disabled = !enabled;
        input.required = enabled;
      });
      if (enabled) {
        if (host && host.dataset.gsocketPinned !== 'true') {
          host.dataset.directValue = host.value;
          host.value = '127.0.0.1';
          host.readOnly = true;
          host.dataset.gsocketPinned = 'true';
        }
        if (port && port.dataset.gsocketPinned !== 'true') {
          port.dataset.directValue = port.value;
          port.value = localPort?.value || '4630';
          port.readOnly = true;
          port.dataset.gsocketPinned = 'true';
        }
      } else {
        if (host?.dataset.gsocketPinned === 'true') {
          host.value = host.dataset.directValue || '';
          host.readOnly = false;
          delete host.dataset.gsocketPinned;
        }
        if (port?.dataset.gsocketPinned === 'true') {
          port.value = port.dataset.directValue || '8081';
          port.readOnly = false;
          delete port.dataset.gsocketPinned;
        }
      }
    }

    function show() {
      steps.forEach((step, index) => { step.hidden = index !== current; });
      tabs.forEach((tab, index) => {
        if (index === current) tab.setAttribute('aria-current', 'step');
        else tab.removeAttribute('aria-current');
        tab.setAttribute('aria-selected', String(index === current));
      });
      if (back) back.disabled = current === 0;
      if (next) next.hidden = current === steps.length - 1;
      if (build) build.hidden = current !== steps.length - 1;
      if (current === steps.length - 1) renderReview(form);
    }

    function stepIsValid() {
      const required = [...(steps[current]?.querySelectorAll?.('[required]') || [])];
      const invalid = required.find(field => !field.checkValidity());
      if (!invalid) return true;
      invalid.reportValidity();
      return false;
    }

    function goTo(index) {
      current = Math.max(0, Math.min(steps.length - 1, Number(index) || 0));
      show();
    }

    back?.addEventListener('click', () => goTo(current - 1));
    next?.addEventListener('click', () => { if (stepIsValid()) goTo(current + 1); });
    tabs.forEach(tab => tab.addEventListener('click', () => {
      const target = Number(tab.dataset.wizardTab);
      if (target <= current) goTo(target);
      else if (stepIsValid()) goTo(current + 1);
    }));
    form.addEventListener?.('input', () => { if (current === steps.length - 1) renderReview(form); });
    form.querySelector('#payload-proto')?.addEventListener('change', syncGSocketFields);
    form.querySelector('#payload-gsocket-port')?.addEventListener('input', event => {
      const port = form.querySelector('#payload-lport');
      if (port && form.querySelector('#payload-proto')?.value === 'gs') port.value = event.target.value;
    });
    form.querySelector('[data-gsocket-secret-generate]')?.addEventListener('click', () => {
      const input = form.querySelector('#payload-gsocket-secret');
      if (input && globalThis.crypto?.randomUUID) input.value = globalThis.crypto.randomUUID();
    });
    syncGSocketFields();
    show();
    return { goTo, get current() { return current; } };
  }

  function init(scope = documentRef) {
    if (!scope) return;
    scope.querySelectorAll('[data-payload-wizard]').forEach(form => {
      if (form.dataset.wizardReady === 'true') return;
      form.dataset.wizardReady = 'true';
      createWizard(form);
    });
  }

  return { createWizard, summarizePayload, init };
});
