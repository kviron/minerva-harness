import { useLayoutEffect, useRef, type ReactNode } from "react";
import type { GroupImperativeHandle } from "react-resizable-panels";
import { SplitGroup, SplitPanel, SplitHandle } from "@/ui";
import { EditorShownProvider, ToolWindowShownProvider, useWorkbenchChrome } from "./contributions";
import styles from "./WorkspaceLayout.module.css";

/** Keeps the native editor mounted while the conversation's workspace is folded. */
export function WorkspaceLayout({ children }: { children: ReactNode }) {
  const { primary: Primary, workspaceOpen, workspaceMaximized, headerActions: HeaderActions } = useWorkbenchChrome();
  const group = useRef<GroupImperativeHandle | null>(null);
  const expanded = !!Primary && !!workspaceOpen;
  const maximized = expanded && !!workspaceMaximized;
  useLayoutEffect(() => {
    let layout = { conversation: Primary ? 100 : 0, workspace: Primary ? 0 : 100 };
    if (maximized) layout = { conversation: 0, workspace: 100 };
    else if (expanded) {
      layout = { conversation: 40, workspace: 60 };
      try {
        const saved = JSON.parse(localStorage.getItem("minerva.workspace.split") ?? "null");
        if (saved && Number.isFinite(saved.conversation) && saved.conversation >= 20 && saved.conversation <= 80)
          layout = { conversation: saved.conversation, workspace: 100 - saved.conversation };
      } catch {
        /* storage unavailable */
      }
    }
    group.current?.setLayout(layout);
    // Panel constraints settle after this commit; reapply folding against their updated minimums.
    const frame = requestAnimationFrame(() => group.current?.setLayout(layout));
    return () => cancelAnimationFrame(frame);
  }, [Primary, expanded, maximized]);
  return (
    <div className={styles.frame} data-primary={Primary ? true : undefined} data-expanded={expanded || undefined}>
      <SplitGroup
        orientation="horizontal"
        className={styles.layout}
        groupRef={group}
        onLayoutChanged={(layout, meta) => {
          if (!expanded || maximized || !meta.isUserInteraction) return;
          try {
            localStorage.setItem("minerva.workspace.split", JSON.stringify(layout));
          } catch {
            /* storage unavailable */
          }
        }}
      >
        <SplitPanel id="conversation" minSize={expanded && !maximized ? "240px" : 0}>
          <div className={styles.primary} hidden={maximized}>
            <EditorShownProvider value={!maximized}>
              <ToolWindowShownProvider value={!maximized}>{Primary && <Primary />}</ToolWindowShownProvider>
            </EditorShownProvider>
          </div>
        </SplitPanel>
        <SplitHandle
          className={styles.divider}
          hairline={false}
          hidden={!expanded || maximized}
          disabled={!expanded || maximized}
          aria-label="Resize workspace"
        />
        <SplitPanel id="workspace" minSize={expanded ? "280px" : 0}>
          <div className={styles.content} hidden={!!Primary && !expanded}>
            {children}
          </div>
        </SplitPanel>
      </SplitGroup>
      {Primary && HeaderActions && (
        <div className={styles.controls}>
          <HeaderActions />
        </div>
      )}
    </div>
  );
}
