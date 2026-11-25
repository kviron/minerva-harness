import { useRef, useState } from "react";
import { useTx } from "@/platform/i18n";
import { useMinervaUsage } from "@/platform/minervaUsage";
import { Popup } from "@/ui";
import { SubscriptionUsageView } from "@/workbench/SubscriptionUsage";
import { useEditorShown } from "@/workbench/contributions";
import { selectedSubscriptionUsage, shortestUsageWindow, subscriptionProviderId } from "../subscriptionUsage";
import styles from "./SubscriptionLimits.module.css";

/** Session entry point; the shared view receives only the usage controller's public data. */
export function SubscriptionLimits({ model }: { model: string | null }) {
  const tx = useTx();
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLButtonElement>(null);
  const shown = useEditorShown();
  const usage = useMinervaUsage(shown && subscriptionProviderId(model) !== null);
  const provider = selectedSubscriptionUsage(usage.providers, model);
  const window = provider ? shortestUsageWindow(provider.windows) : null;
  if (!provider) return null;
  const percent = window ? `${Math.round(window.usedPercent)}%` : "—";
  const stale = usage.error || provider.status === "unavailable";
  const description = window ? tx`${Math.round(window.usedPercent)}% used` : tx("Subscription limits are unavailable.");
  return (
    <>
      <button
        type="button"
        ref={anchor}
        className={styles.trigger}
        aria-label={tx("Subscription limits")}
        title={`${provider.providerName}: ${description}${stale ? ` · ${tx("Last known usage. Could not refresh.")}` : ""}`}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        {percent}
      </button>
      <Popup
        open={open && shown}
        anchor={anchor}
        onClose={() => setOpen(false)}
        placement="top-end"
        trapFocus
        aria-label={tx("Subscription limits")}
        className={styles.popup}
      >
        <SubscriptionUsageView {...usage} providers={[provider]} onRefresh={() => void usage.refresh()} />
      </Popup>
    </>
  );
}
