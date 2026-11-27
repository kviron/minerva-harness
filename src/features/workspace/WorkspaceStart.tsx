import { executeAction } from "@/platform/actions";
import { Button, T, showContextMenu } from "@/ui";
import { editorService, useWorkspaceItems, useContributions, type EditorInput } from "@/workbench/contributions";
import styles from "./Workspace.module.css";

export function WorkspaceStart({ input }: { input: EditorInput }) {
  const tools = useContributions((s) => s.toolWindows);
  const items = useWorkspaceItems((s) => s.items);
  const newTab = input.kind === "workspace.newTab";
  const shortcuts = newTab ? ["todo", "git"] : [];
  return (
    <section className={styles.start} aria-label="Workspace">
      <h2>
        <T>{newTab ? "New Tab" : "Workspace"}</T>
      </h2>
      <div className={styles.tiles}>
        <Button icon="folder" autoFocus={newTab} onClick={() => void executeAction("workspace.files")}>
          <T>Files</T>
        </Button>
        <Button icon="terminal" onClick={() => void executeAction("workspace.terminal")}>
          <T>New terminal</T>
        </Button>
        {shortcuts.map((id) => {
          const tool = tools.get(id);
          return (
            tool && (
              <Button key={id} icon={tool.icon} onClick={() => void executeAction("workspace.tool", id)}>
                {tool.verbatim ? tool.title : <T>{tool.title}</T>}
              </Button>
            )
          );
        })}
        <Button icon="web" onClick={() => void executeAction("workspace.browser")}>
          <T>Browser</T>
        </Button>
        <Button icon="plus" onClick={() => void executeAction("file.newUntitled")}>
          <T>New File</T>
        </Button>
        <Button
          icon="more"
          onClick={(event) => {
            const rect = event.currentTarget.getBoundingClientRect();
            showContextMenu(
              { x: rect.left, y: rect.bottom },
              [...tools.values()]
                .filter((tool) => !["agents", "project", "terminal", ...shortcuts].includes(tool.id))
                .map((tool) => ({
                  label: tool.title,
                  icon: tool.icon,
                  verbatim: tool.verbatim,
                  run: () => void executeAction("workspace.tool", tool.id),
                })),
            );
          }}
        >
          <T>More tools</T>
        </Button>
        {[...items.values()].map((item) => (
          <Button
            key={item.id}
            icon={item.icon}
            onClick={() => void editorService().openInput(item.input, { focus: true })}
          >
            {item.verbatim ? item.title : <T>{item.title}</T>}
          </Button>
        ))}
      </div>
      <Button variant="ghost" icon="search" onClick={() => void executeAction("navigate.gotoFile")}>
        <T>Go to File</T>
      </Button>
    </section>
  );
}
