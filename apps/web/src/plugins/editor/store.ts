/// <reference types="vite/client" />
import * as monaco from 'monaco-editor';
import EditorWorker from 'monaco-editor/editor/editor.worker.js?worker';
import JsonWorker from 'monaco-editor/language/json/json.worker.js?worker';
import CssWorker from 'monaco-editor/language/css/css.worker.js?worker';
import HtmlWorker from 'monaco-editor/language/html/html.worker.js?worker';
import TsWorker from 'monaco-editor/language/typescript/ts.worker.js?worker';
import { invoke } from '../../client';
import { DocumentPool } from './documentPool';
import { editorEnabled, editorReady, ownEditor } from './lifecycle';
(globalThis as typeof globalThis & { MonacoEnvironment?: unknown }).MonacoEnvironment = {
  getWorker: (_id: string, label: string) =>
    label === 'json'
      ? new JsonWorker()
      : ['css', 'scss', 'less'].includes(label)
        ? new CssWorker()
        : ['html', 'handlebars', 'razor'].includes(label)
          ? new HtmlWorker()
          : ['typescript', 'javascript'].includes(label)
            ? new TsWorker()
            : new EditorWorker(),
};
// sessionId owns the file (its workspace answers read/save/LSP); view is the editor whose tabs show it.
export type Doc = {
  key: string;
  path: string;
  sessionId: string;
  model: monaco.editor.ITextModel;
  diskVersion: string;
  saved: string;
  lspVersion: number;
  lsp: boolean;
  pending: Promise<void>;
  error: string;
  preview?: { url: string; mime: string; size: number };
  readOnlyReason?: string;
};
export type State = {
  docs: Doc[];
  active: string;
  error: string;
  loading: string;
  pin: string;
  preview: string;
  positions: Map<string, monaco.editor.ICodeEditorViewState>;
  reveal?: { key: string; line: number; column: number };
};
export function textOf(doc: Doc) {
  return doc.model.getValue(monaco.editor.EndOfLinePreference.TextDefined, true);
}
const states = new Map<string, State>();
const listeners = new Set<() => void>();
let tick = 0;
const beforeUnload = (event: BeforeUnloadEvent) => {
  if ([...states.values()].some((s) => s.docs.some((doc) => textOf(doc) !== doc.saved))) {
    event.preventDefault();
    event.returnValue = '';
  }
};
let generation = 0;
let started = false;
// Disable keeps dirty text in memory and restores its tabs; it is not recovery after page reload.
const recovery = new Map<
  string,
  {
    text: string;
    saved: string;
    diskVersion: string;
    sessionId: string;
    path: string;
    views: string[];
  }
