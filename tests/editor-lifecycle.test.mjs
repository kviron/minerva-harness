import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import ts from 'typescript';
import { DocumentPool } from '../apps/web/src/plugins/editor/documentPool.ts';

async function harness() {
  const lifecycle = await import(
    `../apps/web/src/plugins/editor/lifecycle.ts?test=${Math.random()}`
  );
  const events = new Set();
  const models = [];
  const revoked = [];
  const providers = [];
  const calls = [];
  let read;
  let lspOpen;
  let save;
  const invoke = async (name, args) => {
    calls.push([name, args]);
    if (name === 'mh-editor.read') return read ? await read() : { text: 'disk', version: 'v1' };
    if (name === 'mh-lsp.open' && lspOpen) return await lspOpen();
    if (name === 'mh-editor.save' && save) return await save(args);
    return {};
  };
  const monaco = {
    editor: {
      EndOfLinePreference: { TextDefined: 0 },
      createModel(text, language, uri) {
        const model = {
          text,
          uri,
          getValue() {
            return this.text;
          },
          setValue(value) {
            this.text = value;
            this.change?.();
          },
          onDidChangeContent(fn) {
            this.change = fn;
          },
          dispose() {
            this.dead = true;
          },
          isDisposed() {
            return !!this.dead;
          },
        };
        models.push(model);
        return model;
      },
    },
    Uri: {
      parse(value) {
        return { toString: () => value };
      },
    },
    languages: { CompletionItemKind: {}, CompletionItemInsertTextRule: {} },
  };
  for (const name of [
    'registerHoverProvider',
    'registerDefinitionProvider',
    'registerCompletionItemProvider',
  ])
    monaco.languages[name] = () => {
      const provider = {
        disposed: false,
        dispose() {
          this.disposed = true;
        },
      };
      providers.push(provider);
      return provider;
    };
  let source = readFileSync(
    new URL('../apps/web/src/plugins/editor/store.ts', import.meta.url),
    'utf8',
  );
  source = source.replace(/^import .*;\r?$/gm, '').replace(/^\(globalThis .*;\r?$/gm, '');
  const js = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const exports = {};
  const window = {
    addEventListener(name, fn) {
      events.add(fn);
    },
    removeEventListener(name, fn) {
      events.delete(fn);
    },
  };
  new Function(
    'exports',
    'monaco',
    'invoke',
    'DocumentPool',
    'editorEnabled',
    'editorReady',
    'ownEditor',
    'window',
    'URL',
    js,
  )(
    exports,
    monaco,
    invoke,
    DocumentPool,
    lifecycle.editorEnabled,
    lifecycle.editorReady,
    lifecycle.ownEditor,
    window,
    { createObjectURL: () => 'blob:preview', revokeObjectURL: (url) => revoked.push(url) },
  );
  return {
    store: exports,
    lifecycle,
    events,
    models,
    calls,
    providers,
    revoked,
    setRead(fn) {
      read = fn;
    },
    setLspOpen(fn) {
      lspOpen = fn;
    },
    setSave(fn) {
      save = fn;
    },
  };
}
test('disable fences pending read and re-enable waits for cleanup', async () => {
  const h = await harness();
  const disable = h.lifecycle.enableEditor();
  let finish;
  h.setRead(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const opening = h.store.open('center', 'file');
  while (!finish) await Promise.resolve();
  disable();
  h.lifecycle.enableEditor();
  finish({ text: 'disk', version: 'v1' });
  await opening;
  await h.lifecycle.editorReady();
  assert.equal(h.models.length, 0);
  h.setRead(null);
  await h.store.open('center', 'file');
  assert.equal(h.models.length, 1);
});
test('disable releases blob previews and language providers; re-enable registers fresh providers', async () => {
  const h = await harness();
  let disable = h.lifecycle.enableEditor();
  h.setRead(async () => ({ text: '', version: 'v1', data: 'YQ==', mimeType: 'image/png' }));
  await h.store.open('center', 'image.png');
  h.store.registerLanguage('plaintext');
  assert.equal(h.providers.length, 3);
  disable();
  await h.lifecycle.editorReady();
  assert.deepEqual(h.revoked, ['blob:preview']);
  assert.ok(h.providers.every((p) => p.disposed));
  disable = h.lifecycle.enableEditor();
  await h.store.open('center', 'image.png');
  h.store.registerLanguage('plaintext');
  assert.equal(h.providers.length, 6);
  disable();
  await h.lifecycle.editorReady();
});
test('pending LSP open drains before a replacement model can load', async () => {
  const h = await harness();
  const disable = h.lifecycle.enableEditor();
  let finish;
  h.setLspOpen(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  await h.store.open('center', 'file');
  disable();
  h.lifecycle.enableEditor();
  h.setLspOpen(null);
  const reopening = h.store.open('center', 'file');
  await Promise.resolve();
  assert.equal(h.models.length, 1);
  assert.equal(h.models[0].isDisposed(), false);
  finish({});
  await reopening;
  assert.equal(h.models[0].isDisposed(), true);
  assert.equal(h.models.length, 2);
  assert.equal(h.calls.filter(([name]) => name === 'mh-lsp.close').length, 1);
});
test('pending save drains on disable and reconciles recovery without stale publication', async () => {
  const h = await harness();
  let disable = h.lifecycle.enableEditor();
  let finish;
  await h.store.open('center', 'file');
  const doc = h.store.state('center').docs[0];
  doc.model.setValue('saving');
  h.setSave(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const saving = h.store.save(doc);
  await Promise.resolve();
  doc.model.setValue('newer unsaved');
  const queued = h.store.save(doc);
  disable();
  const tick = h.store.snapshot();
  h.lifecycle.enableEditor();
  h.setRead(async () => ({ text: 'saving', version: 'v2' }));
  const restoring = h.store.restoreRecovery();
  await Promise.resolve();
  assert.equal(h.models.length, 1);
  finish({ version: 'v2' });
  await saving;
  await queued;
  assert.equal(h.store.snapshot(), tick);
  await restoring;
  const restored = h.store.state('center').docs[0];
  assert.equal(restored.model.getValue(), 'newer unsaved');
  assert.equal(restored.saved, 'saving');
  assert.equal(restored.diskVersion, 'v2');
  assert.equal(doc.saved, 'disk');
  assert.equal(h.calls.filter(([name]) => name === 'mh-editor.save').length, 1);
});
test('concurrent saves serialize version checks and save the latest edits', async () => {
  const h = await harness();
  h.lifecycle.enableEditor();
  await h.store.open('center', 'file');
  const doc = h.store.state('center').docs[0];
  let finish;
  h.setSave(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  doc.model.setValue('first');
  const first = h.store.save(doc);
  await Promise.resolve();
  doc.model.setValue('second');
  const second = h.store.save(doc);
  h.setSave(async (args) => {
    assert.equal(args.version, 'v2');
    return { version: 'v3' };
  });
  finish({ version: 'v2' });
  await first;
  await second;
  assert.equal(doc.saved, 'second');
  assert.equal(doc.diskVersion, 'v3');
});
test('shared dirty document survives view remount and disable/re-enable recovery', async () => {
  const h = await harness();
  let disable = h.lifecycle.enableEditor();
  await h.store.open('center', 'file');
  await h.store.open('right', 'file', false, 'center');
  const doc = h.store.state('center').docs[0];
  assert.equal(doc, h.store.state('right').docs[0]);
  doc.model.setValue('unsaved');
  assert.equal(h.store.state('center').docs[0].model.getValue(), 'unsaved');
  disable();
  assert.equal(h.events.size, 0);
  await h.lifecycle.editorReady();
  assert.equal(doc.model.isDisposed(), true);
  disable = h.lifecycle.enableEditor();
  await h.store.restoreRecovery();
  const restored = h.store.state('center').docs[0];
  assert.equal(restored.model.getValue(), 'unsaved');
  assert.equal(restored.saved, 'disk');
  assert.equal(restored, h.store.state('right').docs[0]);
  assert.match(restored.error, /Recovered unsaved/);
  disable();
  await h.lifecycle.editorReady();
  assert.equal(h.calls.filter(([name]) => name === 'mh-lsp.close').length, 2);
});
