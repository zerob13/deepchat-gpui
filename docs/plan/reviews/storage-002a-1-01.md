# Review: storage-002a-1

**Reviewer**: verify agent (independent adversarial review)
**Date**: 2026-09-02
**Task**: [storage-002a-1](../tasks/storage-002a-1.md)
**Frozen reference**: `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd` (tag v1.1.1) at `/Users/colab/Documents/workspace/deepchat-2`

## Judgment: PASS (with findings)

All 11 production-schema tests pass, the complete workspace (53 tests) passes, `cargo fmt --check`/`cargo clippy`/`parity-audit validate.py` all pass. The core topology counts (41/39/38/38), migration version map through 69, 19 empty markers, all per-version programmatic finalizers, and required rebuild behavior are implemented and independently verified against the frozen reference. The findings below are material but none violates a blocking contract: the implementation correctly serves the declared `storage-002a-1` scope.

---

## Checks run (real results)

| Check | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace` | PASS — 53 passed, 0 failed |
| `cargo test -p deepchat-services --test production_schema` | PASS — 11 passed, 0 failed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |
| `git diff --check` | PASS (weak — repo has no tracked files/commits) |
| Catalog JSON topology inspection (Python script) | 41 defs, 39 owners, 38 runtime, 38 startup, 19 empty versions, v25/v26 distinction, v30/v32/v68 copy-drop-rename, v23 conditional columns ✓ |
| Frozen reference cross-check (`git show ca75acfdc...` × `schemaCatalog.ts`, `agentMemory.ts`, `deepchatSessions.ts`, `deepchatUsageStats.ts`, `deepchatPendingInputs.ts`, `liveDelegations.ts`) | Owner list, finalizer version-to-identity map, v23 17 columns, v67 SQL, v69 category-aware PK probe, v60 trigger names matched |
| Error redaction audit (`production_schema.rs` error types, `error.rs`/`startup.rs` `Debug` impls, test `required_dynamic_finalizers_install_real_triggers_and_errors_are_redacted`) | PASS — public errors never embed table names, raw SQL, or driver text |

---

## Findings

### F1 — `existing_baseline_sql` creates latest schema, not v10 baseline (P2, non-blocking)

**Locations**:
`crates/deepchat-services/src/production_catalog.json` — `existingBaselineSql` (53968 bytes)
`crates/deepchat-services/src/production_schema.rs:267-270` — `create_existing_baseline`
`crates/deepchat-services/tests/production_schema.rs:205-229` — test `existing_generated_fixture_runs_every_version_and_exact_empty_markers`

**Evidence**: The baseline SQL (extracted from `production_catalog.json`) contains columns added in v11+:
- `session_kind`, `parent_session_id` (v11) — FOUND
- `system_prompt`, `temperature`, `reasoning_effort` (v12) — FOUND
- `usage_id`, `usage_category`, `compaction_attempt_id` (v68) — FOUND

The SQL uses `CREATE TABLE` (not `CREATE TABLE IF NOT EXISTS`) and creates the latest (v69) schema shape. The method's doc comment says "version-10 owner shapes" — this is incorrect.

**Impact**: The test `existing_generated_fixture_runs_every_version_and_exact_empty_markers` proves that migrations from v10→v69 run *idempotently* (record markers, tolerate duplicate columns via the allowlist), but it does NOT prove that migrations *transform* a real v10 schema into a v69 schema. All ALTER TABLE ADD COLUMN statements hit the `duplicate column name` tolerance path.

**Mitigation**: Individual migration tests (`v23_recovers_only_missing_deepchat_session_columns`, `usage_stats_v32_and_v68_are_copy_drop_rename_rebuilds`, `v69_repairs_bad_v68_shape_and_leaves_correct_table_unchanged`, `acp_v30_rebuild_preserves_rows_and_replaces_schema`) each create hand-crafted pre-migration tables and verify transformation behavior. The per-migration SQL IS adversarially tested; only the end-to-end v10→v69 sequential interaction is not.

**Recommendation**: Either (a) replace `existingBaselineSql` with a true v10 schema extracted from the frozen reference's v10 state, or (b) rename the method/doc to "latest owner shapes" and add a note that end-to-end transformation coverage relies on the per-version tests. Option (b) is sufficient for this task scope.

---

### F2 — Fresh-database static schema and v69 marker are separate transactions (P3, non-blocking)

**Locations**:
`crates/deepchat-services/src/production_schema.rs:272-281` — `create_static_schema` (commits its own tx)
`crates/deepchat-services/src/production_schema.rs:303-316` — `initialize` (calls `create_static_schema`, then `MigrationRunner.run`)
`crates/deepchat-services/src/schema.rs:145-179` — `run_classified` (inserts v69 marker in a separate step)

**Evidence**: `create_static_schema` opens and commits a transaction for all 39 physical owners (line 273-280). `initialize` then calls `MigrationRunner.run(fresh=true)`, which starts a new transaction, creates `schema_versions`, and inserts marker 69. A crash between these two commits would leave a full schema with no version marker.

**Impact**: On restart, `db_path.exists()` returns true → `fresh_database=false` → migrations run v1..v69 on an already-latest-schema database. The allowlist (`duplicate column name`, `already exists`, `no such column`) absorbs most errors, making the path idempotent in practice. The reference (`initializeDatabase` → `createTables()` → `migrate()`) has the same two-phase pattern.

**Recommendation**: Document the resilience guarantee explicitly. Optionally merge both steps into one transaction by passing the tx into the migration runner, but this is not required for current correctness.

---

### F3 — v42 `agent_memory` finalizer subquery may fail if `agent_memory_v42_added_columns` is absent (P3, non-blocking)

**Locations**:
`crates/deepchat-services/src/production_schema.rs:492-506` — v42 finalizer branch
Frozen reference: `agentMemory.ts:1650-1670` — `finalizeMigration(42)` checks `sqlite_temp_master` before acting

**Evidence**: Our v42 finalizer uses a subquery `(SELECT COUNT(*) FROM agent_memory_v42_added_columns)=2` inside an UPDATE WHERE clause (line 501). If the marker table doesn't exist when the UPDATE runs, the subquery produces a SQL error that rolls back the entire v42 transaction. The reference guards this by checking `sqlite_temp_master` for the table existence first and returning early if absent.

**Practical severity**: Low. The v42 migration SQL creates `agent_memory_v42_added_columns` in the same transaction, so the table should always exist when the finalizer runs. This is a robustness edge case for unexpected intermediate states (e.g., a v42 re-run on a partially-migrated database).

**Recommendation**: Add a `SELECT EXISTS(...)` guard before the UPDATE, matching the reference pattern.

---

### F4 — `assert_current_schema` depth is narrower than reference (P3, non-blocking)

**Locations**:
`crates/deepchat-services/src/production_schema.rs:318-391` — `assert_current_schema`
Frozen reference: `agentMemory.ts` `assertCurrentSchema` (~120 lines of constraint + row validation)

**Evidence**: Our assertion checks: 11 column names, 5 auxiliary tables, 6 indexes, 10 triggers, and 5 `agent_memory_directive` SQL fragments. The reference additionally validates: column `NOT NULL`/default-value constraints, `CHECK` constraint SQL text for `lifecycle_state`/`embedding_state`/`temporal_kind`/`scope_type`, invalid temporal row counts, and invalid scope row counts. The reference also runs additional index maintenance SQL (`AGENT_MEMORY_BASE_INDEX_SQL`, `AGENT_MEMORY_RETIRED_INDEX_SQL`, `AGENT_MEMORY_CONFLICT_INDEX_SQL`).

**Impact**: A database could pass our assertion while having columns with wrong NOT NULL defaults, missing CHECK constraints, or rows with invalid temporal/scope data. The connection-scoped FTS/projection lifecycle and index maintenance belong to later `storage-002` tasks, but the CHECK/default constraint validation gap is not explicitly deferred.

**Recommendation**: Note the scope difference in a code comment. The column-level constraint validation could be added incrementally without destabilizing the current contract.

---

## Verified contracts

| # | Contract | Status | Evidence |
|---|---|---|---|
| 1 | 41 definitions, 39 physical, 38 runtime, 38 startup, legacy exclusions, settings placement, `acp_turns` exclusion | ✅ | `catalog_topology_and_membership_are_exact` test + JSON inspection |
| 2 | Every owner has expected latest version; aggregate max = 69 | ✅ | `metadata_preserves_columns_indexes_repair_hooks_and_owner_versions` test |
| 3 | Fresh SQLCipher creates complete static schema before v69 marker | ✅ | `fresh_production_storage_creates_static_schema_before_only_v69_marker` test |
| 4 | Existing database records versions 10..69 including 19 empty markers | ✅ | `existing_generated_fixture_runs_every_version_and_exact_empty_markers` test (see F1) |
| 5 | SQL + finalizer + marker share one transaction and roll back together | ✅ | `owner_sql_finalizer_and_marker_roll_back_together` test |
| 6 | v23 conditional recovery (17 columns), v25/v26 `config_migrations` distinction | ✅ | `v23_recovers_only_missing_deepchat_session_columns` + `v26_normalization_creates_all_normalized_tables_without_recreating_config_migrations` tests |
| 7 | `acp_sessions` v30 and `deepchat_usage_stats` v32/v68 copy→drop→rename | ✅ | `acp_v30_rebuild_preserves_rows_and_replaces_schema` + `usage_stats_v32_and_v68_are_copy_drop_rename_rebuilds` tests |
| 8 | v69 detects/repairs category-aware PK gap; leaves correct table unchanged | ✅ | `v69_repairs_bad_v68_shape_and_leaves_correct_table_unchanged` test |
| 9 | Static creation has no FTS virtual table SQL; dynamic finalizers are real | ✅ | Test assertion line 201 + `required_dynamic_finalizers_install_real_triggers_and_errors_are_redacted` test |
| 10 | Public errors redact raw SQL/driver text | ✅ | `required_dynamic_finalizers_install_real_triggers_and_errors_are_redacted` test lines 515-517; `Debug` impls in `startup.rs:41-53`, `error.rs:15-23` |
| — | Tolerated-error allowlist not widened | ✅ | `schema.rs:335-355` — same 3 cases as `storage-001` |
| — | Schema-level finalizer distinction preserved | ✅ | `assert_current_schema` runs after migration runner in `initialize`; not a per-version marker |

---

## Remaining uncertainty

1. **No real SQLCipher file I/O tested**: All tests use `Connection::open_in_memory()`. The SQLCipher bundling (`rusqlite` with `bundled-sqlcipher-vendored-openssl`) is compile-time verified but not exercised through file-based database paths. The `storage_contract.rs` integration tests (13 passing) use generated temp directories and DO exercise file I/O, but they use the `storage-001` catalog, not the full production schema. The `fresh_production_storage` test uses `tempfile::tempdir()` and a file path, so this is partially covered.

2. **Existing baseline fixture generation source**: The `existingBaselineSql` JSON value (53968 bytes) was produced by executing reference owner classes against a generated in-memory database per the evidence README. Without reproducing this extraction, the reviewer can only verify that the SQL contents are internally consistent (all 51 tables exist, no FTS, CREATE TABLE not IF NOT EXISTS). The provenance is documented but not independently reproducible within this review.

3. **Keychain service/account naming**: Declared as target policy in manifest. The frozen reference has no Keychain naming contract; no parity verification needed per `docs/storage.md:268`.

4. **`deepchat_tape_entries` owner has 4247-byte create_sql with 11 indexes**: This is the largest create SQL (11 index definitions). The table and its indexes are created by the static catalog but not directly tested in `production_schema.rs`. The tape-related migration and FTS lifecycle belong to later tasks.