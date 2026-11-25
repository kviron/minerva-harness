import { useState } from "react";
import { Icon, IconButton } from "@/ui";
import { tx } from "@/platform/i18n";
import styles from "./Settings.module.css";
export function ProviderModels({
  models,
  busy,
  onMove,
  onRemove,
  label = "Models",
}: {
  label?: string;
  models: string[];
  busy: boolean;
  onMove: (from: number, to: number) => void;
  onRemove: (index: number) => void;
}) {
  const [dragged, setDragged] = useState<number | null>(null);
  return (
    <ul className={styles.models} aria-label={tx(label)}>
      {models.map((model, index) => (
        <li
          key={model}
          className={styles.modelRow}
          data-reordering={dragged !== null && dragged !== index}
          onDragOver={(event) => {
            if (!busy && dragged !== null) event.preventDefault();
          }}
          onDrop={(event) => {
            event.preventDefault();
            if (!busy && dragged !== null) onMove(dragged, index);
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
            onClick={() => onMove(index, index - 1)}
          />
          <IconButton
            icon="chevron-down"
            title="Move model down"
            size="small"
            disabled={busy || index === models.length - 1}
            onClick={() => onMove(index, index + 1)}
          />
          <IconButton icon="trash" title="Remove model" size="small" disabled={busy} onClick={() => onRemove(index)} />
        </li>
      ))}
    </ul>
  );
}
