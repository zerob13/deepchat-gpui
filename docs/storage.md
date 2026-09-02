# Storage Contract

## Purpose and boundary

Storage owns the durable application database lifecycle and the platform secret-store boundary. `deepchat-services` owns the SQLCipher connection, the complete schema catalog, migration execution, startup classification, schema repair, FTS/dynamic DDL, backup/import/restore, encryption-change, and migration-overwrite behavior. `deepchat-platform` owns the macOS Keychain adapter and the manual-password fallback abstraction. The application composition root owns dependency injection and user-facing recovery decisions; storage never reads a real profile, provider credential, database, Keychain, or environment secret during tests.

This document is the final storage contract. `storage-001` delivered the SQLCipher connection, the high-water-mark runner, startup classification, quarantine, and the secret-store port. `storage-002` completes the production schema catalog, repair, FTS, dynamic DDL, backup/import, and migration-overwrite surface. It is a target contract for the completed surface, not a claim that every referenced Rust implementation already exists; only facts provable from the frozen reference at `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd` are stated as reference behavior.

```text
application composition root
          │ injects resolver + paths + clock
          ▼
 deepchat-services ───────────────► deepchat-platform
   connection / startup /             Keychain target adapter /
   catalog / repair / FTS /           manual fallback abstraction
   backup / import / encryption
          │
          ▼
   DB path + -wal + -shm ──► quarantine directory
```

## Dependency binding

The SQLCipher-compatible SQLite binding is locked to `rusqlite 0.40.2` with `default-features = false` and `features = ["bundled-sqlcipher-vendored-openssl"]`. This bundles SQLCipher 4 and a vendored OpenSSL, keeps the graph free of GPL dependencies, and reproduces the reference's UTF-8 key, compatibility 4, and WAL-after-key ordering. The choice is proven by generated SQLCipher contract tests; `default-features = false` is required so the bundled SQLCipher build is selected rather than a host `libsqlite3`.

## Security and connection contract

- The database is SQLCipher-compatible SQLite.
- Opening an encrypted database applies `cipher='sqlcipher'`, then `legacy=4` (SQLCipher compatibility 4), then supplies the password as UTF-8 bytes. A password is never logged, serialized into evidence, or included in an error display.
- WAL mode is enabled only after the key has been applied for an encrypted database. Unencrypted fixtures may enable WAL directly.
- A missing main file with an existing `-wal` sidecar is an `OrphanWal` startup failure. The opener must not create a replacement database or silently delete the sidecar.
- Quarantine moves the main database, `-wal`, and `-shm` files that exist into one newly allocated directory. Partial quarantine is an error and must remain observable; it must not be presented as successful recovery.
- All logs use safe error codes/classifications and non-secret paths only where needed. SQL statements, keys, passwords, credential values, and raw driver errors are not logged.

## Schema high-water mark

`schema_versions(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL)` is the monotonic high-water mark. The factual reference latest global version is `69`; this is a compatibility fact. The runner preserves the distinction between a fresh file and an existing file.

Startup behavior:

1. A fresh database with no prior file and a non-zero catalog latest version records one latest-version marker (`69`) instead of replaying historical migrations.
2. An existing database reads `MAX(version)` and runs every version from `current + 1` through `69` in ascending order.
3. Every version receives its own transaction, including versions with no SQL (an empty marker). The marker is inserted only after that version's SQL and programmatic finalization succeed.
4. A failed transaction rolls back its SQL, its programmatic finalization, and its version marker. Earlier committed versions remain committed; later versions do not run.

## Schema catalog and physical object accounting

The reference keeps three distinct "catalogs" that must not be conflated. The figure "41 tables" is a catalog-definition count, never a physical-table or complete-schema claim.

