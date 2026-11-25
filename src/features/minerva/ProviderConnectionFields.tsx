import { Input, Select, T } from "@/ui";
import { tx } from "@/platform/i18n";
import type { MinervaKnownProvider } from "@/platform/minerva";
import type { ProviderDraft } from "./providerDraft";
import styles from "./Settings.module.css";
export function ProviderConnectionFields({
  draft,
  onChange,
  known,
  busy,
  editing,
}: {
  draft: ProviderDraft;
  onChange: (draft: ProviderDraft) => void;
  known: MinervaKnownProvider[];
  busy: boolean;
  editing: boolean;
}) {
  return (
    <>
      <label className={styles.field}>
        <span>
          <T>Known service</T>
        </span>
        <Select
          aria-label="Known service"
          value={known.some((preset) => preset.id === draft.id) ? draft.id : ""}
          disabled={busy || editing}
          searchable
          options={[
            { value: "", label: tx("Custom endpoint") },
            ...known.map((preset) => ({ value: preset.id, label: preset.name })),
          ]}
          onChange={(id) => {
            const preset = known.find((item) => item.id === id);
            if (preset) {
              onChange({
                ...draft,
                apiKey: "",
                id: preset.id,
                name: preset.name,
                baseUrl: preset.baseUrl,
                protocol: preset.protocol,
                models: preset.models,
              });
            } else {
              onChange({ ...draft, id: "", name: "", baseUrl: "", models: [], apiKey: "", protocol: "openai-chat" });
            }
          }}
        />
      </label>
      <label className={styles.field}>
        <span>
          <T>Provider ID</T>
        </span>
        <Input
          aria-label="Provider ID"
          required
          disabled={busy || editing}
          value={draft.id}
          onChange={(e) => onChange({ ...draft, id: e.target.value })}
        />
      </label>
      <label className={styles.field}>
        <span>
          <T>Provider name</T>
        </span>
        <Input
          aria-label="Provider name"
          required
          disabled={busy}
          value={draft.name}
          onChange={(e) => onChange({ ...draft, name: e.target.value })}
        />
      </label>
      <label className={styles.field}>
        <span>
          <T>API base URL</T>
        </span>
        <Input
          aria-label="API base URL"
          required
          disabled={busy}
          value={draft.baseUrl}
          onChange={(e) => onChange({ ...draft, baseUrl: e.target.value })}
        />
      </label>
      <label className={styles.field}>
        <span>
          <T>API protocol</T>
        </span>
        <Select
          aria-label="API protocol"
          value={draft.protocol}
          disabled={busy}
          options={[
            { value: "openai-chat", label: "OpenAI Chat" },
            { value: "openai-responses", label: "OpenAI Responses" },
            { value: "anthropic-messages", label: "Anthropic Messages" },
            { value: "ollama", label: "Ollama" },
          ]}
          onChange={(protocol) => onChange({ ...draft, protocol })}
        />
      </label>
      <label className={styles.field}>
        <span>
          <T>API key</T>
        </span>
        <Input
          aria-label="API key"
          placeholder={tx(editing ? "Leave empty to keep the saved API key" : "API key")}
          type="password"
          autoComplete="new-password"
          disabled={busy}
          value={draft.apiKey}
          onChange={(e) => onChange({ ...draft, apiKey: e.target.value })}
        />
      </label>
    </>
  );
}
