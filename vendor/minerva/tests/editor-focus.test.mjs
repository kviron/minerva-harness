import test from 'node:test';
import assert from 'node:assert/strict';
import { ownsEditorShortcut } from '../apps/web/src/plugins/editor/focus.ts';

test('only the visible editor containing the target owns a shortcut', () => {
  const previous = globalThis.Node;
  class FakeNode {}
  globalThis.Node = FakeNode;
  try {
    const inside = new FakeNode();
    const outside = new FakeNode();
    const root = { getClientRects: () => [{}], contains: (target) => target === inside };
    assert.equal(ownsEditorShortcut(root, inside), true);
    assert.equal(ownsEditorShortcut(root, outside), false);
    assert.equal(ownsEditorShortcut(null, inside), false);
    assert.equal(ownsEditorShortcut(root, null), false);
    assert.equal(ownsEditorShortcut({ ...root, getClientRects: () => [] }, inside), false);
  } finally {
    if (previous === undefined) delete globalThis.Node;
    else globalThis.Node = previous;
  }
});
