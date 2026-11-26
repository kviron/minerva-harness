import { auxiliaryWindow } from "@/platform/auxWindow";
import { useEffect, useRef, useState, type PointerEvent, type ReactNode } from "react";
import { Button, IconButton, T, lockDragCursor } from "@/ui";
import { useChromeIconSize } from "@/ui/density";
import { maybeEditorService, useWorkbenchChrome } from "@/workbench/contributions";
import { AgentConversation } from "./AgentConversation";
import { AgentsView } from "./AgentsView";
import { openHome, useAgents } from "./store";
import styles from "./AgentNavigation.module.css";

export function AgentNavigation({ children }: { children?: ReactNode }) {
  const iconSize = useChromeIconSize();
  return (
    <nav className={children ? styles.embeddedRail : styles.rail} aria-label="Agent navigation">
      <IconButton icon="home" iconSize={iconSize} title="Chats" pressed onClick={() => openHome()} />
      <IconButton icon="plugin" iconSize={iconSize} title="Plugins" action="plugins.settings" />
      {children ?? <div className={styles.spacer} />}
      <IconButton icon="settings" iconSize={iconSize} title="Settings…" action="file.settings" />
    </nav>
  );
}

export function AgentHistory() {
  const pane = useRef<HTMLElement>(null);
  const drag = useRef<(() => void) | null>(null);
  const workspaceOpen = useWorkbenchChrome((s) => s.workspaceOpen);
  const maximum = () =>
    Math.max(200, Math.min(workspaceOpen ? 320 : 600, window.innerWidth - (workspaceOpen ? 900 : 360)));
  const clamp = (value: number) => Math.round(Math.max(200, Math.min(maximum(), value)));
  const [width, setWidth] = useState(() => {
    try {
      const saved = Number(localStorage.getItem("minerva.chatHistory.width"));
      if (Number.isFinite(saved) && saved >= 200) return clamp(saved);
    } catch {
      /* storage unavailable */
    }
    return clamp(Math.min(300, window.innerWidth * 0.23));
  });
  useEffect(() => () => drag.current?.(), []);
  const commit = (value: number) => {
    const next = clamp(value);
    setWidth(next);
    try {
      localStorage.setItem("minerva.chatHistory.width", String(next));
    } catch {
      /* storage unavailable */
    }
  };
  const resize = (event: PointerEvent<HTMLDivElement>) => {
    if (event.button !== 0 || !pane.current) return;
    event.preventDefault();
    drag.current?.();
    const handle = event.currentTarget;
    handle.setPointerCapture(event.pointerId);
    const origin = event.clientX;
    const start = pane.current.getBoundingClientRect().width;
    let next = start;
    const release = lockDragCursor("col-resize");
    const move = (event: globalThis.PointerEvent) => {
      next = clamp(start + event.clientX - origin);
      if (pane.current) pane.current.style.width = `${next}px`;
    };
    const cleanup = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", end);
      handle.removeEventListener("pointercancel", end);
      handle.removeEventListener("lostpointercapture", end);
      release();
      drag.current = null;
    };
    const end = () => {
      cleanup();
      commit(next);
    };
    drag.current = cleanup;
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", end);
    handle.addEventListener("pointercancel", end);
    handle.addEventListener("lostpointercapture", end);
  };
  return (
    <aside
      ref={pane}
      className={styles.history}
      aria-label="Chat history"
      data-workspace-open={workspaceOpen || undefined}
      style={{ width: clamp(width) }}
    >
      <header className={styles.header}>
        <strong>
          <T>Chat history</T>
        </strong>
        <Button variant="ghost" icon="plus" onClick={() => openHome()}>
          <T>New chat</T>
        </Button>
      </header>
      <AgentsView historyOnly />
      <div
        className={styles.resize}
        role="separator"
        aria-label="Resize chat history"
        aria-orientation="vertical"
        aria-valuemin={200}
        aria-valuemax={maximum()}
        aria-valuenow={clamp(width)}
        tabIndex={0}
        onPointerDown={resize}
        onKeyDown={(event) => {
          if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
          event.preventDefault();
          commit((pane.current?.getBoundingClientRect().width ?? width) + (event.key === "ArrowRight" ? 8 : -8));
        }}
      />
    </aside>
  );
}

export function activateAgentNavigation() {
  if (auxiliaryWindow()) return () => {};
  const sync = () => {
    const mainAgent = useAgents.getState().sessionLocation === "editor";
    useAgents.setState({ mainAgent });
    if (mainAgent) {
      for (const { input } of maybeEditorService()?.openTabs?.() ?? []) {
        if (input.kind === "agent-home" || input.kind === "agent-session")
          void maybeEditorService()?.closeInput?.(input.key);
      }
    }
    useWorkbenchChrome.setState(
      useAgents.getState().sessionLocation === "editor"
        ? { navigation: AgentNavigation, sidebar: AgentHistory, primary: AgentConversation }
        : { navigation: null, sidebar: null, primary: null, workspaceOpen: false, workspaceMaximized: false },
    );
  };
  sync();
  const stop = useAgents.subscribe((next, previous) => {
    if (next.sessionLocation !== previous.sessionLocation) sync();
  });
  return () => {
    stop();
    useAgents.setState({ mainAgent: false });
    useWorkbenchChrome.setState({ navigation: null, sidebar: null, primary: null });
  };
}
