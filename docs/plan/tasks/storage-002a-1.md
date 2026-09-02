---
id: storage-002a-1
scope: storage-sqlcipher
status: done
depends-on: [storage-001]
---

# Production catalog, migration owners, and rebuilds

## Objective

Implement the complete production static-schema and global-migration owner layer for `storage-002a`. Replace the generated-catalog-only boundary with real catalog metadata, all physical create owners, all runtime migration owners, every global version through 69, and the required table rebuild/recovery behavior. Connect one real production initialization path so a fresh generated SQLCipher database receives the complete static schema before the v69 marker, while an existing generated database runs the real owner SQL and programmatic finalizers in ascending per-version transactions.

This is not a reduced sample catalog. It must reproduce all 41 diagnosis/repair definitions, 39 physical create owners, 38 runtime migration owners, and 38 fresh-startup diagnosis entries. It must not claim schema repair, the complete connection-scoped FTS/projection lifecycle, backup/import, or encryption-change completion; later `storage-002a`/`storage-002b` tasks own those surfaces. Do not wire a production path with a stub, fake owner, no-op required finalizer, or incomplete table set.

## Context

- `docs/INDEX.md`
- `docs/architecture.md`
- `docs/storage.md`
- `docs/plan/README.md`
- `docs/plan/analysis/porting-roadmap.md`
- `docs/plan/tasks/storage-001.md`
- `docs/plan/tasks/storage-002.md`
- `PORTING.md`
- `parity/manifest.json` (`storage-sqlcipher`)
- Frozen reference commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`

Reference selectors (read-only oracle):

- `src/main/data/schemaCatalog.ts`, `schemaCatalogMetadata.ts`, `schemaTypes.ts`, `baseTable.ts`, `mainDatabase.ts`
- Every owner imported by `createMainSchemaCatalog(db)` and `CATALOG_DEFINITIONS`
- `src/main/session/data/tables/deepchatUsageStats.ts`
- `src/main/agent/data/tables/acpSessions.ts`
- `src/main/memory/data/tables/agentMemory.ts`, `agentMemoryDirective.ts`
- `src/main/settings/data/tables/*`
- Tests under `test/main/data/` that cover schema creation and migration behavior

`/Users/colab/Documents/workspace/deepchat-2` is a read-only oracle. Do not modify it. Do not read or mutate real profiles, user databases, Keychain items, provider credentials, or environment secrets.

## Path

- `crates/deepchat-services/src/`
- `crates/deepchat-services/tests/`
- `tests/fixtures/` (generated helpers/metadata only)
- `docs/storage.md`
- `docs/plan/tasks/storage-002a-1.md`
- `parity/evidence/storage-sqlcipher/`
- `parity/manifest.json`

Do not add a `deepchat-core` crate or place Rust code under `deepchat-core`.

## Contracts

### Catalog topology

- Reproduce exactly 41 `CATALOG_DEFINITIONS`, 39 physical create owners, 38 runtime migration owners (`createTables` minus `acp_turns`), and 38 fresh-startup diagnosis entries.
- `conversations`, `messages`, and `message_attachments` are full-catalog legacy definitions with `createdOnFreshInstall: false`; they are absent from physical creation, runtime migration, and fresh-startup diagnosis.
- `acp_turns` participates in physical creation but not runtime migrations.
- The app-settings, provider-settings, MCP-settings, and agent-settings owners participate in physical creation and runtime migration but are absent from both diagnosis/repair catalogs. App settings creates both `app_settings` and `config_migrations`; reproduce the reference distinction where v25 includes both and v26 normalizes `app_settings` without recreating `config_migrations`.
- Catalog metadata must preserve every table name, complete create SQL, repairable column metadata (`addColumnSql`), type-checked columns, indexes, `afterRepair` hook identity, owner latest version, per-version SQL, and programmatic finalizer needed by later repair/FTS tasks. Do not collapse different owners merely to satisfy the counts.
- Static owner creation includes each owner's auxiliary tables, indexes, and triggers. FTS5 virtual tables remain dynamic and must not appear in static catalog SQL.

### Fresh creation and existing migration

- A real production-schema initialization API creates all 39 physical owners and their static auxiliary objects before recording the fresh v69 marker. A fresh database must never become an empty schema carrying a v69 marker.
- An existing database executes versions `current + 1..=69` in ascending order, one transaction per version.
- For each version: gather SQL from every runtime migration owner, execute the statements, run every matching owner `finalizeMigration(version)`, then insert the marker, all inside the same transaction.
- Roll back that version's SQL, programmatic finalizers, and marker together on any failure; preserve earlier committed versions and do not run later versions.
- Record functionally empty markers for 1–10, 39, 40, 53–58, and 63.
- Preserve the statement-specific tolerated-error allowlist from `storage-001`; do not widen it.
- Preserve the schema-level finalizer distinction: `agent_memory.assertCurrentSchema` and `agent_memory_directive.assertCurrentSchema` run after global migration completion and are not version markers.

### Required version-specific behavior

- Implement every owner SQL and owner latest-version fact needed for the version map in `docs/storage.md`, including the conditional v23 recovery and all v26 normalization tables.
- Implement all reference programmatic migration finalizers: `agent_memory` v42/v46/v48/v49/v51/v52, `deepchat_pending_inputs` v67, `deepchat_usage_stats` v69, and the three live-delegation v60 trigger installers.
- Reproduce `acp_sessions` v30 via copy → drop → rename with its unique constraints/indexes.
- Reproduce `deepchat_usage_stats` v32 and v68 via copy → drop → rename while preserving compatible rows.
- Reproduce v69 recovery: `hasCategoryAwarePrimaryKey()` requires `usage_id` to be the primary key, `message_id` not to be primary, and `usage_category` to exist; rebuild through the category migration when any part is absent.
- Programmatic rebuild/finalizer failures use safe typed public errors and must not expose raw SQL, driver text, or secrets.

## Verification

Run from the repository root:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
uv run python tools/parity-audit/validate.py
git diff --check
```

Generated-fixture tests must prove:

1. Catalog topology and exact membership: 41 / 39 / 38 / 38, legacy exclusions, settings-owner placement, and `acp_turns` migration exclusion.
2. Every owner has the expected latest version and the aggregate owner map reaches exactly 69.
3. A fresh generated SQLCipher database creates the complete static production schema before recording only marker 69.
4. An existing generated database executes the real owner map through 69 and records the exact 19 empty versions.
5. Owner SQL + all matching owner finalizers + marker share one transaction and roll back together.
6. v23 conditional recovery and v26 normalization behavior, including the v25/v26 `config_migrations` distinction.
7. `acp_sessions` v30 and `deepchat_usage_stats` v32/v68 rebuild semantics preserve data and replace the schema rather than appending columns.
8. v69 detects and repairs a recorded-v68 database lacking the category-aware primary key, and leaves an already-correct table unchanged.
9. Static creation contains no FTS virtual table SQL; required dynamic finalizer behavior is real, not a no-op placeholder.
10. Public errors remain source-free and redact raw SQL/driver text.

All fixtures must be generated in isolated temporary directories. Update `parity/evidence/storage-sqlcipher/` with reproducible commands and counts only after the implementation passes review. Keep `storage-sqlcipher` at `implemented`; do not remove repair/FTS/backup/import gaps or promote it to `verified` in this task.
