---
id: storage-002a-2
scope: storage-sqlcipher
status: done
depends-on: [storage-002a-1]
---

# Schema diagnosis, repair, and startup recovery

## Objective

Complete the schema-integrity slice on top of the real catalog and migration owners delivered by `storage-002a-1`. This task owns diagnosis, repair, pre-repair preservation, all four production `afterRepair` hooks, and the startup one-shot repair path over the real fresh-startup catalog. It must be a complete vertical slice: no mock catalog, stub hook, placeholder hook identity, independent hook transaction, or production-only partial owner set.

The `storage-sqlcipher` feature remains `implemented` until later storage slices and their evidence close every remaining gap. This task does not promote the manifest feature to `verified`, remove FTS/projection or backup/import/encryption gaps, or change the frozen reference.

## Frozen oracle and required reading

Use frozen reference commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd` at `/Users/colab/Documents/workspace/deepchat-2` as a read-only oracle. Relevant selectors are:

- `src/main/data/schemaRepair.ts`, `schemaErrorClassifier.ts`, `databaseInitializer.ts`, `schemaCatalog.ts`, `schemaTypes.ts`, and `baseTable.ts`;
- every catalog owner implementation referenced by the four repair hooks, especially `newEnvironments`, `deepchatPendingInputs`, `agentMemory`, and `agentMemoryAudit`;
- the focused schema-repair/startup tests and the agent-memory FTS/policy, artifact, and projection maintenance code.

Read the target contracts in `docs/INDEX.md`, `docs/storage.md`, `docs/plan/README.md`, `docs/plan/tasks/storage-002.md`, and `docs/plan/tasks/storage-002a-1.md`, plus the latest reviews in `docs/plan/reviews/storage-002a-1-01.md`, `storage-002a-1-02.md`, and `storage-002a-1-03.md` before implementation. Do not access real profiles, databases, Keychain items, credentials, or provider sessions.

## Scope and locked translations

### Catalog boundary and public diagnosis

- Reuse the real `storage-002a-1` metadata and owner identities. The full manual diagnosis/repair catalog has **41 definitions**. The startup catalog has **38 entries**, excluding exactly the legacy `conversations`, `messages`, and `message_attachments` definitions. The four settings owners remain outside both diagnosis catalogs, even though their physical/runtime owner behavior exists: app settings, provider settings, MCP settings, and agent settings.
- Preserve the public diagnosis kinds exactly: `missing_table`, `missing_column`, `column_type_mismatch`, and `missing_index`.
- Preserve normalization exactly: declared and observed column types are trimmed and uppercased; empty types normalize to `null`.
- Read table names from `sqlite_master`, exclude SQLite internals, inspect columns with safely quoted `PRAGMA table_info(...)`, and inspect named non-SQLite indexes by owning table. Quoting must not interpolate an unchecked table name.
- A missing table is repairable. A missing column is repairable only when its catalog column has `addColumnSql`. A type mismatch is non-repairable and is emitted only for `typeCheckedColumns` when the declared type is present and the actual type is absent or different. A missing declared index is repairable.
- Preserve issue order: catalog table order; for each existing table, declared-column order followed by declared-index order. Preserve diagnosis fields and report fields exactly as the frozen public contract: `checkedAt`, `isHealthy`, `issues`, `repairableIssues`, `manualIssues`; reports contain `startedAt`, `finishedAt`, `status`, `backupPath`, `diagnosisBeforeRepair`, `diagnosisAfterRepair`, `repairedIssues`, and `remainingIssues`.
- Internal table/column identities may be used for diagnosis matching and issue deduplication. Public startup observation must expose only stable category/reason/count/duration fields; never expose table names, column names, database paths, raw SQL, or raw rusqlite/IO error chains.

### Repair ordering, backup, and transaction

- Healthy and manual-only diagnosis paths create **no backup**. Manual-only means there are issues but no repairable issues and returns `manual-action-required` without schema mutation.
- A repairable path must execute in this exact order: diagnose; `PRAGMA wal_checkpoint(TRUNCATE)`; copy only the main database file to `<dbPath>.<UTC ISO timestamp with ':' and '.' replaced by '-'>.repair.bak`; then begin one transaction.
- Use the existing injected Clock milliseconds and service-side UTC ISO formatting. Do not add a collision suffix or preflight collision allocator. Inject a dedicated `RepairFileSystem` copy port for this operation; do not reuse `QuarantineFileSystem`. The production adapter uses `std::fs::copy`, matching the frozen reference's overwrite-capable copy semantics when the exact timestamped destination already exists. Missing/unavailable backup paths and copy failures are typed repair errors.
- Checkpoint/copy failure happens before schema mutation and returns a typed repair error. A successfully created backup remains on disk after any later transaction or hook failure; the repair path never deletes it during rollback.
- One transaction must contain, in catalog order, missing-table creation, repairable-column additions, missing-index creation, and every affected `afterRepair` hook. The hook receives the set of columns actually added for its table; a missing-table repair receives an empty added-column set. Hooks must run through the current Rust `Transaction`, never by issuing an independent `BEGIN` or opening a second connection.
- A SQL or hook failure rolls back all schema and hook effects in that transaction while retaining the successful backup and returns a safe typed repair error. No public error may expose raw SQL, table/column identities, filesystem path, rusqlite text, or an `Error::source()` chain.
- Re-diagnose only after a successful transaction. Return `repaired` when the post-repair diagnosis is healthy and at least one issue was repaired; `healthy` for a no-op healthy result; and `manual-action-required` when non-repairable or residual issues remain. A successful repair report may retain manual issues and must preserve before/after diagnoses and repaired/remaining issue sets.

### Four required real hooks

Port and wire the complete frozen behavior, constants, SQL, trigger names, and exact trigger conditions for all four owners. Hook identity must be explicit in catalog metadata and covered by generated-fixture tests.

1. **`new_environments`**: run the real rebuild-from-sessions behavior after the table is created or otherwise affected; do not replace it with a no-op or a simplified copy.
2. **`deepchat_pending_inputs`**: when `retry_required_at` is added, run the real normalization that changes `retry_required` rows to `blocked`, fills `retry_required_at` with `COALESCE(retry_required_at, updated_at, created_at)`, and clears `blocking_json`. Do not normalize unrelated rows. A missing-table hook still receives an empty added-column set.
3. **`agent_memory`**: implement the complete canonical-state repair behavior: lifecycle and embedding backfills, shadow reconciliation, retired and canonical index maintenance, legacy-status bridge replacement, scope artifacts, temporal artifacts, and all required lineage/dirty/clear artifacts according to the exact added-column trigger conditions. Include required FTS meta invalidation when lifecycle/embedding state repair requires it, and preserve the frozen agent-memory FTS meta key/version/policy invalidation behavior. Do not defer required artifact maintenance to `storage-002a-3`; only connection-scoped tokenizer probing, dynamic FTS virtual-table creation, and tape/memory projection lifecycle are deferred.
4. **`agent_memory_audit`**: run the real `memory_ref_id` backfill after creation or repair, preserving the frozen update semantics and transaction participation.

The implementation must not omit the agent-memory repair hook's FTS meta invalidation or artifact maintenance merely because the broader FTS/projection slice is deferred.

### Classifier and startup one-shot behavior

- Keep the public schema-repair reason domain stable: `missing-table`, `missing-column`, `column-count-mismatch`, and `type-mismatch`. The raw-error classifier recognizes the frozen patterns for `no such table`, `has no column named`, `no such column`, and `table ... has N columns but M values were supplied`; `type-mismatch` comes from diagnosis/reporting rather than a raw SQLite-message pattern. Preserve stable internal dedupe keys while redacting them from startup observations.
- Startup normal path is: open/validate; diagnose against the real startup catalog of 38; at most one repair attempt; close/reopen around file repair; then diagnose again. Diagnosis failure is `unavailable` and startup continues. Manual-only issues continue. Repairable residual issues after the single attempt continue and are observable. Repair or reopen failure fails startup. Observer failures cannot affect startup.
- Construction-time schema errors classified as the recognized non-destructive reasons trigger one startup-scoped repair. Destructive, unclassified, and second failures do not trigger repair. Preserve the existing storage-owned password/corruption classification and do not reintroduce caller-provided verification flags.
- Public startup observations contain only stable outcome/category/reason/count/duration fields. No table/column/path/raw error text may enter logs, observer payloads, evidence, or user-facing errors.

## Implementation paths

- `crates/deepchat-services`: public diagnosis/report types, catalog-driven inspector and repair service, transaction-scoped hook dispatch, classifier, startup loop, typed redacted errors, the injected repair filesystem boundary, and its standard-library production adapter. This matches the existing storage/quarantine filesystem-port convention and avoids introducing a reverse dependency from `deepchat-services` to `deepchat-platform`.
- `crates/deepchat-platform`: no change is required unless a concrete application-owned platform caller appears; do not add an unused adapter or alter secret-store ownership.
- `crates/deepchat-services/tests/` and generated isolated fixture helpers: SQLCipher fixture databases, deterministic clocks, fake repair filesystem, failure injection, and Storage-level integration coverage.
- `docs/plan/tasks/storage-002a-2.md`, `docs/INDEX.md`, `docs/plan/README.md`, `PORTING.md`, `parity/manifest.json`, and (only if necessary) `docs/plan/backlog.md` are documentation/status files for this slice. Do not modify Rust code, manifests, evidence, or the frozen reference while preparing this contract.

## Acceptance evidence

Use generated isolated SQLCipher fixture databases only. Use deterministic clocks and a fake filesystem only in tests; production must use the injected port. Enumerate and implement adversarial tests for:

1. exact 41-entry manual versus 38-entry startup catalog, three legacy exclusions, four settings-owner exclusions, and issue ordering;
2. type normalization, safe identifier quoting, each diagnosis kind, repairability, dedupe, and exact diagnosis/report fields;
3. healthy/manual-only no-backup paths;
4. checkpoint-before-copy-before-transaction ordering, UTC backup naming, main-file-only copy, no collision suffix/preflight allocator, exact-destination overwrite semantics, checkpoint failure, copy failure, and backup retention;
5. missing-table creation, all repairable-column additions, missing indexes, added-column sets, empty set for missing-table hooks, and one transaction for SQL plus all affected hooks;
6. transaction rollback of missing tables/columns/indexes and every hook failure while retaining the backup;
7. all four hooks, including environment rebuild, pending-input normalization, audit backfill, agent-memory lifecycle/embedding repair, canonical indexes, scope/temporal/lineage/dirty/clear artifacts, legacy bridge, and FTS meta invalidation;
8. classifier stable reasons/patterns, adversarial raw-driver text, redaction, and no public source chains;
9. startup one-shot repair loop, close/reopen, diagnosis unavailable continuation, manual-only continuation, residual continuation, repair/reopen failure, construction-time recognized errors, destructive/unclassified/second-failure refusal, and observer-failure isolation;
10. fresh/full catalog distinctions and real end-to-end integration through `Storage`, proving the startup path uses the 38-entry catalog while explicit/manual repair can use all 41 definitions.

Keep FTS/projection tokenizer probing, dynamic virtual-table DDL, and projection micro-version invalidation assigned to `storage-002a-3`, but do not omit the agent-memory hook behavior explicitly required above.

## Required verification commands

Run from the repository root after implementation:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
uv run python tools/parity-audit/validate.py
git diff --check
```

Also run a targeted storage test command for the diagnosis/repair/startup suite, a parity audit, and a whitespace scan appropriate to this repository's currently untracked state. Record reproducible evidence only after all checks pass. Do not commit.

## Completion boundary

This task is complete only when the real diagnosis, repair, backup ordering, all four hooks, classifier, and one-shot startup path are implemented and covered by generated-fixture and Storage integration evidence. Leave `storage-sqlcipher` at manifest status `implemented` and preserve every remaining gap, including FTS/projection, backup/import, encryption-change, integration-gate work outside this slice, and target-policy Keychain naming.
