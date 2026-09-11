const { test } = require('node:test');
const assert = require('node:assert/strict');
const { createWorkspace } = require('../static/callback-workspace.js');

class Element {
  constructor(tag = 'div') {
    this.tagName = tag.toUpperCase();
    this.children = [];
    this.dataset = {};
    this.attributes = {};
    this.listeners = {};
    this.className = '';
    this._textContent = '';
    this.value = '';
    this.hidden = false;
    this.disabled = false;
    this.parentNode = null;
  }
  append(...children) { for (const child of children) { child.parentNode = this; this.children.push(child); } }
  prepend(child) { child.parentNode = this; this.children.unshift(child); }
  replaceChildren(...children) { this._textContent = ''; this.children = []; this.append(...children); }
  get textContent() { return this._textContent + this.children.map(child => child.textContent).join(''); }
  set textContent(value) {
    this._textContent = String(value);
    for (const child of this.children) child.parentNode = null;
    this.children = [];
  }
  remove() { if (this.parentNode) this.parentNode.children = this.parentNode.children.filter(child => child !== this); }
  setAttribute(name, value) { this.attributes[name] = String(value); }
  getAttribute(name) { return this.attributes[name] ?? null; }
  addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); }
  dispatch(type, values = {}) {
    const event = { target: this, preventDefault() { this.defaultPrevented = true; }, ...values };
    event.pending = Promise.all((this.listeners[type] || []).map(listener => listener(event)));
    return event;
  }
  matches(selector) {
    if (selector.startsWith('[data-')) {
      const key = selector.slice(6, -1).replace(/-([a-z])/g, (_, c) => c.toUpperCase());
      return key in this.dataset;
    }
    if (selector.startsWith('.')) return this.className.split(/\s+/).includes(selector.slice(1));
    if (selector.startsWith('#')) return this.attributes.id === selector.slice(1);
    return this.tagName.toLowerCase() === selector.toLowerCase();
  }
  querySelectorAll(selector) {
    const found = [];
    const visit = node => {
      for (const child of node.children) {
        if (child.matches(selector)) found.push(child);
        visit(child);
      }
    };
    visit(this);
    return found;
  }
  querySelector(selector) { return this.querySelectorAll(selector)[0] || null; }
  closest(selector) {
    let node = this;
    while (node) { if (node.matches(selector)) return node; node = node.parentNode; }
    return null;
  }
}

const document = { createElement: tag => new Element(tag) };

function skeleton() {
  const root = new Element('section');
  root.dataset.callbackTasking = '';
  root.dataset.tasksEndpoint = '/api/callbacks/callback-1/tasks';
  root.dataset.eventsEndpoint = '/api/callbacks/callback-1/events';
  root.dataset.csrfToken = 'csrf';
  root.dataset.online = 'false';
  for (const key of ['taskList', 'taskEmpty', 'taskSearch', 'taskStateFilter', 'taskErrorFilter', 'loadOlder', 'connectionState', 'offlineQueue', 'commandInput', 'commandArguments', 'taskForm']) {
    const element = new Element(key === 'taskForm' ? 'form' : key.includes('Filter') ? 'select' : key.includes('Input') || key === 'taskSearch' || key === 'commandArguments' ? 'input' : 'div');
    element.dataset[key] = '';
    root.append(element);
  }
  for (const key of ['processPanel', 'processSearch', 'processTableBody', 'processEmpty', 'processSnapshotAge', 'processMessage', 'processRefresh', 'processDetail', 'processConfirm', 'processConfirmText', 'processConfirmSubmit', 'processConfirmCancel', 'processTaskLink']) {
    const element = new Element(key === 'processSearch' ? 'input' : key === 'processRefresh' || key.includes('Submit') || key.includes('Cancel') ? 'button' : 'div');
    element.dataset[key] = '';
    root.append(element);
  }
  const processPanel = root.querySelector('[data-process-panel]');
  processPanel.dataset.processesEndpoint = '/api/callbacks/callback-1/processes';
  processPanel.dataset.processCapable = 'true';
  const sortName = new Element('button');
  sortName.dataset.processSort = 'name';
  const sortPid = new Element('button');
  sortPid.dataset.processSort = 'pid';
  root.append(sortName, sortPid);
  for (const key of ['filePanel', 'filePathForm', 'filePath', 'fileBreadcrumbs', 'fileParent', 'fileTableBody', 'fileEmpty', 'fileSnapshotAge', 'fileMessage', 'fileRefresh', 'fileMkdirForm', 'fileMkdirName', 'fileUpload', 'fileDownload', 'fileUploadForm', 'fileUploadInput', 'fileUploadDestination', 'fileTransferList', 'fileTaskLink', 'fileConfirm', 'fileConfirmText', 'fileConfirmSubmit', 'fileConfirmCancel', 'fileMoveFields', 'fileMoveDestination', 'fileDeleteFields', 'fileDeleteRecursive']) {
    const isButton = ['fileParent', 'fileRefresh', 'fileUpload', 'fileDownload', 'fileConfirmSubmit', 'fileConfirmCancel'].includes(key);
    const isInput = ['filePath', 'fileMkdirName', 'fileMoveDestination', 'fileDeleteRecursive', 'fileUploadInput', 'fileUploadDestination'].includes(key);
    const isForm = ['filePathForm', 'fileMkdirForm', 'fileUploadForm'].includes(key);
    const element = new Element(isButton ? 'button' : isInput ? 'input' : isForm ? 'form' : 'div');
    element.dataset[key] = '';
    root.append(element);
  }
  const filePanel = root.querySelector('[data-file-panel]');
  filePanel.dataset.filesEndpoint = '/api/callbacks/callback-1/files';
  filePanel.dataset.transfersEndpoint = '/api/callbacks/callback-1/transfers';
  filePanel.dataset.fileCapable = 'true';
  filePanel.dataset.transferCapable = 'false';
  filePanel.dataset.defaultPath = '/';
  for (const key of ['name', 'size', 'modified_at']) {
    const sort = new Element('button');
    sort.dataset.fileSort = key;
    root.append(sort);
  }
  for (const tab of ['tasking', 'processes', 'files']) {
    const link = new Element('a');
    link.dataset.callbackTab = tab;
    link.setAttribute('href', `?tab=${tab}`);
    root.append(link);
  }
  return root;
}

