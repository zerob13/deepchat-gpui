---
id: storage-002
scope: storage-sqlcipher
status: ready
depends-on: [storage-001]
---

# Production schema catalog, repair, FTS, dynamic DDL, and backup/import

## Objective

Complete the `storage-sqlcipher` production surface on top of `storage-001`'s connection, high-water-mark runner, startup classification, quarantine, and secret-store port. Implement the real production schema catalog, repair/FTS/dynamic-DDL, and backup/import/restore/encryption-change workflows end to end. This task ships the complete production schema surface — not a fake schema and not a narrowed-to-easy subset. The manifest feature remains `implemented` until every check here has reproducible evidence; it is not promoted to `verified` until the full platform/runtime loop and `full-parity-audit` close.

The full contract is `docs/storage.md`; this task names the executable boundary and verification intent without redefining feature status.

## Context

- `docs/INDEX.md`, `docs/architecture.md`, `docs/storage.md`, `docs/plan/README.md`, `docs/plan/analysis/porting-roadmap.md`, `PORTING.md`
- `parity/manifest.json` (`storage-sqlcipher`)
- Frozen reference commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`

Reference selectors (read-only oracle):

- `src/main/data/schemaCatalog.ts` — `CATALOG_DEFINITIONS` (41), `getStartupSchemaCatalog`, `createMainSchemaCatalog`, `createTables`/`migrationTables`/`finalize`
- `src/main/data/schemaCatalogMetadata.ts` — `SCHEMA_TABLES_NOT_CREATED_ON_FRESH_INSTALL`
- `src/main/data/schemaTypes.ts`, `src/main/data/schemaRepair.ts`, `src/main/data/schemaErrorClassifier.ts`
- `src/main/data/baseTable.ts` — `getCreateTableSQL`/`getMigrationSQL`/`finalizeMigration`/`getLatestVersion`
- `src/main/data/mainDatabase.ts` — `migrate` (empty markers, per-version transaction, SQL + finalizer + marker), `createDatabaseBackup`
- `src/main/data/sqliteCopyExclusions.ts`, `src/main/data/sqliteCopyOrder.ts` — copy order and exclusions
- `src/main/app/databaseSecurity.ts` — encryption enable/change-password/disable, validation tables, rollback
- `src/main/sync/index.ts`, `src/main/sync/dataImporter.ts`, `src/main/sync/configImportService.ts` — backup archive, import increment/overwrite, restore
- `src/main/session/data/tables/deepchatUsageStats.ts` — v32/v68/v69 rebuild and recovery
- `src/main/memory/data/tables/agentMemory.ts`, `src/main/memory/data/tables/agentMemoryFtsPolicy.ts` — FTS meta/policy micro-versioning, finalizers
- `src/main/tape/infrastructure/sqlite/tapeSearchProjectionStore.ts` — projection/FTS micro-versioning and dynamic DDL
- `src/main/app/databaseInitializer.ts` — startup diagnosis/repair loop and fresh-vs-full catalog

## Path

- `crates/deepchat-services/` — catalog, migration owners, repair, FTS/dynamic DDL, backup/import/encryption services
- `crates/deepchat-platform/` — filesystem/path/clock ports consumed by backup/import/encryption (no real Keychain/credentials)
- `tests/fixtures/` — generated schema/backup/import fixtures only
- `docs/storage.md`, `docs/plan/tasks/storage-002.md`
- `parity/evidence/storage-sqlcipher/`, `parity/manifest.json`

Do not read or modify `/Users/colab/Documents/workspace/deepchat-2`; it is a read-only oracle. Do not access real profiles, databases, Keychain items, or provider credentials. No Rust code goes under `deepchat-core`; there is no `deepchat-core` crate.

## Delivery split

The full surface is large, so split it into dependency-ordered implementation tasks and one integration gate. No subtask may narrow its own objective to samples, mocks, or partial owner sets, and no lane may claim completion without the integration task proving the real implementations connect.

- `storage-002a-1` — complete production catalog topology, all physical/runtime owners, global migrations through 69, programmatic finalizers, and required table rebuild/recovery behavior.
- `storage-002a-2` — schema diagnosis/repair, pre-repair backup, `afterRepair` hooks, and startup one-shot repair over the real catalog from `storage-002a-1`.
- `storage-002a-3` — connection-scoped tokenizer probing, dynamic FTS DDL, memory/tape projections, and their independent micro-version invalidation over the real catalog/repair surface.
- `storage-002b` — backup archive, import increment/overwrite, restore-on-failure, encryption enable/change-password/disable, copy order/exclusions, validation tables, and rollback.
- `storage-002` (integration) — one end-to-end vertical slice: a generated SQLCipher database is migrated to v69, repaired, backed up, then restored via overwrite and encryption-change, proving the real catalog/FTS/backup/import implementations interoperate.

## Contracts

### Catalog and migration owners

- Reproduce the three distinct catalogs: 41 `CATALOG_DEFINITIONS`, 39 physical create owners, 38 runtime migration owners (`createTables` minus `acp_turns`). Never assert "41 tables = complete schema".
- Mark `conversations`/`messages`/`message_attachments` as `createdOnFreshInstall: false` legacy tables; they exist only in the diagnosis/repair catalog.
- The fresh-startup diagnosis catalog excludes the three legacy tables (38 entries); the manual repair catalog uses the full 41 entries.
- `finalizeMigration(version)` runs after that version's SQL inside the same transaction, before the marker, for every owner that defines it. Empty markers are recorded for 1–10, 39, 40, 53–58, 63 (see `docs/storage.md` for the corrected list).

### Rebuild / overwrite and recovery

- Implement `acp_sessions` v30, `deepchat_usage_stats` v32/v68 rebuilds, and the v69 recovery finalizer with `hasCategoryAwarePrimaryKey()` (usage-id primary key, message-id non-primary, `usage_category` present).
- Preserve the exact rebuild copy→drop→rename semantics, not append-only column adds.

### Schema repair and startup integrity

- Implement `SchemaInspector` diagnosis (`missing_table`, `missing_column` with `addColumnSql` repairability, `column_type_mismatch` for `typeCheckedColumns`, `missing_index`) and `DatabaseRepairService` repair (pre-repair `.repair.bak` after `wal_checkpoint(TRUNCATE)`, one transaction, `afterRepair` hooks, `healthy`/`repaired`/`manual-action-required`).
- Implement the startup one-shot repair loop using the fresh-startup catalog and `schemaErrorClassifier` reasons.

### FTS, dynamic DDL, projection micro-versioning

- Probe FTS5 tokenizer (`trigram` → `unicode61`) once per connection and create FTS virtual tables dynamically, never from catalog SQL. `agent_memory_fts` requires trigram and is dropped/disabled when only unicode61 is available; `deepchat_tape_search_fts` uses unicode61 as its working fallback.
- Implement `agent_memory_fts`/`agent_memory_fts_meta` (`schema_version = 4`, `policy_version = 3`), `deepchat_tape_search_projection`/`_meta`/`_fts_meta` (`projection_version = 9`) and `deepchat_memory_ingestion_projection`/`_meta` (`projection_version = 1`), independent of the global high-water mark.
- Keep the app-settings, provider-settings, MCP-settings, and agent-settings owners in the physical create/runtime migration lists but outside both diagnosis/repair catalogs; the user-triggered repair action in the Settings UI uses the full 41-entry catalog and does not imply that those settings tables are repair-catalog members.
- Apply `SQLITE_COPY_EXCLUDED_OBJECTS` exclusions.

### Backup / import / encryption

- Backup: `wal_checkpoint(TRUNCATE)`, zip layout `database/agent.db` + `configs/app-settings.json` + optional prompts + `manifest.json` (version, `configStorage=sqlite`, `databaseEncrypted`, `databaseCipher`), sanitize machine-local and migrated provider keys, atomic `tmp`→rename.
- Import: safe `backup-\d+\.zip` name validation, zip-slip-guarded extraction, increment (merge) vs overwrite (replace + sidecar cleanup), snapshot temp `.bak` of agent.db and configs, restore-on-failure + reopen.
- Encryption: enable/change-password/disable with `checkpointAndClose`, `journal_mode=DELETE`, `ATTACH` target key, FK/trigger dependency copy order (`live_delegations → new_sessions`, cycle = error), FTS/virtual-table exclusion, `PRAGMA quick_check` + validation-table row counts, temp/rollback sidecars, rollback on reopen failure, metadata persistence.
- Validation tables: `schema_versions`, `new_sessions`, `deepchat_sessions`, `deepchat_tape_entries`, `providers`, `mcp_servers`, `agents`.

## Verification

Run from the repository root:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
uv run python tools/parity-audit/validate.py
git diff --check
```

