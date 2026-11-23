import { useCallback, useEffect, useState } from "react";
import { Button, Checkbox, Input, Select, T } from "@/ui";
import { tx } from "@/platform/i18n";
import { minervaCall, type MinervaKnownProvider, type MinervaPlugin, type MinervaProvider } from "@/platform/minerva";
import styles from "./Settings.module.css";

const empty = { id: "", name: "", baseUrl: "", models: "", apiKey: "", protocol: "openai-chat" };

export function MinervaSettings() {
  const [providers, setProviders] = useState<MinervaProvider[]>([]);
  const [known, setKnown] = useState<MinervaKnownProvider[]>([]);
  const [plugins, setPlugins] = useState<MinervaPlugin[]>([]);
  const [draft, setDraft] = useState(empty);
  const [editing, setEditing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [packagePath, setPackagePath] = useState("");
  const [installed, setInstalled] = useState(false);
  const load = useCallback(async () => {
    const [models, presets, packages] = await Promise.all([
      minervaCall<MinervaProvider[]>("providers.list"),
      minervaCall<MinervaKnownProvider[]>("providers.known"),
      minervaCall<MinervaPlugin[]>("plugins.list"),
    ]);
    return { models, presets, packages };
  }, []);
  useEffect(() => {
    let active = true;
    void load()
      .then(({ models, presets, packages }) => {
        if (active) {
          setProviders(models);
          setKnown(presets);
          setPlugins(packages);
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
  return (
    <section className={styles.body} aria-label="Model Providers">
      <h3>
        <T>API models</T>
      </h3>
      <p>
        <T>Add a provider here, then select it and one of its models in the agent.</T>
      </p>
      {error && <p role="alert">{error}</p>}
      {providers.map((provider) => (
        <div className={styles.row} key={provider.id}>
          <span>
            {provider.name} · {provider.models.join(", ")}
          </span>
          <Button
            disabled={busy}
            onClick={() => {
              setEditing(true);
              setDraft({ ...provider, models: provider.models.join(", "), apiKey: "" });
            }}
          >
            <T>Edit</T>
          </Button>
          <Button
            disabled={busy}
            onClick={() =>
              void action(async () => {
                setProviders(await minervaCall("providers.delete", { id: provider.id }));
              })
            }
          >
            <T>Remove</T>
          </Button>
        </div>
      ))}
      <form
        className={styles.form}
        onSubmit={(event) => {
          event.preventDefault();
          void action(async () => {
            setProviders(
              await minervaCall("providers.save", {
                ...draft,
                models: draft.models
                  .split(",")
                  .map((m) => m.trim())
                  .filter(Boolean),
                apiKey: draft.apiKey || null,
                isNew: !editing,
              }),
            );
            setDraft(empty);
            setEditing(false);
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
                  models: preset.models.join(", "),
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
            <T>Models separated by commas</T>
          </span>
          <Input
            aria-label="Models separated by commas"
            required
            disabled={busy}
            value={draft.models}
            onChange={(e) => setDraft({ ...draft, models: e.target.value })}
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
          <Button type="submit" disabled={busy}>
            <T>Save API provider</T>
          </Button>
          {editing && (
            <Button
              disabled={busy}
              onClick={() => {
                setDraft(empty);
                setEditing(false);
              }}
            >
              <T>Cancel</T>
            </Button>
          )}
        </div>
      </form>
      <h3>
        <T>Minerva plugins</T>
      </h3>
      <p>
        <T>Minerva packages run in the embedded plugin host, separately from OpenIDEA plugins.</T>
      </p>
      <form
        className={styles.form}
        onSubmit={(event) => {
          event.preventDefault();
          void action(async () => {
            await minervaCall("plugins.install", { path: packagePath });
            setPackagePath("");
            setInstalled(true);
          });
        }}
      >
        <label className={styles.field}>
          <span>
            <T>Plugin package directory</T>
          </span>
          <Input
            aria-label="Plugin package directory"
            required
            disabled={busy}
            value={packagePath}
            onChange={(event) => setPackagePath(event.target.value)}
          />
        </label>
        <Button type="submit" disabled={busy}>
          <T>Install Minerva package</T>
        </Button>
      </form>
      {installed && (
        <p role="status">
          <T>Package installed disabled. Restart the IDE, then enable it here.</T>
        </p>
      )}
      {plugins.map((plugin) => (
        <div key={plugin.id} className={styles.plugin}>
          <Checkbox
            label={plugin.name}
            checked={plugin.enabled}
            disabled={busy || !plugin.canToggle}
            onChange={(enabled) =>
              void action(async () => {
                setPlugins(await minervaCall("plugins.setEnabled", { id: plugin.id, enabled }));
              })
            }
          />
          <p>{plugin.description}</p>
          {plugin.error && <p role="alert">{plugin.error}</p>}
        </div>
      ))}
    </section>
  );
}
