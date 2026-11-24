import { useCallback, useEffect, useState } from "react";
import { Button, Icon, IconButton, Input, Select, T } from "@/ui";
import { tx } from "@/platform/i18n";
import { minervaCall, type MinervaKnownProvider, type MinervaProvider } from "@/platform/minerva";
import styles from "./Settings.module.css";

const empty = { id: "", name: "", baseUrl: "", models: [] as string[], apiKey: "", protocol: "openai-chat" };

export function MinervaSettings() {
  const [providers, setProviders] = useState<MinervaProvider[]>([]);
  const [known, setKnown] = useState<MinervaKnownProvider[]>([]);
  const [draft, setDraft] = useState(empty);
  const [formMode, setFormMode] = useState<"add" | "edit" | null>(null);
  const editing = formMode === "edit";
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [newModel, setNewModel] = useState("");
  const [dragged, setDragged] = useState<number | null>(null);
  const [fetching, setFetching] = useState(false);
  const load = useCallback(async () => {
    const [models, presets] = await Promise.all([
      minervaCall<MinervaProvider[]>("providers.list"),
      minervaCall<MinervaKnownProvider[]>("providers.known"),
    ]);
    return { models, presets };
  }, []);
  useEffect(() => {
    let active = true;
    void load()
      .then(({ models, presets }) => {
        if (active) {
          setProviders(models);
          setKnown(presets);
        }
      })
      .catch((e) => {
        if (active) setError(String(e));
      });
    return () => {
      active = false;
    };
  }, [load]);
  const action = async (run: () => Promise<void>) => {
    setBusy(true);
    setError("");
    try {
      await run();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const closeForm = () => {
    setFormMode(null);
    setDraft(empty);
    setNewModel("");
    setError("");
  };
  const input = { ...draft, apiKey: draft.apiKey || null, isNew: !editing };
  const moveModel = (from: number, to: number) => {
    if (from === to || to < 0 || to >= draft.models.length) return;
    const models = [...draft.models];
    models.splice(to, 0, models.splice(from, 1)[0]!);
    setDraft({ ...draft, models });
  };
  const form = (
    <form
      className={styles.form}
      onSubmit={(event) => {
        event.preventDefault();
        void action(async () => {
          setProviders(await minervaCall("providers.save", input));
          closeForm();
        });
      }}
    >
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
              setDraft({
                ...draft,
                id: preset.id,
                name: preset.name,
                baseUrl: preset.baseUrl,
                protocol: preset.protocol,
                models: preset.models,
              });
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
          onChange={(e) => setDraft({ ...draft, id: e.target.value })}
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
          onChange={(e) => setDraft({ ...draft, name: e.target.value })}
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
          onChange={(e) => setDraft({ ...draft, baseUrl: e.target.value })}
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
          onChange={(protocol) => setDraft({ ...draft, protocol })}
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
          onChange={(e) => setDraft({ ...draft, apiKey: e.target.value })}
        />
      </label>
      <div className={styles.row}>
        <span>
          <T>Models</T>
        </span>
        <Button
          disabled={busy || !draft.id.trim() || !draft.baseUrl.trim()}
          onClick={() =>
            void action(async () => {
              setFetching(true);
              try {
                const models = await minervaCall<string[]>("providers.discover", input);
                setDraft({ ...draft, models: [...new Set(models)] });
              } finally {
                setFetching(false);
              }
            })
          }
        >
          {fetching ? <T>Loading models…</T> : <T>Fetch model list</T>}
        </Button>
      </div>
      {error && <p role="alert">{error}</p>}
      <ul className={styles.models} aria-label={tx("Models")}>
        {draft.models.map((model, index) => (
          <li
            key={model}
            className={styles.modelRow}
            data-reordering={dragged !== null && dragged !== index}
            onDragOver={(event) => {
              if (!busy && dragged !== null) event.preventDefault();
            }}
            onDrop={(event) => {
              event.preventDefault();
              if (!busy && dragged !== null) moveModel(dragged, index);
              setDragged(null);
            }}
          >
            <span
              draggable={!busy}
              className={styles.dragHandle}
              onDragStart={(event) => {
                event.dataTransfer.setData("text/plain", model);
                event.dataTransfer.effectAllowed = "move";
                setDragged(index);
              }}
              onDragEnd={() => setDragged(null)}
            >
              <Icon name="main-menu" size={14} />
            </span>
            <span className={styles.modelId}>{model}</span>
            <IconButton
              icon="chevron-up"
              title="Move model up"
              size="small"
              disabled={busy || index === 0}
              onClick={() => moveModel(index, index - 1)}
            />
            <IconButton
              icon="chevron-down"
              title="Move model down"
              size="small"
              disabled={busy || index === draft.models.length - 1}
              onClick={() => moveModel(index, index + 1)}
            />
            <IconButton
              icon="trash"
              title="Remove model"
              size="small"
              disabled={busy}
              onClick={() => setDraft({ ...draft, models: draft.models.filter((_, i) => i !== index) })}
            />
          </li>
        ))}
      </ul>
      <div className={styles.row}>
        <Input
          aria-label={tx("Model ID")}
          placeholder={tx("Model ID")}
          disabled={busy}
          value={newModel}
          onChange={(event) => setNewModel(event.target.value)}
        />
        <Button
          disabled={busy || !newModel.trim() || draft.models.includes(newModel.trim())}
          onClick={() => {
            setDraft({ ...draft, models: [...draft.models, newModel.trim()] });
            setNewModel("");
          }}
        >
          <T>Add model</T>
        </Button>
      </div>
      <div className={styles.row}>
        <Button type="submit" disabled={busy}>
          <T>Save</T>
        </Button>
        <Button disabled={busy} onClick={closeForm}>
          <T>Cancel</T>
        </Button>
      </div>
    </form>
  );
  return (
    <section className={styles.body} aria-label="Model Providers">
      <h3>
        <T>API models</T>
      </h3>
      <p>
        <T>Add a provider here, then select it and one of its models in the agent.</T>
      </p>
      <div className={styles.providers}>
        {!formMode && error && <p role="alert">{error}</p>}
        {providers.map((provider) => (
          <div className={styles.providerCard} key={provider.id}>
            <div className={styles.providerRow}>
              <span className={styles.providerName} title={provider.name}>
                {provider.name}
              </span>
              <span className={styles.modelCount}>{tx`Models (${provider.models.length})`}</span>
              <Button
                disabled={busy}
                aria-expanded={editing && draft.id === provider.id}
                onClick={() => {
                  if (editing && draft.id === provider.id) {
                    closeForm();
                    return;
                  }
                  setFormMode("edit");
                  setError("");
                  setNewModel("");
                  setDraft({
                    id: provider.id,
                    name: provider.name,
                    baseUrl: provider.baseUrl,
                    protocol: provider.protocol,
                    models: [...new Set(provider.models)],
                    apiKey: "",
                  });
                }}
              >
                <T>Edit</T>
              </Button>
              <Button
                disabled={busy}
                onClick={() =>
                  void action(async () => {
                    setProviders(await minervaCall("providers.delete", { id: provider.id }));
                    if (editing && draft.id === provider.id) closeForm();
                  })
                }
              >
                <T>Remove</T>
              </Button>
            </div>
            {editing && draft.id === provider.id && form}
          </div>
        ))}
        <Button
          icon="plus"
          aria-expanded={formMode === "add"}
          className={styles.addProvider}
          disabled={busy}
          onClick={() => {
            closeForm();
            if (formMode !== "add") setFormMode("add");
          }}
        >
          <T>Add model provider…</T>
        </Button>
        {formMode === "add" && form}
      </div>
    </section>
  );
}
