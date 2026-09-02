---
task: storage-002a-2
scope: schema diagnosis, repair hooks, and startup recovery
review-kind: independent-final-gate
conclusion: blocked
---

# storage-002a-2 — independent review 02

## Scope and safety

This review inspected the current implementation against [storage-002a-2](../tasks/storage-002a-2.md), the storage contract, and frozen reference commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`. It used generated temporary databases only. No real profile, database, Keychain item, credential, provider login, or provider session was accessed or modified.

## Findings

### P1 — blocking: startup fails instead of continuing after manual-only or residual repairable diagnosis

- **Contract:** [storage-002a-2.md:62](../tasks/storage-002a-2.md#L62)
- **Code:** [startup.rs:195](../../../crates/deepchat-services/src/startup.rs#L195), [startup.rs:202](../../../crates/deepchat-services/src/startup.rs#L202), [startup.rs:233](../../../crates/deepchat-services/src/startup.rs#L233)
- **Coverage:** [schema_repair.rs:408](../../../crates/deepchat-services/tests/schema_repair.rs#L408), [schema_repair.rs:475](../../../crates/deepchat-services/tests/schema_repair.rs#L475)

After diagnosis, the implementation attempts repair only when `repairable > 0 && !repair_attempted`. On a second diagnosis with residual repairable issues it falls through to `assert_current_schema`; manual-only diagnoses also fall through to the same strict assertion. Any manual mismatch or residual missing required shape therefore becomes `StartupError::ProductionSchema` instead of the required successful continuation with observable counts. The two startup tests cover a fully successful repair and a healthy catalog only; they do not exercise manual-only or residual continuation.

### P1 — blocking: the `agent_memory` hook is not the complete frozen artifact repair

- **Contract:** [storage-002a-2.md:50](../tasks/storage-002a-2.md#L50), [storage-002a-2.md:54](../tasks/storage-002a-2.md#L54), [storage-002a-2.md:57](../tasks/storage-002a-2.md#L57)
- **Code:** [schema_repair.rs:447](../../../crates/deepchat-services/src/schema_repair.rs#L447), [schema_repair.rs:497](../../../crates/deepchat-services/src/schema_repair.rs#L497), [schema_repair.rs:510](../../../crates/deepchat-services/src/schema_repair.rs#L510), [schema_repair.rs:517](../../../crates/deepchat-services/src/schema_repair.rs#L517)
- **Reference:** `src/main/memory/data/tables/agentMemory.ts` (`repairCanonicalStateAfterSchemaRepair`, `ensureLineageAndDirtyArtifacts`, `ensureTemporalArtifacts`, and canonical/conflict artifact constants)

The hook creates `agent_memory_dirty` and backfills it when `decision_revision` is added, but it never installs the required `agent_memory_dirty_ai`, `agent_memory_dirty_au`, and `agent_memory_dirty_ad` triggers. Its derivation table also omits the frozen `derivation_kind` CHECK constraint. State maintenance omits `idx_agent_memory_conflict_state_anomaly_v2`. Temporal repair is reduced to normalizing every invalid row to `atemporal`; the frozen behavior archives/quarantines invalid claim rows while only normalizing internal persona/working rows, then replaces the validation triggers. These are production semantics, not deferred tokenizer/projection work.

The focused suite contains no generated-fixture test for any of these lineage/dirty/temporal artifacts, so the required all-four-hooks evidence cannot catch the omissions.

### P1 — blocking: required repair/startup failure and hook evidence is largely absent

- **Contract:** [storage-002a-2.md:75](../tasks/storage-002a-2.md#L75) through [storage-002a-2.md:86](../tasks/storage-002a-2.md#L86)
- **Tests:** [schema_repair.rs:81](../../../crates/deepchat-services/tests/schema_repair.rs#L81) through [schema_repair.rs:503](../../../crates/deepchat-services/tests/schema_repair.rs#L503)

Only nine focused tests exist. They do not prove checkpoint failure, exact checkpoint/copy/transaction ordering, destination overwrite behavior, missing-table hook empty-column sets, missing-table/index rollback, the four production hooks, FTS-meta invalidation, construction-time recognized repair, destructive/unclassified/second-failure refusal, diagnosis-unavailable continuation, manual-only continuation, residual continuation, repair/reopen failure, or explicit/manual repair through all 41 definitions. Passing the current suite is therefore not evidence for the task's enumerated acceptance boundary.

### P2 — blocking: audit backfill does not preserve the frozen update predicate

- **Contract:** [storage-002a-2.md:55](../tasks/storage-002a-2.md#L55)
- **Code:** [schema_repair.rs:519](../../../crates/deepchat-services/src/schema_repair.rs#L519)
- **Reference:** `src/main/memory/data/tables/agentMemoryAudit.ts` (`AGENT_MEMORY_AUDIT_BACKFILL_MEMORY_REF_SQL`)

The frozen statement includes a final `AND COALESCE(outputMemoryRef, inputMemoryRef) IS NOT NULL` predicate. The Rust statement omits it, so matching completed events with no usable reference are still updated from `NULL` to `NULL` and counted as changed by SQLite. That is not the exact frozen update semantics required by the task, and no focused hook fixture covers it.

### P2 — blocking verification: the required trailing-whitespace scan fails

- **Contract:** [storage-002a-2.md:102](../tasks/storage-002a-2.md#L102)

A repository-wide scan excluding `.git` and `target` found trailing whitespace in all three `storage-002a-1` review files, including lines 3–5 of each and additional lines in `storage-002a-1-01.md`. `git diff --check` cannot detect this because the repository has zero tracked files. The task explicitly requires a scan suitable for the untracked state, so this gate is not green.

## Verified behavior

- Diagnosis preserves the stable issue kinds, normalization, issue order, repairability projections, and safely quoted table inspection.
- Repair uses diagnose → WAL truncate checkpoint → main-file copy → one Rust transaction; healthy/manual-only paths skip backup, and an injected hook failure rolls back the column while retaining the backup.
- Public classifier and repair error debug surfaces are redacted.
- Catalog selection is 41 manual definitions and 38 startup definitions with the three legacy and four settings exclusions.
- A successful `agent_memory_audit` missing-column repair closes/reopens through `Storage`; observer panic is isolated.
- No database, WAL/SHM, SQLite, or `.repair.bak` artifacts exist outside `target`; absolute workspace paths found by the scan are deliberate frozen-reference documentation/evidence entries rather than generated runtime leaks.

## Checks

| Command | Result |
| --- | --- |
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test -p deepchat-services --test schema_repair` | PASS — 9 passed, 0 failed |
| `cargo test --workspace` | PASS — 68 passed, 0 failed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |
| `git diff --check` | PASS, but ineffective for the zero-tracked-file repository |
| explicit trailing-whitespace scan excluding `.git`/`target` | FAIL — prior review documents contain trailing whitespace |
| absolute-path scan | PASS for safety — matches are declared frozen-reference paths only |
| database/sidecar/repair-backup artifact scan excluding `target` | PASS — no artifacts found |

## Conclusion

**blocked.** The task cannot be marked done: startup continuation semantics are wrong, the agent-memory and audit hooks do not preserve the complete frozen behavior, the enumerated adversarial/integration evidence is missing, and the required untracked-tree whitespace gate fails. No production code was modified by this review and no commit or staging operation was performed.