| Set | Count | Source and role |
|---|---|---|
| `CATALOG_DEFINITIONS` | 41 entries | `src/main/data/schemaCatalog.ts` — the static definition registry used by `SchemaInspector`/`DatabaseRepairService` (diagnose/repair). Includes three legacy tables. |
| Fresh-startup catalog | 38 entries | `getStartupSchemaCatalog()` = the 41 definitions minus the three legacy tables; used for boot-time diagnosis and automatic repair. |
| Physical create list | 39 owner instances | `createMainSchemaCatalog(db).createTables` — the tables actually created on open. |
| Runtime migration owners | 38 instances | `createTables.filter(t => t !== acpTurns)` — the set whose `getMigrationSQL`/`getLatestVersion`/`finalizeMigration` the runner consults. |

The three legacy tables — `conversations`, `messages`, `message_attachments` — appear only in `CATALOG_DEFINITIONS` (for legacy/manual repair) and are marked `createdOnFreshInstall: false` via `SCHEMA_TABLES_NOT_CREATED_ON_FRESH_INSTALL` in `schemaCatalogMetadata.ts`. They are not created on fresh startup and are not runtime migration owners.

`CATALOG_DEFINITIONS` also contains three entries that are not independent physical table owners: `deepchat_memory_ingestion_projection_meta`, `deepchat_tape_search_projection_meta`, and `deepchat_tape_search_fts_meta`. Each reuses the same projection table class as its base entry, and each `createTable`/`getCreateTableSQL` call is idempotent (`CREATE ... IF NOT EXISTS`). The physical objects actually materialized therefore exceed and differ from a naive reading of the catalog:

- Four settings owners participate in the physical create and runtime migration lists but are not members of `CATALOG_DEFINITIONS`: app settings creates `app_settings` and `config_migrations`; provider settings creates `providers`, `provider_models`, `model_status`, and `model_configs`; MCP settings creates `mcp_servers` and `mcp_settings`; agent settings creates `agent_settings` and `agent_mcp_selections`. Their tables are not diagnosed or repaired by either the 38-entry startup catalog or the full 41-entry repair catalog.
- Catalog owners can also materialize multiple physical objects. `agent_memory` creates auxiliary tables (`agent_memory_tombstone`, `agent_memory_clear_job`, `agent_memory_derivation`, `agent_memory_dirty`) plus insert/update/delete triggers.
- FTS5 virtual tables (`agent_memory_fts`, `deepchat_tape_search_fts`) and their meta tables are dynamic objects created at runtime by capability probing, never by static catalog SQL.
- `deepchat_tape_search_projection` creates `deepchat_tape_search_projection`, `deepchat_tape_search_projection_meta`, and `deepchat_tape_search_fts_meta`, then dynamically creates `deepchat_tape_search_fts`.

The correct contract is therefore: **41 catalog definitions; 39 physical create owners; 38 runtime migration owners; plus auxiliary, meta, and dynamically-created FTS objects not counted in any of those numbers.** No task or evidence may claim "41 tables = complete schema".

## Version → owner / SQL / finalizer map

The global high-water mark is `69`. The runner iterates `1..69`; a version is functionally empty when no runtime migration owner emits SQL for it. `finalizeMigration(version)` runs after that version's SQL, inside the same transaction, for every owner that defines one, before the version marker is inserted.

