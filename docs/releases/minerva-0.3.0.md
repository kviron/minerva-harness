# Minerva IDE 0.3.0

- Updated the OpenIDEA foundation to v0.1.50 and extended the Rig agent integration.
- Replaced the embedded MH package interface with a list of API providers. Provider settings expand in place; models can be discovered, reordered and removed.
- Added route-aware reasoning options for known providers and models, including OpenCode Go.
- Preserved the API conversation when changing providers between turns. Within one provider, model and effort changes apply at the next model request during an active turn.
- Allowed permission changes during an active Minerva API turn. Tools check the current policy before execution and after an approval answer.
- Added durable model-selection revisions and migration backups for the session database.
- Updated `url` to 2.5.9 and `cookie` to 0.18.3 for their latest security fixes.

Windows x64: install the `setup.exe` asset. Automatic updates use the existing Minerva signing key and update channel.

An approval already waiting for an answer remains explicit after a permission change. Actions that have already started are not undone. Cross-provider switching during an active turn, the planned compaction redesign and agent orchestration remain future work.
