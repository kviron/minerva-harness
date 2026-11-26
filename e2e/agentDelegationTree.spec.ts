import type { AgentEntry, AgentQueuedFollowup, AgentTask } from "../src/bindings";
import { expect, ipcCalls, stubDesktop, test } from "./fixtures";

const task: AgentTask = {
  id: "delegation-tree",
  title: "Review workspace",
  provider: "minerva",
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
};

const child: AgentEntry = {
  type: "subagent",
  id: "del_one",
  title: "Review config",
  description: null,
  status: "running",
  error: null,
  prompt: "Read the config",
  reply: null,
  tools: 0,
  activity: null,
  startedMs: null,
  endedMs: null,
  cancellable: true,
  deliveryRunId: "run-one",
};

for (const provider of ["minerva", "codex"]) {
  test(`tree stop is scoped to Minerva (${provider})`, async ({ workbench: page }, testInfo) => {
    await stubDesktop(page);
    await page.evaluate(
      async ({ task, child }) => {
        const path = "/src/features/agents/store.ts";
        const store = await import(path);
        const handlers = window.__ipc.handlers;
        let history: AgentEntry[] = [];
        let queuedFollowups: AgentQueuedFollowup[] = [];
        let deliveryRunId: string | null = "run-one";
        let publishQueue = () => {};
        handlers.agent_subscribe = ({ onEvent }) => {
          publishQueue = () => {
            window.__ipc.send(onEvent, {
              type: "entry",
              task: task.id,
              subagent: null,
              index: 0,
              entry: { ...child, deliveryRunId, queuedFollowups },
            });
          };
          document.addEventListener("test:delegate-next-run", () => {
            deliveryRunId = null;
            publishQueue();
          });
          document.addEventListener("test:delegate-question", () => {
            const entry: AgentEntry = {
              type: "question",
              id: "minerva-delegation-question:child:interaction",
              title: "Subagent question",
              questions: [
                {
                  answerKey: "answer",
                  header: "api:fixture/model · Review config",
                  question: "Which approach?",
                  multiSelect: false,
                  options: [{ label: "Option one" }, { label: "Option two" }],
                },
              ],
              chosen: null,
            };
            const publish = (chosen: string | null) =>
              window.__ipc.send(onEvent, {
                type: "entry",
                task: task.id,
                subagent: null,
                index: 2,
                entry: { ...entry, chosen },
              });
            publish(null);
            handlers.agent_answer_question = () => {
              publish("Option one");
              return null;
            };
          });
          document.addEventListener("test:delegate-finished", () => {
            history = [{ type: "agent", text: "Second native turn finished" }];
            window.__ipc.send(onEvent, {
              type: "entry",
              task: task.id,
              subagent: null,
              index: 0,
              entry: { ...child, status: "done" },
            });
          });
          document.addEventListener("test:delegate-reply", (event) => {
            const text = (event as CustomEvent<string>).detail;
            window.__ipc.send(onEvent, {
              type: "entry",
              task: task.id,
              subagent: child.id,
              index: 0,
              entry: { type: "message", incoming: false, text },
            });
          });
          document.addEventListener("test:delegate-steered-message", () => {
            queuedFollowups = queuedFollowups.filter((item) => item.id !== "work-one");
            publishQueue();
            window.__ipc.send(onEvent, {
              type: "entry",
              task: task.id,
              subagent: child.id,
              index: 1,
              entry: {
                type: "user",
                text: "Check the alternate config",
                attachments: [],
                images: [],
                sentMs: null,
                doneMs: null,
                steered: true,
              },
            });
          });
          return [task];
        };
        handlers.agent_providers = () => [
          {
            id: "codex",
            name: "Codex",
            command: "codex",
            program: "codex",
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
        handlers.agent_entries = ({ subagent }) =>
          subagent
            ? history
            : [
                child,
                { ...child, id: "done", title: "Read logs", prompt: "Read logs", status: "done", fallback: undefined },
              ];
        handlers.agent_stop_delegation_tree = () => null;
        let submissions = 0;
        handlers.agent_delegation_followup = ({ prompt }) => {
          if (++submissions === 1) throw new Error("Receipt unavailable");
          const workItemId = submissions === 2 ? "work-one" : "work-two";
          queuedFollowups = [...queuedFollowups, { id: workItemId, prompt: String(prompt), state: "queued" }];
          publishQueue();
          return { queued: true, workItemId };
        };
        handlers.agent_delegation_followup_now = ({ workItem }) => {
          queuedFollowups = queuedFollowups.map((item) =>
            item.id === workItem ? { ...item, state: "requested" } : item,
          );
          publishQueue();
          return true;
        };
        handlers.agent_changes = () => [];
        handlers.agent_grouped_changes = () => ({ uncommitted: [], commits: [] });
        await store.connect();
        store.useAgents.setState({ homeOpen: false, sessionLocation: "editor", docked: null });
        store.openTask(task.id);
      },
      {
        task: { ...task, provider },
        child:
          provider === "minerva"
            ? { ...child, title: "codex", fallback: { requested: "missing-codex", reason: "Executable unavailable" } }
            : child,
      },
    );
    const counter = page.getByRole("button", { name: /^Subagents: / });
    const roster = page.getByRole("dialog", { name: "Subagents" });
    const stop = roster.getByRole("button", { name: "Stop tree", exact: true });
    await expect(page.getByLabel("Subagents at work")).toHaveCount(0);
    await counter.click();
    await expect(roster.getByRole("button", { name: /Read the config/ })).toBeVisible();
    if (provider === "minerva") {
      await expect(page.getByText("Replaced executor missing-codex · Executable unavailable")).toBeVisible();
      await expect(stop).toBeVisible();
      await page.screenshot({ path: testInfo.outputPath("delegation-tree.png"), animations: "disabled" });
      await page.setViewportSize({ width: 850, height: 720 });
      await page.evaluate(async () => {
        const path = "/src/workbench/theme.ts";
        (await import(path)).previewTheme("dark");
      });
      await expect(stop).toBeVisible();
      await page.screenshot({ path: testInfo.outputPath("delegation-tree-dark.png"), animations: "disabled" });
      await stop.click();
      expect(await ipcCalls(page, "agent_stop_delegation_tree")).toEqual([{ task: task.id }]);
      expect(await ipcCalls(page, "agent_cancel")).toEqual([]);
      await expect(counter).toHaveText("Subagents: 2");
      await expect(counter).toHaveCount(1);
      await expect(counter.locator("xpath=ancestor::header[1]")).toContainText(task.title);
      expect(
        await counter.evaluate((button) => {
          const label = button.closest("header")?.querySelector("strong");
          if (!label) return false;
          const title = label.getBoundingClientRect();
          const control = button.getBoundingClientRect();
          return (
            control.left >= title.right - 1 &&
            Math.abs((control.top + control.bottom - title.top - title.bottom) / 2) < 3
          );
        }),
      ).toBe(true);
      await expect(roster.getByRole("button", { name: /Read logs/ })).toBeVisible();
      await page.screenshot({ path: testInfo.outputPath("subagent-roster.png"), animations: "disabled" });
      await roster.getByRole("button", { name: /Read the config/ }).click();
      await expect(roster).toHaveCount(0);
      await expect(page.getByRole("navigation", { name: "Subagent" })).toBeVisible();
      const composer = page.getByRole("form", { name: "Message the subagent" });
      const input = composer.getByRole("textbox");
      await input.fill("Check the alternate config");
      await composer.getByRole("button", { name: "Queue", exact: true }).click();
      await expect(composer.getByRole("alert")).toContainText("Receipt unavailable");
      await expect(input).toHaveValue("Check the alternate config");
      await composer.getByRole("button", { name: "Queue", exact: true }).click();
      await expect(composer.getByRole("status")).toHaveText("Follow-up queued");
      await expect(input).toHaveValue("");
      const sent = await ipcCalls(page, "agent_delegation_followup");
      expect(sent).toHaveLength(2);
      expect(sent[0]).toMatchObject({ task: task.id, subagent: child.id, prompt: "Check the alternate config" });
      expect(sent[1]).toEqual(sent[0]);
      await page.screenshot({ path: testInfo.outputPath("subagent-composer.png"), animations: "disabled" });
      await input.fill("Draft for later");
      await composer.getByRole("button", { name: "Send now", exact: true }).click();
      await expect(composer.getByRole("status")).toHaveText("Follow-up requested for next model step");
      await expect(input).toHaveValue("Draft for later");
      expect(await ipcCalls(page, "agent_delegation_followup_now")).toEqual([
        { task: task.id, subagent: child.id, workItem: "work-one", expectedRun: "run-one" },
      ]);
      await composer.getByRole("button", { name: "Queue", exact: true }).click();
      await expect(composer.getByRole("button", { name: "Send now", exact: true })).toBeVisible();
      await page.evaluate(() => document.dispatchEvent(new Event("test:delegate-next-run")));
      await expect(composer.getByRole("button", { name: "Send now", exact: true })).toHaveCount(0);
      await expect(composer.getByRole("status")).toHaveText("Follow-up queued");
      expect(await ipcCalls(page, "agent_delegation_followup_now")).toHaveLength(1);
      await page.getByRole("button", { name: "Back", exact: true }).click();
      await counter.click();
      await roster.getByRole("button", { name: /Read the config/ }).click();
      await expect(composer.getByText("Draft for later", { exact: true })).toBeVisible();
      await expect(composer.getByText("Check the alternate config", { exact: true })).toBeVisible();
      await page.screenshot({ path: testInfo.outputPath("subagent-followup-queue.png"), animations: "disabled" });
      await page.evaluate(() =>
        document.dispatchEvent(new CustomEvent("test:delegate-reply", { detail: "Config checked" })),
      );
      await expect(page.getByText("Config checked", { exact: true })).toBeVisible();
      await page.evaluate(() =>
        document.dispatchEvent(new CustomEvent("test:delegate-reply", { detail: "Config checked. Two files read." })),
      );
      await expect(page.getByText("Config checked. Two files read.", { exact: true })).toBeVisible();
      await expect(page.getByText("Config checked", { exact: true })).toHaveCount(0);
      await page.evaluate(() => document.dispatchEvent(new Event("test:delegate-steered-message")));
      await expect(page.getByText("Check the alternate config", { exact: true })).toBeVisible();
      await page.screenshot({ path: testInfo.outputPath("delegation-live-reply.png"), animations: "disabled" });
      await page.evaluate(() => document.dispatchEvent(new Event("test:delegate-finished")));
      await expect(page.getByText("Second native turn finished", { exact: true })).toBeVisible();
      await expect(page.locator("[data-author]").getByText("Codex", { exact: true })).toBeVisible();
      await page.getByRole("button", { name: "Back", exact: true }).click();
      await page.evaluate(() => document.dispatchEvent(new Event("test:delegate-question")));
      const question = page.getByRole("form", { name: "Subagent question" });
      await expect(question.getByText("Which approach?")).toBeVisible();
      await expect(question.getByText("api:fixture/model · Review config", { exact: true })).toBeVisible();
      await expect(question.getByText("Other", { exact: true })).toHaveCount(0);
      await expect(question.getByRole("button", { name: "Skip" })).toHaveCount(0);
      await page.screenshot({ path: testInfo.outputPath("delegation-question.png"), animations: "disabled" });
      await question.getByText("Option one", { exact: true }).click();
      await question.getByRole("button", { name: "Submit" }).click();
      await expect(question).toHaveCount(0);
      expect(await ipcCalls(page, "agent_answer_question")).toEqual([
        {
          task: task.id,
          question: "minerva-delegation-question:child:interaction",
          answer: JSON.stringify({ answer: ["Option one"] }),
        },
      ]);
    } else {
      await expect(stop).toHaveCount(0);
      await page.keyboard.press("Escape");
    }
  });
}
