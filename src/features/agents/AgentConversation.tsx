import { useEffect, useState } from "react";
import { useProject } from "@/platform/project";
import { T } from "@/ui";
import { useWorkbenchChrome } from "@/workbench/contributions";
import { HomeEditor } from "./HomeEditor";
import { SessionEditor } from "./SessionEditor";
import { HOME_KIND, HOME_KEY, SESSION_KIND, sessionKey } from "./model";
import { useAgents } from "./store";
import styles from "./AgentConversation.module.css";

/** Conversation ownership is independent of the files and tools open beside it. */
export function AgentConversation() {
  const root = useProject((s) => s.project?.root);
  const cwd = useAgents((s) => (!s.homeOpen && s.lastTask ? s.tasks.get(s.lastTask)?.cwd : null));
  useEffect(() => {
    useWorkbenchChrome.setState({ workspaceCwd: cwd ?? root });
  }, [cwd, root]);
  const home = useAgents((s) => s.homeOpen);
  const task = useAgents((s) => s.lastTask);
  const title = useAgents((s) => (s.lastTask ? s.tasks.get(s.lastTask)?.title : null));
  const [headerTools, setHeaderTools] = useState<HTMLDivElement | null>(null);
  return (
    <section className={styles.conversation} aria-label="Agent conversation">
      <header className={styles.header}>
        <strong>{!home && title ? title : <T>New chat</T>}</strong>
        {!home && task && <div className={styles.tools} ref={setHeaderTools} />}
      </header>
      {home || !task ? (
        <HomeEditor input={{ kind: HOME_KIND, key: HOME_KEY, data: {} }} />
      ) : (
        <SessionEditor
          docked
          headerTools={headerTools}
          input={{ kind: SESSION_KIND, key: sessionKey(task), data: { task } }}
        />
      )}
    </section>
  );
}
