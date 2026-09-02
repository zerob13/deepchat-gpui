# Backlog

Backlog items are sequencing notes; feature completion remains exclusively in `parity/manifest.json`.

- Audit GPUI feature selection independently on macOS x86_64, Windows, and Linux targets.
- Resolve GPUI 0.2.2 hidden-until-ready parity or record an approved waiver.
- Design and verify an OS accessibility adapter because GPUI 0.2.2 exposes no confirmed public accessibility-tree API.
- Execute the ready `storage-002a-3` dynamic FTS/projection slice, then continue `storage-002b` and the `storage-002` integration gate before connecting persistence and chat streaming through real storage implementations.
- `storage-002`: complete production schema catalog and repair/FTS/dynamic-DDL/backup-import/migration-overwrite workflows after `storage-001`; `storage-002a-1` and `storage-002a-2` are done, `storage-002a-3` is ready, and later slices cover backup/import/encryption and integration.
- Add provider/model registry before agent, MCP, skills, and memory integrations.
- Implement isolated Code Mode and WebView policy before browser/computer-use surfaces.
- Complete platform integrations, settings, managed runtimes, CLI, packaging, and full parity rediscovery.
