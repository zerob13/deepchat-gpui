# Review: storage-002a-1

**Reviewer**: independent verify agent
**Date**: 2026-09-02
**Task**: [storage-002a-1](../tasks/storage-002a-1.md)
**Frozen reference**: `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd` (read-only) at `/Users/colab/Documents/workspace/deepchat-2`

## Judgment: FAIL — blocking finding remains

The implementation closes prior findings F1, F2, and F3, and materially closes the constraint/invalid-row portion of F4. However, the requested F4 parity contract is not complete: `assert_current_schema` does not perform the full reference index-maintenance set. The task remains `in-progress`.

## Required checks

| Check | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace` | PASS — 58 passed, 0 failed (app 9, platform 3, services unit 19, production schema 14, storage contract 13, doc tests 0) |
| `cargo test -p deepchat-services --test storage_contract sqlcipher_4_utf8_keying_and_wal_ordering -- --exact` | PASS — 1 passed, 0 failed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |
| Whitespace scan | PASS for touched implementation/evidence paths; `git diff --check` was not used as evidence because this repository has no tracked files/commits and the work is untracked |

## Blocking finding

### F5 — `assert_current_schema` omits reference canonical index maintenance (P1, blocking)

**Location**: [crates/deepchat-services/src/production_schema.rs:499-501](../../crates/deepchat-services/src/production_schema.rs#L499)

`maintain_agent_memory_indexes` executes `AGENT_MEMORY_INDEX_MAINTENANCE_SQL`, which currently contains the base, retired, and conflict index groups, but not the reference's canonical index SQL. The frozen reference's `agentMemory.assertCurrentSchema` executes all of:

- `AGENT_MEMORY_BASE_INDEX_SQL`
- `AGENT_MEMORY_RETIRED_INDEX_SQL`
- `AGENT_MEMORY_CONFLICT_INDEX_SQL`
- `AGENT_MEMORY_CANONICAL_INDEX_SQL`

The current Rust maintenance block has no equivalent of `AGENT_MEMORY_CANONICAL_INDEX_SQL` (including the active-recall, management-page-v3, archive-eligible-v3, cognitive-top-v3, conflict-fairness-v3, recent-activity-v3, and related canonical indexes). The implementation's own test only checks a subset and therefore does not detect this omission: [crates/deepchat-services/tests/production_schema.rs:671-696](../../crates/deepchat-services/tests/production_schema.rs#L671).

**Reproduction**:

1. Create the static schema.
2. Create `schema_versions` with marker 69.
3. Call `ProductionSchemaCatalog::frozen().initialize(&conn, false, clock)`.
4. Query `sqlite_master` for a canonical index such as `idx_agent_memory_active_recall`.

The finalizer succeeds, but the canonical index is absent. By contrast, the frozen reference's `assertCurrentSchema` creates that index family on the same path.

**Required fix**: Port the complete reference `AGENT_MEMORY_CANONICAL_INDEX_SQL` into the maintenance/finalizer path, preserve its drops and predicates, and add generated-fixture assertions that remove canonical indexes before initialization and prove all required canonical indexes are recreated. Do not weaken the existing deferred boundary for connection-scoped FTS/tokenizer/projection behavior.

## Prior findings re-reviewed

- **F1 — true v10 baseline transformation: CLOSED.** The former latest-schema `existingBaselineSql` path is gone. [crates/deepchat-services/tests/fixtures/production_v10.sql](../../crates/deepchat-services/tests/fixtures/production_v10.sql) is a file-backed v10 fixture; `historical_v10_file_runs_real_transformations_through_production_path` proves pre-v11 columns are absent, data survives, versions 10 through 69 are recorded, and selected schema signatures match fresh creation ([crates/deepchat-services/tests/production_schema.rs:242-326](../../crates/deepchat-services/tests/production_schema.rs#L242)).
- **F2 — fresh-schema versus marker transaction behavior: CLOSED for this task contract.** `create_static_schema` and marker migration remain separate commits, but [crates/deepchat-services/src/production_schema.rs:300-308](../../crates/deepchat-services/src/production_schema.rs#L300) documents the reference two-phase behavior and the reopen recovery test proves an interruption after static creation reaches marker 69 while preserving data ([crates/deepchat-services/tests/production_schema.rs:328-361](../../crates/deepchat-services/tests/production_schema.rs#L328)).
- **F3 — v42 `sqlite_temp_master` guard: CLOSED.** The v42 finalizer checks `sqlite_temp_master` before querying the temporary marker and safely returns when absent ([crates/deepchat-services/src/production_schema.rs:589-601](../../crates/deepchat-services/src/production_schema.rs#L589)). Unit tests cover absent-marker safety, guarded update, and cleanup ([crates/deepchat-services/src/production_schema.rs:719-774](../../crates/deepchat-services/src/production_schema.rs#L719)).
- **F4 — `assertCurrentSchema` constraints and invalid rows: PARTIALLY CLOSED.** The current code validates NOT NULL/default values, CHECK fragments, invalid temporal rows, invalid scope rows, and performs base/retired/conflict index maintenance ([crates/deepchat-services/src/production_schema.rs:311-355](../../crates/deepchat-services/src/production_schema.rs#L311)). The missing canonical group is recorded as F5 above, so F4 is not fully closed.

## Contract audit

- Catalog topology: **PASS** — 41 definitions, 39 physical owners, 38 runtime owners, 38 startup definitions; legacy exclusions, settings-owner placement, and `acp_turns` runtime exclusion verified by test and JSON inspection.
- Versions and empty markers: **PASS** — latest version 69; exact empty set is `1-10, 23, 39, 40, 53-58, 63` in the raw owner map, with v23 handled conditionally; the declared exact 19 functionally empty markers are preserved.
- Owner metadata and finalizers: **PASS** — owner create SQL, columns, indexes, repair-hook identities, latest versions, and finalizers for agent memory (42/46/48/49/51/52), live delegation (60), pending inputs (67), and usage stats (69) are present and tested.
- v23/v25-v26/v30/v32/v68/v69 semantics: **PASS** — targeted tests cover conditional session recovery, settings normalization distinction, copy/drop/rename rebuilds, compatible-row preservation, and the three-part category-aware primary-key probe.
- Transactions and tolerated errors: **PASS** — per-version SQL, finalizer, and marker share the runner transaction; rollback test passes; the narrow allowlist remains unchanged.
- Typed redacted errors: **PASS** — public production-schema/startup/migration errors have no raw SQL, driver text, table-specific detail, or `source()`.
- Real SQLCipher file-path behavior: **PASS with limited scope** — production schema tests use temporary file paths, and the SQLCipher file-path/keying integration test passes; no real user profile, database, Keychain, credential, or provider login was accessed.
- Static FTS boundary: **PASS** — catalog owner SQL contains no `CREATE VIRTUAL TABLE`; deferred connection-scoped FTS/tokenizer/projection work remains explicitly deferred.
- Full `assertCurrentSchema` parity: **FAIL** — canonical index maintenance is missing (F5).

## Changed files

- Added [docs/plan/reviews/storage-002a-1-02.md](storage-002a-1-02.md).
- No production code was changed.
- [docs/plan/tasks/storage-002a-1.md](../tasks/storage-002a-1.md) was intentionally left `status: in-progress`.

## Remaining uncertainty

The required checks establish build/test/parity correctness for the current tree, but they do not replace a direct exhaustive comparison of every generated catalog SQL statement against every frozen owner implementation. The blocking omission above is directly established from the frozen `agentMemory.ts` canonical index SQL and the current Rust finalizer. No evidence was gathered from real profiles, databases, Keychain, credentials, or provider logins.
