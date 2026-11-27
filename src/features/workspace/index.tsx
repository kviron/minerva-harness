import { WorkspaceControls } from "./WorkspaceControls";
import { executeAction, executeActionOrThrow, registerActions } from "@/platform/actions";
import { lazyComponent } from "@/ui";
import {
  editorService,
  maybeEditorService,
  registerEditorKind,
  useWorkbenchChrome,
  useContributions,
} from "@/workbench/contributions";
import { requestToolWindowFocus } from "@/workbench/toolwindows/store";

const START = { kind: "workspace.start", key: "workspace.start", data: {} };
const NEW_TAB = "workspace.newTab";
const TOOL_TAB = "workspace.tool";
const toolTabKey = (id: string) => `${TOOL_TAB}.${id}`;
const StartPage = lazyComponent(() => import("./WorkspaceStart").then((m) => m.WorkspaceStart));

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
  const openTool = async (id: string, focus = true) => {
    if (!useContributions.getState().toolWindows.has(id)) return;
    reveal();
    const key = toolTabKey(id);
    const editor = editorService();
    await editor.openInput({ kind: TOOL_TAB, key, data: { tool: id } }, { focus: false });
    if (focus && editor.activeInput?.()?.key === key) requestToolWindowFocus(id);
  };
  const disposers = [
    () => useWorkbenchChrome.setState({ headerActions: null, closeWorkspace: undefined, toolWindowHost: undefined }),
    registerEditorKind({
      kind: TOOL_TAB,
      title: (input) =>
        useContributions.getState().toolWindows.get((input.data as { tool: string }).tool)?.title ?? "Tool unavailable",
      icon: (input) =>
        useContributions.getState().toolWindows.get((input.data as { tool: string }).tool)?.icon ?? "plugin",
      keepAlive: true,
      singleton: true,
      component: lazyComponent(() => import("./ToolWindowEditor").then((m) => m.ToolWindowEditor)),
    }),
    registerEditorKind({
      kind: START.kind,
      title: () => "Workspace",
      icon: () => "split",
      restorable: true,
      component: StartPage,
    }),
    registerEditorKind({
      kind: NEW_TAB,
      title: () => "New Tab",
      icon: () => "plus",
      replaceOnOpen: true,
      component: StartPage,
    }),
    registerEditorKind({
      kind: "workspace.browser",
      title: () => "Browser",
      icon: () => "web",
      keepAlive: true,
      component: lazyComponent(() => import("./BrowserEditor").then((m) => m.BrowserEditor)),
    }),
    registerActions([
      {
        id: NEW_TAB,
        title: "New Tab",
        category: "Window",
        when: "projectOpen",
        hidden: true,
        run: async ({ arg }) => {
          if (!useWorkbenchChrome.getState().primary) return;
          const editor = editorService();
          reveal();
          await editor.openInput(
            { kind: NEW_TAB, key: crypto.randomUUID(), data: {} },
            { pane: typeof arg === "string" ? arg : undefined, index: editor.openTabs?.().length, focus: true },
          );
        },
      },
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
          await executeActionOrThrow("terminal.openAt", {
            cwd: chrome.primary ? chrome.workspaceCwd : undefined,
            inEditor: true,
          });
        },
      },
      {
        id: TOOL_TAB,
        title: "More tools",
        category: "Window",
        when: "projectOpen",
        hidden: true,
        run: async ({ arg }) => {
          if (typeof arg === "string") await openTool(arg);
          else if (arg && typeof arg === "object" && "id" in arg && typeof arg.id === "string")
            await openTool(arg.id, !("focus" in arg) || arg.focus !== false);
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
      useWorkbenchChrome.setState({
        activeWorkspaceTool: input?.kind === TOOL_TAB ? (input.data as { tool: string }).tool : null,
      });
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
    maybeEditorService()?.onInputOpened?.((input) => {
      if (useWorkbenchChrome.getState().primary && input.kind !== "agent-home" && input.kind !== "agent-session")
        reveal();
    }) ?? (() => {}),
  ];
  const host = {
    show: (id: string, focus: boolean) => void executeAction(TOOL_TAB, { id, focus }),
    hide: (id: string) => void maybeEditorService()?.closeInput?.(toolTabKey(id)),
  };
  const syncHost = () =>
    useWorkbenchChrome.setState({
      toolWindowHost: useWorkbenchChrome.getState().primary ? host : undefined,
    });
  syncHost();
  disposers.push(
    useWorkbenchChrome.subscribe((s, prev) => {
      if (s.primary !== prev.primary) syncHost();
    }),
  );
  return () => disposers.reverse().forEach((dispose) => dispose());
}
