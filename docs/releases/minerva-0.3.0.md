# Minerva IDE 0.3.0

- Updated the OpenIDEA foundation to v0.1.54 and extended the Rig agent integration.
- Replaced the embedded MH package interface with a list of API providers. Provider settings expand in place; models can be discovered, reordered and removed.
- Added route-aware reasoning options for known providers and models, including OpenCode Go.
- Preserved the API conversation when changing providers between turns. Within one provider, model and effort changes apply at the next model request during an active turn.
- Allowed permission changes during an active Minerva API turn. Tools check the current policy before execution and after an approval answer.
- Added durable model-selection revisions and migration backups for the session database.
- Added an Agent workspace tab chooser, a chat history toggle, and tabs for tools and notifications.
- Added persistent subagent orchestration, Codex account integration and background jobs.
- Added per-model context window settings and a chat context meter. Long API turns can summarize completed tool exchanges while preserving the saved transcript.
- Added provider-specific usage information in settings, including subscription limits and balances when the provider reports them.
- Added a Subversion plugin for working with multiple working copies in one project.
- Updated `url` to 2.5.9 and `cookie` to 0.18.3 for their latest security fixes.

Windows x64: install the `setup.exe` asset. Automatic updates use the existing Minerva signing key and update channel.

An approval already waiting for an answer still requires a response after a permission change. Permission changes apply to subsequent actions. Switch between API providers between turns.
