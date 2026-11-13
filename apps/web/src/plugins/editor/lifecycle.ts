// This module stays small: activating the plugin must not import Monaco.
let enabled = false;
let cleanup: (() => Promise<void>) | undefined;
let draining = Promise.resolve();
export function editorEnabled() {
  return enabled;
}
export function editorReady() {
  return draining;
}
export function ownEditor(dispose: () => Promise<void>) {
  cleanup = dispose;
  if (!enabled) draining = dispose();
}
export function enableEditor() {
  enabled = true;
  return () => {
    enabled = false;
    if (cleanup) {
      const prior = draining;
      const next = cleanup();
      draining = Promise.allSettled([prior, next]).then(() => {});
    }
  };
}