function task(overrides = {}) {
  return {
    id: 'task-1', session_id: 'callback-1', command: 'whoami', arguments: [], timeout_ms: 30000,
    status: 'pending', state_label: 'Queued', state_class: 'neutral', operator_id: 'op-1',
    operator_name: 'alice', parent_task_id: null, created_at: '2026-09-10T12:00:00Z',
    updated_at: '2026-09-10T12:00:00Z', processing_at: null, completed_at: null,
    cancellation_requested_at: null, stdout: null, stderr: null, exit_code: null, ...overrides,
  };
}

function dependencies(pages = [{ tasks: [], next_before: null }]) {
  let index = 0;
  const deps = {};
  class EventSource {
    constructor() { this.listeners = {}; deps.source = this; }
    addEventListener(type, listener) { this.listeners[type] = listener; }
    close() {}
  }
  Object.assign(deps, {
    document, EventSource,
    fetch: async url => url.endsWith('/processes') || url.includes('/files?')
      ? ({ ok: true, json: async () => null })
      : ({ ok: true, json: async () => pages[Math.min(index++, pages.length - 1)] }),
  });
  return deps;
}

const flush = () => new Promise(resolve => setImmediate(resolve));

function processSnapshot(overrides = {}) {
  return {
    session_id: 'callback-1',
    task_id: 'process-task-1',
    schema_version: 'nw.process-list.v1',
    captured_at: '2026-09-10T10:00:00Z',
    snapshot_json: {
      schema: 'nw.process-list.v1', captured_at: '2026-09-10T10:00:00Z',
      processes: [
        { pid: 7331, parent_pid: 1, name: 'zeta-worker', executable: '/opt/zeta', user: 'alice', architecture: null, cpu_percent: 2.5, memory_bytes: 8192, started_at: null },
        { pid: 42, parent_pid: null, name: 'alpha-worker', executable: null, user: null, architecture: 'x86_64', cpu_percent: 0.25, memory_bytes: 4096, started_at: '2026-09-10T09:00:00Z' },
      ],
    },
    ...overrides,
  };
}

function filesystemSnapshot(path = '/srv/lab', overrides = {}) {
  return {
    session_id: 'callback-1', path, task_id: 'file-task-1', schema_version: 'nw.fs-list.v1',
    captured_at: '2026-09-10T10:00:00Z',
    snapshot_json: {
      schema: 'nw.fs-list.v1', path, captured_at: '2026-09-10T10:00:00Z',
      entries: [
        { name: '<img src=x onerror=alert(1)>.txt', path: `${path}/<img src=x onerror=alert(1)>.txt`, kind: 'file', size: 4, modified_at: null, permissions: '0644', owner: 'alice' },
        { name: 'archive', path: `${path}/archive`, kind: 'directory', size: 0, modified_at: '2026-09-10T09:00:00Z', permissions: '0755', owner: null },
      ],
    },
    ...overrides,
  };
}

