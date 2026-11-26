import { useTx } from "@/platform/i18n";
import { isImeKey } from "@/platform/ime";
import { T } from "@/ui";
import type { AgentQueuedFollowup } from "@/bindings";
import { useRef, useState } from "react";
import { sendKeyAction } from "../Composer";
import { SubmitButton } from "../SubmitButton";
import { useAgentSettings } from "../agentSettings";
import { sendDelegationMessage, sendDelegationMessageNow } from "../delegationSubmission";
import styles from "../Session.module.css";
import { SubagentFollowupQueue } from "./SubagentFollowupQueue";

export function SubagentComposer({
  task,
  subagent,
  working,
  deliveryRunId,
  queuedFollowups,
}: {
  task: string;
  subagent: string;
  working: boolean;
  deliveryRunId?: string | null;
  queuedFollowups: readonly AgentQueuedFollowup[];
}) {
  const tx = useTx();
  const { sendWith } = useAgentSettings();
  const [text, setText] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [status, setStatus] = useState<
    "Follow-up sent" | "Follow-up queued" | "Follow-up requested for next model step" | null
  >(null);
  const [error, setError] = useState<string | null>(null);
  const pending = useRef(false);
  const submit = async () => {
    const prompt = text.trim();
    if (!prompt || pending.current) return;
    pending.current = true;
    setSubmitting(true);
    setError(null);
    setStatus(null);
    try {
      const receipt = await sendDelegationMessage(task, subagent, prompt);
      setText("");
      setStatus(receipt.queued ? "Follow-up queued" : "Follow-up sent");
    } catch (error) {
      setError(String(error));
    } finally {
      pending.current = false;
      setSubmitting(false);
    }
  };
  const sendNow = async (workItem: string) => {
    if (!deliveryRunId || !working || pending.current) return;
    pending.current = true;
    setSubmitting(true);
    setError(null);
    try {
      const preferred = await sendDelegationMessageNow(task, subagent, workItem, deliveryRunId);
      if (preferred) setStatus("Follow-up requested for next model step");
      else setError(tx("The follow-up is no longer available for this step"));
    } catch (error) {
      setError(String(error));
    } finally {
      pending.current = false;
      setSubmitting(false);
    }
  };
  const label = tx("Message the subagent");
  return (
    <form
      className={styles.composerArea}
      aria-label={label}
      onSubmit={(event) => {
        event.preventDefault();
        void submit();
      }}
    >
      <SubagentFollowupQueue
        items={queuedFollowups}
        disabled={submitting}
        onSendNow={working && deliveryRunId ? (id) => void sendNow(id) : undefined}
      />
      <div className={styles.subagentComposer}>
        <textarea
          className={styles.subagentComposerInput}
          rows={2}
          aria-label={label}
          placeholder={label}
          value={text}
          disabled={submitting}
          onChange={(event) => {
            setText(event.target.value);
            setError(null);
          }}
          onKeyDown={(event) => {
            if (!isImeKey(event.nativeEvent) && sendKeyAction(event, sendWith, "queue")) {
              event.preventDefault();
              void submit();
            }
          }}
        />
        <SubmitButton
          label={working ? "Queue" : "Send"}
          pending={submitting}
          disabled={submitting || !text.trim()}
          onClick={() => void submit()}
        />
      </div>
      {error && (
        <span className={styles.error} role="alert">
          {error}
        </span>
      )}
      {status && (
        <span className={styles.subagentComposerStatus} role="status">
          <T>{status}</T>
        </span>
      )}
    </form>
  );
}
