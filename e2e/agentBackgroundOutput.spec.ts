import type { AgentTask } from "../src/bindings";
import { expect, ipcCalls, stubDesktop, test } from "./fixtures";

const command = "npm run tauri dev -- --a-very-long-command-argument-that-must-not-cover-the-stop-control";
const task: AgentTask = {
  id: "background-output",
  title: "Dev server",
  provider: "fixture",
  status: "ready",
  isolation: "local",
  model: null,
  cwd: "/dev/acme-api",
  branch: null,
  base: null,
  baseCommit: null,
  error: null,
  createdMs: 0,
  updatedMs: 0,
  config: [],
  commands: [],
  usage: null,
  queue: [],
  capabilities: { backgroundTasks: true },
  background: [
    {
      id: "shell-1",
      kind: "local_bash",
      description: command,
      startedMs: Date.now() - 12000,
      output: "[stdout]\nStarting dev server\n[stderr]\nA diagnostic",
      status: "running",
    },
    ...Array.from({ length: 7 }, (_, index) => ({
      id: `finished-${index}`,
      kind: "local_bash",
      description: `Completed build ${index}`,
      startedMs: Date.now() - 20000,
      output: `exit code: ${index === 1 ? 1 : 0}`,
      status: index === 1 ? "failed" : "completed",
    })),
  ],
};

for (const theme of ["dark", "light"] as const) {
  test(`header jobs popup in ${theme} shows output and separates stopping from finished history`, async ({
    workbench: page,
  }, testInfo) => {
    await stubDesktop(page);
    await page.evaluate(async (task) => {
      const storePath = "/src/features/agents/store.ts";
      const store = await import(storePath);
      let current = task;
      let channel: unknown;
      let revision = 0;
      const handlers = window.__ipc.handlers;
      handlers.agent_subscribe = ({ onEvent }) => {
        channel = onEvent;
        return [current];
      };
      handlers.agent_providers = () => [
        {
          id: "fixture",
          name: "Fixture",
          command: "fixture",
          program: "fixture",
          args: [],
          defaultProgram: null,
          defaultArgs: [],
          loginHint: null,
          commandOverridden: false,
          commandEditable: true,
          commandReadOnlyReason: null,
          installed: true,
          env: {},
          commands: [],
          source: { kind: "custom" },
          config: [],
        },
      ];
      handlers.agent_probe = () => null;
      handlers.agent_commands = () => [];
      handlers.agent_entries = () => [
        { type: "user", text: "Start the development server", attachments: [] },
        { type: "agent", text: "The development server is running in the background." },
      ];
      handlers.agent_changes = () => [];
      handlers.agent_grouped_changes = () => ({ uncommitted: [], commits: [] });
      const publish = () => window.__ipc.send(channel, { type: "task", task: current });
      handlers.agent_stop_background = ({ background }) => {
        current = {
          ...current,
          background: current.background?.map((job) => (job.id === background ? { ...job, status: "stopping" } : job)),
        };
        publish();
        return null;
      };
      document.addEventListener("test:job-stopped", () => {
        current = {
          ...current,
          background: current.background?.map((job) => (job.id === "shell-1" ? { ...job, status: "stopped" } : job)),
        };
        publish();
      });
      document.addEventListener("test:job-output", () => {
        revision += 1;
        current = {
          ...current,
          background: current.background?.map((job) =>
            job.id === "shell-1"
              ? {
                  ...job,
                  output:
                    "[stdout]\nReady http://localhost:1420\n" +
                    Array.from(
                      { length: 60 },
                      (_, index) => `line ${index} revision ${revision} ${"x".repeat(100)}`,
                    ).join("\n"),
                }
              : job,
          ),
        };
        publish();
      });
      await store.connect();
      store.useAgents.setState({ homeOpen: false, sessionLocation: "editor", docked: null });
      store.openTask(task.id);
    }, task);
    await page.evaluate(async (theme) => {
      const path = "/src/workbench/theme.ts";
      (await import(path)).previewTheme(theme);
    }, theme);
    const trigger = page.getByRole("button", { name: "Background tasks: 1", exact: true });
    await expect(trigger).toBeVisible();
    const triggerRect = await trigger.boundingBox();
    expect(triggerRect!.y).toBeLessThan(180);
    await trigger.click();
    const dialog = page.getByRole("dialog", { name: "Background task output" });
    const outputToggle = dialog.getByRole("button", { name: `View output: ${command}` });
    await expect(dialog.getByRole("button", { name: "Finished 7" })).toHaveAttribute("aria-expanded", "false");
    await expect(dialog.getByText("Completed build 0", { exact: true })).toHaveCount(0);
    await page.screenshot({ path: testInfo.outputPath("background-roster-desktop.png"), animations: "disabled" });
    await outputToggle.click();
    await expect(dialog).toContainText("Starting dev server");
    await page.evaluate(() => document.dispatchEvent(new Event("test:job-output")));
    const output = dialog.getByRole("region", { name: "Output", exact: true });
    await expect(output).toContainText("Ready http://localhost:1420");
    expect(
      await output.evaluate((element) => element.scrollTop + element.clientHeight >= element.scrollHeight - 1),
    ).toBe(true);
    await page.screenshot({ path: testInfo.outputPath("background-output-desktop.png"), animations: "disabled" });
    await output.evaluate((element) => {
      element.scrollTop = 0;
      element.dispatchEvent(new Event("scroll"));
    });
    await page.evaluate(() => document.dispatchEvent(new Event("test:job-output")));
    await expect(output).toContainText("revision 2");
    expect(await output.evaluate((element) => element.scrollTop)).toBe(0);
    await outputToggle.click();
    expect(await ipcCalls(page, "agent_stop_background")).toEqual([]);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await expect(trigger).toBeFocused();
    await page.setViewportSize({ width: 900, height: 650 });
    await trigger.click();
    await dialog.getByRole("button", { name: "Finished 7" }).click();
    await expect(dialog.getByText("Completed build 0", { exact: true })).toBeVisible();
    await dialog.getByRole("button", { name: `View output: ${command}` }).click();
    const rect = await dialog.boundingBox();
    expect(rect!.x).toBeGreaterThanOrEqual(0);
    expect(rect!.x + rect!.width).toBeLessThanOrEqual(900);
    await expect(dialog.locator("pre")).toHaveCSS("overflow", "auto");
    await page.screenshot({ path: testInfo.outputPath("background-output-narrow.png"), animations: "disabled" });
    await dialog.getByRole("button", { name: "Clear", exact: true }).click();
    await expect(dialog.getByRole("button", { name: /Finished/ })).toHaveCount(0);
    expect(await ipcCalls(page, "agent_stop_background")).toEqual([]);
    await dialog.getByRole("button", { name: "Stop", exact: true }).click();
    await expect(dialog.getByRole("button", { name: "Stopping…", exact: true })).toBeDisabled();
    expect(await ipcCalls(page, "agent_stop_background")).toEqual([{ task: task.id, background: "shell-1" }]);
    await page.evaluate(() => document.dispatchEvent(new Event("test:job-stopped")));
    await expect(dialog.getByRole("button", { name: "Stopping…", exact: true })).toHaveCount(0);
    await expect(dialog.getByRole("button", { name: "Finished 1" })).toBeVisible();
  });
}
