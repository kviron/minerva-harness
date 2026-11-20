import { IconButton } from "@/ui";
import { useChromeIconSize } from "@/ui/density";
import { useWorkbenchChrome } from "@/workbench/contributions";

export function WorkspaceControls() {
  const open = useWorkbenchChrome((s) => !!s.workspaceOpen);
  const maximized = useWorkbenchChrome((s) => !!s.workspaceMaximized);
  const size = useChromeIconSize();
  return (
    <>
      {open && (
        <IconButton
          icon={maximized ? "minimize" : "maximize"}
          iconSize={size}
          title={maximized ? "Restore workspace" : "Maximize workspace"}
          action="workspace.maximize"
          pressed={maximized}
        />
      )}
      <IconButton icon="split" iconSize={size} title="Toggle workspace" action="workspace.toggle" pressed={open} />
    </>
  );
}
