import { beforeEach, expect, it, vi } from "vitest";

const fixture = vi.hoisted(() => ({
  tasks: new Map(),
  openHome: vi.fn(),
  placement: vi.fn(),
  show: vi.fn(),
  hide: vi.fn(),
  close: vi.fn<() => Promise<boolean>>(),
  reset: vi.fn(),
}));
vi.mock("./store", () => ({
  openHome: fixture.openHome,
  openTask: vi.fn(),
  setSessionLocation: fixture.placement,
  useAgents: Object.assign(vi.fn(), { getState: () => ({ tasks: fixture.tasks }) }),
}));
vi.mock("@/workbench/contributions", () => ({
  toolWindowService: () => ({ show: fixture.show, hide: fixture.hide }),
  useWorkbenchChrome: { getState: () => ({ closeWorkspace: fixture.close }), setState: fixture.reset },
}));
import { setWorkspaceMode } from "./WorkspaceMode";

beforeEach(() => {
  vi.clearAllMocks();
  fixture.close.mockResolvedValue(true);
});

it("opens a new chat when switching from an empty IDE, and restores the IDE panels", async () => {
  await setWorkspaceMode("chat");
  expect(fixture.placement).toHaveBeenCalledWith("editor");
  expect(fixture.openHome).toHaveBeenCalledOnce();
  expect(fixture.hide).not.toHaveBeenCalled();
  await setWorkspaceMode("development");
  expect(fixture.placement).toHaveBeenLastCalledWith("toolWindow");
  expect(fixture.show).toHaveBeenCalledWith("project", false);
  expect(fixture.show).toHaveBeenCalledWith("agents", false);
});

it("keeps Agent mode when a workspace view cancels closing", async () => {
  fixture.close.mockResolvedValue(false);
  await setWorkspaceMode("development");
  expect(fixture.placement).not.toHaveBeenCalled();
  expect(fixture.show).not.toHaveBeenCalled();
});

it("does not apply an old Development transition after the user returns to Agent", async () => {
  let finish!: (value: boolean) => void;
  fixture.close.mockReturnValueOnce(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  const leaving = setWorkspaceMode("development");
  await setWorkspaceMode("chat");
  finish(true);
  await leaving;
  expect(fixture.placement).toHaveBeenCalledTimes(1);
  expect(fixture.placement).toHaveBeenLastCalledWith("editor");
});
