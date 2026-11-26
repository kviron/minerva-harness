import type { AgentEntry } from "@/bindings";
import { useTx } from "@/platform/i18n";
import { Button, Icon, Popup, T } from "@/ui";
import { useRef, useState } from "react";
import { subagentStatusLabel, subagentTaskTitle, subagentTitle, subagentWorking, type SubagentEntry } from "../model";
import { SubagentStatusIcon } from "./toolEntries";
import header from "./BackgroundTasks.module.css";
import styles from "./SubagentRoster.module.css";

export function SubagentRoster({
  entries,
  onOpen,
  onStopTree,
}: {
  entries: readonly AgentEntry[] | undefined;
  onOpen: (id: string) => void;
  onStopTree?: () => void;
}) {
  const tx = useTx();
  const anchor = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const cards = (entries ?? []).filter((entry): entry is SubagentEntry => entry.type === "subagent");
  if (!cards.length) return null;
  const sorted = cards.toSorted((a, b) => Number(subagentWorking(b.status)) - Number(subagentWorking(a.status)));
  return (
    <>
      <button
        type="button"
        ref={anchor}
        className={header.trigger}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <Icon name="toolwindow-agents" size={12} />
        {tx`Subagents: ${cards.length}`}
      </button>
      <Popup
        open={open}
        onClose={() => setOpen(false)}
        anchor={anchor}
        placement="bottom-start"
        aria-label={tx("Subagents")}
        trapFocus
        className={styles.popup}
      >
        {sorted.map((card) => {
          const agent = subagentTitle(card);
          const title = subagentTaskTitle(card);
          return (
            <button
              key={card.id}
              type="button"
              className={styles.row}
              onClick={() => {
                setOpen(false);
                onOpen(card.id);
              }}
            >
              <SubagentStatusIcon status={card.status} />
              <span className={styles.labels}>
                <span className={styles.title} title={title}>
                  {title}
                </span>
                <span className={styles.detail}>
                  {agent !== title && `${agent} · `}
                  {tx(subagentStatusLabel(card.status))}
                </span>
              </span>
              <Icon name="chevron-right" size={12} />
            </button>
          );
        })}
        {onStopTree && sorted.some((card) => subagentWorking(card.status)) && (
          <div className={styles.actions}>
            <Button size="small" title={tx("Stop all delegated subagents")} onClick={onStopTree}>
              <T>Stop tree</T>
            </Button>
          </div>
        )}
      </Popup>
    </>
  );
}