function urlDependencies(path = '/srv/lab') {
  const deps = dependencies();
  deps.location = { href: `https://lab.test/callbacks/callback-1?tab=files&path=${encodeURIComponent(path)}` };
  deps.history = {
    calls: [],
    pushState(_state, _title, value) {
      this.calls.push(value);
      deps.location.href = new URL(value, deps.location.href).href;
    },
  };
  deps.window = {
    listeners: {},
    addEventListener(type, listener) { (this.listeners[type] ||= []).push(listener); },
    removeEventListener(type, listener) { this.listeners[type] = (this.listeners[type] || []).filter(candidate => candidate !== listener); },
    dispatch(type) { return Promise.all((this.listeners[type] || []).map(listener => listener())); },
  };
  return deps;
}

test('same task snapshots dedupe by ID and update the existing card state', async () => {
  const root = skeleton();
  const workspace = createWorkspace(root, dependencies());
  await workspace.ready;
  workspace.upsert(task());
  const firstCard = root.querySelector('[data-task-card]');
  workspace.upsert(task({ status: 'completed', state_label: 'Completed', state_class: 'success', stdout: 'root', exit_code: 0 }));
  assert.equal(root.querySelectorAll('[data-task-card]').length, 1);
  assert.equal(root.querySelector('[data-task-card]'), firstCard);
  assert.equal(firstCard.dataset.taskStatus, 'completed');
  assert.equal(firstCard.querySelector('[data-task-state]').textContent, 'Completed');
  assert.equal(firstCard.querySelector('[data-task-stdout]').textContent, 'root');
});

test('authoritative reload data restores prior output', async () => {
  const root = skeleton();
  const workspace = createWorkspace(root, dependencies([{ tasks: [task({ status: 'completed', state_label: 'Completed', stdout: 'persisted output', exit_code: 0 })], next_before: null }]));
  await workspace.ready;
  assert.equal(root.querySelectorAll('[data-task-card]').length, 1);
  assert.equal(root.querySelector('[data-task-stdout]').textContent, 'persisted output');
});

test('filters hide cards without deleting task history', async () => {
  const root = skeleton();
  const workspace = createWorkspace(root, dependencies());
  await workspace.ready;
  workspace.upsert(task({ id: 'ok', command: 'hostname', status: 'completed', state_label: 'Completed', stdout: 'lab' }));
  workspace.upsert(task({ id: 'bad', command: 'cat', status: 'error', state_label: 'Error', stderr: 'denied' }));
  const search = root.querySelector('[data-task-search]');
  search.value = 'hostname';
  search.dispatch('input');
  assert.equal(root.querySelectorAll('[data-task-card]').length, 2);
  assert.equal(root.querySelectorAll('[data-task-card]').filter(card => !card.hidden).length, 1);
  search.value = '';
  search.dispatch('input');
  assert.equal(root.querySelectorAll('[data-task-card]').filter(card => !card.hidden).length, 2);
});

test('ArrowUp selects the newest persisted command after reload', async () => {
  const root = skeleton();
  const newest = task({ id: 'new', command: 'uname', created_at: '2026-09-10T12:01:00Z' });
  const older = task({ id: 'old', command: 'pwd', created_at: '2026-09-10T12:00:00Z' });
  const workspace = createWorkspace(root, dependencies([{ tasks: [newest, older], next_before: null }]));
  await workspace.ready;
  const input = root.querySelector('[data-command-input]');
  const event = input.dispatch('keydown', { key: 'ArrowUp' });
  assert.equal(event.defaultPrevented, true);
  assert.equal(input.value, 'uname');
});

test('SSE reconnect refetches the first page and upserts by ID', async () => {
  const root = skeleton();
  const deps = dependencies([
    { tasks: [task()], next_before: null },
    { tasks: [task({ status: 'completed', state_label: 'Completed', stdout: 'after reconnect' })], next_before: null },
  ]);
  const workspace = createWorkspace(root, deps);
  await workspace.ready;
  deps.source.onerror();
  deps.source.onopen();
  await flush();
  assert.equal(root.querySelectorAll('[data-task-card]').length, 1);
  assert.equal(root.querySelector('[data-task-stdout]').textContent, 'after reconnect');
});

