import { useMinervaUsage } from "@/platform/minervaUsage";
import { SubscriptionUsageView } from "@/workbench/SubscriptionUsage";
export function SubscriptionUsage({ providerId }: { providerId: string }) {
  const usage = useMinervaUsage();
  return (
    <SubscriptionUsageView
      {...usage}
      providers={usage.providers.filter((provider) => provider.providerId === providerId)}
      onRefresh={() => void usage.refresh()}
    />
  );
}
