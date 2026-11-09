# FME embedding experiment — 2026-11-09

Worktree: `codex/fme-embed`. Target: the existing Windows x64 FME 3.7 client,
`C:\OEStudio\releases\trunk-3.7-c821c14a\bin\FME.exe`.
SHA-256: `C0B280D8C1AAAE5E15CDE5C3152F75AB33456AF9D50CC54FEFA747F10FEE4F83`.

Observed with Windows Computer Use: a real working form, toolbar and FME tab
strip rendered in the central panel while Minerva's sidebar and top bar stayed
visible (1280×860 and maximized desktop). The first blank-panel attempt had two
causes: selecting the `TApplication` helper instead of a form, and missing
`WS_CLIPSIBLINGS` on `WRY_WEBVIEW`. Real forms can have a Delphi owner, so filtering
all owned windows incorrectly removed them from the picker.

REA attached to the selected test client using Frida. Passive inspection found:

- `GetMenu(TMainFrame)` and `GetMenu(TApplication)` returned zero in this mode.
- Window property `Delphi...` identified the `TMainFrame` instance.
- Verified x64 VMT class-name pointer at −136 and instance size at −128.
- The frame referenced `TMainMenu`, whose root `TMenuItem` contained a live HMENU
  at +352. Native menu titles were Система, Сервис, Агенты, Окна, Справка.
- The implementation scans validated object fields instead of using instance
  addresses or a fixed frame-field offset. The compiler-layout offsets remain
  an explicit prototype limitation.
- Owner-drawn submenu captions live at `TMenuItem +120`; command IDs at +176;
  children at +192 reference `TList` (array +8, count +16). Runtime command IDs
  matched the native menu's `GetMenuItemID` values. Captions are mapped by ID,
  not by Delphi child-list position, since hidden children can exist.

REA evidence: `ev_e09597370c3bfc330dbb5ddec5531ef0323892b613d6984cda7540e0bc9ad163`
(Win32 menu state), `ev_4ffec3d8c3aae54137958033e15125e4018ce9cc6aecea6573febc43f3c17652`
(VMT), `ev_5707e789b682d26a4fccadf2d8947aa6fdcefe44cbf0ebdc3ebb1556aaabead2`
(object classes), `ev_9e89214c414c7ae765c34a69daaa9ae9cc04fdc299c7da80d21c805dea826a48`
(native menu handle and titles). These are runtime observations, not source-code
recovery. Instrumentation was detached after inspection; FME memory was not patched.
Additional caption/command evidence: `ev_68712d2f076232040a3cbd5d48b90b13c18b82f8ec53db4681ada60fcb27f702`,
`ev_82c539f26e8a2a9ab0ddd8009207e4800c5b7106fa8557ac286946250ef3602b`,
`ev_b08177a6b3a0ed28d0f5d7c84f0289de44af7e81d3a21a35e827480ebfc3b116`.

Final live observation: the embedded working form, FME toolbar/tab strip and all
five main-menu headings rendered together with Minerva's sidebar. The Help popup
opened with real captions, including Contents, Chat, Website and About. Popups
belong to the host process; leaf command IDs are forwarded to the original frame.
After attempting About, a `Could not convert variant of type (Null) into type
(OleStr)` message was visible in FME. Other operations were using that client
concurrently, so causality and command execution correctness remain unresolved.

Detach QA exposed FME recreating its native menu when styles are restored. The
prototype no longer restores an old HMENU: FME owns that lifecycle, and the
current menu is resolved again for each popup request. An earlier host instance
with this stale-menu bug was stopped only after native enumeration proved that
all FME windows had already been detached from it.

Checks passed: frontend production build; experimental native build; normal
desktop binary `cargo check`; tests for DPI/bounds, bounded remote reads, native
menu labels/disabled states; live window enumeration and Delphi-menu extraction.

## Whole-client tracking