| Version | Runtime migration owner(s) | SQL / behavior | Programmatic finalizer |
|---|---|---|---|
| 1–10 | *(none at runtime)* | empty marker — owned by legacy `conversations` only inside `CATALOG_DEFINITIONS`; that table is not a runtime migration owner | — |
| 11 | `new_sessions` | add `is_draft` | — |
| 12 | `deepchat_sessions` | add `system_prompt`, `temperature`, `context_length`, `max_tokens`, `thinking_budget`, `reasoning_effort`, `verbosity` | — |
| 13 | `deepchat_message_traces` | create table | — |
| 14 | `deepchat_sessions` | add `summary_text`, `summary_cursor_order_seq`, `summary_updated_at` | — |
| 15 | `new_sessions` | add `active_skills` | — |
| 16 | `new_sessions` | add `disabled_agent_tools` | — |
| 17 | `new_environments`, `deepchat_pending_inputs`, `deepchat_usage_stats` | create tables | — |
| 18 | `new_environments` | rebuild from session usage | — |
| 19 | `deepchat_sessions` | add `force_interleaved_thinking_compat` | — |
| 20 | `new_sessions`, `deepchat_sessions`, `agents` | subagent columns; `reasoning_visibility`; create `agents` | — |
| 21 | `new_sessions` | add `revision` | — |
| 22 | `deepchat_usage_stats` | add `cache_write_input_tokens` | — |
| 23 | `deepchat_sessions` | conditional recovery migration (empty when nothing to recover) | — |
| 24 | `deepchat_sessions` | add `timeout_ms` | — |
| 25 | `app_settings`, `providers`, `mcp_servers`, `agent_settings` | create settings tables | — |
| 26 | `app_settings`, `deepchat_user_messages`, `deepchat_user_message_files`, `deepchat_user_message_links`, `deepchat_assistant_blocks`, `deepchat_search_documents`, `new_session_active_skills`, `new_session_disabled_agent_tools` | normalization create-table migrations | — |
| 27 | `deepchat_sessions` | add `image_generation_options_json` | — |
| 28 | `deepchat_sessions` | add `video_generation_options_json` | — |
| 29 | `deepchat_sessions` | add `top_p` | — |
| 30 | `acp_sessions` | table rebuild to add unique constraints and indexes | — |
| 31 | `deepchat_sessions` | add `memory_cursor_order_seq` | — |
| 32 | `new_environment_preferences`, `deepchat_usage_stats`, `agent_memory` | create preferences; usage-stats table rebuild; add `embedding_model`/`source_entry_ids` | — |
| 33 | `agent_memory` | add `confidence`, `last_consolidated_at`, `conflict_state` | — |
| 34 | `agent_memory` | add `persona_state` | — |
| 35 | `agent_memory` | add `conflict_with` + index | — |
| 36 | `agent_memory_audit` | create table | — |
| 37 | `agent_memory` | add `category` | — |
| 38 | `agent_memory_audit` | add `memory_ref_id` + backfill + index | — |
| 39 | *(none)* | empty marker | — |
| 40 | *(none)* | empty marker (`cron_jobs` declares latest `40` but emits no SQL) | — |
| 41 | `agent_memory` | add `decision_revision` | — |
| 42 | `agent_memory` | state-model columns + backfill | `replaceLegacyStatusBridge`; FTS policy bump; drop temp markers |
| 43 | `deepchat_pending_inputs` | add `blocking_json` | — |
| 44 | `new_sessions` | forward-recovery add `revision` | — |
| 45 | `deepchat_message_traces` | add `logical_round`, `physical_attempt`; rebuild sequence index | — |
| 46 | `deepchat_pending_inputs`, `agent_memory` | add `message_ids_json`/`assistant_message_id`; temporal columns | `ensureTemporalArtifacts` |
| 47 | `agent_memory` | tombstone table | — |
| 48 | `agent_memory` | derivation + dirty tables + backfill | `DIRTY_TRIGGER_SQL` |
| 49 | `agent_memory` | dirty-trigger drop + backfill | `DIRTY_TRIGGER_SQL` |
| 50 | `agent_memory_directive` | create table | — |
| 51 | `agent_memory` | scope columns + index | `ensureScopeArtifacts` |
| 52 | `agent_memory` | clear-job table | `ensureClearArtifacts` |
| 53–58 | *(none)* | empty markers | — |
| 59 | `new_sessions` | add `orchestration_policy` | — |
| 60 | `live_delegations`, `live_delegation_turns`, `live_delegation_events` | create base schemas | each installs its trigger |
| 61 | `live_delegation_turns` | add effect columns (idempotent `SELECT 1` when present) | — |
| 62 | `live_delegation_turns` | add `result_ref_json` (idempotent) | — |
| 63 | *(none)* | empty marker | — |
| 64 | `live_delegations` | retired-workflow schema | — |
| 65 | `live_delegation_turns` | contract/evaluation columns (idempotent) | — |
| 66 | `live_delegation_events` | evaluation columns (idempotent) | — |
| 67 | `deepchat_pending_inputs` | add `retry_required_at` | `normalizeRetryRequiredRows` |
| 68 | `new_sessions`, `deepchat_message_traces`, `deepchat_usage_stats` | add `tool_mode_override`; append index; usage-stats category rebuild | — |
| 69 | `deepchat_usage_stats` | recovery no-op `SELECT 1` | rebuild category-aware primary key if absent |

