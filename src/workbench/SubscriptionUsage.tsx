import { Button, ProgressBar, T } from "@/ui";
import { tx, useTx, useI18n } from "@/platform/i18n";
import type { MinervaUsageProvider, MinervaUsageWindow } from "@/platform/minervaUsageTypes";
import styles from "./SubscriptionUsage.module.css";

function windowLabel(window: MinervaUsageWindow): string {
  if (window.id === "pass") return tx("Weekly Pass allowance");
  if (window.id === "free") return tx("Weekly free allowance");
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
export function hasUsage(provider: MinervaUsageProvider) {
  return provider.windows.length > 0 || provider.balance != null;
}

export function balanceLabel(balance: NonNullable<MinervaUsageProvider["balance"]>, locale: string) {
  if (balance.unit === "unlimited") return tx("Unlimited");
  if (balance.unit === "usd")
    return new Intl.NumberFormat(locale, { style: "currency", currency: "USD", maximumFractionDigits: 6 }).format(
      balance.amount,
    );
  return tx`${new Intl.NumberFormat(locale).format(balance.amount)} balance tokens`;
}

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
      {provider.status === "unavailable" && (
        <p role="status">
          <T>Last known usage. Could not refresh.</T>
        </p>
      )}
      {provider.balance && (
        <div className={styles.header}>
          <span>{t("Remaining balance")}</span>
          <strong>{balanceLabel(provider.balance, locale)}</strong>
        </div>
      )}
      {provider.windows.map((window) => (
        <div className={styles.window} key={window.id}>
          <div className={styles.header}>
            <span>{windowLabel(window)}</span>
            <span>{t`${Math.round(100 - window.usedPercent)}% remaining`}</span>
          </div>
          <ProgressBar
            className={styles.progress}
            value={1 - window.usedPercent / 100}
            aria-label={`${provider.providerName}: ${windowLabel(window)}`}
          />
          <span className={styles.reset}>
            {window.resetAt === null
              ? t("Reset time unavailable")
              : t`Resets ${new Date(window.resetAt).toLocaleString(locale)}`}
          </span>
        </div>
      ))}
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
  const observations = providers.filter(hasUsage);
  if (observations.length === 0) return null;
  return (
    <section className={styles.body} aria-label={tx("Provider limits")} aria-busy={loading}>
      <div className={styles.header}>
        <h3>
          <T>Provider limits</T>
        </h3>
        <Button disabled={loading} onClick={onRefresh}>
          <T>Refresh limits</T>
        </Button>
      </div>
      {error && (
        <p role="status">
          <T>Last known usage. Could not refresh.</T>
        </p>
      )}
      {observations.map((provider) => (
        <UsageCard provider={provider} key={provider.providerId} />
      ))}
    </section>
  );
}
