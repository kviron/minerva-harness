import { EmptyState } from "@/ui";
import { useContributions, useEditorShown, useWorkbenchChrome, type EditorInput } from "../contributions";
import { ToolWindowView } from "./ToolWindows";

export function ToolWindowEditor({ input }: { input: EditorInput }) {
  const id = (input.data as { tool: string }).tool;
  const tool = useContributions((s) => s.toolWindows.get(id));
  const editorShown = useEditorShown();
  const workspaceShown = useWorkbenchChrome((s) => !s.primary || !!s.workspaceOpen);
  const shown = editorShown && workspaceShown;
  return tool ? <ToolWindowView tw={tool} hidden={!shown} inEditor /> : <EmptyState title="Tool unavailable" />;
}