Functionally empty markers at runtime are therefore **1–10, 39, 40, 53, 54, 55, 56, 57, 58, and 63** (19 markers). A prior audit listed only `39, 40, 53–58, 63`; that list treats the legacy `conversations` table as a runtime owner, but it is catalog-only and its 1–10 migrations never execute. The evidence-correct list above adds 1–10.

## Migration execution semantics

- The runner uses `createMainSchemaCatalog(db).migrationTables` (`createTables` minus `acp_turns`), not the 41-entry `CATALOG_DEFINITIONS`.
- For each version, SQL blocks are split into statements (comment/single-quote/double-quote aware) and executed one statement at a time.
- Tolerated errors are statement-specific and narrow: `ALTER TABLE … ADD COLUMN` with `duplicate column name`, `CREATE [UNIQUE] INDEX` with `already exists`, and `ALTER TABLE … DROP COLUMN` with `no such column`. Nothing else is blanket-ignored.
- After a version's SQL succeeds, every owner with `finalizeMigration(version)` runs its programmatic finalizer inside the same transaction, then the version marker is inserted.
- Empty markers are intentionally recorded so a removed or abandoned version number can never be reused after a newer version ships.
- `acp_turns` is a create-only table and is excluded from the migration owner list even though it participates in physical creation.

### Table-rebuild / overwrite semantics

Rebuild migrations copy into a temporary table, drop the original, and rename, so they replace both schema and data in place. Reference rebuilds:

- `acp_sessions` v30 rebuilds to add unique constraints and indexes.
- `deepchat_usage_stats` v32 rebuilds to add `cache_write_input_tokens` with the `source`/`usage_date` columns.
- `deepchat_usage_stats` v68 rebuilds to introduce `usage_category` (`chat`/`compaction`), assigning `usage_id = message_id` and `usage_category = 'chat'` for legacy rows.
- `deepchat_usage_stats` v69 is a recovery finalizer: it runs a no-op SQL, then `finalizeMigration` checks `hasCategoryAwarePrimaryKey()` (a `usage_id` primary key with `message_id` not primary and `usage_category` present) and, if absent, rebuilds via the same category migration (`deepchat_usage_stats_v69`). This recovers databases that already recorded v68 but never received the category-aware rebuild.

The target must reproduce these rebuild and recovery semantics, including the v68/v69 primary-key probe, not merely append columns.

## Schema repair and startup integrity

`SchemaInspector.diagnose()` compares the live `sqlite_master` snapshot against a catalog and reports `missing_table`, `missing_column` (repairable only when an `addColumnSql` exists), `column_type_mismatch` (only for `typeCheckedColumns`, non-repairable), and `missing_index`. `DatabaseRepairService.repair()`:

1. Diagnoses; returns `healthy` immediately when no issues exist.
2. Returns `manual-action-required` when there are only non-repairable issues.
3. Otherwise creates a pre-repair backup (`<dbPath>.<timestamp>.repair.bak` after `wal_checkpoint(TRUNCATE)`), then, in one transaction, creates missing tables, adds missing repairable columns, recreates missing indexes, and runs each affected table's `afterRepair` hook with the set of columns actually added.
4. Re-diagnoses and returns `repaired`, `healthy`, or `manual-action-required` with before/after diagnoses.

Startup uses the fresh-install catalog (`getStartupSchemaCatalog`, 38 entries) so automatic repair never materializes the retired legacy tables. The user-triggered repair action exposed from the application's Settings UI uses the full 41-entry catalog; this describes the UI entry point, not the settings storage owners. The `app_settings`, provider, MCP, and agent-settings tables are outside both repair catalogs. Startup repair is one-shot (a second attempt leaves residual issues observable rather than looping forever), and construction-time schema failures classified by `schemaErrorClassifier` (`missing-table`, `missing-column`, `column-count-mismatch`) trigger the same startup-scoped repair path.

