/** Global shortcuts belong only to the visible editor containing keyboard focus. */
export function ownsEditorShortcut(root: HTMLElement | null, target: EventTarget | null): boolean {
  return (
    !!root && root.getClientRects().length > 0 && target instanceof Node && root.contains(target)
  );
}
