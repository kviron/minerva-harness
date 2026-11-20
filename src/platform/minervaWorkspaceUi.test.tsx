import { expect, it, vi } from "vitest";
import { BrowserPluginHost } from "../../vendor/minerva/packages/plugin-sdk/src/context";
import { workspaceServiceKey } from "../../vendor/minerva/packages/plugin-sdk/src/workspace";
import { provideEditorService, useContributions, useWorkspaceItems } from "@/workbench/contributions";
import { bridgeMinervaWorkspace } from "./minervaWorkspaceUi";

it("bridges owned plugin tabs without remounting on unrelated slot changes and removes them on disable", async () => {
  const host = new BrowserPluginHost();
  const scope = { kind: "workspace" as const, workspaceId: "fixture" };
  const openInput = vi.fn().mockResolvedValue(undefined);
  const openFile = vi.fn().mockResolvedValue(undefined);
  const closeInput = vi.fn().mockResolvedValue(true);
  provideEditorService({ openInput, openFile, closeInput } as never);
  const stop = bridgeMinervaWorkspace(
    host,
    scope,
    "fixture",
    () => () => null,
    (id) => id,
  );
  await host.activate([
    {
      id: "fixture-plugin",
      apply(ctx) {
        ctx.scope(scope).slots.register("shell.canvas.tabs", {
          entryId: "report",
          title: "Report",
          icon: "file",
          component: () => null,
        });
      },
    },
  ]);
  const id = "minerva.shell.canvas.tabs.fixture-plugin.report";
  const kind = useContributions.getState().editorKinds.get(id);
  expect(useWorkspaceItems.getState().items.get(id)?.title).toBe("Report");
  await host.services.get(workspaceServiceKey, scope).open("report");
  expect(openInput).toHaveBeenCalledWith({ kind: id, key: id, data: {} }, { focus: true });
  await host.activate([
    {
      id: "settings-fixture",
      apply(ctx) {
        ctx.scope(scope).slots.register("settings.pages", { entryId: "settings", component: () => null });
      },
    },
  ]);
  expect(useContributions.getState().editorKinds.get(id)).toBe(kind);
  await host.activate([
    {
      id: "other-plugin",
      apply(ctx) {
        ctx.scope(scope).slots.register("workspace.panels", { entryId: "report", component: () => null });
      },
    },
  ]);
  await expect(host.services.get(workspaceServiceKey, scope).open("report")).rejects.toThrow(
    "Ambiguous workspace entry",
  );
  await host.services.get(workspaceServiceKey, scope).open("report", "fixture-plugin");
  await host.disable("fixture-plugin");
  expect(useWorkspaceItems.getState().items.has(id)).toBe(false);
  expect(useContributions.getState().editorKinds.has(id)).toBe(false);
  expect(closeInput).toHaveBeenCalledWith(id);
  await expect(host.services.get(workspaceServiceKey, scope).open("missing")).rejects.toThrow(
    "Unknown workspace entry",
  );
  stop();
  await host.dispose();
});
