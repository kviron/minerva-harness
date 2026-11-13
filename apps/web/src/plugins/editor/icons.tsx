import React, { useSyncExternalStore } from 'react';
import { File, FileImage, FileText, Folder, FolderOpen } from 'lucide-react';
import { createIconRegistry, type IconContext } from './iconRegistry';

export const editorIcons = createIconRegistry<React.ReactNode>();
export function EntryIcon({ size = 15, ...entry }: IconContext & { size?: number }) {
  useSyncExternalStore(editorIcons.subscribe, editorIcons.snapshot, editorIcons.snapshot);
  const custom = editorIcons.resolve(entry);
  if (custom !== undefined)
    return (
      <span className="editor-custom-icon" aria-hidden="true">
        {custom}
      </span>
    );
  const Icon = entry.directory
    ? entry.expanded
      ? FolderOpen
      : Folder
    : entry.mime === 'application/pdf'
      ? FileText
      : entry.mime?.startsWith('image/')
        ? FileImage
        : File;
  return <Icon size={size} aria-hidden="true" />;
}
