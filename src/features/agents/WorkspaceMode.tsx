import { auxiliaryWindow } from "@/platform/auxWindow";
import { Select } from "@/ui";
import { executeAction } from "@/platform/actions";
import { useProject } from "@/platform/project";
import { toolWindowService, useWorkbenchChrome } from "@/workbench/contributions";
import { openHome, openTask, setSessionLocation, useAgents } from "./store";

let modeRequest = 0;

export async function setWorkspaceMode(value: "chat" | "development") {
  if (auxiliaryWindow()) return;
  const request = ++modeRequest;
  const root = useProject.getState().project?.root;
  if (value === "development") {
    if ((await useWorkbenchChrome.getState().closeWorkspace?.()) === false) return;
    if (request !== modeRequest || root !== useProject.getState().project?.root) return;
    useWorkbenchChrome.setState({ workspaceOpen: false, workspaceMaximized: false });
  }
  setSessionLocation(value === "development" ? "toolWindow" : "editor");
  const windows = toolWindowService();
  if (value === "development") {
    windows.show("project", false);
    windows.show("agents", false);
    const state = useAgents.getState();
    if (!state.docked && !state.homeOpen) {
      const task = state.lastTask && state.tasks.get(state.lastTask);
      if (task && task.status !== "archived") openTask(task.id);
      else openHome();
    }
  } else {
    const state = useAgents.getState();
    const task = state.lastTask && state.tasks.get(state.lastTask);
    if (state.homeOpen) openHome();
    else if (task && task.status !== "archived") openTask(task.id);
    else openHome();
  }
}

export function WorkspaceMode() {
  const location = useAgents((s) => s.sessionLocation);
  const project = useProject((s) => s.project);
  if (!project || auxiliaryWindow()) return null;
  return (
    <Select
      aria-label="Workspace mode"
      variant="ghost"
      value={location === "toolWindow" ? "development" : "chat"}
      options={[
        { value: "chat", label: "Agent" },
        { value: "development", label: "Development" },
      ]}
      onChange={(value) => {
        void executeAction(value === "development" ? "minerva.developmentMode" : "minerva.chatMode");
      }}
    />
  );
}
