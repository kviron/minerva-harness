# Minerva source snapshot

Imported from `C:\dev\minerva-harness` at commit
`d3613ef9eccecca1282cb35472accd20b8901dad` on 2026-11-20.

This is a tracked source snapshot, not a second running application. The IDE
links `crates/runtime` and its backend plugins as Rust dependencies; the legacy
application UI is not used. The plugin SDK is reused for lifecycle and package
UI contracts, with OpenIDEA components supplied by the new host.

Integration changes: rusqlite 0.41 for the IDE's libsqlite3-sys 0.39; typed backup
callback and bounded integer revision conversion; strict indexed-access fixes
in the browser SDK. Keep these changes when refreshing the snapshot.

## Дополнение SDK рабочей области

В `packages/plugin-sdk/src/slots.types.ts` добавлены необязательные `title`/`icon`,
в `workspace.ts` — типизированный scoped service навигации; он экспортирован из `index.ts`.
Загрузка, ownership и RPC SDK сохранены. Адаптер находится в новом приложении:
`src/platform/minervaWorkspaceUi.tsx`; контракт — `docs/minerva-workspace.md`.
