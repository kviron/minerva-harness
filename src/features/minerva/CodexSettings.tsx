import { useEffect } from "react";
import { Button, Checkbox, T } from "@/ui";
import { tx } from "@/platform/i18n";
import type { MinervaProvider } from "@/platform/minerva";
import { useCodexSettings } from "./useCodexSettings";
import { ProviderModels } from "./ProviderModels";
import styles from "./Settings.module.css";

const states = {
  "signed-out": "Not signed in",
  "signing-in": "Waiting for browser sign-in…",
  "signed-in": "Signed in",
  "signing-out": "Signing out…",
  error: "Sign-in failed",
} as const;

export function CodexSettings({
  models: configuredModels,
  onSaved,
  onDirty,
  onBusy,
}: {
  models: string[];
  onSaved?: (providers: MinervaProvider[]) => void;
  onDirty?: (dirty: boolean) => void;
  onBusy?: (busy: boolean) => void;
}) {
  const account = useCodexSettings(configuredModels, onSaved);
  const dirty = JSON.stringify(account.models) !== JSON.stringify(configuredModels);
  useEffect(() => onDirty?.(dirty), [dirty, onDirty]);
  const { models, setModels, busy, status } = account;
  useEffect(() => onBusy?.(busy), [busy, onBusy]);
  const move = (from: number, to: number) => {
    if (from === to || to < 0 || to >= models.length) return;
    const next = [...models];
    next.splice(to, 0, next.splice(from, 1)[0]!);
    setModels(next);
  };
  return (
    <section className={styles.body} aria-label={tx("Codex subscription")}>
      <h3>
        <T>Codex subscription</T>
      </h3>
      <p>
        <T>Use Codex models in Minerva with your ChatGPT account. The conversation and tools stay in Minerva.</T>
      </p>
      <div className={styles.row}>
        <span role="status">
          <T>{states[status.state]}</T>
        </span>
        {status.state === "signed-in" || status.state === "signing-in" ? (
          <Button disabled={busy} onClick={() => void account.signOut()}>
            <T>Sign out</T>
          </Button>
        ) : (
          <Button disabled={busy} onClick={() => void account.signIn()}>
            <T>Sign in with ChatGPT</T>
          </Button>
        )}
        {status.state === "signing-in" && status.authorizationUrl && (
          <Button disabled={busy} onClick={() => void account.reopen()}>
            <T>Open sign-in page</T>
          </Button>
        )}
      </div>
      {account.error && <p role="alert">{account.error}</p>}
      <div className={styles.row}>
        <span>
          <T>Models</T>
        </span>
        <Button disabled={busy || status.state !== "signed-in"} onClick={() => void account.fetchModels()}>
          <T>Fetch model list</T>
        </Button>
      </div>
      <p>
        <T>Available models depend on your ChatGPT account.</T>
      </p>
      {account.catalog.length > 0 && (
        <div className={styles.models} aria-label={tx("Available Codex models")}>
          {account.catalog.map((model) => (
            <Checkbox
              key={model}
              label={model}
              checked={models.includes(model)}
              disabled={busy}
              onChange={(checked) => setModels(checked ? [...models, model] : models.filter((id) => id !== model))}
            />
          ))}
        </div>
      )}
      <ProviderModels
        label="Selected Codex models"
        models={models}
        busy={busy}
        onMove={move}
        onRemove={(index) => setModels(models.filter((_, i) => i !== index))}
      />

      <p>
        <T>Save an empty model list to disable this provider.</T>
      </p>
      <div className={styles.row}>
        <Button disabled={busy} onClick={() => void account.save()}>
          <T>Save</T>
        </Button>
        {account.saved && (
          <span role="status">
            <T>Codex settings saved</T>
          </span>
        )}
      </div>
    </section>
  );
}
