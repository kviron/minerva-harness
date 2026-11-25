# Codex subscription inside Minerva

Minerva owns the conversation, Rig agent loop, tools, permissions and context preparation. Codex subscription is a model backend in that loop. Switching API providers or Codex changes the model route of the same Minerva session, rather than creating an OpenIDEA Codex task.

## Settings and credentials

Settings → Agents → Minerva contains the existing API providers and a Codex subscription section. Sign in opens the ChatGPT authorization page. The runtime owns the pending login, so closing settings does not cancel it. Sign out cancels pending login and removes Minerva's credential. Codex CLI authentication is separate.

The reserved route is `openai-codex`, with protocol `openai-codex`. Its endpoint comes from Rig's ChatGPT dialect and cannot be edited as an API gateway. Selected model IDs and their order are saved in `providers.json`; OAuth records use the existing injected credential store. Neither tokens nor the credential record appear in settings RPC responses. Saving an empty model list disables this route.

Fetch model list requests the account's current Codex catalog through Minerva's OAuth session. It reads model IDs, context limits, input modalities and supported reasoning efforts from `/codex/models`, using the verified 0.172.0 discovery contract. Public metadata is cached atomically and cleared when signing in or out. A successful refresh is authoritative; removed models are not restored from Rig's bundled snapshot. Before the first refresh, that snapshot remains an offline fallback. Generation and subscription-limit errors remain explicit.

## Execution boundaries

- `mh-providers/codex.rs` adapts Rig authentication and signed-in transport to Minerva's credential store. `codex_catalog.rs` owns validated server metadata and its public cache. Rig supplies OAuth, refresh, account headers and Codex Responses serialization. A discovered model does not need an entry in Rig's bundled catalog.
- `mh-providers/routes.rs` registers configured, admitted routes for the current run. An API key is frozen by value; subscription authentication is frozen by login generation, while the access token may refresh between dispatches. Logout invalidates the route.
- `selection_hook.rs` and `route_handler.rs` retain the frozen selection, context budget, effort and dispatch checks. A newly configured route becomes available on the next run; a model switch among admitted routes applies at the next model step.
- Each route freezes one metadata snapshot for both its signed-in transport and budget validation. Server-advertised reasoning efforts, including values absent from Rig's enum, are allowlisted and encoded through the Codex reasoning field. Optional unavailable Codex routes do not block unrelated API providers.
- `rig_history.rs` remains the shared history format. Switching the backend never copies a native Codex thread into the conversation or re-executes historical tool calls.
- `ide-minerva` exposes one Minerva connection. Existing `minerva.api.<provider>` task IDs remain resumable; their old provider prefix only prioritizes the initial model choice.
- The settings UI receives safe account state and delegates protocol work through `minerva_call`. It does not read credentials or implement refresh.

The existing OpenIDEA Codex adapter remains available for full native Codex tasks. Its own loop, history and orchestration contract are separate from this model backend.

## Model inactivity

`mh-providers/timeout.rs` bounds each inactive model request at 300 seconds. Text, reasoning and tool-argument output reset the deadline. Rig's model-turn completion hook disarms it before host tool execution and permission waits; the next completion dispatch arms it again. Healthy multi-step work has no 90-second whole-run cutoff. Tools retain their own execution limits and cancellation remains independent of the model watchdog. This policy applies to all Minerva backends, including OpenCode Go and DeepSeek.

## Sources

DSH implements these as two distinct features: the [pi-ai model adapter](https://github.com/deepseek-ai/deepseek-harness/blob/4b399c39a6d1479cd84ca591b4d7c88e24d38a96/packages/llm/llm-pi-ai/src/provider.ts) supports the OAuth model provider; the [Codex subagent](https://github.com/deepseek-ai/deepseek-harness/blob/4b399c39a6d1479cd84ca591b4d7c88e24d38a96/packages/subagent/subagent-codex/README.md) drives app-server with a separate native loop. Minerva uses Rig 0.46's `providers::chatgpt` and `client::signed_in::SignedInModel` for the former boundary.

Live discovery follows the official [Codex models endpoint](https://github.com/openai/codex/blob/ebd0765aafa255d63ebf690ebdffe0164dc14a0c/codex-rs/codex-api/src/endpoint/models.rs), independently of its agent loop.

## Verification boundary

Automated checks cover the shared model catalog, retained legacy session identity, cross-provider switching after a tool, revoked credentials, safe settings responses and rendered settings states. Discovery and generation also have opt-in live tests against an explicitly selected Minerva account. On 25 November 2026, live discovery returned `gpt-6.2-sol` and `gpt-6.2-astra`; a `gpt-6.2-sol`/low nonce request completed without tools or conversation persistence. Mock protocol checks alone do not establish account entitlement.
