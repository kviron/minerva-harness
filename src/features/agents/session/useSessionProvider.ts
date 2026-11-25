import { useEffect } from "react";
import type { AgentConfigOption } from "@/bindings";
import { refreshTaskModels, useAgents } from "../store";

const API_PROVIDER_PREFIX = "minerva.api.";

export function useSessionProvider(
  taskId: string,
  providerId: string,
  model: string | null,
  config: AgentConfigOption[],
) {
  const providers = useAgents((state) => state.providers);
  const embedded = providerId === "minerva" || providerId.startsWith(API_PROVIDER_PREFIX);
  useEffect(() => {
    if (embedded) void refreshTaskModels(taskId, false);
  }, [taskId, providerId, embedded]);
  if (!embedded) return null;
  const provider = providers.find((provider) => provider.id === "minerva");
  const option = config.find((option) => option.category === "model");
  const selected = model ?? option?.current;
  return {
    value: "minerva",
    name: provider?.name ?? "Minerva",
    options: provider?.installed ? [{ value: "minerva", label: provider.name }] : [],
    modelAvailable: option === undefined ? selected != null : option.values.some((value) => value.value === selected),
  };
}