>();
async function ready() {
  await editorReady();
  if (!editorEnabled()) return false;
  if (!started) {
    started = true;
    documents = new DocumentPool<Doc>();
    window.addEventListener('beforeunload', beforeUnload);
  }
  return true;
}
export function subscribe(fn: () => void) {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}
export function snapshot() {
  return tick;
}
export function state(id: string): State {
  let s = states.get(id);
  if (!s) {
    s = {
      docs: [],
      active: '',
      error: '',
      loading: '',
      pin: '',
      preview: '',
      positions: new Map(),
    };
    states.set(id, s);
  }
  return s;
}
export function changed() {
  tick++;
  for (const l of listeners) l();
}
const languages: Record<string, string> = {
  rs: 'rust',
  ts: 'typescript',
  tsx: 'typescript',
  js: 'javascript',
  jsx: 'javascript',
  json: 'json',
  py: 'python',
  md: 'markdown',
  html: 'html',
  css: 'css',
  c: 'c',
  cpp: 'cpp',
  h: 'cpp',
  toml: 'ini',
  yml: 'yaml',
  yaml: 'yaml',
  ps1: 'powershell',
  sql: 'sql',
};
const docsByUri = new Map<string, Doc>();
// Blob URLs work for both <img> and the webview's native PDF viewer, unlike data: URLs.
function blobPreview(base64: string, mime: string) {
  const bytes = Uint8Array.from(atob(base64), (c) => c.charCodeAt(0));
  return { url: URL.createObjectURL(new Blob([bytes], { type: mime })), mime, size: bytes.length };
}
/** Single click opens a transient preview tab that the next preview replaces; pin keeps the tab (VS Code style). */
export const docKey = (sessionId: string, path: string) => `${sessionId}\n${path}`;
let documents = new DocumentPool<Doc>();
let focusedView = '';
export function focusView(view: string) {
  focusedView = view;
}
async function loadDocument(sessionId: string, path: string): Promise<Doc> {
  const key = docKey(sessionId, path);
  const epoch = generation;
  const result = await invoke('mh-editor.read', { sessionId, path });
  if (!editorEnabled() || epoch !== generation) throw new Error('Editor disabled');
  const uri = monaco.Uri.parse(
    `inmemory://minerva/${encodeURIComponent(sessionId)}/${path.split('/').map(encodeURIComponent).join('/')}`,
  );
  const extension = /[^/]+\.([^./]+)$/.exec(path)?.[1]?.toLowerCase() ?? '';
  const language =
    !result.readOnlyReason && Object.hasOwn(languages, extension)
      ? languages[extension]
      : 'plaintext';
  const model = monaco.editor.createModel(result.text, language, uri);
  // Previews (images, PDF) keep an empty model so tab, close and dirty tracking stay uniform.
  const doc: Doc = {
    key,
    path,
    sessionId,
    model,
    diskVersion: result.version,
    saved: model.getValue(monaco.editor.EndOfLinePreference.TextDefined, true),
    readOnlyReason: result.readOnlyReason,
    lspVersion: 1,
    lsp: false,
    pending: Promise.resolve(),
    error: '',
    preview: result.data
      ? blobPreview(result.data, result.mimeType ?? 'application/octet-stream')
      : undefined,
  };
  docsByUri.set(uri.toString(), doc);
  const recovered = recovery.get(key);
  if (recovered) {
    model.setValue(recovered.text);
    doc.saved = recovered.saved;
    doc.diskVersion = recovered.diskVersion;
    doc.error = 'Recovered unsaved edits from before editor disable (memory only).';
  }
  model.onDidChangeContent(() => {
    if (!editorEnabled() || epoch !== generation) return;
    for (const view of states.values()) if (view.preview === key) view.preview = '';
    const text = model.getValue();
    if (doc.lsp) {
      doc.lspVersion++;
      const version = doc.lspVersion;
      doc.pending = doc.pending
        .then(async () => {
          if (editorEnabled() && epoch === generation)
            await invoke('mh-lsp.change', { sessionId, path, text, version });
        })
        .catch((e) => {
          doc.error = String(e);
          changed();
        });
    }
    changed();
  });
  if (doc.preview || doc.readOnlyReason) return doc;
  const initialText = model.getValue();
  doc.pending = (async () => {
    try {
      await invoke('mh-lsp.open', { sessionId, path, version: 1, text: initialText });
      doc.lsp = true;
      if (editorEnabled() && epoch === generation && model.getValue() !== initialText) {
        doc.lspVersion++;
        await invoke('mh-lsp.change', {
          sessionId,
          path,
          version: doc.lspVersion,
          text: model.getValue(),
        });
      }
    } catch (e) {
      if ((e instanceof Error ? e.message : String(e)) !== 'No language server for file')
        doc.error = `LSP: ${String(e)}`;
    }
    changed();
  })();
  return doc;
}
export async function open(view: string, path: string, pin = false, sessionId = view) {
  const epoch = generation;
  if (!(await ready()) || epoch !== generation) return;
  const s = state(view);
  const key = docKey(sessionId, path);
  const existing = s.docs.find((d) => d.key === key);
  if (existing) {
    if (pin && s.preview === key) s.preview = '';
    s.active = key;
    changed();
    return;
  }
  if (s.loading) {
    if (pin && s.loading === key) s.pin = key;
    return;
  }
  s.loading = key;
  s.pin = pin ? key : '';
  s.error = '';
  changed();
  try {
    const doc = await documents.acquire(key, () => loadDocument(sessionId, path));
    if (!editorEnabled() || epoch !== generation) return;
    const transient = s.pin !== key && textOf(doc) === doc.saved;
    const replaced = transient
      ? s.docs.find((d) => d.key === s.preview && textOf(d) === d.saved)
      : undefined;
    s.docs.splice(replaced ? s.docs.indexOf(replaced) + 1 : s.docs.length, 0, doc);
    s.active = key;
    recovery.delete(key);
    if (transient) s.preview = key;
    changed();
    if (replaced) void close(replaced, false, view);
  } catch (e) {
    if (epoch === generation) s.error = String(e);
  } finally {
    if (epoch === generation) {
      s.loading = '';
      changed();
    }
  }
}
/** Opens a file and asks the editor to put the cursor on `line`/`column` (search results). */
export async function openAt(
  view: string,
  path: string,
  sessionId: string,
  line: number,
  column: number,
) {
  const epoch = generation;
  await open(view, path, false, sessionId);
  if (!editorEnabled() || epoch !== generation) return;
  state(view).reveal = { key: docKey(sessionId, path), line, column };
  changed();
}
const saves = new WeakMap<Doc, Promise<void>>();
export function save(doc: Doc): Promise<void> {
  const epoch = generation;
  const saving = (saves.get(doc) ?? Promise.resolve()).then(async () => {
    if (
      !editorEnabled() ||
      epoch !== generation ||
      doc.model.isDisposed() ||
      doc.preview ||
      doc.readOnlyReason
    )
      return;
    const text = textOf(doc);
    // Monaco normalizes line endings on open. Keep the original bytes until the user edits.
    if (text === doc.saved) return;
    try {
      const result = await invoke('mh-editor.save', {
        sessionId: doc.sessionId,
        path: doc.path,
        text,
        version: doc.diskVersion,
      });
      if (epoch !== generation || !editorEnabled()) {
        const recovered = recovery.get(doc.key);
        if (recovered) {
          recovered.saved = text;
          recovered.diskVersion = result.version;
        }
        return;
      }
      doc.diskVersion = result.version;
      doc.saved = text;
      doc.error = '';
    } catch (e) {
      if (epoch === generation && editorEnabled()) doc.error = String(e);
    }
    if (epoch === generation && editorEnabled()) changed();
  });
  saves.set(doc, saving);
  return saving;
}
export async function close(doc: Doc, discard: boolean, view: string) {
  if (!editorEnabled() || doc.model.isDisposed()) return;
  if (textOf(doc) !== doc.saved && !discard) return;
  const s = state(view);
  if (!s.docs.includes(doc)) return;
  s.docs = s.docs.filter((d) => d !== doc);
  if (s.active === doc.key) s.active = s.docs.at(-1)?.key ?? '';
  if (s.preview === doc.key) s.preview = '';
  changed();
  await documents.release(doc.key, disposeDocument);
}
async function query(
  model: monaco.editor.ITextModel,
  pos: monaco.Position,
  operation: 'hover' | 'completion' | 'definition' | 'references',
) {
  const doc = docsByUri.get(model.uri.toString());
  if (!editorEnabled() || !doc?.lsp) return null;
  const epoch = generation;
  await doc.pending;
  if (!editorEnabled() || epoch !== generation) return null;
  const result = await invoke('mh-lsp.query', {
    sessionId: doc.sessionId,
    path: doc.path,
    operation,
    line: pos.lineNumber - 1,
    character: pos.column - 1,
  });
  return editorEnabled() && epoch === generation ? result : null;
}
const completionKinds = [
  monaco.languages.CompletionItemKind.Text,
  monaco.languages.CompletionItemKind.Text,
  monaco.languages.CompletionItemKind.Method,
  monaco.languages.CompletionItemKind.Function,
  monaco.languages.CompletionItemKind.Constructor,
  monaco.languages.CompletionItemKind.Field,
  monaco.languages.CompletionItemKind.Variable,
  monaco.languages.CompletionItemKind.Class,
  monaco.languages.CompletionItemKind.Interface,
  monaco.languages.CompletionItemKind.Module,
  monaco.languages.CompletionItemKind.Property,
  monaco.languages.CompletionItemKind.Unit,
  monaco.languages.CompletionItemKind.Value,
  monaco.languages.CompletionItemKind.Enum,
  monaco.languages.CompletionItemKind.Keyword,
  monaco.languages.CompletionItemKind.Snippet,
  monaco.languages.CompletionItemKind.Color,
  monaco.languages.CompletionItemKind.File,
  monaco.languages.CompletionItemKind.Reference,
  monaco.languages.CompletionItemKind.Folder,
  monaco.languages.CompletionItemKind.EnumMember,
  monaco.languages.CompletionItemKind.Constant,
  monaco.languages.CompletionItemKind.Struct,
  monaco.languages.CompletionItemKind.Event,
  monaco.languages.CompletionItemKind.Operator,
  monaco.languages.CompletionItemKind.TypeParameter,
];
const registered = new Set<string>();
const providers: monaco.IDisposable[] = [];
export function registerLanguage(language: string) {
  if (!editorEnabled() || registered.has(language)) return;
  registered.add(language);
  providers.push(
    monaco.languages.registerHoverProvider(language, {
      async provideHover(model, pos) {
        try {
          const value = (await query(model, pos, 'hover')) as any;
          if (!value?.contents) return null;
          const values = Array.isArray(value.contents) ? value.contents : [value.contents];
          return {
            contents: values.map((x: any) => ({
              value: typeof x === 'string' ? x : (x.value ?? ''),
            })),
          };
        } catch {
          return null;
        }
      },
    }),
  );
  providers.push(
    monaco.languages.registerDefinitionProvider(language, {
      async provideDefinition(model, pos) {
        try {
          const source = docsByUri.get(model.uri.toString());
          if (!source) return null;
          const view = states.get(focusedView)?.docs.includes(source)
            ? focusedView
            : [...states].find(([, s]) => s.docs.includes(source))?.[0];
          if (!view) return null;
          const result = (await query(model, pos, 'definition')) as any;
          const locations = Array.isArray(result) ? result : result ? [result] : [];
          const workspaces = await invoke('workspaces.list');
          const root = workspaces.find((w) => w.sessionIds.includes(source.sessionId))?.path;
          if (!root || !editorEnabled() || source.model.isDisposed()) return null;
          const base = root
            .replaceAll('\\', '/')
            .replace(/^\/\/\?\//, '')
            .replace(/^UNC\//, '//')
            .replace(/\/$/, '');
          const windows = /^[A-Za-z]:\//.test(base);
          for (const item of locations) {
            const url = new URL(item.uri ?? item.targetUri);
            if (url.protocol !== 'file:') continue;
            let absolute = decodeURIComponent(url.pathname);
            if (windows && absolute.startsWith('/')) absolute = absolute.slice(1);
            const prefix = base + '/';
            if (
              !(windows
                ? absolute.toLowerCase().startsWith(prefix.toLowerCase())
                : absolute.startsWith(prefix))
            )
              continue;
            const path = absolute.slice(prefix.length);
            await open(view, path, false, source.sessionId);
            const target = state(view).docs.find((d) => d.key === docKey(source.sessionId, path));
            const range = item.range ?? item.targetSelectionRange ?? item.targetRange;
            if (target && range)
              return {
                uri: target.model.uri,
                range: new monaco.Range(
                  range.start.line + 1,
                  range.start.character + 1,
                  range.end.line + 1,
                  range.end.character + 1,
                ),
              };
          }
          return null;
        } catch {
          return null;
        }
      },
    }),
  );
  providers.push(
    monaco.languages.registerCompletionItemProvider(language, {
      async provideCompletionItems(model, pos) {
        try {
          const value = (await query(model, pos, 'completion')) as any;
          const items = Array.isArray(value) ? value : (value?.items ?? []);
          const word = model.getWordUntilPosition(pos);
          const range = new monaco.Range(
            pos.lineNumber,
            word.startColumn,
            pos.lineNumber,
            word.endColumn,
          );
          return {
            suggestions: items
              .filter((item: any) => !item.additionalTextEdits?.length)
              .map((item: any) => {
                const edit = item.textEdit;
                const span = edit?.range ?? edit?.replace;
                const replacement = span
                  ? new monaco.Range(
                      span.start.line + 1,
                      span.start.character + 1,
                      span.end.line + 1,
                      span.end.character + 1,
                    )
                  : range;
                return {
                  label: item.label,
                  kind: completionKinds[item.kind ?? 1] ?? monaco.languages.CompletionItemKind.Text,
                  insertText: edit?.newText ?? item.insertText ?? item.label,
                  insertTextRules:
                    item.insertTextFormat === 2
                      ? monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet
                      : undefined,
                  range: replacement,
                  detail: item.detail,
                };
              }),
          };
        } catch {
          return { suggestions: [] };
        }
      },
    }),
  );
}
async function disposeDocument(document: Doc) {
  await saves.get(document);
  await document.pending;
  try {
    if (document.lsp)
      await invoke('mh-lsp.close', { sessionId: document.sessionId, path: document.path }).catch(
        () => {},
      );
  } finally {
    docsByUri.delete(document.model.uri.toString());
    document.model.dispose();
    if (document.preview) URL.revokeObjectURL(document.preview.url);
  }
}
ownEditor(() => {
  generation++;
  started = false;
  window.removeEventListener('beforeunload', beforeUnload);
  for (const [view, s] of states)
    for (const doc of s.docs)
      if (!doc.model.isDisposed() && textOf(doc) !== doc.saved) {
        const prior = recovery.get(doc.key);
        recovery.set(doc.key, {
          text: textOf(doc),
          saved: doc.saved,
          diskVersion: doc.diskVersion,
          sessionId: doc.sessionId,
          path: doc.path,
          views: [...new Set([...(prior?.views ?? []), view])],
        });
      }
  for (const provider of providers.splice(0)) provider.dispose();
  registered.clear();
  focusedView = '';
  states.clear();
  changed();
  return documents.dispose(disposeDocument);
});
export async function restoreRecovery() {
  if (!(await ready())) return;
  for (const item of [...recovery.values()])
    for (const view of item.views) await open(view, item.path, true, item.sessionId);
}
export { monaco };
