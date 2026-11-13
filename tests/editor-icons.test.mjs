import test from 'node:test';
import assert from 'node:assert/strict';
import { createIconRegistry } from '../apps/web/src/plugins/editor/iconRegistry.ts';

test('icon packs resolve by entry and release their registration', () => {
  const icons = createIconRegistry();
  let updates = 0;
  const unsubscribe = icons.subscribe(() => updates++);
  const entry = { path: 'src/main.rs', directory: false };
  assert.equal(icons.resolve(entry), undefined);
  const pack = icons.register((item) => (item.path.endsWith('.rs') ? 'rust' : undefined));
  assert.equal(icons.resolve(entry), 'rust');
  assert.equal(icons.resolve({ path: 'src', directory: true }), undefined);
  pack.dispose();
  pack.dispose();
  assert.equal(icons.resolve(entry), undefined);
  assert.equal(updates, 2);
  unsubscribe();
});
test('resolver fallback preserves order and accepts an intentionally hidden icon', () => {
  const icons = createIconRegistry();
  icons.register(() => undefined);
  const hidden = icons.register(() => null);
  icons.register((entry) => (entry.expanded ? 'open' : 'closed'));
  assert.equal(icons.resolve({ path: 'src', directory: true, expanded: true }), null);
  hidden.dispose();
  assert.equal(icons.resolve({ path: 'src', directory: true, expanded: true }), 'open');
});
