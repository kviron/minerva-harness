import { expect, stubDesktop, test } from "./fixtures";

for (const theme of ["dark", "light"] as const) {
  test(`Minerva subscription limits in ${theme}`, async ({ workbench: page }) => {
    await stubDesktop(page);
    await page.evaluate(async (theme) => {
      const themePath = "/src/workbench/theme.ts";
      (await import(themePath)).previewTheme(theme);
      let phase = 0;
      document.addEventListener("test:usage-next", () => phase++);
      window.__ipc.handlers.minerva_call = ({ method }) => {
        if (method === "usage.list") {
          return JSON.stringify([
            {
              providerId: "openai-codex",
              providerName: "Codex",
              status: phase === 1 ? "not-configured" : "ok",
              windows:
                phase === 1
                  ? []
                  : [{ id: "primary", windowSeconds: 7200, usedPercent: 37, resetAt: Date.now() + 3600000 }],
              updatedAt: phase === 1 ? null : Date.now(),
              errorCode: null,
            },
            {
              providerId: "opencode-go",
              providerName: "OpenCode Go",
              status: phase === 2 ? "unavailable" : "ok",
              windows: phase === 3 ? [] : [{ id: "weekly", windowSeconds: null, usedPercent: 82, resetAt: null }],
              updatedAt: Date.now(),
              balance: phase === 3 ? { unit: "usd", amount: 0 } : null,
              errorCode: phase === 2 ? "network" : null,
            },
          ]);
        }
        if (method === "codex.status") return JSON.stringify({ state: "signed-out" });
        if (method === "providers.list")
          return JSON.stringify([
            {
              id: "opencode-go",
              name: "OpenCode Go",
              baseUrl: "https://opencode.ai/inference/go",
              models: ["deepseek"],
              protocol: "openai-chat",
              hasKey: true,
            },
          ]);
        if (method === "providers.known") return "[]";
        throw new Error(`Unexpected method ${method}`);
      };
      const settingsPath = "/src/features/settings/model/staging.ts";
      (await import(settingsPath)).openSettings("agents.minerva");
    }, theme);
    await page.getByRole("tab", { name: "Provider limits", exact: true }).click();
    const region = page.getByRole("region", { name: "Provider limits", exact: true });
    const codex = region.getByRole("article", { name: "Codex", exact: true });
    const go = region.getByRole("article", { name: "OpenCode Go", exact: true });
    await expect(codex.getByText("63% remaining")).toBeVisible();
    await expect(codex.getByText("2 hours", { exact: true })).toBeVisible();
    await expect(go).toHaveCount(0);
    await page.screenshot({ path: `output/minerva-usage-review/limits-${theme}.png`, animations: "disabled" });
    await page.setViewportSize({ width: 900, height: 650 });
    await page.screenshot({ path: `output/minerva-usage-review/limits-narrow-${theme}.png`, animations: "disabled" });
    await page.evaluate(() => document.dispatchEvent(new Event("test:usage-next")));
    await region.getByRole("button", { name: "Refresh limits" }).click();
    await expect(page.getByRole("tab", { name: "Provider limits" })).toHaveCount(0);
    await expect(codex.getByRole("progressbar")).toHaveCount(0);
    await page
      .getByRole("listbox", { name: "Model providers" })
      .getByRole("option")
      .filter({ hasText: "OpenCode Go" })
      .click();
    await expect(codex).toHaveCount(0);
    await expect(go.getByText("18% remaining")).toBeVisible();
    await page.evaluate(() => document.dispatchEvent(new Event("test:usage-next")));
    await region.getByRole("button", { name: "Refresh limits" }).click();
    await expect(go.getByText(/last.*known/i)).toBeVisible();
    await page.evaluate(() => document.dispatchEvent(new Event("test:usage-next")));
    await region.getByRole("button", { name: "Refresh limits" }).click();
    await expect(go.getByText("$0.00", { exact: true })).toBeVisible();
    await page.screenshot({ path: `output/minerva-usage-review/limits-balance-${theme}.png`, animations: "disabled" });
  });
}
