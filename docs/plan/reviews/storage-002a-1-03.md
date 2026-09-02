# Review: storage-002a-1

**Reviewer**: independent verify agent
**Date**: 2026-09-02
**Task**: [storage-002a-1](../tasks/storage-002a-1.md)
**Judgment**: PASS — no blocking contract remains

## Scope and safety

This review re-read [storage-002a-1-01](storage-002a-1-01.md) and [storage-002a-1-02](storage-002a-1-02.md), inspected the current implementation/tests/evidence, and compared the F5 fix with frozen reference commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd` in read-only `/Users/colab/Documents/workspace/deepchat-2`. No real profile, database, Keychain item, credential, provider login, or environment secret was accessed or modified. The reference worktree already contained unrelated modifications; they were not touched.

## Required checks

| Check | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace` | PASS — 58 passed, 0 failed (app 9, platform 3, services unit 19, production schema 14, storage contract 13, doc tests 0) |
| `cargo test -p deepchat-services --test production_schema schema_finalizer_maintains_memory_and_directive_indexes -- --exact` | PASS — 1 passed, 0 failed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |
| `git diff --check` | PASS; repository has no tracked baseline, so this is only a whitespace check |

## F5 re-review: canonical index maintenance

**Status: CLOSED.** The current maintenance block at [production_schema.rs:423-454](../../crates/deepchat-services/src/production_schema.rs#L423) contains the complete canonical-index behavior required by the frozen reference:

- drops `idx_agent_memory_embedding_queue` and `idx_agent_memory_lifecycle_maintenance`;
- creates `idx_agent_memory_active_recall`;
- creates `idx_agent_memory_management_page_v3` with the reference predicate excluding conflicted, superseded, persona, and working rows;
- creates `idx_agent_memory_archive_eligible_v3` with the reference `COALESCE(last_accessed, created_at)`, active/non-superseded/non-conflicted/non-anchor predicate;
- creates `idx_agent_memory_cognitive_top_v3` with the reference importance ordering and kind predicate;
- creates `idx_agent_memory_conflict_fairness_v3`;
- creates `idx_agent_memory_recent_activity_v3`;
- creates both pending-embedding indexes with the reference predicates;
- creates `idx_agent_memory_conflict_target_v2`.

Direct comparison against frozen `agentMemory.ts` `AGENT_MEMORY_CANONICAL_INDEX_SQL` found the same 11 statements, names, expressions, ordering, and predicates. `assert_current_schema` invokes this maintenance after constraint and invalid-row checks at [production_schema.rs:349-355](../../crates/deepchat-services/src/production_schema.rs#L349). The generated-fixture test [production_schema.rs:671-720](../../crates/deepchat-services/tests/production_schema.rs#L671) removes all canonical indexes, installs obsolete indexes, runs initialization, and proves every required base/conflict/canonical/directive index is recreated while the obsolete conflicting names are removed.

The fix does not expand the deferred boundary: connection-scoped tokenizer selection, FTS virtual tables/triggers, and projection rebuilds remain assigned to later storage-002 work, as documented at [production_schema.rs:352-355](../../crates/deepchat-services/src/production_schema.rs#L352).

## Blocking-contract audit

| Contract | Judgment | Evidence |
|---|---|---|
| 41 catalog definitions; 39 physical owners; 38 runtime owners; 38 startup definitions | PASS | `catalog_topology_and_membership_are_exact` |
| Legacy definitions excluded correctly; `acp_turns` physical-only; settings-owner placement | PASS | topology test and checked-in catalog metadata |
| Complete metadata: create SQL, columns, indexes, repair identities, versions/finalizers | PASS | `metadata_preserves_columns_indexes_repair_hooks_and_owner_versions` |
| Fresh production path creates complete static schema before marker 69 | PASS | `fresh_production_storage_creates_static_schema_before_only_v69_marker` |
| Existing v10 fixture transforms through ordered migrations to 69 and preserves rows | PASS | `historical_v10_file_runs_real_transformations_through_production_path` |
| One transaction for version SQL, finalizers, and marker; rollback preserves earlier versions | PASS | `owner_sql_finalizer_and_marker_roll_back_together` |
| Exact 19 functionally empty versions and narrow tolerated-error allowlist | PASS | catalog/version tests and `schema.rs` allowlist tests |
| v23 conditional recovery and v25/v26 settings distinction | PASS | `v23_recovers_only_missing_deepchat_session_columns`; `v26_normalization_creates_all_normalized_tables_without_recreating_config_migrations` |
| `acp_sessions` v30 and usage-stats v32/v68 copy-drop-rename rebuilds | PASS | `acp_v30_rebuild_preserves_rows_and_replaces_schema`; `usage_stats_v32_and_v68_are_copy_drop_rename_rebuilds` |
| v69 category-aware primary-key detection/recovery/no-op | PASS | `v69_repairs_bad_v68_shape_and_leaves_correct_table_unchanged` |
| v42 absent temporary marker guard | PASS | production-schema unit tests `v42_finalizer_returns_safely_without_temp_marker` and cleanup path |
| Schema-level `assertCurrentSchema` ordering/distinction | PASS | initialization runs migration completion before `assert_current_schema`; canonical, constraint, row, ordinary-index, trigger, and directive checks are present |
| Static FTS boundary and real dynamic finalizers | PASS | static catalog assertion and `required_dynamic_finalizers_install_real_triggers_and_errors_are_redacted` |
| Public error redaction | PASS | same dynamic-finalizer/error test; typed errors expose no SQL/driver/source details |

No regression from the prior review findings F1–F4 was found. F1’s file-backed historical v10 fixture, F2’s documented reference two-phase recovery, F3’s `sqlite_temp_master` guard, and F4’s constraint/invalid-row/index maintenance remain present.

## Decision and changed files

Because all blocking contracts pass, the task is now marked `status: done` in [storage-002a-1.md](../tasks/storage-002a-1.md#L4). The `storage-sqlcipher` feature remains `implemented` in [parity/manifest.json](../../parity/manifest.json#L362); only the stale independent-review gap was removed. Later schema diagnosis/repair, connection-scoped FTS/projection, backup/import/overwrite, encryption-change, integration-gate, and Keychain naming gaps remain unchanged.

Changed files:

- [docs/plan/reviews/storage-002a-1-03.md](storage-002a-1-03.md) — this review.
- [docs/plan/tasks/storage-002a-1.md](../tasks/storage-002a-1.md) — status `done`.
- [parity/manifest.json](../../parity/manifest.json) — removed the stale `storage-002a-1 awaits independent review` gap; preserved later gaps and `implemented` status.

No production code was modified. No commit was created.

## Remaining uncertainty

The review verifies the required task contracts and the complete canonical index SQL against the frozen selector, but it does not establish full byte-for-byte equivalence of every generated catalog SQL statement with every frozen owner implementation. Non-macOS runtime behavior and later storage-002 integration surfaces remain outside this task.
