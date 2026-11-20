import { useProject } from "@/platform/project";
import { minervaWorkspace } from "@/platform/minerva";
import { startMinervaPlugins } from "@/platform/minervaPlugins";
import { logError } from "@/platform/errorLog";
import { lazyComponent } from "@/ui";
import { registerSettingsPages } from "@/workbench/contributions";

export function activateSettingsPages() {
  return registerSettingsPages([
    {
      id: "agents.minerva",
      title: "Model Providers",
      group: "Agents",
      order: 480,
      keywords: ["api", "models", "plugins", "minerva"],
      component: lazyComponent(() => import("./Settings").then((m) => m.MinervaSettings)),
    },
  ]);
}

export function activate() {
  let disposed = false;
  let generation = 0;
  let cleanup: (() => void) | undefined;
  const sync = () => {
    const project = useProject.getState().project;
    const token = ++generation;
    cleanup?.();
    cleanup = undefined;
    if (!project?.trusted) return;
    void minervaWorkspace()
      .then(async (workspace) => {
        if (disposed || token !== generation) return;
        const stop = await startMinervaPlugins(workspace.id);
        if (disposed || token !== generation) stop();
        else cleanup = stop;
      })
      .catch((error) => {
        if (!disposed && token === generation) logError("Minerva workspace", error);
      });
  };
  sync();
  const unsubscribe = useProject.subscribe((next, previous) => {
    if (next.project?.root !== previous.project?.root || next.project?.trusted !== previous.project?.trusted) sync();
  });
  const settings = activateSettingsPages();
  return () => {
    disposed = true;
    generation++;
    cleanup?.();
    unsubscribe();
    settings();
  };
}