## FTS, dynamic DDL, and projection micro-versioning

- FTS5 tokenizer capability is probed at runtime and cached per database connection. The probe tries `trigram` first and reports `unicode61` when trigram is unavailable. FTS virtual tables are created dynamically from that probe, never from static catalog SQL.
- `agent_memory` FTS: virtual table `agent_memory_fts` plus `agent_memory_fts_meta`, keyed by `agent_memory_fts` with a `schema_version` (`AGENT_MEMORY_FTS_META_VERSION = 4`) and a `policy_version` (`AGENT_MEMORY_FTS_POLICY_VERSION = 3`). Scope matching uses a truncated SHA-256 `agent_memory_fts_scope` function. This filtered external-content mirror requires the `trigram` tokenizer: when the probe reports `unicode61`, the implementation drops/disables `agent_memory_fts` and marks it unavailable rather than creating a unicode61 fallback index.
- `deepchat_tape_search_projection`: `DEEPCHAT_TAPE_SEARCH_PROJECTION_VERSION = 9` is a projection micro-version stored in `deepchat_tape_search_projection_meta`/`deepchat_tape_search_fts_meta`, independent of the global `schema_versions` high-water mark. The FTS index (`deepchat_tape_search_fts`) uses `unicode61` as a working fallback when `trigram` is unavailable and is rebuilt when the projection head is stale.
- `deepchat_memory_ingestion_projection`: `DEEPCHAT_MEMORY_INGESTION_PROJECTION_VERSION = 1` stored in `deepchat_memory_ingestion_projection_meta`, also independent of the global high-water mark.
- The FTS maintenance objects are excluded from SQLite copy by `SQLITE_COPY_EXCLUDED_OBJECTS`: `agent_memory_dirty`, `agent_memory_dirty_ai`, `agent_memory_dirty_au`, `agent_memory_dirty_ad`, `agent_memory_fts_meta`, and `deepchat_tape_search_fts_meta`.

Projection/FTS micro-versions and the global schema version are two separate monotonic axes. A task must not record a projection rebuild as a global `schema_versions` migration.

## Backup, export, import, restore, and encryption change

### Backup (sync archive)

```text
performBackup
  │  requires agent.db + app-settings.json present
  ▼
wal_checkpoint(TRUNCATE) on the live DB
  ▼
read agent.db bytes ──► zip: database/agent.db
read sanitized app-settings.json ──► zip: configs/app-settings.json
optional custom/system prompts ──► zip: configs/*.json
  ▼
manifest.json (version, createdAt, configStorage=sqlite,
               configSchemaVersion, databaseEncrypted, databaseCipher, files)
  ▼
compress (fflate, level 6) ──► backup-<epochMs>.zip.tmp ──► rename to backup-<epochMs>.zip
```

Backups strip machine-local settings (`cloudSyncConfig`, `cloudSyncSecret`, `agentCommandShell`) and migrated/legacy provider keys; the receiving machine's values are preserved on import.

### Import / restore

```text
importFromSync(fileName, increment | overwrite)
  │  safe file-name validation (backup-\d+\.zip)
  ▼
extract archive (zip-slip guarded) ──► manifest + database/agent.db + app-settings.json
  │  resolve backup version + configStorage + encryption password
  │  assert overwrite encryption compatibility (agent + overwrite only)
  ▼
close live DB ──► snapshot current agent.db + configs to temp .bak files
  ▼
  ├─ source = agent.db, overwrite:
  │     validate backup DB, count rows, copyFile backup→live, delete -wal/-shm,
  │     import sqlite config, merge app-settings preserving machine-local keys,
  │     reset shell windows
  ├─ source = agent.db, increment:
  │     DataImporter merge (normalizes/skips malformed rows),
  │     import sqlite config, merge app-settings, merge prompt stores
  └─ source = chat.db (legacy): importLegacyChatDb, merge configs/prompts
  ▼
reopen live DB; on any failure restore every temp .bak and reopen
```

