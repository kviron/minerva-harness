import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import type { AgentBackgroundTask } from "@/bindings";
import { BackgroundTasks } from "./BackgroundTasks";

const job: AgentBackgroundTask = {
  id: "shell-1",
  kind: "local_bash",
  description: "npm run tauri dev",
  startedMs: null,
  output: "[stdout]\nStarting dev server",
};
const openPanel = () => fireEvent.click(screen.getByRole("button", { name: "Background tasks: 1" }));

it("keeps live output current while disclosure and panel closing never stop the job", () => {
  const onStop = vi.fn();
  const { rerender } = render(<BackgroundTasks tasks={[job]} canStop onStop={onStop} />);
  openPanel();
  const toggle = screen.getByRole("button", { name: "View output: npm run tauri dev" });
  fireEvent.click(toggle);
  expect(toggle).toHaveAttribute("aria-expanded", "true");
  expect(screen.getByRole("region", { name: "Output" })).toHaveTextContent("Starting dev server");
  rerender(
    <BackgroundTasks
      tasks={[{ ...job, output: "\u001b[32mReady\u001b[0m\nhttp://localhost:1420" }]}
      canStop
      onStop={onStop}
    />,
  );
  expect(screen.getByRole("region", { name: "Output" })).toHaveTextContent("Ready");
  expect(screen.getByRole("region", { name: "Output" })).not.toHaveTextContent("[32m");
  fireEvent.click(toggle);
  expect(screen.queryByRole("region", { name: "Output" })).not.toBeInTheDocument();
  fireEvent.keyDown(document.activeElement!, { key: "Escape" });
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(onStop).not.toHaveBeenCalled();
  openPanel();
  fireEvent.click(screen.getByRole("button", { name: "Stop" }));
  expect(onStop).toHaveBeenCalledExactlyOnceWith("shell-1");
});

it("preserves legacy rows and their stop capability without requiring output or status", () => {
  render(
    <BackgroundTasks
      tasks={[{ id: "monitor", kind: "monitor", description: "Watching tests", startedMs: null, output: null }]}
      canStop={false}
      onStop={vi.fn()}
    />,
  );
  openPanel();
  expect(screen.getByText("Watching tests")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Stop" })).not.toBeInTheDocument();
  expect(screen.queryByRole("button", { name: /View output/ })).not.toBeInTheDocument();
});

it("opens an empty stream and releases its view when the job disappears", () => {
  const onStop = vi.fn();
  const { rerender } = render(<BackgroundTasks tasks={[{ ...job, output: "" }]} canStop onStop={onStop} />);
  openPanel();
  fireEvent.click(screen.getByRole("button", { name: /View output/ }));
  expect(screen.getByRole("region", { name: "Output" })).toHaveTextContent("Waiting for output…");
  rerender(<BackgroundTasks tasks={[]} canStop onStop={onStop} />);
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  rerender(<BackgroundTasks tasks={[{ ...job, id: "new-server" }]} canStop onStop={onStop} />);
  expect(screen.getByRole("button", { name: "Background tasks: 1" })).toHaveAttribute("aria-expanded", "false");
  expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  expect(onStop).not.toHaveBeenCalled();
});

it("folds finished work by default and Clear only hides those rows locally", () => {
  const onStop = vi.fn();
  const done = { ...job, id: "finished", description: "Completed build", status: "completed" };
  const { rerender } = render(<BackgroundTasks tasks={[job, done]} canStop onStop={onStop} />);
  openPanel();
  const fold = screen.getByRole("button", { name: "Finished 1" });
  expect(fold).toHaveAttribute("aria-expanded", "false");
  expect(screen.queryByText("Completed build")).not.toBeInTheDocument();
  fireEvent.click(fold);
  expect(screen.getByText("Completed build")).toBeInTheDocument();
  expect(
    within(screen.getByRole("group", { name: "Completed build" })).queryByRole("button", { name: "Stop" }),
  ).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Clear" }));
  expect(screen.queryByText("Completed build")).not.toBeInTheDocument();
  expect(onStop).not.toHaveBeenCalled();
  rerender(<BackgroundTasks tasks={[]} canStop onStop={onStop} />);
  rerender(
    <BackgroundTasks
      tasks={[job, done, { ...done, id: "new-finished", description: "New result" }]}
      canStop
      onStop={onStop}
    />,
  );
  openPanel();
  expect(screen.getByRole("button", { name: "Finished 1" })).toBeInTheDocument();
  expect(screen.queryByText("Completed build")).not.toBeInTheDocument();
});

it("treats killed legacy work as finished and never offers Stop", () => {
  const onStop = vi.fn();
  render(<BackgroundTasks tasks={[{ ...job, status: "killed" }]} canStop onStop={onStop} />);
  fireEvent.click(screen.getByRole("button", { name: "Background tasks: 0" }));
  fireEvent.click(screen.getByRole("button", { name: "Finished 1" }));
  expect(screen.getByText("Stopped")).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Stop" })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Clear" }));
  expect(onStop).not.toHaveBeenCalled();
});

it("keeps stopping visibly pending without offering another stop", () => {
  render(<BackgroundTasks tasks={[{ ...job, status: "stopping" }]} canStop onStop={vi.fn()} />);
  openPanel();
  expect(screen.getByRole("button", { name: "Stopping…" })).toBeDisabled();
  expect(screen.getByText("Stopping…")).toBeInTheDocument();
});
