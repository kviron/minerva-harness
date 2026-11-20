import { WorkspaceControls } from "./WorkspaceControls";
import { registerActions } from "@/platform/actions";
import { IconButton, lazyComponent } from "@/ui";
import {
  editorService,
  maybeEditorService,
  registerEditorKind,
  registerTitleItem,
  useWorkbenchChrome,
  useContributions,
} from "@/workbench/contributions";
import { executeActionOrThrow } from "@/platform/actions";

const START = { kind: "workspace.start", key: "workspace.start", data: {} };
function WorkspaceButton() {
  const primary = useWorkbenchChrome((s) => s.primary);
  const open = useWorkbenchChrome((s) => s.workspaceOpen);
  return primary && open ? <IconButton icon="plus" title="Open workspace" action="workspace.start" /> : null;
}

export function activate() {
  async function closeWorkspace() {
    const editor = maybeEditorService();
    const keys = new Set(
      (editor?.openTabs?.() ?? [])
        .filter(
          ({ input }) =>
            input.kind.startsWith("workspace.") ||
            input.kind.startsWith("minerva.shell.canvas.tabs.") ||
            input.kind.startsWith("minerva.workspace.panels."),
        )
        .map(({ input }) => input.key),
    );
    for (const key of keys) if ((await editor?.closeInput?.(key)) === false) return false;
    useWorkbenchChrome.setState({ workspaceOpen: false, workspaceMaximized: false });
    return true;
  }
  useWorkbenchChrome.setState({ headerActions: WorkspaceControls, closeWorkspace });
  const reveal = () => useWorkbenchChrome.setState({ workspaceOpen: true });
  const openTool = async (id: string) => {
    if (!useContributions.getState().toolWindows.has(id)) return;
    reveal();
    await editorService().openInput(
      { kind: "workspace.tool", key: `workspace.tool.${id}`, data: { tool: id } },
      { focus: true },
    );
  };
  const disposers = [
    () => useWorkbenchChrome.setState({ headerActions: null, closeWorkspace: undefined }),
    registerEditorKind({
      kind: "workspace.tool",
      title: (input) =>
        useContributions.getState().toolWindows.get((input.data as { tool: string }).tool)?.title ?? "Tool unavailable",
      icon: (input) =>
        useContributions.getState().toolWindows.get((input.data as { tool: string }).tool)?.icon ?? "plugin",
      keepAlive: true,
      component: lazyComponent(() =>
        import("@/workbench/toolwindows/ToolWindowEditor").then((m) => m.ToolWindowEditor),
      ),
    }),
    registerEditorKind({
      kind: START.kind,
      title: () => "Workspace",
      icon: () => "split",
      restorable: true,
      component: lazyComponent(() => import("./WorkspaceStart").then((m) => m.WorkspaceStart)),
    }),
    registerEditorKind({
      kind: "workspace.browser",
      title: () => "Browser",
      icon: () => "web",
      keepAlive: true,
      component: lazyComponent(() => import("./BrowserEditor").then((m) => m.BrowserEditor)),
    }),
    registerTitleItem({ id: "workspace.toggle", align: "right", priority: 5, component: WorkspaceButton }),
    registerActions([
      {
        id: "workspace.maximize",
        title: "Maximize workspace",
        category: "Window",
        when: "projectOpen",
        toggled: () => !!useWorkbenchChrome.getState().workspaceMaximized,
        run: () => {
          const state = useWorkbenchChrome.getState();
          if (state.workspaceOpen) useWorkbenchChrome.setState({ workspaceMaximized: !state.workspaceMaximized });
        },
      },
      {
        id: "workspace.toggle",
        title: "Toggle workspace",
        category: "Window",
        when: "projectOpen",
        run: async () => {
          const open = !useWorkbenchChrome.getState().workspaceOpen;
          useWorkbenchChrome.setState({ workspaceOpen: open, workspaceMaximized: false });
          if (open && !(editorService().openTabs?.() ?? []).some((t) => !t.input.kind.startsWith("agent")))
            await editorService().openInput(START);
        },
      },
      {
        id: "workspace.start",
        title: "Open workspace",
        category: "Window",
        when: "projectOpen",
        run: async () => {
          reveal();
          await editorService().openInput(START, { focus: true });
        },
      },
      {
        id: "workspace.files",
        title: "Files",
        category: "Window",
        when: "projectOpen",
        run: () => openTool("project"),
      },
      {
        id: "workspace.terminal",
        title: "New terminal",
        category: "Window",
        when: "projectOpen",
        run: async () => {
          reveal();
          const chrome = useWorkbenchChrome.getState();
          await executeActionOrThrow("terminal.openAt", chrome.primary ? chrome.workspaceCwd : undefined);
          await executeActionOrThrow("terminal.moveToEditor");
        },
      },
      {
        id: "workspace.tool",
        title: "More tools",
        category: "Window",
        when: "projectOpen",
        hidden: true,
        run: async ({ arg }) => {
          if (typeof arg === "string") await openTool(arg);
        },
      },
      {
        id: "workspace.browser",
        title: "Browser",
        category: "Window",
        when: "projectOpen",
        run: async () => {
          reveal();
          await editorService().openInput({ kind: "workspace.browser", key: crypto.randomUUID(), data: {} });
        },
      },
    ]),
    maybeEditorService()?.onActiveInputChange?.((input) => {
      if (!useWorkbenchChrome.getState().primary) return;
      if (!input) {
        if (!(editorService().openTabs?.() ?? []).length)
          useWorkbenchChrome.setState({ workspaceOpen: false, workspaceMaximized: false });
        return;
      }
      if (input.kind === "agent-home" || input.kind === "agent-session") {
        void editorService().closeInput?.(input.key);
      } else useWorkbenchChrome.setState({ workspaceOpen: true });
    }) ?? (() => {}),
  ];
  return () => disposers.reverse().forEach((dispose) => dispose());
}
