# Minerva subscription usage

Read-only metering for the existing Minerva Codex OAuth account and the configured
OpenCode Go API key. No separate account login or generation request is involved.

Protocol and cache references inspected on 2026-11-25, upstream commit
`9909e71b0ead5b9f49548d5462c0a36b6c38053f`:

- https://github.com/openchamber/openchamber/blob/9909e71b0ead5b9f49548d5462c0a36b6c38053f/packages/web/server/lib/quota/providers/codex.ts
- https://github.com/openchamber/openchamber/blob/9909e71b0ead5b9f49548d5462c0a36b6c38053f/packages/web/server/lib/quota/providers/opencode-go.ts
- https://github.com/openchamber/openchamber/blob/9909e71b0ead5b9f49548d5462c0a36b6c38053f/packages/ui/src/stores/useQuotaStore.ts

The adapters are an independent Rust implementation of those response formats;
OpenChamber implementation code is not incorporated. OpenChamber is MIT licensed.

## Boundaries

- `usage/parse.rs`: pure window normalization; missing percentages are never zero.
- `usage/transport.rs`: GET, 15-second total timeout, no redirects, 1 MiB limit,
  fixed sanitized error codes. Service code alone chooses production endpoints.
- `codex_usage.rs`: obtains current OAuth through Rig, refreshes as needed and
  rejects an account generation change. It does not read another application's auth.
- `usage/mod.rs`: DTOs, scoped 60-second memory cache, 15-second manual-refresh
  throttle, concurrent-call deduplication and account/config snapshot validation.
  Credential-bearing snapshots stay in backend memory, are not Debug/Serialize,
  and never reach UI, files or logs.
- Hosted environments return no subscription data, before credential access.

Supported Go configuration is the exact built-in `opencode-go`, `openai-chat`,
`https://opencode.ai/inference/go` route, optionally with trailing slash.
Custom gateways are never queried with their key against the official Go service.
Console OAuth and balances are outside this implementation. Invalid or inaccessible
usage is unavailable, never an inferred empty subscription.

DTO percentages mean **used**, resetAt and updatedAt are Unix milliseconds.
Codex windows use server-provided durations. Go monthly is a calendar window and
has no invented fixed duration. Reset timestamps accept seconds, milliseconds or
RFC3339. Transient network, timeout, rate-limit and malformed-response errors retain the last
successful sample for at most 15 minutes with its original updatedAt, status
unavailable and errorCode. Authentication failures and account/config changes clear
the sample. Successful lower usage after an actual reset is accepted immediately.
Cache entries are memory-only; neither samples nor credentials are persisted by this
feature. Restart requests a fresh sample.

The cache uses an independent async gate, separate from provider mutation and
agent execution locks. Provider/account changes invalidate cache identity and a
request that observes a changed snapshot discards its result. A new call retrieves
the current account. Individual provider requests run concurrently.

## Verification

`cargo test --manifest-path vendor/minerva/Cargo.toml -p mh-providers usage`

Optional authorized read-only live probe:

Set `MINERVA_CODEX_VERIFY_CONFIG_DIR` to the current Minerva configuration directory,
then run `cargo test --manifest-path vendor/minerva/Cargo.toml -p mh-providers
usage::tests::live_usage_probe -- --ignored --nocapture`. It prints only provider
IDs, safe status/error codes and window counts.

## Frontend boundaries

