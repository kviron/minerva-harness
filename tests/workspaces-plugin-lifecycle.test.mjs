import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { transform } from 'esbuild';
import { BrowserPluginHost } from '../packages/plugin-sdk/src/context.ts';
import { Store } from '../packages/client-models/src/store.ts';

const require = createRequire(import.meta.url);
test('disabling workspace UI leaves the app-owned model and other subscribers alive', async () => {
  const workspaces = new Store([{ id: 'project', sessionIds: ['chat'] }]);
  let disposed = 0;
  let notifications = 0;
  workspaces.dispose = () => {
    disposed++;
    workspaces.clear([]);
  };
  const unsubscribe = workspaces.subscribe(() => notifications++);
  const { code } = await transform(
    await readFile(new URL('../plugins/mh-workspaces/ui/index.tsx', import.meta.url), 'utf8'),
    { loader: 'tsx', format: 'cjs', jsx: 'automatic' },
  );
  const module = { exports: {} };
  const mocks = {
    './Workspaces': { Workspaces: () => null },
    './actions': { workspaceActions: new Store(null) },
    './SessionMenu': { sessionActions: new Store(null) },
    '../../../apps/web/src/client': {
      invoke: async () => {
        throw Error('Unexpected RPC');
      },
    },
    '../../../apps/web/src/app/models': { models: { workspaces } },
  };
  new Function('require', 'module', 'exports', code)(
    (name) => (Object.hasOwn(mocks, name) ? mocks[name] : require(name)),
    module,
    module.exports,
  );
  const host = new BrowserPluginHost();
  await host.activate([
    { id: 'minerva-ui', apply() {} },
    module.exports.workspacePlugin({ kind: 'workspace', workspaceId: 'default' }),
  ]);
  await host.disable('workspaces-ui');
  assert.equal(disposed, 0);
  assert.equal(workspaces.getSnapshot()[0].id, 'project');
  workspaces.set([{ id: 'updated', sessionIds: ['chat'] }]);
  assert.equal(notifications, 1);
  unsubscribe();
  await host.dispose();
});