Acceptance evidence under `parity/evidence/storage-sqlcipher/` must prove, with generated fixtures only:

1. Catalog counts (41 / 39 / 38 / 38) and legacy-table fresh-install exclusion.
2. Ordered per-version transactions, empty markers 1–10/39/40/53–58/63, and SQL + finalizer + marker rollback.
3. `acp_sessions` v30 and `deepchat_usage_stats` v32/v68/v69 rebuild/recovery, including the category-aware primary-key probe.
4. Schema diagnosis/repair with pre-repair backup and `afterRepair` hooks; fresh vs full repair catalog behavior.
5. FTS tokenizer probing, dynamic virtual-table creation, and projection/FTS micro-version invalidation.
6. Backup archive layout + sanitization + atomic rename.
7. Import increment/overwrite with restore-on-failure and machine-local settings preservation.
8. Encryption enable/change-password/disable with rollback, copy order, exclusions, and validation-table row counts.
9. The `storage-002` integration vertical slice: migrate → repair → backup → overwrite restore → encryption change on one generated SQLCipher database.

Generated fixtures must be reproducible and isolated; no fixture may embed a real database, profile, credential, or Keychain item.

## Completion

When all checks above have real-implementation evidence, update `parity/manifest.json` `storage-sqlcipher.remainingGaps` to remove the catalog/repair/FTS/backup/import gaps and the deferred-schema gap, leaving the feature at `implemented` with the target-policy Keychain note as the only remaining gap. Do not set `verified`.
