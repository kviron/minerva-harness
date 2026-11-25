import { useState } from "react";
import type { MinervaKnownProvider, MinervaProvider } from "@/platform/minerva";
import { confirm } from "@/workbench/messages";
import { Dialog } from "@/ui";
import { tx } from "@/platform/i18n";
import styles from "./Settings.module.css";
import { ApiProviderEditor } from "./ApiProviderEditor";
export function AddProviderDialog({
  known,
  onSaved,
  onClose,
}: {
  known: MinervaKnownProvider[];
  onSaved: (rows: MinervaProvider[]) => void;
  onClose: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [dirty, setDirty] = useState(false);
  const close = async () => {
    if (busy) return;
    if (
      !dirty ||
      (await confirm({
        title: tx("Discard changes?"),
        message: tx("Your unsaved provider changes will be discarded."),
        ok: tx("Discard"),
      }))
    )
      onClose();
  };
  return (
    <Dialog open title={tx("Add model provider…")} onClose={() => void close()} width={650} maxHeight={650}>
      <div className={styles.dialogBody}>
        <ApiProviderEditor
          provider={null}
          known={known}
          onSaved={onSaved}
          onCancel={() => void close()}
          onDirty={setDirty}
          onBusy={setBusy}
        />
      </div>
    </Dialog>
  );
}
