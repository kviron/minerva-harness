import type { AgentQueuedFollowup } from "@/bindings";
import { T, Button } from "@/ui";
import styles from "../Session.module.css";

const labels = {
  queued: "Queued",
  requested: "Follow-up requested for next model step",
  unknown: "Delivery outcome unknown",
} as const;

export function SubagentFollowupQueue({
  items,
  disabled,
  onSendNow,
}: {
  items: readonly AgentQueuedFollowup[];
  disabled?: boolean;
  onSendNow?: (id: string) => void;
}) {
  if (items.length === 0) return null;
  return (
    <ul className={styles.subagentFollowupQueue}>
      {items.map((item) => (
        <li key={item.id} className={styles.queued}>
          <span className={styles.queuedText} title={item.prompt}>
            {item.prompt}
          </span>
          {item.state === "queued" && onSendNow ? (
            <Button type="button" size="small" disabled={disabled} onClick={() => onSendNow(item.id)}>
              <T>Send now</T>
            </Button>
          ) : (
            <span className={styles.subagentComposerStatus}>
              <T>{labels[item.state]}</T>
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}
