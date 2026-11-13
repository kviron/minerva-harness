type Entry<T> = { value: Promise<T>; refs: number; closing?: Promise<void> };
/** Owns document lifetime independently of the panels displaying it. */
export class DocumentPool<T> {
  private entries = new Map<string, Entry<T>>();
  private disposed = false;
  async acquire(key: string, load: () => Promise<T>): Promise<T> {
    if (this.disposed) throw new Error('Editor disabled');
    let entry = this.entries.get(key);
    if (entry?.closing) {
      await entry.closing;
      return this.acquire(key, load);
    }
    if (!entry) {
      entry = {
        value: Promise.resolve().then(() => {
          if (this.disposed) throw new Error('Editor disabled');
          return load();
        }),
        refs: 0,
      };
      this.entries.set(key, entry);
    }
    entry.refs++;
    try {
      const value = await entry.value;
      if (this.disposed) throw new Error('Editor disabled');
      return value;
    } catch (error) {
      if (this.entries.get(key) === entry && --entry.refs === 0 && !entry.closing)
        this.entries.delete(key);
      throw error;
    }
  }
  async release(key: string, dispose: (value: T) => Promise<void>): Promise<void> {
    const entry = this.entries.get(key);
    if (!entry || entry.closing) return;
    if (--entry.refs > 0) return;
    entry.closing = entry.value.then(dispose).finally(() => {
      if (this.entries.get(key) === entry) this.entries.delete(key);
    });
    await entry.closing;
  }
  dispose(dispose: (value: T) => Promise<void>): Promise<void> {
    this.disposed = true;
    const pending = [...this.entries.values()].map(
      (entry) => entry.closing ?? (entry.closing = entry.value.then(dispose, () => {})),
    );
    return Promise.allSettled(pending).then(() => {
      this.entries.clear();
    });
  }
}
