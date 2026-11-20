import type { BrowserPluginHost } from "../../vendor/minerva/packages/plugin-sdk/src/context";
import type { Scope } from "../../vendor/minerva/packages/plugin-sdk/src/lifecycle";
import type { SlotEntry, SlotName } from "../../vendor/minerva/packages/plugin-sdk/src/slots.types";
import { workspaceServiceKey } from "../../vendor/minerva/packages/plugin-sdk/src/workspace";
import {
  editorService,
  registerEditorKind,
  registerWorkspaceItem,
  useWorkbenchChrome,
} from "@/workbench/contributions";
import type { ComponentType } from "react";
import { logError } from "./errorLog";

type Surface = (slot: SlotName, id: string, props: Record<string, unknown>) => ComponentType;

/** Adapts existing owned slots; it does not introduce a second plugin host or tab store. */
export function bridgeMinervaWorkspace(
  host: BrowserPluginHost,
  scope: Scope,
  workspaceId: string,
  surface: Surface,
  name: (owner: string) => string,
) {
  const active = new Map<string, { entry: SlotEntry; dispose: () => void }>();
  const reconcile = () => {
    const next = new Map<string, { slot: SlotName; entry: SlotEntry }>();
    for (const slot of ["shell.canvas.tabs", "workspace.panels"] as const)
      for (const entry of host.slots.entries(slot, scope))
        next.set(`minerva.${slot}.${entry.ownerPluginId}.${entry.entryId}`, { slot, entry });
    for (const [id, existing] of active)
      if (next.get(id)?.entry !== existing.entry) {
        existing.dispose();
        active.delete(id);
      }
    for (const [id, { slot, entry }] of next) {
      if (active.has(id)) continue;
      const title = entry.title ?? name(entry.ownerPluginId);
      const icon = entry.icon ?? "plugin";
      const Surface = surface(slot, entry.entryId, { part: "body", placement: "right", selected: true, workspaceId });
      const unregisterKind = registerEditorKind({
        kind: id,
        title: () => title,
        icon: () => icon,
        keepAlive: true,
        component: () => <Surface />,
      });
      const unregisterItem = registerWorkspaceItem({
        id,
        title,
        icon,
        verbatim: true,
        input: { kind: id, key: id, data: {} },
      });
      active.set(id, {
        entry,
        dispose: () => {
          unregisterItem();
          unregisterKind();
          void editorService()
            .closeInput?.(id)
            .catch((error) => logError("Plugin workspace cleanup", error));
        },
      });
    }
  };
  const service = host.services.provide(workspaceServiceKey, scope, "minerva-ui", {
    open: async (entryId, ownerPluginId) => {
      const entries = [...active].filter(
        ([, { entry }]) => entry.entryId === entryId && (!ownerPluginId || entry.ownerPluginId === ownerPluginId),
      );
      if (!entries.length) throw Error(`Unknown workspace entry: ${entryId}`);
      if (entries.length !== 1) throw Error(`Ambiguous workspace entry: ${entryId}; specify ownerPluginId`);
      const [id] = entries[0]!;
      useWorkbenchChrome.setState({ workspaceOpen: true });
      await editorService().openInput({ kind: id, key: id, data: {} }, { focus: true });
    },
    openFile: async (path) => {
      useWorkbenchChrome.setState({ workspaceOpen: true });
      await editorService().openFile(path, { focus: true });
    },
    close: () => useWorkbenchChrome.setState({ workspaceOpen: false, workspaceMaximized: false }),
  });
  const subscription = host.slots.subscribe(reconcile);
  reconcile();
  return () => {
    subscription.dispose();
    service.dispose();
    for (const { dispose } of active.values()) dispose();
    active.clear();
  };
}
