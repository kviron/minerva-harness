import type { MinervaProvider } from "@/platform/minerva";
import { SearchField, List, Button, Icon, T } from "@/ui";
import { tx } from "@/platform/i18n";
import styles from "./Settings.module.css";
export function ProviderList({
  providers,
  selected,
  query,
  onQuery,
  onSelect,
  onAdd,
  disabled,
}: {
  providers: MinervaProvider[];
  selected: string;
  query: string;
  onQuery: (value: string) => void;
  onSelect: (id: string) => void;
  onAdd: () => void;
  disabled: boolean;
}) {
  const rows = [{ id: "openai-codex", name: "Codex" }, ...providers.filter((p) => p.id !== "openai-codex")].filter(
    (p) => p.name.toLowerCase().includes(query.toLowerCase()),
  );
  return (
    <div className={styles.master}>
      <SearchField
        className={styles.search}
        value={query}
        onChange={onQuery}
        placeholder={tx("Search providers")}
        aria-label={tx("Search providers")}
        clearable
      />
      <List
        className={styles.connectionList}
        items={rows}
        getKey={(p) => p.id}
        activeIndex={rows.findIndex((p) => p.id === selected)}
        onActiveChange={(i) => {
          if (rows[i]) onSelect(rows[i].id);
        }}
        rowHeight={28}
        aria-label={tx("Model providers")}
        renderItem={(p) => (
          <span className={styles.connectionRow}>
            <Icon name={p.id === "openai-codex" ? "agent-codex" : "plugin"} size={18} />
            <span className={styles.providerName}>{p.name}</span>
          </span>
        )}
      />
      <div className={styles.listActions}>
        <Button size="small" disabled={disabled} onClick={onAdd}>
          <T>Add model provider…</T>
        </Button>
      </div>
    </div>
  );
}
