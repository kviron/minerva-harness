---
name: minerva-acceptance
description: Verify changes to Minerva delegation, child chat recovery, approvals, or the Windows preview in OpenIDEA.
---

# Minerva acceptance

Before editing, write down the user-visible outcome, the smallest reproducible
scenario and the evidence that will establish it. For permission, recovery or
cross-module changes, ask an independent reviewer to check the contract and the
final diff when delegated review is requested or required by the repo.

- Run `pnpm verify:delegation` for the coordinator, isolated Minerva Runtime,
  SQLite, host adapter and Chromium UI. The default run uses local fixtures.
- Add `--live-codex` on Windows when the changed path needs the installed Codex.
  Record expired credentials separately from adapter failures. Reuse the same
  failing test when retrying after authorization.
- For native window behavior, follow the repo's `verify` skill once after the
  focused tests pass. Reuse one app session while its binary is current. Avoid
  rebuilding after test-only or documentation edits.
- On Windows, use `pnpm preview:windows` to build the standalone preview and
  update the desktop shortcut. The preview uses this worktree's existing
  identity. The smoke launch uses disposable data and leaves the old shortcut
  in place if the build or startup fails.

Report the verified scenarios, the exact remaining gap if any, and the build
commit or path when delivering a preview.
