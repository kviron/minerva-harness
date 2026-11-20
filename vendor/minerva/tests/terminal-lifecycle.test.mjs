import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';

function load(name, mocks) {
  const source = readFileSync(
    new URL(`../apps/web/src/plugins/terminal/${name}`, import.meta.url),
    'utf8',
  );
  const js = ts.transpileModule(source, {
    compilerOptions: {
      target: ts.ScriptTarget.ES2022,
      module: ts.ModuleKind.CommonJS,
      jsx: ts.JsxEmit.React,
    },
  }).outputText;
  const exports = {};
  new Function('require', 'exports', js)((id) => {
    if (id in mocks) return mocks[id];
    if (id.endsWith('.css')) return {};
    throw Error(`Unexpected import: ${id}`);
  }, exports);
  return exports;
}
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((a, b) => {
    resolve = a;
    reject = b;
  });
  return { promise, resolve, reject };
};
const tick = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
};
function setup(initial) {
  const store = load('store.ts', {});
  if (initial) store.terminals.set('session', initial);
  let cleanup,
    disposed = 0;
  const calls = [],
    response = new Map();
  const react = {
    createElement: () => ({}),
    useEffect: (fn) => {
      cleanup = fn();
    },
    useRef: () => ({ current: { clientWidth: 1, clientHeight: 1 } }),
    useState: (value) => [value, () => {}],
    useSyncExternalStore: () => [],
  };
  const term = {
    cols: 80,
    rows: 24,
    loadAddon() {},
    open() {},
    onData() {
      return { dispose() {} };
    },
    dispose() {
      disposed++;
    },
    focus() {},
    write() {},
  };
  const Panel = load('panel.tsx', {
    react: { ...react, default: react },
    '@xterm/xterm': {
      Terminal: class {
        constructor() {
          return term;
        }
      },
    },
    '@xterm/addon-fit': {
      FitAddon: class {
        fit() {}
      },
    },
    'lucide-react': {},
    '../../client': {
      invoke: (method, args) => {
        calls.push([method, args]);
        return response.get(method)?.(args) ?? new Promise(() => {});
      },
    },
    '../../app/models': { models: { workspaces: {} } },
    '../../components/ui/button': {},
    './store': store,
  }).default;
  return {
    store,
    calls,
    response,
    mount: (props = {}) => Panel({ sessionId: 'session', ...props }),
    unmount: () => cleanup(),
    disposed: () => disposed,
  };
}
globalThis.getComputedStyle = () => ({ getPropertyValue: () => '' });
globalThis.ResizeObserver = class {
  observe() {}
  disconnect() {}
};