test('closing during the initial fetch prevents a late EventSource connection', async () => {
  const root = skeleton();
  let resolveFetch;
  const deps = dependencies();
  deps.fetch = () => new Promise(resolve => { resolveFetch = resolve; });
  const workspace = createWorkspace(root, deps);

  workspace.close();
  resolveFetch({ ok: true, json: async () => ({ tasks: [], next_before: null }) });
  await workspace.ready;

  assert.equal(deps.source, undefined);
});

test('task submission failure is handled and shows actionable retry status', async () => {
  const root = skeleton();
  const deps = dependencies();
  let calls = 0;
  deps.fetch = async () => {
    calls++;
    if (calls === 1) return { ok: true, json: async () => ({ tasks: [], next_before: null }) };
    return { ok: false, status: 503 };
  };
  const workspace = createWorkspace(root, deps);
  await workspace.ready;
  root.querySelector('[data-command-input]').value = 'whoami';

  const event = root.querySelector('[data-task-form]').dispatch('submit');
  await event.pending;

  assert.match(root.querySelector('[data-connection-state]').textContent, /Unable to submit task.*retry/i);
});

for (const action of ['retry', 'cancel']) {
  test(`${action} failure is handled and shows actionable retry status`, async () => {
    const root = skeleton();
    const deps = dependencies();
    let calls = 0;
    deps.fetch = async () => {
      calls++;
      if (calls === 1) return { ok: true, json: async () => ({ tasks: [], next_before: null }) };
      throw new Error('offline');
    };
    const workspace = createWorkspace(root, deps);
    await workspace.ready;
    workspace.upsert(task({ status: action === 'retry' ? 'completed' : 'pending', state_label: action === 'retry' ? 'Completed' : 'Queued' }));
    const card = root.querySelector('[data-task-card]');
    const button = card.querySelectorAll('[data-task-action]').find(candidate => candidate.dataset.taskAction === action);

    const event = root.querySelector('[data-task-list]').dispatch('click', { target: button });
    await event.pending;

    assert.match(root.querySelector('[data-connection-state]').textContent, new RegExp(`Unable to ${action} task.*retry`, 'i'));
  });
}

test('process snapshot shows stale age and supports sorting, search, and detail safely', async () => {
  const root = skeleton();
  const deps = dependencies();
  deps.now = () => Date.parse('2026-09-10T12:30:00Z');
  deps.fetch = async url => url.endsWith('/processes')
    ? ({ ok: true, json: async () => processSnapshot() })
    : ({ ok: true, json: async () => ({ tasks: [], next_before: null }) });
  const workspace = createWorkspace(root, deps);
  await workspace.ready;

  assert.match(root.querySelector('[data-process-snapshot-age]').textContent, /2h 30m old.*stale/i);
  assert.equal(root.querySelectorAll('[data-process-row]')[0].dataset.processPid, '42');
  root.querySelector('[data-process-sort]').dispatch('click');
  assert.equal(root.querySelectorAll('[data-process-row]')[0].dataset.processPid, '7331');
  const search = root.querySelector('[data-process-search]');
  search.value = 'alpha';
  search.dispatch('input');
  assert.equal(root.querySelectorAll('[data-process-row]').filter(row => !row.hidden).length, 1);

  const visible = root.querySelectorAll('[data-process-row]').find(row => !row.hidden);
  root.querySelector('[data-process-table-body]').dispatch('click', { target: visible });
  assert.match(root.querySelector('[data-process-detail]').textContent, /alpha-worker.*PID 42.*Unavailable/s);
  assert.equal(root.querySelector('[data-process-detail]').querySelector('.process-detail-name').textContent, 'alpha-worker');
  assert.equal(root.querySelector('[data-process-detail]').querySelector('.process-detail-pid').textContent, 'PID 42');
  assert.equal(root.querySelector('[data-process-task-link]').getAttribute('href'), '#task-process-task-1');
});