Failure restores the snapshot, reopens the database, and surfaces a stable known-error message; unknown errors collapse to `sync.error.importFailed`.

### Encryption enable / change-password / disable

```text
migrateDatabase(direction)
  │  acquire in-process migration lock (reject concurrent)
  │  recoverInterruptedMigrationFiles() at construction
  ▼
checkpointAndClose live DB
  ▼
collectValidationCounts(source) over validation tables
  │  schema_versions, new_sessions, deepchat_sessions,
  │  deepchat_tape_entries, providers, mcp_servers, agents
  ▼
exportDatabaseToTemp: wal_checkpoint(TRUNCATE) ──► journal_mode=DELETE
  │  ATTACH temp target (KEY target password / '' for disable)
  │  copy migratable tables in FK/trigger-dependency order,
  │  copy indexes/triggers/views, copy sqlite_sequence,
  │  exclude FTS maintenance + virtual-table objects
  ▼
verifyMigratedDatabase: PRAGMA quick_check == ok + row counts match
  ▼
replaceDatabaseWithRollback: remove sidecars, rename live→rollback,
  │  rename temp→live, remove sidecars; on rename failure restore rollback
  ▼
reopenWithPassword(target); on reopen failure restore rollback + reopen source
  ▼
remove rollback, persist new encryption metadata
```

- Migration directions are `enable`, `change-password`, and `disable`; metadata records `lastMigrationAt` and `lastMigrationDirection`.
- Temp/rollback sidecars are `<dbPath>.migration-tmp` and `<dbPath>.migration-rollback`; interrupted migrations are recovered at construction (remove temp; restore rollback only when the live file is missing, otherwise discard the stale rollback).
- The copy order derives parent-before-child ordering from `PRAGMA foreign_key_list` plus the trigger-enforced dependency `live_delegations → new_sessions`; a dependency cycle is a hard error, never a silent lexical fallback.
- `enable`/`change-password`/`disable` validate the current password first; `enable` rejects an already-enabled database, `disable` rejects a not-enabled database, and `change-password` rejects an unchanged password.

## Startup classification and password lifecycle

Typed startup outcomes are at minimum `OrphanWal`, `Unreadable`, `TrueCorruption`, and `Cancelled`; implementation may add stable non-secret variants for I/O, unsupported cipher, and migration failures. Classification rules:

- A leftover WAL without its main file is always `OrphanWal`.
- A destructive database error before a password has been verified is `Unreadable`; an encrypted file cannot be called corrupt merely because it is not readable.
- A destructive error after successful password validation is `TrueCorruption`.
- The password resolver owns an inner retry loop. Every non-terminal manual validation failure (wrong password, open, or I/O) transitions the next reason to `invalid`; only orphan WAL and cancellation are terminal. A wrong password is consumed by that loop and retried without handing control back as a new outer startup cycle.
- A `VerifiedPassword` marker is an in-process typed fact attached to the successful resolver result. It is not a secret and does not persist the password. Only `VerifiedPassword` followed by a destructive database error may promote the outcome to `TrueCorruption`.
- Cancellation exits without destructive changes. "Start empty" is an explicit recovery decision, after which quarantine must complete before a new database is created.

## Secret-store boundary

`deepchat-platform` exposes a typed secret-store port with a macOS Keychain target adapter and a manual fallback abstraction. The adapter is injected and tested against generated fakes; tests must not access the user's Keychain. Keychain service and account names have no frozen reference contract. Any service/account naming in the target is a documented target policy and must never be reported as reference parity. The reference wraps database-encryption passwords with Electron `safeStorage` and stores `passwordStorage: 'safeStorage' | 'manual' | 'none'`; that wrapping is an Electron-specific implementation detail, and the target records the same three-way policy without claiming byte-for-byte `safeStorage` parity.

## Ownership and lifecycle

