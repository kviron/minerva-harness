import * as React from "react";
import * as jsxRuntime from "react/jsx-runtime";
import * as ReactDOM from "react-dom";
import * as ReactDOMClient from "react-dom/client";
import * as ui from "@/ui";
import { BrowserPluginHost } from "../../vendor/minerva/packages/plugin-sdk/src/context";
import {
  ExternalClientModules,
  type ClientModuleFactory,
} from "../../vendor/minerva/packages/plugin-sdk/src/client-modules";
import { SlotRenderer } from "../../vendor/minerva/packages/plugin-sdk/src/renderer";
import type { SlotName } from "../../vendor/minerva/packages/plugin-sdk/src/slots.types";
import { registerSettingsPages, registerToolWindow, notifications } from "@/workbench/contributions";
import { minervaCall, type MinervaPlugin } from "./minerva";
import { logError } from "./errorLog";
import { bridgeMinervaWorkspace } from "./minervaWorkspaceUi";
import { minervaWidgets } from "./minervaWidgets";
import styles from "./minervaPlugins.module.css";

declare module "../../vendor/minerva/packages/plugin-sdk/src/context-menus" {
  interface ContextMenuTargetMap {
    workspace: { id: string; path: string | null };
    session: { id: string; workspaceId: string };
    file: { path: string; workspaceId: string };
  }
}

async function compile(source: string): Promise<ClientModuleFactory> {
  const url = URL.createObjectURL(
    new Blob([`export default function(require,module,exports){\n${source}\n}`], { type: "text/javascript" }),
  );
  try {
    return (await import(/* @vite-ignore */ url)).default;
  } finally {
    URL.revokeObjectURL(url);
  }
}

/** Keeps the package SDK; only the surfaces and shared widgets belong to the new IDE. */
export async function startMinervaPlugins(workspaceId: string): Promise<() => void> {
  const host = new BrowserPluginHost();
  await host.activate([{ id: "minerva-ui", apply() {} }]);
  const scope = { kind: "workspace" as const, workspaceId };
  const loader = new ExternalClientModules({
    host,
    workspaceId,
    compile,
    modules: {
      react: React,
      "react/jsx-runtime": jsxRuntime,
      "react-dom": ReactDOM,
      "react-dom/client": ReactDOMClient,
      "@minerva/ui": minervaWidgets,
    },
    load: (id) => minervaCall<string>("plugins.ui", { id }),
    rpc: (id, method, params) => minervaCall("plugins.call", { id, method, params }),
  });
  let disposed = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let contributions: Array<() => void> = [];
  let pluginNames = new Map<string, string>();
  const disable = async (id: string) => {
    await minervaCall("plugins.setEnabled", { id, enabled: false });
    await refresh();
  };
  const surface = (slot: SlotName, entryId: string, props: Record<string, unknown>) =>
    function PluginSurface() {
      return (
        <div className={styles.surface}>
          <SlotRenderer
            registry={host.slots}
            scope={scope}
            slot={slot}
            entryId={entryId}
            componentProps={props}
            onOpenSettings={() => {}}
            onDisable={disable}
            renderError={(error) => (
              <section role="alert">
                <p>{error.error}</p>
                <ui.Button onClick={() => void error.onDisable()}>
                  <ui.T>Disable plugin</ui.T>
                </ui.Button>
              </section>
            )}
          />
        </div>
      );
    };
  const reconcile = () => {
    contributions.reverse().forEach((dispose) => dispose());
    contributions = [];
    if (disposed) return;
    for (const entry of host.slots.entries("settings.pages", scope)) {
      contributions.push(
        registerSettingsPages([
          {
            id: `minerva.plugin.${entry.ownerPluginId}.${entry.entryId}`,
            title: pluginNames.get(entry.ownerPluginId) ?? entry.ownerPluginId,
            group: "Minerva",
            component: surface("settings.pages", entry.entryId, {
              placement: "body",
              selectedPage: entry.entryId,
              onSelectPage() {},
            }),
          },
        ]),
      );
    }
    for (const slot of ["shell.sections"] as const) {
      for (const entry of host.slots.entries(slot, scope)) {
        const id = `minerva.${slot}.${entry.ownerPluginId}.${entry.entryId}`;
        contributions.push(
          registerToolWindow({
            id,
            title: pluginNames.get(entry.ownerPluginId) ?? entry.ownerPluginId,
            verbatim: true,
            icon: "toolwindow-services",
            anchor: "right",
            component: surface(slot, entry.entryId, { part: "body", placement: "body", selected: true, workspaceId }),
          }),
        );
      }
    }
  };
  const workspace = bridgeMinervaWorkspace(
    host,
    scope,
    workspaceId,
    surface,
    (owner) => pluginNames.get(owner) ?? owner,
  );
  const subscription = host.slots.subscribe(reconcile);
  const errors = loader.subscribe(() => {
    for (const error of loader.getSnapshot()) {
      notifications().notify({
        level: "error",
        title: "Minerva plugin",
        body: `${error.id}: ${error.message}`,
        key: `minerva-plugin:${error.id}`,
        verbatim: true,
      });
    }
  });
  let refreshing = false;
  async function refresh() {
    if (disposed || refreshing) return;
    refreshing = true;
    try {
      const plugins = await minervaCall<MinervaPlugin[]>("plugins.list");
      if (!disposed) {
        pluginNames = new Map(plugins.map((plugin) => [plugin.id, plugin.name]));
        await loader.sync(plugins);
      }
    } catch (error) {
      if (!disposed) logError("Minerva plugins", error);
    } finally {
      refreshing = false;
      if (!disposed) {
        clearTimeout(timer);
        timer = setTimeout(() => void refresh(), 2000);
      }
    }
  }
  void refresh();
  return () => {
    disposed = true;
    clearTimeout(timer);
    subscription.dispose();
    workspace();
    errors();
    contributions.reverse().forEach((dispose) => dispose());
    contributions = [];
    void loader
      .dispose()
      .then(() => host.dispose())
      .catch((error) => logError("Minerva plugin cleanup", error));
  };
}
