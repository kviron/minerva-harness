import { lazyComponent } from "@/ui";
import { registerSettingsPages } from "@/workbench/contributions";

export function activateSettingsPages() {
  return registerSettingsPages([
    {
      id: "agents.minerva",
      title: "Minerva",
      group: "Agents",
      order: 480,
      keywords: ["api", "models", "providers", "minerva", "codex"],
      component: lazyComponent(() => import("./Settings").then((m) => m.MinervaSettings)),
    },
  ]);
}

export function activate() {
  return activateSettingsPages();
}
