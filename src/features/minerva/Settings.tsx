import { useState, useCallback, useRef } from "react";
import { Button, T, Tabs } from "@/ui";
import { tx } from "@/platform/i18n";
import { confirm } from "@/workbench/messages";
import { ApiProviderEditor } from "./ApiProviderEditor";
import { AddProviderDialog } from "./AddProviderDialog";
import { CodexSettings } from "./CodexSettings";
import { ProviderList } from "./ProviderList";
import { useMinervaUsage } from "@/platform/minervaUsage";
import { hasUsage, SubscriptionUsageView } from "@/workbench/SubscriptionUsage";
import { useProviderConnections } from "./useProviderConnections";
import styles from "./Settings.module.css";
const EMPTY_MODELS: string[] = [];
export function MinervaSettings() {
  const connections = useProviderConnections();
  const [selected, setSelected] = useState("openai-codex");
  const [query, setQuery] = useState("");
  const [tab, setTab] = useState("settings");
  const [adding, setAdding] = useState(false);
  const dirty = useRef(false);
  const [editorBusy, setEditorBusy] = useState(false);
  const onDirty = useCallback((value: boolean) => {
    dirty.current = value;
  }, []);
  const busy = editorBusy || connections.loading || connections.removing;
  const leave = async () =>
    !busy &&
    (!dirty.current ||
      (await confirm({
        title: tx("Discard changes?"),
        message: tx("Your unsaved provider changes will be discarded."),
        ok: tx("Discard"),
      })));
  const select = async (id: string) => {
    if (id === selected || !(await leave())) return;
    dirty.current = false;
    setSelected(id);
  };
  const provider = connections.providers.find((p) => p.id === selected) ?? null;
  const usage = useMinervaUsage();
  const observations = usage.providers.filter((p) => p.providerId === selected && hasUsage(p));
  const showLimits = observations.length > 0;
  const activeTab = showLimits ? tab : "settings";
  const remove = async () => {
    if (
      busy ||
      !provider ||
      !(await confirm({ title: tx("Remove provider?"), message: provider.name, ok: tx("Remove") }))
    )
      return;
    if (!(await connections.remove(selected))) return;
    dirty.current = false;
    setSelected("openai-codex");
  };
  return (
    <section className={styles.split} aria-label={tx("Minerva")}>
      <ProviderList
        disabled={busy}
        providers={connections.providers}
        selected={selected}
        query={query}
        onQuery={setQuery}
        onSelect={(id) => void select(id)}
        onAdd={() =>
          void leave().then((ok) => {
            if (ok) setAdding(true);
          })
        }
      />
      <div className={styles.details}>
        <div className={styles.detailHeading}>
          <h3>{selected === "openai-codex" ? "Codex" : provider?.name}</h3>
          {provider && selected !== "openai-codex" && (
            <Button size="small" disabled={busy} onClick={() => void remove()}>
              <T>Remove</T>
            </Button>
          )}
        </div>
        <Tabs
          items={[
            { id: "settings", label: tx("Settings") },
            ...(showLimits ? [{ id: "limits", label: tx("Provider limits") }] : []),
          ]}
          activeId={activeTab}
          onSelect={setTab}
        />
        {connections.error && <p role="alert">{connections.error}</p>}
        <div hidden={activeTab !== "settings"} className={styles.detailBody} key={selected}>
          {!connections.loading &&
            (selected === "openai-codex" ? (
              <CodexSettings
                models={provider?.models ?? EMPTY_MODELS}
                onSaved={connections.setProviders}
                onDirty={onDirty}
                onBusy={setEditorBusy}
              />
            ) : (
              provider && (
                <ApiProviderEditor
                  provider={provider}
                  known={connections.known}
                  onSaved={(rows) => {
                    connections.setProviders(rows);
                    dirty.current = false;
                  }}
                  onCancel={() => {
                    dirty.current = false;
                  }}
                  onDirty={onDirty}
                  onBusy={setEditorBusy}
                />
              )
            ))}
        </div>
        {activeTab === "limits" && (
          <div className={styles.detailBody}>
            <SubscriptionUsageView {...usage} providers={observations} onRefresh={() => void usage.refresh()} />
          </div>
        )}
      </div>
      {adding && (
        <AddProviderDialog
          known={connections.known}
          onClose={() => setAdding(false)}
          onSaved={(rows) => {
            const created = rows.find((p) => !connections.providers.some((old) => old.id === p.id));
            connections.setProviders(rows);
            setAdding(false);
            dirty.current = false;
            if (created) setSelected(created.id);
            setTab("settings");
          }}
        />
      )}
    </section>
  );
}
