import { useRef, useState } from "react";
import type { AgentBackgroundTask } from "@/bindings";
import { useTx } from "@/platform/i18n";
import { Icon, IconButton, Popup, T } from "@/ui";
import { BackgroundJobRow, isActiveBackground } from "./BackgroundJobRow";
import styles from "./BackgroundTasks.module.css";

const HIDDEN_FINISHED_LIMIT = 256;

export function BackgroundTasks({
  tasks,
  canStop,
  onStop,
}: {
  tasks: readonly AgentBackgroundTask[];
  canStop: boolean;
  onStop: (id: string) => void;
}) {
  const tx = useTx();
  const anchor = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const [finishedOpen, setFinishedOpen] = useState(false);
  const [hidden, setHidden] = useState<ReadonlySet<string>>(() => new Set());
  const active = tasks.filter(isActiveBackground);
  const finished = tasks.filter((task) => !isActiveBackground(task) && !hidden.has(task.id));
  const count = active.length;
  if (open && !active.length && !finished.length) setOpen(false);
  if (!active.length && !finished.length) return null;
  const row = (task: AgentBackgroundTask) => (
    <BackgroundJobRow key={task.id} task={task} canStop={canStop} onStop={onStop} />
  );
  return (
    <div className={styles.header}>
      <button
        type="button"
        ref={anchor}
        className={styles.trigger}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <Icon name="terminal" size={12} />
        {tx`Background tasks: ${count}`}
      </button>
      <Popup
        open={open}
        onClose={() => setOpen(false)}
        anchor={anchor}
        placement="bottom-end"
        trapFocus
        aria-label={tx("Background task output")}
        className={styles.popup}
      >
        <div className={styles.panelHeading}>
          <span>
            <T>Running</T> · {active.length}
          </span>
          <IconButton icon="close" title="Close" size="small" onClick={() => setOpen(false)} />
        </div>
        <div role="group" aria-label={tx("Running in the background")}>
          {active.map(row)}
        </div>
        {finished.length > 0 && (
          <>
            <div className={styles.finishedHeading}>
              <button
                type="button"
                className={styles.fold}
                aria-expanded={finishedOpen}
                onClick={() => setFinishedOpen(!finishedOpen)}
              >
                <Icon name={finishedOpen ? "chevron-down" : "chevron-right"} size={12} />
                <T>Finished</T> {finished.length}
              </button>
              <button
                type="button"
                className={styles.clear}
                onClick={() =>
                  setHidden(
                    (old) => new Set([...old, ...finished.map((task) => task.id)].slice(-HIDDEN_FINISHED_LIMIT)),
                  )
                }
              >
                <T>Clear</T>
              </button>
            </div>
            {finishedOpen && finished.map(row)}
          </>
        )}
      </Popup>
    </div>
  );
}