- The composition root creates one storage owner and injects the database path, password resolver, secret-store port, filesystem/quarantine port, and clock.
- The storage owner opens one connection, applies connection pragmas, creates physical tables, initializes `schema_versions`, runs migrations, runs the schema finalizer (`agent_memory.assertCurrentSchema` and `agent_memory_directive.assertCurrentSchema`), and exposes an explicit close operation.
- A migration transaction owns its SQL, its programmatic finalizers, and its marker. Callers cannot insert version markers directly.
- Quarantine owns file movement and returns a typed result containing only safe paths/classification data.
- Backup/import and encryption-change own their temp/rollback sidecars and always attempt restore/reopen on failure; they never leave a half-replaced database presented as success.
- Shutdown closes the connection before releasing the injected platform resources. Recovery and quarantine are idempotent only where the filesystem operation is demonstrably safe; no retry may overwrite preserved evidence.

## Typed errors

Errors must distinguish at least: invalid input/path, open/I/O failure, orphan WAL, unreadable encrypted file, true corruption, wrong password (internal resolver event), password cancelled, Keychain unavailable, manual fallback required, migration SQL failure, migration marker failure, programmatic-finalizer failure, schema diagnosis/repair status (`healthy`/`repaired`/`manual-action-required`), missing/type-mismatched column or index, backup validation failure, unsupported backup version, encrypted-backup password missing, overwrite encryption mismatch, encryption already enabled/not enabled, unchanged password, migration lock conflict, quarantine partial failure, and close failure. Error display is safe and stable; wrapped driver messages remain diagnostics-only and are sanitized before logging.

## Target translation notes

The reference depends on Electron and `better-sqlite3-multiple-ciphers`; the target maps each dependency to an idiomatic Rust equivalent without claiming byte-for-byte parity where the surface is Electron-specific:

- `better-sqlite3-multiple-ciphers` → `rusqlite` with the bundled SQLCipher feature (see dependency binding). The `sqlite_master`/`PRAGMA table_info`/`PRAGMA foreign_key_list` introspection calls map to equivalent `rusqlite` prepared statements.
- Electron `safeStorage` password wrapping → the `deepchat-platform` secret-store port (Keychain on macOS, manual fallback elsewhere); the `passwordStorage` three-way policy is preserved as a typed fact.
- Node `fs` copy/rename/`process.getBuiltinModule('fs')` bypass for backup → the injected filesystem/quarantine port, so backup copies the real file without depending on process-level module mocking.
- `node:fflate` zip → a Rust zip crate; the archive layout, manifest schema, sanitization, and zip-slip defenses are preserved as data contracts.
- Electron `app.getPath('userData')` / `app.getPath('temp')` → injected path ports.
- In-process migration lock (`activeMigrationDbPaths` set) → a per-owner in-process lock keyed by database path.

## Tests and fixtures

Use generated temporary directories, generated SQLCipher databases, generated WAL/SHM sidecars, deterministic clocks, injected password resolvers, injected Keychain/manual-store fakes, and injected filesystem ports. Required tests cover: UTF-8 keying, compatibility 4 and WAL ordering, fresh latest marker, old-database ordered transactions, empty markers (including 1–10, 39, 40, 53–58, 63), rollback of SQL + finalizer + marker, narrow tolerated-error allowlisting, programmatic-finalizer execution inside the transaction, usage-stats v32/v68/v69 rebuild and recovery, schema diagnosis/repair with pre-repair backup and `afterRepair` hooks, fresh-vs-full repair catalogs, FTS tokenizer probing and dynamic DDL, projection/FTS micro-version invalidation, backup archive layout and sanitization, import increment/overwrite with restore-on-failure, encryption enable/change-password/disable with rollback and validation-table row counts, copy-order dependency resolution and cycle rejection, orphan WAL protection, all startup classifications, inner wrong-password retry, `VerifiedPassword` promotion, cancellation, quarantine of DB/WAL/SHM, partial quarantine failure, no-secret logging, and macOS adapter/fallback behavior without real credentials.

`storage-001` acceptance is limited to the foundation contracts and real implementations in `crates/deepchat-services` and `crates/deepchat-platform`. `storage-002` completes the catalog, repair, FTS, dynamic DDL, backup/import, and migration-overwrite surface described above; only then may the remaining production-schema gaps close.
