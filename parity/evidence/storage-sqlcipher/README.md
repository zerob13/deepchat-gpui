# storage-sqlcipher evidence

## Judgment

`storage-sqlcipher` is **implemented**, not verified. The implemented surface includes the `storage-001` SQLCipher/startup foundation, the `storage-002a-1` production static catalog and global migration-owner layer, and the independently reviewed `storage-002a-2` schema diagnosis/repair, production repair hooks, backup ordering, and startup one-shot recovery slice. The complete connection-scoped FTS/projection lifecycle, backup/import, migration overwrite, encryption change, non-macOS runtime evidence, and the full integration gate remain later tasks.

## Frozen reference

- Tag: `v1.1.1`
- Commit: `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`
- Read-only source: `/Users/colab/Documents/workspace/deepchat-2`

The checked-in catalog metadata was extracted by executing the reference owner classes against generated in-memory databases. The file-backed historical v10 fixture is traced to the eight static owner implementations at frozen-reference ancestor `f9adbcb6b7807c91e544b0e7fd24d46df53d4fd3^`, whose owner high-water mark was v10. No real profile, database, Keychain item, provider credential, or environment secret was read.

## Reproduced topology and migration facts

| Fact | Reproduced result |
|---|---:|
| Diagnosis/repair catalog definitions | 41 |
| Physical create owners | 39 |
| Runtime migration owners | 38 |
| Fresh-startup diagnosis definitions | 38 |
| Latest global version | 69 |
| Functionally empty runtime versions | 19 (`1-10`, `39`, `40`, `53-58`, `63`) |
| Focused `storage-002a-1` integration tests | 14 passed, 0 failed |
| Focused `storage-002a-2` integration tests | 24 passed, 0 failed |
| Private startup fault tests | 4 passed, 0 failed |
| Full workspace tests | 88 passed, 0 failed |

The three legacy definitions (`conversations`, `messages`, `message_attachments`) remain catalog-only and are excluded from fresh creation/runtime migration. `acp_turns` remains physical-create-only. The four settings owners remain physical/runtime owners but stay outside both diagnosis catalogs. Static creation contains no FTS virtual-table SQL.

## Reproducible commands and results

| Command/check | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace` | PASS — 88 passed, 0 failed |
| `cargo test -p deepchat-services --test production_schema` | PASS — 14 passed, 0 failed |
| `cargo test -p deepchat-services --test schema_repair` | PASS — 24 passed, 0 failed |
| `cargo test -p deepchat-services startup::tests -- --nocapture` | PASS — 4 startup unit tests passed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |
| `git diff --check` | PASS, but weak because the repository has no tracked files/commit |
| Explicit untracked-tree trailing-whitespace scan | PASS |
| Database/WAL/SHM/repair-backup pollution scan | PASS |

Full workspace test accounting: 88 passed, 0 failed. The focused storage suites include 14 production-schema integration tests, 24 schema-repair integration tests, and 4 private startup fault tests.

## Acceptance evidence

1. `catalog_topology_and_membership_are_exact` proves 41/39/38/38 membership, legacy/settings placement, and `acp_turns` exclusion.
2. `metadata_preserves_columns_indexes_repair_hooks_and_owner_versions` proves complete create metadata, column/index/repair-hook identities, owner latest versions, v69 aggregate maximum, exact empty versions, and the v25/v26 `config_migrations` distinction.
3. `fresh_production_storage_creates_static_schema_before_only_v69_marker` proves the real production entry point creates all owner schemas before recording only marker 69 and emits no static FTS virtual table.
4. `historical_v10_file_runs_real_transformations_through_production_path` opens a generated file-backed historical v10 database through the production entry point, proves v11+ columns were initially absent, preserves data, records exact markers through 69, and matches the fresh static schema's object/column/constraint signature.
5. `static_create_marker_crash_gap_recovers_through_production_reopen` simulates interruption after static creation but before marker creation, then proves production reopen reaches marker 69 without data loss. `owner_sql_finalizer_and_marker_roll_back_together` separately forces marker failure after v67 owner SQL/finalization and proves all three roll back.
6. `v23_recovers_only_missing_deepchat_session_columns` and `v26_normalization_creates_all_normalized_tables_without_recreating_config_migrations` cover required conditional/normalization behavior.
7. `acp_v30_rebuild_preserves_rows_and_replaces_schema` and `usage_stats_v32_and_v68_are_copy_drop_rename_rebuilds` prove row-preserving rebuild semantics.
8. `v69_repairs_bad_v68_shape_and_leaves_correct_table_unchanged` proves the three-part primary-key probe, recovery rebuild, and correct-table no-op.
9. Unit tests cover the exact v42 `sqlite_temp_master` guard, absent-marker return, matching-path update, and marker cleanup. `schema_finalizer_rejects_corrupted_constraints_and_rows` adversarially covers NOT NULL/default/CHECK and invalid temporal/scope rows; `schema_finalizer_maintains_memory_and_directive_indexes` removes every canonical agent-memory index, installs obsolete/conflicting indexes, then proves the full base/retired/conflict/canonical maintenance sequence recreates the canonical identities and drops retired/conflicting names.
10. `required_dynamic_finalizers_install_real_triggers_and_errors_are_redacted` proves live-delegation and agent-memory finalizers install real trigger artifacts and public failures remain source-free/redacted.

The static finalizer deliberately excludes only connection-scoped tokenizer selection, FTS virtual tables/triggers, and projection rebuild lifecycle, which remain assigned to `storage-002a-3`.

## Schema repair and startup recovery evidence

Independent review [storage-002a-2 review 05](../../../docs/plan/reviews/storage-002a-2-05.md) passed all ten acceptance-evidence groups. The generated-fixture suites prove the exact ordered 41-definition manual and 38-definition startup catalogs, diagnosis kinds and ordering, checkpoint/copy/transaction sequencing, overwrite-capable main-file backup, rollback with backup retention, exact added-column sets, all four production repair hooks, safe classifier behavior, and the complete one-shot startup continuation/failure/refusal matrix. Public observations and errors remain free of table, column, path, SQL, and raw driver identities.

## Remaining scope

- Complete connection-scoped FTS/tokenizer/projection lifecycle.
- Backup/import/overwrite and encryption enable/change/disable workflows.
- Full integration gate and non-macOS runtime verification.
- Keychain service/account naming remains target policy because the frozen reference has no corresponding contract.
