export type IconContext = { path: string; directory: boolean; expanded?: boolean; mime?: string };
/** Register a pack resolver; returning undefined lets the next pack or default handle the entry. */
export function createIconRegistry<T>() {
  const resolvers = new Set<(entry: IconContext) => T | undefined>();
  const listeners = new Set<() => void>();
  let version = 0;
  const changed = () => {
    version++;
    for (const listener of listeners) listener();
  };
  return {
    register(resolve: (entry: IconContext) => T | undefined) {
      resolvers.add(resolve);
      changed();
      return {
        dispose() {
          if (resolvers.delete(resolve)) changed();
        },
      };
    },
    resolve(entry: IconContext): T | undefined {
      for (const resolver of resolvers) {
        const icon = resolver(entry);
        if (icon !== undefined) return icon;
      }
      return undefined;
    },
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    snapshot: () => version,
  };
}