test('process kill confirmation names the exact process and PID and links the queued task', async () => {
  const root = skeleton();
  const deps = dependencies();
  const calls = [];
  deps.fetch = async (url, options = {}) => {
    calls.push([url, options]);
    if (url.endsWith('/processes')) return { ok: true, json: async () => processSnapshot() };
    if (url.includes('/processes/7331/kill')) return { ok: true, json: async () => task({ id: 'kill-task-9', command: 'nw/process-kill', arguments: ['7331'] }) };
    return { ok: true, json: async () => ({ tasks: [], next_before: null }) };
  };
  const workspace = createWorkspace(root, deps);
  await workspace.ready;
  const row = root.querySelectorAll('[data-process-row]').find(candidate => candidate.dataset.processPid === '7331');
  const kill = row.querySelector('[data-process-kill]');
  root.querySelector('[data-process-table-body]').dispatch('click', { target: kill });
  assert.match(root.querySelector('[data-process-confirm-text]').textContent, /zeta-worker.*PID 7331/);

  const event = root.querySelector('[data-process-confirm-submit]').dispatch('click');
  await event.pending;
  assert.equal(calls.filter(([url]) => url.includes('/processes/7331/kill')).length, 1);
  assert.equal(root.querySelector('[data-process-task-link]').getAttribute('href'), '#task-kill-task-9');
  assert.match(root.querySelector('[data-process-task-link]').textContent, /kill-task-9/);
});

test('process controls explain unsupported capability and offline state', async () => {
  const unsupported = skeleton();
  unsupported.querySelector('[data-process-panel]').dataset.processCapable = 'false';
  const unsupportedWorkspace = createWorkspace(unsupported, dependencies());
  await unsupportedWorkspace.ready;
  assert.equal(unsupported.querySelector('[data-process-refresh]').disabled, true);
  assert.match(unsupported.querySelector('[data-process-message]').textContent, /does not advertise process control/i);

  const offline = skeleton();
  const offlineWorkspace = createWorkspace(offline, dependencies());
  await offlineWorkspace.ready;
  assert.match(offline.querySelector('[data-process-message]').textContent, /offline.*queued/i);
});

test('manual process refresh links its generated task in Tasking', async () => {
  const root = skeleton();
  const deps = dependencies();
  deps.fetch = async url => {
    if (url.endsWith('/processes')) return { ok: true, json: async () => null };
    if (url.endsWith('/processes/refresh')) return { ok: true, json: async () => task({ id: 'refresh-task-manual', command: 'nw/process-list' }) };
    return { ok: true, json: async () => ({ tasks: [], next_before: null }) };
  };
  const workspace = createWorkspace(root, deps);
  await workspace.ready;
  const refresh = root.querySelector('[data-process-refresh]').dispatch('click');
  await refresh.pending;
  assert.equal(root.querySelector('[data-process-task-link]').getAttribute('href'), '#task-refresh-task-manual');
});

test('process snapshot failure does not prevent Tasking SSE connection', async () => {
  const root = skeleton();
  const deps = dependencies();
  deps.fetch = async url => url.endsWith('/processes')
    ? ({ ok: false, status: 503 })
    : ({ ok: true, json: async () => ({ tasks: [], next_before: null }) });
  const workspace = createWorkspace(root, deps);
  await workspace.ready;
  assert.ok(deps.source, 'Tasking EventSource should connect despite process failure');
  assert.match(root.querySelector('[data-process-message]').textContent, /unavailable.*tasking.*connected/i);
});

test('process kill refreshes the snapshot only after the linked task completes', async () => {
  const root = skeleton();
  const deps = dependencies();
  const calls = [];
  deps.fetch = async (url, options = {}) => {
    calls.push([url, options]);
    if (url.endsWith('/processes')) return { ok: true, json: async () => processSnapshot() };
    if (url.includes('/processes/7331/kill')) return { ok: true, json: async () => task({ id: 'kill-task-10', command: 'nw/process-kill', arguments: ['7331'] }) };
    return { ok: true, json: async () => ({ tasks: [], next_before: null }) };
  };
  const workspace = createWorkspace(root, deps);
  await workspace.ready;
  const row = root.querySelectorAll('[data-process-row]').find(candidate => candidate.dataset.processPid === '7331');
  root.querySelector('[data-process-table-body]').dispatch('click', { target: row.querySelector('[data-process-kill]') });
  const submit = root.querySelector('[data-process-confirm-submit]').dispatch('click');
  await submit.pending;
  assert.equal(calls.filter(([url]) => url.endsWith('/processes/refresh')).length, 0);

  deps.source.listeners.task({ data: JSON.stringify(task({ id: 'kill-task-10', command: 'nw/process-kill', status: 'completed', state_label: 'Completed' })) });
  await flush();
  assert.equal(calls.filter(([url]) => url.endsWith('/processes')).length, 1);
  deps.source.listeners.task({ data: JSON.stringify(task({ id: 'refresh-task-10', command: 'nw/process-list', parent_task_id: 'kill-task-10', status: 'completed', state_label: 'Completed' })) });
  await flush();
  assert.equal(calls.filter(([url]) => url.endsWith('/processes')).length, 2);
});

