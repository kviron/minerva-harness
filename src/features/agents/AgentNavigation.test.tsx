import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { useWorkbenchChrome } from "@/workbench/contributions";
import { useAgents } from "./store";
import { AgentHistory, AgentNavigation, activateAgentNavigation } from "./AgentNavigation";
import * as actions from "@/platform/actions";

const windowState = vi.hoisted(() => ({ auxiliary: false }));
vi.mock("@/platform/auxWindow", () => ({
  auxiliaryWindow: () => (windowState.auxiliary ? { kind: "editor", parent: "main" } : null),
}));

vi.mock("./AgentsView", () => ({
  AgentsView: ({ historyOnly }: { historyOnly: boolean }) => <div>{historyOnly ? "History only" : "Conversation"}</div>,
}));

it("replaces IDE chrome only in Agent mode and restores it for Development", () => {
  useAgents.setState({ sessionLocation: "editor" });
  const stop = activateAgentNavigation();
  expect(useWorkbenchChrome.getState().navigation).toBe(AgentNavigation);
  expect(useWorkbenchChrome.getState().sidebar).toBe(AgentHistory);
  useAgents.setState({ sessionLocation: "toolWindow" });
  expect(useWorkbenchChrome.getState()).toMatchObject({ navigation: null, sidebar: null, primary: null });
  useAgents.setState({ sessionLocation: "editor" });
  expect(useWorkbenchChrome.getState().sidebar).toBe(AgentHistory);
  stop();
  expect(useWorkbenchChrome.getState().navigation).toBeNull();
});

it("opens native plugin settings and displays a separate history surface", () => {
  const execute = vi.spyOn(actions, "executeAction").mockResolvedValue(true);
  render(
    <>
      <AgentNavigation />
      <AgentHistory />
    </>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Plugins" }));
  expect(execute).toHaveBeenCalledWith("plugins.settings", undefined);
  expect(screen.getByRole("complementary", { name: "Chat history" })).toHaveTextContent("History only");
  expect(screen.getByRole("button", { name: "New chat" })).toBeVisible();
  execute.mockRestore();
});

it("resizes chat history by dragging and restores the saved width on remount", () => {
  localStorage.removeItem("minerva.chatHistory.width");
  const view = render(<AgentHistory />);
  const pane = screen.getByRole("complementary", { name: "Chat history" });
  vi.spyOn(pane, "getBoundingClientRect").mockReturnValue({ width: 280 } as DOMRect);
  const handle = screen.getByRole("separator", { name: "Resize chat history" });
  handle.setPointerCapture = vi.fn();
  fireEvent.pointerDown(handle, { button: 0, clientX: 280, pointerId: 1 });
  fireEvent.pointerMove(handle, { clientX: 400, pointerId: 1 });
  expect(pane).toHaveStyle({ width: "400px" });
  fireEvent.pointerUp(handle, { pointerId: 1 });
  expect(localStorage.getItem("minerva.chatHistory.width")).toBe("400");
  view.unmount();
  render(<AgentHistory />);
  const restoredPane = screen.getByRole("complementary", { name: "Chat history" });
  expect(restoredPane).toHaveStyle({ width: "400px" });
  vi.spyOn(restoredPane, "getBoundingClientRect").mockReturnValue({ width: 400 } as DOMRect);
  const restored = screen.getByRole("separator", { name: "Resize chat history" });
  fireEvent.keyDown(restored, { key: "ArrowLeft" });
  expect(localStorage.getItem("minerva.chatHistory.width")).toBe("392");
  localStorage.removeItem("minerva.chatHistory.width");
});

it("preserves the chrome of an auxiliary task window", () => {
  windowState.auxiliary = true;
  const previous = useWorkbenchChrome.getState();
  try {
    const stop = activateAgentNavigation();
    expect(useWorkbenchChrome.getState()).toBe(previous);
    stop();
    expect(useWorkbenchChrome.getState()).toBe(previous);
  } finally {
    windowState.auxiliary = false;
  }
});
