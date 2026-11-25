import { useLayoutEffect, useRef, useState } from "react";
import type { AgentBackgroundTask } from "@/bindings";
import { useTx } from "@/platform/i18n";
import { Icon, IconButton, T, type IconName } from "@/ui";
import { stripAnsi } from "@/ui/ansi";
import { Elapsed } from "./WorkingRow";
import styles from "./BackgroundTasks.module.css";

const ICONS: Record<string, IconName> = { local_bash: "terminal", monitor: "eye" };
export const isActiveBackground = (task: AgentBackgroundTask) =>
  !task.status || !["completed", "failed", "stopped", "killed"].includes(task.status);

export function BackgroundJobRow({
  task,
  canStop,
  onStop,
}: {
  task: AgentBackgroundTask;
  canStop: boolean;
  onStop: (id: string) => void;
}) {
  const tx = useTx();
  const [expanded, setExpanded] = useState(false);
  const output = useRef<HTMLPreElement>(null);
  const following = useRef(true);
  const active = isActiveBackground(task);
  const stopping = task.status === "stopping";
  const description = task.description || task.kind;
  useLayoutEffect(() => {
    if (expanded && output.current && following.current) output.current.scrollTop = output.current.scrollHeight;
  }, [expanded, task.output]);
  const status =
    task.status === "stopping"
      ? tx("Stopping…")
      : task.status === "failed"
        ? tx("Failed")
        : task.status === "stopped" || task.status === "killed"
          ? tx("Stopped")
          : undefined;
  return (
    <div className={styles.job} role="group" aria-label={description}>
      <div className={styles.row}>
        <Icon name={ICONS[task.kind] ?? "clock"} size={12} className={styles.icon} />
        <div className={styles.summary}>
          <span className={styles.description} title={description}>
            {description}
          </span>
          <div className={styles.details}>
            <span className={styles.kind}>{task.kind === "local_bash" ? tx("Shell") : task.kind}</span>
            {active && task.startedMs != null && <Elapsed since={task.startedMs} />}
            {status && <span className={task.status === "failed" ? styles.failure : undefined}>{status}</span>}
          </div>
        </div>
        {task.output != null && (
          <IconButton
            icon={expanded ? "chevron-up" : "chevron-down"}
            title={tx`View output: ${description}`}
            size="small"
            aria-expanded={expanded}
            onClick={() => {
              following.current = true;
              setExpanded(!expanded);
            }}
          />
        )}
        {active && canStop && (
          <IconButton
            icon="stop"
            title={stopping ? "Stopping…" : "Stop"}
            disabled={stopping}
            size="small"
            onClick={() => onStop(task.id)}
          />
        )}
      </div>
      {expanded && task.output != null && (
        <pre
          ref={output}
          className={styles.output}
          tabIndex={0}
          role="region"
          aria-label={tx("Output")}
          onScroll={() => {
            const element = output.current;
            if (element) following.current = element.scrollHeight - element.scrollTop - element.clientHeight < 24;
          }}
        >
          {stripAnsi(task.output) || <T>Waiting for output…</T>}
        </pre>
      )}
    </div>
  );
}
