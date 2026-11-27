import { afterEach, expect, it, vi } from "vitest";
import { executeActionOrThrow, registerActions } from "@/platform/actions";
import { setContext } from "@/platform/context";
import {
  provideEditorService,
  registerToolWindow,
  useWorkbenchChrome,
  type EditorInput,
} from "@/workbench/contributions";
import { activate } from "./index";

const cleanups: Array<() => void> = [];

it("closes Agent workspace views on exit while keeping native documents and terminal sessions", async () => {
  const closeInput = vi.fn().mockResolvedValue(true);
  const kinds = [
    "workspace.start",
    "workspace.tool",
    "workspace.browser",
    "minerva.shell.canvas.tabs.plugin.view",
    "minerva.workspace.panels.plugin.view",
    "file",
    "cli",
  ];
  provideEditorService({
    openTabs: () => kinds.map((kind) => ({ input: { kind, key: kind } })),
    closeInput,
    onActiveInputChange: () => () => {},
  } as never);
  cleanups.push(activate());
  useWorkbenchChrome.setState({ workspaceOpen: true, workspaceMaximized: true });
  expect(await useWorkbenchChrome.getState().closeWorkspace?.()).toBe(true);
  expect(closeInput.mock.calls.map(([key]) => key)).toEqual(kinds.slice(0, 5));
  expect(useWorkbenchChrome.getState()).toMatchObject({ workspaceOpen: false, workspaceMaximized: false });
  closeInput.mockClear().mockResolvedValueOnce(false);
  useWorkbenchChrome.setState({ workspaceOpen: true });
  expect(await useWorkbenchChrome.getState().closeWorkspace?.()).toBe(false);
  expect(closeInput).toHaveBeenCalledTimes(1);
  expect(useWorkbenchChrome.getState().workspaceOpen).toBe(true);
});

it("folds an empty workspace after the last tab closes, but keeps other panes and Development open", () => {
  let changed!: (input: EditorInput | null) => void;
  const openTabs = vi.fn().mockReturnValue([]);
  provideEditorService({
    openTabs,
    onActiveInputChange: (listener: typeof changed) => {
      changed = listener;
      return () => {};
    },
  } as never);
  cleanups.push(activate());
  useWorkbenchChrome.setState({ primary: () => null, workspaceOpen: true, workspaceMaximized: true });
  openTabs.mockReturnValue([{ input: { kind: "file", key: "remaining" } }]);
  changed(null);
  expect(useWorkbenchChrome.getState().workspaceOpen).toBe(true);
  openTabs.mockReturnValue([]);
  changed(null);
  expect(useWorkbenchChrome.getState().workspaceOpen).toBe(false);
  expect(useWorkbenchChrome.getState().workspaceMaximized).toBe(false);
  useWorkbenchChrome.setState({ primary: null, workspaceOpen: true });
  changed(null);
  expect(useWorkbenchChrome.getState().workspaceOpen).toBe(true);
});
afterEach(() => {
  cleanups
    .splice(0)
    .reverse()
    .forEach((dispose) => dispose());
  useWorkbenchChrome.setState({ primary: null, workspaceOpen: false, workspaceMaximized: false, workspaceCwd: null });
});

it("restores split mode when maximization is toggled or the workspace is folded", async () => {
  setContext("projectOpen", true);
  provideEditorService({ onActiveInputChange: () => () => {} } as never);
  cleanups.push(activate());
  useWorkbenchChrome.setState({ primary: () => null, workspaceOpen: true });
  await executeActionOrThrow("workspace.maximize");
  expect(useWorkbenchChrome.getState().workspaceMaximized).toBe(true);
  await executeActionOrThrow("workspace.maximize");
  expect(useWorkbenchChrome.getState().workspaceMaximized).toBe(false);
  await executeActionOrThrow("workspace.maximize");
  await executeActionOrThrow("workspace.toggle");
  expect(useWorkbenchChrome.getState()).toMatchObject({ workspaceOpen: false, workspaceMaximized: false });
});

it("opens every contributed tool in a workspace tab regardless of its dock anchor", async () => {
  setContext("projectOpen", true);
  const openInput = vi.fn().mockResolvedValue(undefined);
  provideEditorService({ openInput, onActiveInputChange: () => () => {} } as never);
  for (const [id, anchor] of [
    ["commit-fixture", "left"],
    ["graphql-fixture", "bottom"],
    ["plugin-fixture", "right"],
  ] as const)
    cleanups.push(registerToolWindow({ id, anchor, title: id, icon: "plugin", component: () => null }));
  cleanups.push(activate());
  await executeActionOrThrow("workspace.tool", "commit-fixture");
  await executeActionOrThrow("workspace.tool", "graphql-fixture");
  await executeActionOrThrow("workspace.tool", "plugin-fixture");
  expect(openInput.mock.calls.map(([input]) => input)).toEqual([
    { kind: "workspace.tool", key: "workspace.tool.commit-fixture", data: { tool: "commit-fixture" } },
    { kind: "workspace.tool", key: "workspace.tool.graphql-fixture", data: { tool: "graphql-fixture" } },
    { kind: "workspace.tool", key: "workspace.tool.plugin-fixture", data: { tool: "plugin-fixture" } },
  ]);
});

it("opens the active task's terminal in the native editor and stops if session creation fails", async () => {
  setContext("projectOpen", true);
  provideEditorService({ onActiveInputChange: () => () => {} } as never);
  const open = vi.fn();
  cleanups.push(
    registerActions([{ id: "terminal.openAt", title: "Terminal", run: ({ arg }) => open(arg) }]),
    activate(),
  );
  useWorkbenchChrome.setState({ primary: () => null, workspaceCwd: "/project/worktree" });
  await executeActionOrThrow("workspace.terminal");
  expect(open).toHaveBeenCalledWith({ cwd: "/project/worktree", inEditor: true });
  open.mockRejectedValueOnce(Error("PTY unavailable"));
  await expect(executeActionOrThrow("workspace.terminal")).rejects.toThrow("PTY unavailable");
});
