import test from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { createRequire } from 'node:module';

const result = await build({
  entryPoints: ['plugins/mh-skills/ui/controller.ts'],
  bundle: true,
  platform: 'node',
  format: 'cjs',
  write: false,
});
const module = { exports: {} };
new Function('require', 'module', 'exports', result.outputFiles[0].text)(
  createRequire(import.meta.url),
  module,
  module.exports,
);
const { SkillsController } = module.exports;
const catalog = (name) => ({ items: [{ name }], incomplete: false, diagnostics: [] });
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
};

test('catalog snapshots and pending requests survive subscriber unmount/remount per workspace', async () => {
  const requests = [];
  const controller = new SkillsController((id) => {
    const request = deferred();
    requests.push({ id, ...request });
    return request.promise;
  });
  const global = controller.catalog(),
    local = controller.catalog('local');
  const unsubscribe = global.subscribe(() => {});
  controller.ensure();
  controller.ensure();
  unsubscribe();
  const remount = global.subscribe(() => {});
  controller.ensure();
  assert.equal(requests.length, 1);
  requests[0].resolve(catalog('global'));
  await requests[0].promise;
  await Promise.resolve();
  const snapshot = global.getSnapshot();
  remount();
  controller.ensure();
  assert.strictEqual(controller.catalog().getSnapshot(), snapshot);
  assert.equal(requests.length, 1);
  controller.ensure('local');
  assert.equal(requests[1].id, 'local');
  requests[1].resolve(catalog('local'));
  await requests[1].promise;
  await Promise.resolve();
  assert.equal(local.getSnapshot().catalog.items[0].name, 'local');
  assert.strictEqual(global.getSnapshot(), snapshot);
  controller.dispose();
});

test('manual refresh retries failures, keeps last catalog, and fences superseded responses', async () => {
  const requests = [];
  const controller = new SkillsController(() => {
    const request = deferred();
    requests.push(request);
    return request.promise;
  });
  const store = controller.catalog();
  const initial = controller.refresh();
  requests[0].reject(new Error('offline'));
  await initial;
  assert.match(store.getSnapshot().error, /offline/);
  const retry = controller.refresh();
  requests[1].resolve(catalog('saved'));
  await retry;
  const saved = store.getSnapshot().catalog;
  const stale = controller.refresh(),
    latest = controller.refresh();
  assert.strictEqual(store.getSnapshot().catalog, saved);
  requests[3].resolve(catalog('latest'));
  await latest;
  requests[2].reject(new Error('stale failure'));
  await stale;
  assert.equal(store.getSnapshot().catalog.items[0].name, 'latest');
  assert.equal(store.getSnapshot().error, '');
  const staleSuccess = controller.refresh(),
    failure = controller.refresh();
  requests[5].reject('retry failed');
  await failure;
  requests[4].resolve(catalog('stale success'));
  await staleSuccess;
  assert.equal(store.getSnapshot().catalog.items[0].name, 'latest');
  assert.equal(store.getSnapshot().error, 'retry failed');
  controller.dispose();
});

test('apply owns controller lifetime and shares it between registered callers', async () => {
  const bundle = await build({
    entryPoints: ['plugins/mh-skills/ui/index.tsx'],
    bundle: true,
    platform: 'node',
    format: 'cjs',
    write: false,
    loader: { '.css': 'empty' },
    plugins: [
      {
        name: 'rpc',
        setup(build) {
          build.onLoad({ filter: /apps[\\/]web[\\/]src[\\/]client\.ts$/ }, () => ({
            contents:
              'export async function invoke(){return {items:[],incomplete:false,diagnostics:[]}}',
            loader: 'ts',
          }));
        },
      },
    ],
  });
  const loaded = { exports: {} };
  new Function('require', 'module', 'exports', bundle.outputFiles[0].text)(
    createRequire(import.meta.url),
    loaded,
    loaded.exports,
  );
  const plugin = loaded.exports.skillsPlugin({ kind: 'workspace', workspaceId: 'default' });
  const activate = () => {
    const slots = new Map(),
      cleanup = [];
    plugin.apply({
      scope: () => ({
        slots: { register: (id, entry) => slots.set(id, entry.component) },
        contextMenus: { register: () => {} },
      }),
      effect: (fn) => cleanup.push(fn),
    });
    const settings = slots.get('settings.pages')({ placement: 'body' });
    return { controller: settings.props.controller, cleanup };
  };
  const first = activate(),
    second = activate();
  assert.notStrictEqual(first.controller, second.controller);
  await first.controller.refresh();
  const snapshot = first.controller.catalog().getSnapshot();
  for (const cleanup of first.cleanup) cleanup();
  await first.controller.refresh();
  assert.equal(first.controller.catalog().getSnapshot().catalog, null);
  assert.equal(snapshot.catalog.items.length, 0);
  await second.controller.refresh();
  assert.deepEqual(second.controller.catalog().getSnapshot().catalog.items, []);
  for (const cleanup of second.cleanup) cleanup();
});

test('disposal fences pending writes and prevents new requests; re-enable starts fresh', async () => {
  const request = deferred();
  let calls = 0,
    notifications = 0;
  const controller = new SkillsController(() => {
    calls++;
    return request.promise;
  });
  const store = controller.catalog();
  store.subscribe(() => notifications++);
  const pending = controller.refresh();
  controller.dispose();
  const snapshot = store.getSnapshot(),
    count = notifications;
  request.resolve(catalog('late'));
  await pending;
  await controller.refresh();
  controller.ensure();
  assert.strictEqual(store.getSnapshot(), snapshot);
  assert.equal(notifications, count);
  assert.equal(calls, 1);
  const enabled = new SkillsController(async () => catalog('fresh'));
  await enabled.refresh();
  assert.equal(enabled.catalog().getSnapshot().catalog.items[0].name, 'fresh');
  enabled.dispose();
});
