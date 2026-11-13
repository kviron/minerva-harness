import test from 'node:test';
import assert from 'node:assert/strict';
import { DocumentPool } from '../apps/web/src/plugins/editor/documentPool.ts';

test('simultaneous views share one document and closing one keeps the model alive', async () => {
  const pool = new DocumentPool();
  let loads = 0;
  let disposed = 0;
  const load = async () => {
    loads++;
    return { text: 'original' };
  };
  const [center, right] = await Promise.all([
    pool.acquire('file', load),
    pool.acquire('file', load),
  ]);
  assert.equal(loads, 1);
  assert.equal(center, right);
  center.text = 'unsaved';
  assert.equal(right.text, 'unsaved');
  await pool.release('file', async () => {
    disposed++;
  });
  assert.equal(disposed, 0);
  await pool.release('file', async () => {
    disposed++;
  });
  assert.equal(disposed, 1);
  await pool.release('file', async () => {
    disposed++;
  });
  assert.equal(disposed, 1);
});
test('reopening during disposal waits for old LSP/model teardown', async () => {
  const pool = new DocumentPool();
  const order = [];
  let finish;
  await pool.acquire('file', async () => ({}));
  const closing = pool.release('file', async () => {
    order.push('closing');
    await new Promise((resolve) => {
      finish = resolve;
    });
    order.push('closed');
  });
  await Promise.resolve();
  const opening = pool.acquire('file', async () => {
    order.push('loaded');
    return {};
  });
  assert.deepEqual(order, ['closing']);
  finish();
  await closing;
  await opening;
  assert.deepEqual(order, ['closing', 'closed', 'loaded']);
  await pool.release('file', async () => {});
});
test('failed loads can be retried and are shared by concurrent callers', async () => {
  const pool = new DocumentPool();
  let loads = 0;
  const fail = async () => {
    loads++;
    throw new Error('read failed');
  };
  const failures = await Promise.allSettled([
    pool.acquire('file', fail),
    pool.acquire('file', fail),
  ]);
  assert.equal(loads, 1);
  assert.ok(failures.every((result) => result.status === 'rejected'));
  assert.equal(await pool.acquire('file', async () => 42), 42);
  await pool.release('file', async () => {});
});
