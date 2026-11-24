import { useEffect } from "react";
import { refreshTaskModels, useAgents } from "../store";

const API_PROVIDER_PREFIX = "minerva.api.";

export function useSessionProvider(taskId: string, providerId: string, model: string | null) {
  const providers = useAgents((state) => state.providers);
  const embedded = providerId === "minerva" || providerId.startsWith(API_PROVIDER_PREFIX);
  useEffect(() => {
    if (embedded) void refreshTaskModels(taskId, false);
  }, [taskId, providerId, embedded]);
  if (!embedded) return null;
  const available = providers.filter((provider) => provider.id.startsWith(API_PROVIDER_PREFIX) && provider.installed);
  const routeProvider = available.find((provider) =>
    model?.startsWith(`api:${provider.id.slice(API_PROVIDER_PREFIX.length)}/`),
  );
  return {
    value: providerId,
    name: providers.find((provider) => provider.id === providerId)?.name ?? providerId,
    options: available.map((provider) => ({ value: provider.id, label: provider.name })),
    modelAvailable: !!routeProvider && (providerId === "minerva" || routeProvider.id === providerId),
  };
}
