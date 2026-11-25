import { expect, ipcCalls, stubDesktop, test } from "./fixtures";

for (const theme of ["dark", "light"] as const) {
  test(`Minerva settings in ${theme} preserve API editing and subscription states`, async ({
    workbench: page,
  }, info) => {
    await stubDesktop(page);
    await page.evaluate(async (theme) => {
      const themePath = "/src/workbench/theme.ts";
      (await import(themePath)).previewTheme(theme);
      let status = { state: "signed-out" } as Record<string, unknown>;
      let providers = [
        {
          id: "gateway",
          name: "A very long configured API gateway provider name",
          baseUrl: "https://example.com/v1",
          models: ["model-a"],
          protocol: "openai-chat",
          hasKey: true,
        },
      ];
      window.__ipc.handlers["plugin:opener|open_url"] = () => null;
      window.__ipc.handlers.minerva_call = ({ method, params }) => {
        const input = JSON.parse(String(params));
        let value: unknown;
        switch (method) {
          case "usage.list":
            value = [];
            break;
          case "providers.list":
            value = providers;
            break;
          case "providers.known":
            value = [];
            break;
          case "providers.save": {
            const saved = {
              id: input.id,
              name: input.name,
              baseUrl: input.baseUrl,
              models: input.models,
              protocol: input.protocol,
              hasKey: input.apiKey ? true : (providers.find((p) => p.id === input.id)?.hasKey ?? false),
            };
            providers = input.isNew ? [...providers, saved] : providers.map((p) => (p.id === input.id ? saved : p));
            value = providers;
            break;
          }
          case "codex.status":
            value = status;
            break;
          case "codex.signIn":
            status = { state: "signing-in", authorizationUrl: "https://auth.openai.com/fixture" };
            value = status;
            break;
          case "codex.signOut":
            status = { state: "signed-out" };
            value = status;
            break;
          case "codex.models":
            value = ["gpt-6.2-sol", "gpt-6.2-astra"];
            break;
          case "codex.configure":
            value = providers;
            break;
          default:
            throw new Error(`Unexpected Minerva method: ${method}`);
        }
        return JSON.stringify(value);
      };
      document.addEventListener("test:codex-signed-in", () => {
        status = { state: "signed-in" };
      });
      document.addEventListener("test:codex-auth-error", () => {
        status = { state: "error", error: "Sign-in was cancelled. Try again." };
      });
      const settingsPath = "/src/features/settings/model/staging.ts";
      (await import(settingsPath)).openSettings("agents.minerva");
    }, theme);
    const settings = page.getByRole("dialog", { name: "Settings", exact: true });
    const codex = settings.getByRole("region", { name: "Codex subscription", exact: true });
    await expect(settings.getByRole("heading", { name: "Codex", exact: true })).toBeVisible();
    await expect(codex.getByRole("status")).toHaveText("Not signed in");
    await expect(codex.getByRole("button", { name: "Fetch model list" })).toBeDisabled();
    const addTrigger = settings.getByRole("button", { name: "Add model provider…", exact: true });
    await expect(addTrigger).toBeInViewport({ ratio: 1 });
    await page.screenshot({ path: info.outputPath("settings-desktop.png"), animations: "disabled" });
    await settings
      .getByRole("listbox", { name: "Model providers" })
      .getByRole("option")
      .filter({ hasText: "A very long configured" })
      .click();
    await settings.getByRole("textbox", { name: "Provider name", exact: true }).fill("Renamed gateway");
    await settings.getByRole("tab", { name: "Subscription limits", exact: true }).click();
    await settings.getByRole("tab", { name: "Settings", exact: true }).click();
    await expect(settings.getByRole("textbox", { name: "Provider name", exact: true })).toHaveValue("Renamed gateway");
    await settings.getByRole("button", { name: "Save", exact: true }).first().click();
    const apiSaves = (await ipcCalls(page, "minerva_call")).filter((call) => call.method === "providers.save");
    expect(JSON.parse(String(apiSaves.at(-1)?.params)).apiKey).toBeNull();
    await expect(settings.getByRole("heading", { name: "Renamed gateway", exact: true })).toBeVisible();
    await settings.getByRole("button", { name: "Add model provider…", exact: true }).click();
    const add = page.getByRole("dialog", { name: "Add model provider…", exact: true });
    await expect(add).toBeVisible();
    await add.getByRole("textbox", { name: "Provider ID", exact: true }).fill("second");
    await add.getByRole("textbox", { name: "Provider name", exact: true }).fill("Second provider");
    await add.getByRole("textbox", { name: "API base URL", exact: true }).fill("https://second.example/v1");
    await page.screenshot({ path: `output/minerva-usage-review/add-provider-${theme}.png`, animations: "disabled" });
    await add.getByRole("button", { name: "Save", exact: true }).click();
    await expect(add).toHaveCount(0);
    await expect(settings.getByRole("heading", { name: "Second provider", exact: true })).toBeVisible();
    await settings
      .getByRole("listbox", { name: "Model providers" })
      .getByRole("option")
      .filter({ hasText: "Codex" })
      .click();
    await codex.getByRole("button", { name: "Sign in with ChatGPT" }).click();
    await expect(codex.getByRole("status")).toHaveText("Waiting for browser sign-in…");
    await expect(codex.getByRole("button", { name: "Open sign-in page" })).toBeVisible();
    await page.screenshot({ path: info.outputPath("settings-signing-in.png"), animations: "disabled" });
    await page.evaluate(() => document.dispatchEvent(new Event("test:codex-auth-error")));
    await expect(codex.getByRole("alert")).toHaveText("Sign-in was cancelled. Try again.");
    await page.screenshot({ path: info.outputPath("settings-auth-error.png"), animations: "disabled" });
    await codex.getByRole("button", { name: "Sign in with ChatGPT" }).click();
    await page.evaluate(() => document.dispatchEvent(new Event("test:codex-signed-in")));
    await expect(codex.getByRole("status")).toHaveText("Signed in");
    await codex.getByRole("button", { name: "Fetch model list" }).click();
    await codex.getByRole("checkbox", { name: "gpt-6.2-sol", exact: true }).check();
    await codex.getByRole("checkbox", { name: "gpt-6.2-astra", exact: true }).check();
    await codex.getByRole("button", { name: "Save", exact: true }).click();
    const saves = (await ipcCalls(page, "minerva_call")).filter((call) => call.method === "codex.configure");
    expect(JSON.parse(String(saves.at(-1)?.params))).toEqual({ models: ["gpt-6.2-sol", "gpt-6.2-astra"] });
    await page.screenshot({ path: `output/minerva-codex-review/catalog-6.2-${theme}.png`, animations: "disabled" });
    await page.setViewportSize({ width: 900, height: 650 });
    await expect(addTrigger).toBeInViewport({ ratio: 1 });
    await codex.getByRole("button", { name: "Save", exact: true }).scrollIntoViewIfNeeded();
    await expect(addTrigger).toBeInViewport({ ratio: 1 });
    await page.screenshot({ path: info.outputPath("settings-narrow.png"), animations: "disabled" });
    await expect(codex.getByRole("button", { name: "Save", exact: true })).toBeInViewport();
  });
}
