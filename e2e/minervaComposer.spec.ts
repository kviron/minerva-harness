import type { AgentConfigOption, AgentProvider, AgentTask } from "../src/bindings";
import { expect, ipcCalls, stubDesktop, test } from "./fixtures";

const models: AgentConfigOption = {
  id: "model",
  category: "model",
  name: "Model",
  description: null,
  via: "option",
  current: "api:gateway/deepseek",
  values: [
    { value: "api:gateway/deepseek", name: "DeepSeek", description: "OpenCode Go" },
    { value: "api:openai-codex/gpt-6.2-astra", name: "Astra", description: "Codex subscription" },
    { value: "api:openai-codex/gpt-6.1-terra", name: "Terra", description: "Codex subscription" },
    { value: "api:gateway/haiku", name: "Haiku", description: "OpenCode Go" },
    { value: "api:gateway/gpt-6.2-astra", name: "Astra via gateway", description: "OpenCode Go" },
  ],
};
const provider: AgentProvider = {
  id: "minerva",
  name: "Minerva",
  command: "builtin:minerva",
  program: "builtin:minerva",
  args: [],
  defaultProgram: null,
  defaultArgs: [],
  loginHint: null,
  commandOverridden: false,
  commandEditable: false,
  commandReadOnlyReason: null,
  installed: true,
  env: {},
  commands: [],
  source: { kind: "builtin" },
  config: [models],
};
for (const providerId of ["minerva", "minerva.api.old"]) {
  test(`${providerId} composer switches backend within its existing chat`, async ({ workbench: page }, info) => {
    const task: AgentTask = {
      id: "minerva-existing-chat",
      title: "Same conversation",
      provider: providerId,
      status: "ready",
      isolation: "local",
      model: models.current,
      cwd: "/dev/acme-api",
      branch: null,
      base: null,
      baseCommit: null,
      error: null,
      createdMs: 0,
      updatedMs: 0,
      config: [models],
      commands: [],
      usage: null,
      queue: [],
    };
    await stubDesktop(page);
    await page.evaluate(
      async ({ provider, task }) => {
        const path = "/src/features/agents/store.ts";
        const store = await import(path);
        let current = task;
        let channel: unknown;
        const handlers = window.__ipc.handlers;
        handlers.agent_providers = () => [provider];
        handlers.agent_subscribe = ({ onEvent }) => {
          channel = onEvent;
          return [current];
        };
        handlers.agent_refresh_models = () => ({
          provider: "minerva",
          config: provider.config,
          thoughtLevels: {},
          refreshing: false,
          refreshedAtMs: Date.now(),
          retryAtMs: null,
          error: null,
        });
        handlers.minerva_call = ({ method }) => {
          if (method !== "usage.list") throw new Error(`Unexpected Minerva method ${method}`);
          return JSON.stringify([
            {
              providerId: "openai-codex",
              providerName: "Codex",
              status: "ok",
              windows: [{ id: "secondary", windowSeconds: 604800, usedPercent: 56, resetAt: null }],
              updatedAt: Date.now(),
              errorCode: null,
            },
            {
              providerId: "gateway",
              providerName: "OpenCode Go",
              status: "ok",
              windows: [
                { id: "weekly", windowSeconds: 604800, usedPercent: 18, resetAt: null },
                { id: "5h", windowSeconds: 18000, usedPercent: 0, resetAt: null },
                { id: "monthly", windowSeconds: null, usedPercent: 69, resetAt: null },
              ],
              updatedAt: Date.now(),
              errorCode: null,
            },
          ]);
        };
        handlers.agent_probe = () => null;
        handlers.agent_commands = () => [];
        handlers.agent_entries = () => [
          { type: "user", text: "Keep this conversation", attachments: [], sentMs: 0, doneMs: 56000 },
          { type: "agent", text: "The previous answer remains here." },
        ];
        handlers.agent_changes = () => [];
        handlers.agent_grouped_changes = () => ({ uncommitted: [], commits: [] });
        handlers.agent_set_config = ({ task: id, option, value }) => {
          if (id !== current.id || option !== "model")
            throw new Error("Backend switch must change the existing task model");
          current = {
            ...current,
            model: String(value),
            config: current.config.map((config) =>
              config.id === option ? { ...config, current: String(value) } : config,
            ),
          };
          window.__ipc.send(channel, { type: "task", task: current });
          return null;
        };
        handlers.agent_prompt = () => null;
        await store.connect();
        store.useAgents.setState({ homeOpen: false, sessionLocation: "editor", docked: null });
        store.openTask(task.id);
      },
      { provider, task },
    );
    await page.evaluate(
      async (theme) => {
        const themePath = "/src/workbench/theme.ts";
        (await import(themePath)).previewTheme(theme);
      },
      providerId === "minerva" ? "dark" : "light",
    );
    const box = page.getByRole("textbox", { name: "Message", exact: true }).filter({ visible: true });
    await expect(box).toBeVisible();
    const usageTrigger = page.getByRole("button", { name: "Subscription limits", exact: true });
    await expect(usageTrigger).toHaveText("0%");
    expect((await ipcCalls(page, "minerva_call")).filter((call) => call.method === "usage.list")).toHaveLength(1);
    const workTimer = page.locator('[title="Time the agent spent on this task"]');
    await expect(workTimer).toHaveText("56s");
    for (const width of [1400, 900]) {
      await page.setViewportSize({ width, height: width === 900 ? 650 : 900 });
      const delta = await page.evaluate(() => {
        const top = (element: Element) => {
          const range = document.createRange();
          range.selectNodeContents(element);
          return range.getBoundingClientRect().top;
        };
        const timer = document.querySelector('[title="Time the agent spent on this task"]')!;
        const quota = document.querySelector('[aria-label="Subscription limits"][aria-haspopup="dialog"]')!;
        return Math.abs(top(timer) - top(quota));
      });
      console.log(JSON.stringify({ providerId, width, textBaselineDelta: delta }));
      expect(delta).toBeLessThanOrEqual(0.1);
      await page.screenshot({
        path: `output/minerva-usage-review/indicator-alignment-${providerId}-${width}.png`,
        animations: "disabled",
      });
    }
    await page.setViewportSize({ width: 1400, height: 900 });
    await page.getByRole("button", { name: "Subscription limits", exact: true }).click();
    const limits = page.getByRole("dialog", { name: "Subscription limits", exact: true });
    await expect(limits.getByText("0% used", { exact: true })).toBeVisible();
    await expect(limits.getByRole("article", { name: "Codex", exact: true })).toHaveCount(0);
    await page.screenshot({
      path: `output/minerva-usage-review/chat-limits-${providerId}.png`,
      animations: "disabled",
    });
    await page.setViewportSize({ width: 900, height: 650 });
    await page.screenshot({
      path: `output/minerva-usage-review/chat-limits-narrow-${providerId}.png`,
      animations: "disabled",
    });
    await page.keyboard.press("Escape");
    await expect(limits).toHaveCount(0);
    const callsBeforeReopen = (await ipcCalls(page, "minerva_call")).filter(
      (call) => call.method === "usage.list",
    ).length;
    await page.getByRole("button", { name: "Subscription limits", exact: true }).click();
    await expect(limits.getByText("0% used", { exact: true })).toBeVisible();
    await expect(limits.getByRole("article", { name: "Codex", exact: true })).toHaveCount(0);
    expect((await ipcCalls(page, "minerva_call")).filter((call) => call.method === "usage.list")).toHaveLength(
      callsBeforeReopen,
    );
    await page.keyboard.press("Escape");
    await page.setViewportSize({ width: 1400, height: 900 });
    await box.fill("Continue the same conversation");
    await expect(page.getByRole("button", { name: "Provider", exact: true })).toHaveText("Minerva");
    await expect(page.getByRole("button", { name: "Send", exact: true })).toBeEnabled();
    await page.getByRole("button", { name: "Model", exact: true }).click();
    await expect(
      page.getByRole("listbox", { name: "Model", exact: true }).getByText("Codex", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("listbox", { name: "Model", exact: true }).getByText("OpenCode Go", { exact: true }),
    ).toBeVisible();
    await expect(page.getByRole("option")).toHaveCount(5);
    await expect(page.getByRole("option").getByText("gpt-6.2-astra", { exact: true })).toHaveCount(0);
    await page.screenshot({
      path: `output/minerva-codex-review/provider-groups-${providerId}.png`,
      animations: "disabled",
    });
    const search = page.getByPlaceholder("Search…", { exact: true });
    await search.fill("Codex");
    await expect(page.getByRole("option")).toHaveCount(2);
    await expect(
      page.getByRole("listbox", { name: "Model", exact: true }).getByText("OpenCode Go", { exact: true }),
    ).toHaveCount(0);
    await search.fill("");
    await search.fill("gpt-6.2-astra");
    await expect(page.getByRole("option")).toHaveCount(2);
    await expect(page.getByRole("option").first()).toContainText("Astra");
    await search.fill("");
    await page
      .getByRole("option")
      .filter({ hasText: "Terra" })
      .getByRole("button", { name: "Pin model", exact: true })
      .click();
    await expect(page.getByRole("option").first()).toContainText("Terra");
    await page
      .getByRole("option")
      .filter({ hasText: "Terra" })
      .getByRole("button", { name: "Unpin model", exact: true })
      .click();
    await expect(
      page.getByRole("listbox", { name: "Model", exact: true }).getByText("Pinned", { exact: true }),
    ).toHaveCount(0);
    await page.getByRole("option", { name: /^Astra(?: Pin model)?$/ }).click();
    await expect(usageTrigger).toHaveText("56%");
    await usageTrigger.click();
    await expect(limits.getByText("56% used", { exact: true })).toBeVisible();
    await expect(limits.getByRole("article", { name: "OpenCode Go", exact: true })).toHaveCount(0);
    await page.screenshot({
      path: `output/minerva-usage-review/selected-codex-${providerId}.png`,
      animations: "disabled",
    });
    await page.keyboard.press("Escape");
    const calls = await ipcCalls(page, "agent_set_config");
    expect(calls).toEqual([{ task: task.id, option: "model", value: "api:openai-codex/gpt-6.2-astra" }]);
    await expect(box).toHaveText("Continue the same conversation");
    await expect(page.getByText("The previous answer remains here.", { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Send", exact: true })).toBeEnabled();
    await expect(page.getByText("Choose a model from this provider to continue.", { exact: true })).toHaveCount(0);
    await page.screenshot({ path: info.outputPath("same-chat-codex.png"), animations: "disabled" });
    await page.getByRole("button", { name: "Model", exact: true }).click();
    await page.getByRole("option", { name: /DeepSeek/ }).click();
    await expect(usageTrigger).toHaveText("0%");
    expect(await ipcCalls(page, "agent_set_config")).toEqual([
      { task: task.id, option: "model", value: "api:openai-codex/gpt-6.2-astra" },
      { task: task.id, option: "model", value: "api:gateway/deepseek" },
    ]);
    await expect(box).toHaveText("Continue the same conversation");
    await expect(page.getByRole("button", { name: "Send", exact: true })).toBeEnabled();
    await page.setViewportSize({ width: 900, height: 650 });
    await page.getByRole("button", { name: "Model", exact: true }).click();
    await expect(
      page.getByRole("listbox", { name: "Model", exact: true }).getByText("Codex", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByRole("listbox", { name: "Model", exact: true }).getByText("OpenCode Go", { exact: true }),
    ).toBeVisible();
    await page.screenshot({
      path: `output/minerva-codex-review/provider-groups-narrow-${providerId}.png`,
      animations: "disabled",
    });
  });
}
