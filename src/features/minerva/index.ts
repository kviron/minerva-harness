import { lazyComponent } from "@/ui";
import { registerSettingsPages } from "@/workbench/contributions";

export function activateSettingsPages() {
  return registerSettingsPages([
    {
      id: "agents.minerva",
      title: "Model Providers",
      group: "Agents",
      order: 480,
      keywords: ["api", "models", "minerva"],
      component: lazyComponent(() => import("./Settings").then((m) => m.MinervaSettings)),
    },
  ]);
}

export function activate() {
  return activateSettingsPages();
}
