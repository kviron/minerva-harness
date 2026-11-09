# FME inside Tauri (Windows experiment)

This separate binary starts the Minerva shell with its sidebar and top bar,
using the isolated `ru.oestudio.rig.fme-experiment` application profile. It hosts
the existing FME main menu and all tabbed forms of one selected FME process in
the central area, including forms opened after attachment.
It does not create a new FME database connection.

From this worktree, install dependencies with `npm ci`. In two terminals:

```powershell
npm run dev -- --port 1427 --strictPort
```

```powershell
cargo run -p oe-rig-prototype --bin fme-embed
```

Open FME normally, select its PID in the compact glass control at the upper right,
then click the plug icon (**Встроить клиент**). Each process appears once in the
picker; the whole client is attached. The unplug icon detaches it. If FME is
minimized and the list is empty, use **Показать FME** (window icon) first.
The `TApplication` helper is excluded; `TMainFrame` is attached above the forms.
Existing hidden document/tool forms are captured without showing them. A bounded
150 ms synchronization loop captures new forms, removes destroyed forms and fixes
their bounds only when necessary. FME retains native visibility, focus and tab
z-order; synchronization does not raise every form or force inactive forms open.
Native main-menu titles appear above the embedded area. Submenus are rebuilt
as host-owned native popups with FME's command IDs, check marks and disabled
states; the selected command is dispatched back to FME. App-specific menu icons
and owner-drawn styling are not copied.
When `GetMenu` returns zero, the prototype reads the existing Delphi menu object
through `ReadProcessMemory`. It never writes FME memory. The x64 VMT and menu-item
offsets were verified with REA against this FME 3.7 build; another compiler/client
build needs revalidation. Class names, object sizes and `IsMenu` are checked before
the handle is used. `PROCESS_VM_READ` access is needed for this fallback.
Owner-drawn submenu captions are read from the validated `TMenuItem` tree and
matched by native command IDs; invisible Delphi items do not shift that mapping.
**Отсоединить** restores original styles, parent, owner and placement while
preserving current visibility. Closing
the host also detaches; it refuses to close if restoration fails. Reloading the
webview recovers the existing connection. Switching to chat
or plugins keeps FME attached and moves its child windows outside the visible
host area. Returning restores their bounds without changing parents or native
tab visibility; newly created forms continue to be captured while away.

Check: keyboard and menus, a harmless modal dialog, resize, minimize/restore,
125–150% display scaling and multiple monitors, explicit detach and host close.
Attach/detach must preserve the FME process and its current session.

Limits: Windows only; one selected process. Tabbed forms are recognized by their
Delphi window class and normal resizable/maximizable window styles. Popup dialogs
keep their native modality and ownership; dialogs that fit are centered inside
the document area if they appear outside it. Oversized dialogs are not clipped.
Native child windows overlay web content, so
HTML overlays cannot cover FME. Cross-process DPI reparenting can reset FME's DPI
awareness (Windows behavior); original DPI awareness is not restored by detach.
An abnormal host termination is not covered by the normal detach handler.
Do not terminate the host forcibly while FME is attached. Use both apps with the
same elevation. A modal dialog already open blocks attachment.

API reference: https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-setparent
