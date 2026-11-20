import type { ServiceKey } from './services.ts';

/** Workspace UI uses the host's native editor, tabs and file lifecycle. */
export interface WorkspaceService {
  open(entryId: string, ownerPluginId?: string): Promise<void>;
  openFile(path: string): Promise<void>;
  close(): void;
}
export const workspaceServiceKey: ServiceKey<WorkspaceService> = {
  name: 'minerva.workspace',
  version: 1,
};