- `src/platform/minervaUsageStore.ts` owns the shared in-memory last-known snapshot, 60-second freshness interval, shared in-flight request and credential-generation guard. Closing a popup does not discard a valid snapshot; another consumer reuses the same request. Backend provider rows are authoritative: the frontend never resurrects windows omitted by the backend. Only IPC transport failures retain a previous same-account snapshot, explicitly marked stale and limited to 15 minutes from its original observation.
- `src/platform/minervaUsageTypes.ts` owns neutral DTOs. `minervaUsage.ts` is a thin React subscription adapter. One `minervaUsagePolling.ts` scheduler runs per webview while any consumer is active; warm reopening reuses the existing snapshot.
- `src/platform/minervaConnections.ts` emits domain connection/catalog events. The usage store chooses its invalidation policy, even with no mounted view. Credential and runtime changes discard old observations; saving model lists preserves counters. BroadcastChannel carries lifecycle metadata only.
- `src/workbench/SubscriptionUsage.tsx` is a props-only view shared by Settings and the chat popup. OpenIDEA Button/ProgressBar primitives, token-based CSS and source-text catalogs are reused. Dates follow the selected IDE locale. Percentages mean **used**. Refreshing keeps the current meters; transient failures display a last-known notice with the original observation timestamp. Auth and not-configured responses show no historical meters.
- `src/features/minerva/SubscriptionUsage.tsx` connects Settings to the hook. The chat adapter activates its hook while the selected-model indicator is visible, so the compact percentage updates without opening its popup.

Frontend verification covers transport errors, late responses, warm reopening, shared requests, catalog preservation, runtime isolation, credential boundaries and first/last-consumer polling. Browser mock-IPC checks and live provider requests are reported separately.

## Observed live read

On 2026-11-25 the read-only probe against the existing local Minerva account
returned `ok` for Codex (one usage window) and OpenCode Go (three usage windows).
It issued no generation requests and printed no credentials or raw responses.
The provider crate suite passed 52 tests, with three opt-in live tests ignored
in the ordinary run; the usage probe was explicitly run separately.

Runtime isolation: the shared usage store observes the remote-session identity and connection-loss state. A runtime boundary synchronously discards the previous snapshot and invalidates pending responses; reloading is deferred to the next microtask, after remote IPC routing is installed. Neither automatic nor manual reads run while the remote connection is lost. Cache reuse remains available within one runtime across ordinary popup and project UI changes. Eight frontend tests include local-to-remote warm-cache isolation and lost-remote/return-to-local late-response rejection.

## Selected-model indicator

The chat indicator resolves the exact provider id from the selected `api:<provider>/<model>`
route and shows only that provider in its popup. Settings retain the full overview.
The compact button displays percent **used** for the shortest available period,
including zero usage. Server durations take priority; a calendar month uses a
31-day upper bound only for ordering, without inventing a displayed duration.
Visible chat indicators share the existing cached loader and refresh lifecycle;
opening the popup does not start a separate request. Unknown usage displays a dash,
never a fabricated zero; providers without usage support have no indicator.

## Native settings and ownership

The settings page uses OpenIDEA List, SearchField, Tabs and Dialog primitives.
Its left list selects a connection; the right detail has Settings and Subscription
limits tabs. Adding a provider opens a dialog. The limits tab contains only the
selected provider. Switching tabs preserves the configuration draft; leaving a
dirty provider requires an explicit discard action. Pending writes block navigation.

- `useProviderConnections.ts` is the page's owner of persisted provider rows and
  presets. Codex receives configured models from that same owner, without a second
  provider-list query. Editable drafts belong to their editor controllers.
- `ProviderList`, `ProviderConnectionFields` and `ProviderModels` are presentation
  components. They receive boundary props rather than importing a settings store.
- `minervaUsageTypes.ts` owns neutral DTOs. Data/store/view dependencies do not
  point back into the React hook.
- `minervaUsagePolling.ts` owns one timer and visibility listener per webview,
  acquired by the first active consumer and released by the last one.
- `minervaConnections.ts` tracks scoped connection operations and emits domain
  lifecycle events; the usage store decides its own invalidation policy. Saving
  models does not announce a credential boundary. Runtime changes reset observed
  connection metadata and reject old results before they can seed it.
- Backend `usage/snapshot.rs` owns typed credential/account/endpoint identity.
  Names, model order and unrelated providers do not invalidate the quota cache.
  Cached rows receive current display names without a network refresh.

Review found no long prop chain: the chat adapter receives one selected model;
settings editor boundaries pass the data and callbacks they actually consume.
The shared workbench usage view remains props-only and does not perform IPC.