test('failed process kill does not refresh the process snapshot', async () => {
  const root = skeleton();
  const deps = dependencies();
  const calls = [];
  deps.fetch = async (url, options = {}) => {
    calls.push([url, options]);
    if (url.endsWith('/processes')) return { ok: true, json: async () => processSnapshot() };
    if (url.includes('/processes/7331/kill')) return { ok: true, json: async () => task({ id: 'kill-task-failed', command: 'nw/process-kill' }) };
    return { ok: true, json: async () => ({ tasks: [], next_before: null }) };
  };
  const workspace = createWorkspace(root, deps);
  await workspace.ready;
  const row = root.querySelectorAll('[data-process-row]').find(candidate => candidate.dataset.processPid === '7331');
  root.querySelector('[data-process-table-body]').dispatch('click', { target: row.querySelector('[data-process-kill]') });
  const submit = root.querySelector('[data-process-confirm-submit]').dispatch('click');
  await submit.pending;
  deps.source.listeners.task({ data: JSON.stringify(task({ id: 'kill-task-failed', command: 'nw/process-kill', status: 'error', state_label: 'Error' })) });
  await flush();
  assert.equal(calls.filter(([url]) => url.endsWith('/processes')).length, 1);
});

test('filesystem URL restores path, safe rows, stale time, breadcrumbs, parent, and sorting', async () => {
  const root = skeleton();
  const deps = urlDependencies('/srv/lab');
  deps.now = () => Date.parse('2026-09-10T12:30:00Z');
  deps.fetch = async url => {
    if (url.includes('/files?')) return { ok: true, json: async () => filesystemSnapshot() };
    if (url.endsWith('/processes')) return { ok: true, json: async () => null };
    return { ok: true, json: async () => ({ tasks: [], next_before: null }) };
  };
  const workspace = createWorkspace(root, deps);
  await workspace.ready;

  assert.equal(root.querySelector('[data-file-path]').value, '/srv/lab');
  assert.equal(root.querySelectorAll('[data-callback-tab]').find(link => link.dataset.callbackTab === 'files').getAttribute('aria-current'), 'page');
  assert.equal(root.querySelector('[data-file-parent]').dataset.filePath, '/srv');
  assert.match(root.querySelector('[data-file-breadcrumbs]').textContent, /\/.*srv.*lab/s);
  assert.match(root.querySelector('[data-file-snapshot-age]').textContent, /2h 30m old.*stale/i);
  const unsafeRow = root.querySelectorAll('[data-file-row]').find(row => row.dataset.filePath.includes('<img'));
  assert.match(unsafeRow.textContent, /<img src=x onerror=alert\(1\)>.txt/);
  assert.equal(unsafeRow.querySelector('img'), null, 'remote names must remain text, never HTML');
  assert.equal(root.querySelectorAll('[data-file-row]')[0].dataset.filePath, '/srv/lab/archive');
  root.querySelectorAll('[data-file-sort]').find(button => button.dataset.fileSort === 'size').dispatch('click');
  assert.equal(root.querySelectorAll('[data-file-row]')[0].dataset.filePath.includes('<img'), true);
  assert.equal(root.querySelector('[data-file-task-link]').getAttribute('href'), '#task-file-task-1');
});

test('filesystem directory, parent, breadcrumb, and path bar navigation update the URL', async () => {
  const root = skeleton();
  const deps = urlDependencies('/srv/lab');
  deps.fetch = async url => {
    if (url.includes('/files?')) {
      const path = new URL(url, deps.location.href).searchParams.get('path');
      return { ok: true, json: async () => path === '/srv/lab' ? filesystemSnapshot() : null };
    }
    if (url.endsWith('/processes')) return { ok: true, json: async () => null };
    return { ok: true, json: async () => ({ tasks: [], next_before: null }) };
  };
  const workspace = createWorkspace(root, deps);
  await workspace.ready;

  const directory = root.querySelectorAll('[data-file-row]').find(row => row.dataset.fileKind === 'directory');
  root.querySelector('[data-file-table-body]').dispatch('click', { target: directory });
  await flush();
  assert.match(deps.history.calls.at(-1), /tab=files/);
  assert.match(deps.history.calls.at(-1), /path=%2Fsrv%2Flab%2Farchive/);

  root.querySelector('[data-file-parent]').dispatch('click');
  await flush();
  assert.equal(new URL(deps.history.calls.at(-1), 'https://lab.test').searchParams.get('path'), '/srv/lab');

  root.querySelector('[data-file-path]').value = '/var/tmp';
  const submit = root.querySelector('[data-file-path-form]').dispatch('submit');
  await submit.pending;
  assert.equal(new URL(deps.history.calls.at(-1), 'https://lab.test').searchParams.get('path'), '/var/tmp');

  deps.location.href = 'https://lab.test/callbacks/callback-1?tab=files&path=%2Fsrv%2Flab#files';
  await deps.window.dispatch('popstate');
  assert.equal(root.querySelector('[data-file-path]').value, '/srv/lab');
});

