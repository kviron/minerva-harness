import { useState, useEffect } from "react";
import { Button, Input, T } from "@/ui";
import { tx } from "@/platform/i18n";
import { type MinervaKnownProvider, type MinervaProvider } from "@/platform/minerva";
import { useProviderEditor } from "./useProviderEditor";
import { ProviderConnectionFields } from "./ProviderConnectionFields";
import { ProviderModels } from "./ProviderModels";
import styles from "./Settings.module.css";
export function ApiProviderEditor({
  provider,
  known,
  onSaved,
  onCancel,
  onDirty,
  onBusy,
}: {
  provider: MinervaProvider | null;
  known: MinervaKnownProvider[];
  onSaved: (providers: MinervaProvider[]) => void;
  onCancel: () => void;
  onDirty: (dirty: boolean) => void;
  onBusy?: (busy: boolean) => void;
}) {
  const editor = useProviderEditor(provider);
  const { draft, setDraft, busy, error, editing, fetching } = editor;
  useEffect(() => onBusy?.(busy), [busy, onBusy]);
  const [newModel, setNewModel] = useState("");

  useEffect(() => onDirty(editor.dirty), [editor.dirty, onDirty]);
  const closeForm = () => {
    editor.reset();
    setNewModel("");
    onCancel();
  };
  const moveModel = (from: number, to: number) => {
    if (from === to || to < 0 || to >= draft.models.length) return;
    const models = [...draft.models];
    models.splice(to, 0, models.splice(from, 1)[0]!);
    setDraft({ ...draft, models });
  };
  return (
    <form
      className={styles.form}
      onSubmit={(event) => {
        event.preventDefault();
        void editor.save(onSaved);
      }}
    >
      <ProviderConnectionFields draft={draft} onChange={setDraft} known={known} busy={busy} editing={editing} />
      <div className={styles.row}>
        <span>
          <T>Models</T>
        </span>
        <Button disabled={busy || !draft.id.trim() || !draft.baseUrl.trim()} onClick={() => void editor.discover()}>
          {fetching ? <T>Loading models…</T> : <T>Fetch model list</T>}
        </Button>
      </div>
      {error && <p role="alert">{error}</p>}
      <ProviderModels
        models={draft.models}
        windows={draft.modelWindows}
        onWindowChange={(model, size) => {
          const modelWindows = { ...draft.modelWindows };
          const next = { ...modelWindows[model] };
          if (size === undefined) delete next.limit;
          else next.limit = size;
          if (Object.keys(next).length) modelWindows[model] = next;
          else delete modelWindows[model];
          setDraft({ ...draft, modelWindows });
        }}
        busy={busy}
        onMove={moveModel}
        onRemove={(index) => {
          const modelWindows = { ...draft.modelWindows };
          delete modelWindows[draft.models[index]!];
          setDraft({ ...draft, modelWindows, models: draft.models.filter((_, i) => i !== index) });
        }}
      />
      <div className={styles.row}>
        <Input
          aria-label={tx("Model ID")}
          placeholder={tx("Model ID")}
          disabled={busy}
          value={newModel}
          onChange={(event) => setNewModel(event.target.value)}
        />
        <Button
          disabled={busy || !newModel.trim() || draft.models.includes(newModel.trim())}
          onClick={() => {
            setDraft({ ...draft, models: [...draft.models, newModel.trim()] });
            setNewModel("");
          }}
        >
          <T>Add model</T>
        </Button>
      </div>
      <div className={styles.row}>
        <Button type="submit" disabled={busy}>
          <T>Save</T>
        </Button>
        <Button disabled={busy} onClick={closeForm}>
          <T>Cancel</T>
        </Button>
      </div>
    </form>
  );
}