The host now captures existing hidden and visible tabbed forms of the chosen
PID and watches for new top-level forms. Visibility and sibling z-order stay
under FME control. Layout preserves those states and only changes actual bounds
when needed; a single request is allowed in flight, including while a menu is open.

Live checks on the separate test process (PID 25048):

- Initial status reported 12 document/tool windows. Native enumeration found
  all of them plus the frame under the host, with no visible FME forms outside.
- The read-only price document, invoice journal, system card and project explorer
  rendered inside Minerva while switching views and resizing the host.
- Opening the SQL executor through FME's own menu command created further forms.
  The host captured them automatically and the status rose to 15. No SQL was run.
- Detach returned every FME form to its native desktop parent: the host had zero
  remaining FME children and the FME process stayed alive. Current visibility is
  preserved. A final pending poll is a no-op after detach, rather than a false error.

REA evidence: `ev_8d4a4f89fd9d511975eb84a00fddf412a1701e4c06a239d95aea8b12dd8e8db8`
(initial captured windows), `ev_08d32d68404566a00360799ef658f96a810349c794c597c5e1c4da5a67e00b7a`
(existing-window activation), `ev_bfb91fdcaf73287e3865e55a80c1a24d0052ce20bc488ccdf237c052d4587b07`
(open SQL editor command), `ev_a9786dbb174e7ab1d681a35f2f735b93f2f9af4579f096d5561ac059c08795d6`
(new forms inside), `ev_c43fd02f195880ea6fef542609d6d5f3d4316637ad9ac22e0b209c61fc805fa5`
(zero FME children after detach).

Eight native checks pass, including tab-form classification, dialog containment
without cropping, live per-process form coverage and menu extraction. The live
coverage test selects a real unembedded `TMainFrame`, not an arbitrary auxiliary
FME window from a concurrent process.

Final attach → detach → attach cycle completed without UI errors after the
pending-poll fix. Final status reported 14 document/tool windows; native inspection
found 15 FME children (14 forms plus the frame) and zero visible FME forms outside
the host: `ev_4c25e369af925fcac510eface6fc607a3055ded679ad4044ca626e5414bc05ef`.
The selected test FME and the separate original FME process remained alive.

Full keyboard/modal-dialog behavior, all business menu commands, multi-monitor
DPI changes and crash recovery still require further live QA.

## Compact header

Removed the FME-view breadcrumbs/top bar, heading, helper text and persistent
connection status. The native-menu row stays 36 CSS pixels high; the connection
control is a 159×36 translucent, blurred panel at its upper-right corner. The
control stays above the native HWND area, so the FME windows cannot obscure it.
PID options are deduplicated; refresh/connect/disconnect actions use labeled icons.
Connection errors retain a visible, dismissible alert rather than a routine row.

Frontend build and live WebView2 checks passed at 1920 and 700 CSS pixels: no
header/control overlap or horizontal overflow, working selector keyboard focus,
and detach/reattach to the test client. Other views retain their normal top bar.
Evidence: `agent-artifacts/compact-header-check.json`,
`agent-artifacts/compact-header-states.json` and matching WebView screenshots
(screenshots contain the HTML UI only, not the foreign native HWND pixels).
 

## Keeping the native parent across views

Navigation now keeps FME attached. Inactive forms move outside the host client
area without changing parent, native visibility or tab order. Synchronization
continues to capture new forms. The connection is recovered after a webview or
keyed chat remount; the selected shell view lives above the keyed chat.
Explicit detach and graceful host close still restore the original parent.

Validated in the running experiment against test FME PID 25048: all 19 native
forms retained host HWND 333450 across Plugins and chat navigation, moved outside
the visible area while away, and returned when opening East Express again.
A webview reload retained the same HWNDs and parents with no frontend errors.
Evidence: `agent-artifacts/view-retention.json`, `view-retention.cjs`,
`retention-chat.cjs`. Six native unit tests and the frontend build passed.
Modal-dialog behavior while away and other DPI settings were not retested.
