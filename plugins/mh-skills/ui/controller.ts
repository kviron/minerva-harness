import { Store } from '../../../packages/client-models/src/store';

export type Skill = {
  name: string;
  description: string;
  providerId: string;
  instructionPath?: string;
  modelInvocable: boolean;
  userInvocable: boolean;
};
export type Catalog = { items: Skill[]; incomplete: boolean; diagnostics: string[] };
type Snapshot = { catalog: Catalog | null; error: string; loading: boolean };
type Entry = { store: Store<Snapshot>; requested: boolean; revision: number };

export class SkillsController {
  private entries = new Map<string | undefined, Entry>();
  private disposed = false;
  constructor(private fetchCatalog: (workspaceId?: string) => Promise<Catalog>) {}
  catalog(workspaceId?: string) {
    let entry = this.entries.get(workspaceId);
    if (!entry) {
      entry = {
        store: new Store<Snapshot>({ catalog: null, error: '', loading: false }),
        requested: false,
        revision: 0,
      };
      this.entries.set(workspaceId, entry);
    }
    return entry.store;
  }
  ensure(workspaceId?: string) {
    if (this.disposed) return;
    this.catalog(workspaceId);
    if (!this.entries.get(workspaceId)!.requested) void this.refresh(workspaceId);
  }
  async refresh(workspaceId?: string) {
    if (this.disposed) return;
    const store = this.catalog(workspaceId),
      entry = this.entries.get(workspaceId)!;
    entry.requested = true;
    const revision = ++entry.revision;
    store.set({ ...store.getSnapshot(), error: '', loading: true });
    try {
      const catalog = await this.fetchCatalog(workspaceId);
      if (!this.disposed && revision === entry.revision)
        store.set({ catalog, error: '', loading: false });
    } catch (reason) {
      if (!this.disposed && revision === entry.revision)
        store.set({ ...store.getSnapshot(), error: String(reason), loading: false });
    }
  }
  dispose() {
    this.disposed = true;
    for (const { store } of this.entries.values()) store.clear(store.getSnapshot());
    this.entries.clear();
  }
}
