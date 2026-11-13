import test from 'node:test';
import assert from 'node:assert/strict';
import { workspaceForSession } from '../apps/web/src/plugins/editor/workspaceContext.ts';

const projects = [
  { id: 'a', sessionIds: ['chat-a'] },
  { id: 'b', sessionIds: ['chat-b'] },
];
test('editor follows the chat workspace without falling back to an unrelated project', () => {
  assert.equal(workspaceForSession(projects, 'chat-a'), projects[0]);
  assert.equal(workspaceForSession(projects, 'chat-b'), projects[1]);
  assert.equal(workspaceForSession(projects, 'unattached'), undefined);
  assert.equal(workspaceForSession([], 'chat-a'), undefined);
});
test('explicit project selection overrides the chat; stale selection falls back to its own workspace', () => {
  assert.equal(workspaceForSession(projects, 'chat-a', 'b'), projects[1]);
  assert.equal(workspaceForSession(projects, 'chat-a', 'removed'), projects[0]);
  assert.equal(workspaceForSession(projects, 'unattached', 'removed'), undefined);
});
