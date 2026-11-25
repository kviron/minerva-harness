import { Button, ProgressBar, T } from "@/ui";
import { tx, useTx, useI18n } from "@/platform/i18n";
import type { MinervaUsageProvider, MinervaUsageWindow } from "@/platform/minervaUsageTypes";
import styles from "./SubscriptionUsage.module.css";

function windowLabel(window: MinervaUsageWindow): string {
  if (window.windowSeconds) {
    const hours = window.windowSeconds / 3600;
    return hours < 24 ? tx`${hours} hours` : tx`${hours / 24} days`;
  }
  return tx(
    (
      {
        weekly: "Weekly",
        monthly: "Monthly",
        "5h": "5 hours",
        primary: "Primary window",
        secondary: "Secondary window",
      } as Record<string, string>
    )[window.id] ?? "Usage window",
  );
}
const errors = {
  auth: "Authorization expired. Check the provider connection.",
  timeout: "Usage request timed out.",
  "rate-limited": "Usage requests are temporarily limited.",
  network: "Could not connect to the usage service.",
  "invalid-response": "The provider returned no readable usage data.",
} as const;

function UsageCard({ provider }: { provider: MinervaUsageProvider }) {
  const t = useTx();
  const locale = useI18n((state) => state.locale);
  return (
    <article className={styles.card} aria-label={provider.providerName}>
      <div className={styles.header}>
        <strong>{provider.providerName}</strong>
        {provider.updatedAt !== null && (
          <span>{t`Updated ${new Date(provider.updatedAt).toLocaleString(locale)}`}</span>
        )}
      </div>
      {provider.status === "unavailable" && provider.windows.length > 0 && (
        <p role="status">
          <T>Last known usage. Could not refresh.</T>
        </p>
      )}
      {provider.status === "not-configured" ? (
        <p>
          <T>Connect this provider to see subscription limits.</T>
        </p>
      ) : provider.status !== "ok" && provider.windows.length === 0 ? (
        <p role="status">
          {t(provider.errorCode ? errors[provider.errorCode] : "Subscription limits are unavailable.")}
        </p>
      ) : provider.windows.length === 0 ? (
        <p>
          <T>The provider returned no readable usage data.</T>
        </p>
      ) : (
        provider.windows.map((window) => (
          <div className={styles.window} key={window.id}>
            <div className={styles.header}>
              <span>{windowLabel(window)}</span>
              <span>{t`${Math.round(window.usedPercent)}% used`}</span>
            </div>
            <ProgressBar
              className={styles.progress}
              value={window.usedPercent / 100}
              aria-label={`${provider.providerName}: ${windowLabel(window)}`}
            />
            <span className={styles.reset}>
              {window.resetAt === null
                ? t("Reset time unavailable")
                : t`Resets ${new Date(window.resetAt).toLocaleString(locale)}`}
            </span>
          </div>
        ))
      )}
    </article>
  );
}

export interface SubscriptionUsageViewProps {
  providers: MinervaUsageProvider[];
  loading: boolean;
  error: boolean;
  onRefresh: () => void;
}
export function SubscriptionUsageView({ providers, loading, error, onRefresh }: SubscriptionUsageViewProps) {
  return (
    <section className={styles.body} aria-label={tx("Subscription limits")} aria-busy={loading}>
      <div className={styles.header}>
        <h3>
          <T>Subscription limits</T>
        </h3>
        <Button disabled={loading} onClick={onRefresh}>
          <T>Refresh limits</T>
        </Button>
      </div>
      {loading && providers.length === 0 && (
        <p role="status">
          <T>Loading subscription limits…</T>
        </p>
      )}
      {error && (
        <p role="alert">
          {providers.length > 0 ? (
            <T>Last known usage. Could not refresh.</T>
          ) : (
            <T>Could not load subscription limits. Try refreshing.</T>
          )}
        </p>
      )}
      {!loading && !error && providers.length === 0 && (
        <p>
          <T>Subscription limits are unavailable.</T>
        </p>
      )}
      {providers.map((provider) => (
        <UsageCard provider={provider} key={provider.providerId} />
      ))}
    </section>
  );
}