test('filesystem mkdir, move, and delete use exact confirmations, CSRF, and task links', async () => {
  const root = skeleton();
  const deps = urlDependencies('/srv/lab');
  const calls = [];
  deps.fetch = async (url, options = {}) => {
    calls.push([url, options]);
    if (url.includes('/files?')) return { ok: true, json: async () => filesystemSnapshot() };
    if (url.endsWith('/processes')) return { ok: true, json: async () => null };
    if (url.includes('/files/')) return { ok: true, json: async () => task({ id: `file-action-${calls.length}`, command: 'nw/fs-mutation' }) };
    return { ok: true, json: async () => ({ tasks: [], next_before: null }) };
  };
  const workspace = createWorkspace(root, deps);
  await workspace.ready;

  root.querySelector('[data-file-mkdir-name]').value = 'new folder';
  root.querySelector('[data-file-mkdir-form]').dispatch('submit');
  assert.match(root.querySelector('[data-file-confirm-text]').textContent, /mkdir.*\/srv\/lab\/new folder/i);
  let confirmation = root.querySelector('[data-file-confirm-submit]').dispatch('click');
  await confirmation.pending;

  const fileRow = root.querySelectorAll('[data-file-row]').find(row => row.dataset.fileKind === 'file');
  root.querySelector('[data-file-table-body]').dispatch('click', { target: fileRow.querySelector('[data-file-move]') });
  root.querySelector('[data-file-move-destination]').value = '/srv/lab/renamed <safe>.txt';
  root.querySelector('[data-file-move-destination]').dispatch('input');
  assert.match(root.querySelector('[data-file-confirm-text]').textContent, /<img.*renamed <safe>.txt/s);
  confirmation = root.querySelector('[data-file-confirm-submit]').dispatch('click');
  await confirmation.pending;

  root.querySelector('[data-file-table-body]').dispatch('click', { target: fileRow.querySelector('[data-file-delete]') });
  root.querySelector('[data-file-delete-recursive]').checked = true;
  root.querySelector('[data-file-delete-recursive]').dispatch('change');
  assert.match(root.querySelector('[data-file-confirm-text]').textContent, /delete.*<img.*recursive/i);
  confirmation = root.querySelector('[data-file-confirm-submit]').dispatch('click');
  await confirmation.pending;

  const mutationCalls = calls.filter(([url]) => /\/files\/(mkdir|move|delete)$/.test(url));
  assert.equal(mutationCalls.length, 3);
  assert.equal(mutationCalls.every(([, options]) => options.headers['x-csrf-token'] === 'csrf'), true);
  assert.deepEqual(JSON.parse(mutationCalls[0][1].body), { path: '/srv/lab/new folder' });
  assert.deepEqual(JSON.parse(mutationCalls[1][1].body), { source: '/srv/lab/<img src=x onerror=alert(1)>.txt', destination: '/srv/lab/renamed <safe>.txt' });
  assert.deepEqual(JSON.parse(mutationCalls[2][1].body), { path: '/srv/lab/<img src=x onerror=alert(1)>.txt', recursive: true });
  assert.match(root.querySelector('[data-file-task-link]').getAttribute('href'), /^#task-file-action-/);
});

test('filesystem refresh is typed while unsupported controls and transfers stay disabled', async () => {
  const root = skeleton();
  const deps = urlDependencies('/');
  const calls = [];
  deps.fetch = async (url, options = {}) => {
    calls.push([url, options]);
    if (url.includes('/files?')) return { ok: true, json: async () => null };
    if (url.endsWith('/files/list')) return { ok: true, json: async () => task({ id: 'file-refresh-1', command: 'nw/fs-list' }) };
    if (url.endsWith('/processes')) return { ok: true, json: async () => null };
    return { ok: true, json: async () => ({ tasks: [], next_before: null }) };
  };
  const workspace = createWorkspace(root, deps);
  await workspace.ready;
  const refresh = root.querySelector('[data-file-refresh]').dispatch('click');
  await refresh.pending;
  const refreshCall = calls.find(([url]) => url.endsWith('/files/list'));
  assert.deepEqual(JSON.parse(refreshCall[1].body), { path: '/' });
  assert.equal(root.querySelector('[data-file-upload]').disabled, true);
  assert.equal(root.querySelector('[data-file-download]').disabled, true);
  assert.equal(root.querySelector('[data-file-upload]').textContent, 'Transfer support is being initialized');
  assert.equal(root.querySelector('[data-file-download]').textContent, 'Transfer support is being initialized');

  const unsupported = skeleton();
  unsupported.querySelector('[data-file-panel]').dataset.fileCapable = 'false';
  const unsupportedWorkspace = createWorkspace(unsupported, urlDependencies('/'));
  await unsupportedWorkspace.ready;
  assert.equal(unsupported.querySelector('[data-file-refresh]').disabled, true);
  assert.match(unsupported.querySelector('[data-file-message]').textContent, /does not advertise filesystem control/i);
});

test('transfer controls queue exact downloads and stream multipart uploads with CSRF', async () => {
  const root = skeleton();
  root.querySelector('[data-file-panel]').dataset.transferCapable = 'true';
  const deps = urlDependencies('/srv/lab');
  class FormData {
    constructor() { this.values = []; }
    append(name, value) { this.values.push([name, value]); }
  }
  deps.FormData = FormData;
  const calls = [];
  deps.fetch = async (url, options = {}) => {
    calls.push([url, options]);
    if (url.includes('/files?')) return { ok: true, json: async () => filesystemSnapshot() };
    if (url.endsWith('/processes')) return { ok: true, json: async () => null };
    if (url.endsWith('/files/download')) return { ok: true, json: async () => ({ id: 'download-safe', remote_path: '/srv/lab/<img>.txt', expected_size: 12, received_bytes: 0, status: 'queued', sha256: null }) };
    if (url.endsWith('/files/upload')) return { ok: true, json: async () => ({ id: 'upload-safe', remote_path: '/srv/lab/upload.bin', expected_size: 3, received_bytes: 0, status: 'queued', sha256: 'abc' }) };
    return { ok: true, json: async () => ({ tasks: [], next_before: null }) };
  };
  const workspace = createWorkspace(root, deps);
  await workspace.ready;
  assert.equal(root.querySelector('[data-file-upload]').disabled, false);
  assert.equal(root.querySelector('[data-file-download]').disabled, false);

  const fileRow = root.querySelectorAll('[data-file-row]').find(row => row.dataset.fileKind === 'file');
  const download = fileRow.querySelector('[data-file-download-action]');
  const event = root.querySelector('[data-file-table-body]').dispatch('click', { target: download });
  await event.pending;
  const downloadCall = calls.find(([url]) => url.endsWith('/files/download'));
  assert.deepEqual(JSON.parse(downloadCall[1].body), { path: '/srv/lab/<img src=x onerror=alert(1)>.txt', expected_size: 4 });
  assert.equal(downloadCall[1].headers['x-csrf-token'], 'csrf');

  root.querySelector('[data-file-upload-destination]').value = '/srv/lab/upload.bin';
  root.querySelector('[data-file-upload-input]').files = [{ name: '<img>.bin', size: 3 }];
  const upload = root.querySelector('[data-file-upload-form]').dispatch('submit');
  await upload.pending;
  const uploadCall = calls.find(([url]) => url.endsWith('/files/upload'));
  assert.equal(uploadCall[1].headers['x-csrf-token'], 'csrf');
  assert.equal(uploadCall[1].headers['content-type'], undefined);
  assert.deepEqual(uploadCall[1].body.values.map(([name]) => name), ['destination', 'file']);
});

test('transfer SSE renders progress checksum and unsafe errors as text only', async () => {
  const root = skeleton();
  root.querySelector('[data-file-panel]').dataset.transferCapable = 'true';
  const deps = urlDependencies('/srv/lab');
  const workspace = createWorkspace(root, deps);
  await workspace.ready;
  deps.source.listeners.transfer({ data: JSON.stringify({
    id: 'transfer-1', direction: 'download', remote_path: '<img src=x onerror=alert(1)>',
    expected_size: 3072, received_bytes: 1536, status: 'error', sha256: 'deadbeef', error: '<script>alert(1)</script>',
  }) });
  const item = root.querySelector('[data-transfer-id]');
  assert.match(item.textContent, /50%.*deadbeef.*<script>alert\(1\)<\/script>/s);
  assert.equal(item.querySelector('script'), null);
});