test('late open closes its new shell without storing, polling, or flushing queued input', async () => {
  const h = setup(),
    opening = deferred();
  h.response.set('mh-terminal.open', () => opening.promise);
  h.store.pending.set('session', ['queued']);
  h.mount();
  h.unmount();
  opening.resolve('late');
  await tick();
  assert.equal(h.store.terminals.get('session'), undefined);
  assert.deepEqual(
    h.calls.map(([method]) => method),
    ['mh-terminal.open', 'mh-terminal.close'],
  );
  assert.deepEqual(h.store.pending.get('session'), ['queued']);
  assert.equal(h.disposed(), 1);
});
test('disable during open rejects ownership and closes the late shell before React unmount', async () => {
  const h = setup(),
    opening = deferred();
  let enabled = true;
  h.response.set('mh-terminal.open', () => opening.promise);
  h.mount({ onAcquire: () => enabled });
  enabled = false;
  opening.resolve('late');
  await tick();
  h.unmount();
  assert.equal(h.store.terminals.get('session'), undefined);
  assert.deepEqual(
    h.calls.map(([method]) => method),
    ['mh-terminal.open', 'mh-terminal.close'],
  );
});
test('remembered primary ID is owned before its resize finishes', async () => {
  const h = setup('remembered'),
    owned = [];
  h.mount({
    onAcquire: (key, id) => {
      owned.push([key, id]);
      return true;
    },
  });
  assert.deepEqual(owned, [['session', 'remembered']]);
  h.unmount();
});
test('unmount during remembered resize does not open or delete a newer shell', async () => {
  const h = setup('old'),
    resize = deferred();
  h.response.set('mh-terminal.resize', () => resize.promise);
  h.mount();
  h.unmount();
  h.store.terminals.set('session', 'new');
  resize.reject(Error('stale'));
  await tick();
  assert.equal(h.store.terminals.get('session'), 'new');
  assert.equal(h.calls.length, 1);
});
test('stale resize failure compares IDs and keeps the replacement', async () => {
  const h = setup('old'),
    resize = deferred();
  h.response.set('mh-terminal.resize', () => resize.promise);
  h.mount();
  h.store.terminals.set('session', 'new');
  resize.reject(Error('stale'));
  await tick();
  h.unmount();
  assert.equal(h.store.terminals.get('session'), 'new');
  assert.deepEqual(
    h.calls.map(([method]) => method),
    ['mh-terminal.resize', 'mh-terminal.read'],
  );
  assert.equal(h.calls[1][1].id, 'new');
});
test('late successful resize neither polls nor flushes after unmount', async () => {
  const h = setup('kept'),
    resize = deferred();
  h.response.set('mh-terminal.resize', () => resize.promise);
  h.mount();
  h.unmount();
  h.store.pending.set('session', ['queued']);
  resize.resolve();
  await tick();
  assert.equal(h.calls.length, 1);
  assert.equal(h.store.terminals.get('session'), 'kept');
  assert.deepEqual(h.store.pending.get('session'), ['queued']);
});
test('stale read failure cannot delete a newer terminal ID', async () => {
  const h = setup('old'),
    read = deferred();
  h.response.set('mh-terminal.resize', () => Promise.resolve());
  h.response.set('mh-terminal.read', () => read.promise);
  h.mount();
  await tick();
  h.store.terminals.set('session', 'new');
  read.reject(Error('stale'));
  await tick();
  h.unmount();
  assert.equal(h.store.terminals.get('session'), 'new');
});
test('ordinary remount reuses the terminal ID without closing it', async () => {
  const h = setup('kept');
  h.response.set('mh-terminal.resize', () => Promise.resolve());
  h.mount();
  await tick();
  h.unmount();
  h.mount();
  await tick();
  h.unmount();
  assert.equal(h.store.terminals.get('session'), 'kept');
  assert.deepEqual(
    h.calls.map(([method]) => method),
    ['mh-terminal.resize', 'mh-terminal.read', 'mh-terminal.resize', 'mh-terminal.read'],
  );
});
test('disable removes only its owned primary shells, pending input, and dynamic tabs', async () => {
  const store = load('store.ts', {}),
    tabs = new Map(),
    calls = [];
  const react = {
    createElement: (component, props, ...children) => ({
      component,
      props: { ...props, children },
    }),
    lazy: () => () => {},
    Suspense: () => {},
  };
  const { terminalPlugin } = load('index.tsx', {
    react: { ...react, default: react },
    'lucide-react': {},
    '../../app/canvas': {
      canvas: {
        show: (tab) => tabs.set(tab.id, tab),
        closeTab: (id) => {
          tabs.get(id)?.onClose();
          tabs.delete(id);
        },
      },
    },
    '../../components/ui/button': {},
    '../../components/ui/tooltip': {},
    './CommandBlock': {},
    '../../client': {
      invoke: (method, args) => {
        calls.push([method, args]);
        return Promise.resolve();
      },
    },
    './store': store,
  });
  const apply = () => {
    let cleanup;
    const entries = [];
    const ctx = {
      effect: (fn) => {
        cleanup = fn;
      },
      slots: { register: (name, entry) => entries.push(entry) },
    };
    terminalPlugin({ kind: 'root' }).apply({ scope: () => ctx });
    const owned = entries.find((entry) => entry.component?.name === 'OwnedTab').component;
    const tab = owned({ part: 'body', sessionId: 'session' });
    const panel = tab.component(tab.props).props.children[0];
    return { open: panel.props.onNew, acquire: panel.props.onAcquire, dispose: () => cleanup() };
  };
  const a = apply(),
    b = apply();
  a.open();
  b.open();
  const [first, second] = [...tabs.values()];
  const slot = first.id.split(':')[1];
  store.terminals.set(`session#${slot}`, 'owned');
  store.terminals.set('primary-a', 'shell-a');
  store.terminals.set('primary-b', 'shell-b');
  store.pending.set('primary-a', ['a']);
  store.pending.set('primary-b', ['b']);
  assert.equal(a.acquire('primary-a', 'shell-a'), true);
  assert.equal(b.acquire('primary-b', 'shell-b'), true);
  store.terminals.set('replaced', 'new');
  a.acquire('replaced', 'old');
  a.dispose();
  assert.equal(store.terminals.get('primary-a'), undefined);
  assert.equal(store.pending.has('primary-a'), false);
  assert.equal(store.terminals.get('primary-b'), 'shell-b');
  assert.deepEqual(store.pending.get('primary-b'), ['b']);
  assert.equal(store.terminals.get('replaced'), 'new');
  assert.equal(a.acquire('late', 'late'), false);
  assert.ok(calls.some(([, args]) => args.id === 'shell-a'));
  assert.ok(!calls.some(([, args]) => args.id === 'shell-b'));
  assert.deepEqual([...tabs.keys()], [second.id]);
  assert.equal(store.terminals.get(`session#${slot}`), undefined);
  assert.equal(calls[0][1].id, 'owned');
  a.open();
  assert.equal(tabs.size, 1);
  b.dispose();
  assert.equal(tabs.size, 0);
});
