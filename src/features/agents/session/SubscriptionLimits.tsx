import { useRef, useState } from "react";
import { useI18n, useTx } from "@/platform/i18n";
import { useMinervaUsage } from "@/platform/minervaUsage";
import { Popup } from "@/ui";
import { balanceLabel, SubscriptionUsageView } from "@/workbench/SubscriptionUsage";
import { useEditorShown } from "@/workbench/contributions";
import { selectedSubscriptionUsage, shortestUsageWindow, subscriptionProviderId } from "../subscriptionUsage";
import styles from "./SubscriptionLimits.module.css";

/** Session entry point; the shared view receives only the usage controller's public data. */
export function SubscriptionLimits({ model }: { model: string | null }) {
  const tx = useTx();
  const locale = useI18n((state) => state.locale);
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const shown = useEditorShown();
  const usage = useMinervaUsage(shown && subscriptionProviderId(model) !== null);
  const provider = selectedSubscriptionUsage(usage.providers, model);
  const window =
    (model?.endsWith(":free") && provider?.windows.find((entry) => entry.id === "free")) ||
    (provider ? shortestUsageWindow(provider.windows) : null);
  if (!provider || (!window && !provider.balance)) return null;
  const summary = provider.balance
    ? balanceLabel(provider.balance, locale)
    : tx`${Math.round(100 - window!.usedPercent)}% remaining`;
  const stale = usage.error || provider.status === "unavailable";
  return (
    <>
      <button
        type="button"
        ref={anchor}
        className={styles.trigger}
        aria-label={tx("Provider limits")}
        title={`${provider.providerName}: ${summary}${stale ? ` · ${tx("Last known usage. Could not refresh.")}` : ""}`}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {window?.id === "free"
          ? tx("Weekly free allowance") + " · "
          : window?.id === "pass"
            ? tx("Weekly Pass allowance") + " · "
            : ""}
        {summary}
      </button>
      <Popup
        open={open && shown}
        anchor={anchor}
        onClose={() => setOpen(false)}
        placement="top-end"
        trapFocus
        aria-label={tx("Provider limits")}
        className={styles.popup}
      >
        <SubscriptionUsageView {...usage} providers={[provider]} onRefresh={() => void usage.refresh()} />
      </Popup>
    </>
  );
}
