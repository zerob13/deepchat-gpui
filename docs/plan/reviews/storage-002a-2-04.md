---
task: storage-002a-2
scope: schema diagnosis, repair hooks, and startup recovery
review-kind: independent-final-gate
conclusion: blocked
---

# storage-002a-2 — independent review 04

## Scope and safety

This review inspected the current implementation against [storage-002a-2](../tasks/storage-002a-2.md), the prior three reviews, the storage contract, and frozen reference commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`. It used generated temporary databases only. No real profile, database, Keychain item, credential, provider login, or provider session was accessed or modified.

## Findings

### P1 — blocking: destructive construction failures are still neither preserved nor refused by the production gate

- **Contract:** [storage-002a-2.md:63](../tasks/storage-002a-2.md#L63), [storage-002a-2.md:85](../tasks/storage-002a-2.md#L85)
- **Code:** [production_schema.rs:305](../../../crates/deepchat-services/src/production_schema.rs#L305), [production_schema.rs:321](../../../crates/deepchat-services/src/production_schema.rs#L321), [startup.rs:215](../../../crates/deepchat-services/src/startup.rs#L215), [startup.rs:223](../../../crates/deepchat-services/src/startup.rs#L223)
- **Coverage:** [schema_repair.rs:1283](../../../crates/deepchat-services/tests/schema_repair.rs#L1283), [schema_repair.rs:1322](../../../crates/deepchat-services/tests/schema_repair.rs#L1322)

`initialize_before_assert` collapses every migration driver failure to `ProductionSchemaError::Migration`. The subsequent classifier no longer has the failed `rusqlite::Error`, so it cannot call the existing destructive-error classifier. Instead it probes the current catalog and returns the first independently observable missing table/column. A destructive migration/construction failure in a database that also has a repairable schema issue can therefore be reclassified as a recognized non-destructive reason and authorize repair at `should_repair = recognized.is_some()`.

The test named `unclassified_and_destructive_construction_failures_never_repair` does not inject a destructive failure. Its two branches both synthesize `ProductionSchemaError::Finalize`: one supplies `None` through `initialization_reason`, and the other uses `StartupFaultPoint::Initialize`. The seam has no destructive error/class input. The real destructive-plus-repairable case required by the locked contract remains untested, and current production error erasure prevents the gate from enforcing it reliably.

### P1 — blocking: startup observations invent schema reasons from wrapper variants

- **Contract:** [storage-002a-2.md:36](../tasks/storage-002a-2.md#L36), [storage-002a-2.md:64](../tasks/storage-002a-2.md#L64)
- **Code:** [startup.rs:223](../../../crates/deepchat-services/src/startup.rs#L223), [startup.rs:379](../../../crates/deepchat-services/src/startup.rs#L379), [startup.rs:469](../../../crates/deepchat-services/src/startup.rs#L469)
- **Coverage gap:** [schema_repair.rs:1283](../../../crates/deepchat-services/tests/schema_repair.rs#L1283), [schema_repair.rs:1322](../../../crates/deepchat-services/tests/schema_repair.rs#L1322)

The recognized classifier reason is local to the construction branch and is discarded when startup fails. `classify_observation_failure` then maps every `ProductionSchemaError::Create` to `missing-table`, every `Migration` to `missing-column`, and every `Finalize` to `type-mismatch`, regardless of the actual classification. Consequently the explicitly unclassified injected construction failure is publicly observed as `Schema(TypeMismatch)` rather than persistence/unclassified, and a recognized `column-count-mismatch` failure would be reported as `missing-column`. The frozen initializer reports a schema reason only when `classifySchemaError(error)` actually recognized that reason; otherwise it reports persistence. Existing refusal tests assert `repair_attempted` but do not assert the public failure category/reason, so this contract defect is not caught.

### P2 — blocking verification: the transaction and hook-input evidence matrix is still incomplete

- **Contract:** [storage-002a-2.md:80](../tasks/storage-002a-2.md#L80) through [storage-002a-2.md:83](../tasks/storage-002a-2.md#L83)
- **Code:** [schema_repair.rs:272](../../../crates/deepchat-services/src/schema_repair.rs#L272) through [schema_repair.rs:423](../../../crates/deepchat-services/src/schema_repair.rs#L423)
- **Tests:** [schema_repair.rs:341](../../../crates/deepchat-services/tests/schema_repair.rs#L341), [schema_repair.rs:1347](../../../crates/deepchat-services/tests/schema_repair.rs#L1347), [schema_repair.rs:1443](../../../crates/deepchat-services/tests/schema_repair.rs#L1443), [schema_repair.rs:1509](../../../crates/deepchat-services/tests/schema_repair.rs#L1509)

The new tests materially improve coverage: every real hook can fail inside the transaction, a later hook rolls back a missing table and index, the fixed destination is overwritten, and the full 41-definition repair succeeds. Mandatory adversarial evidence is nevertheless still missing for a repair SQL failure (`DatabaseRepairError::Sql`) rolling back earlier schema work while retaining the backup, and for the complete added-column-set contract across multiple additions. The full-catalog empty-database test repairs missing tables, not all repairable-column additions, so it does not prove every production `addColumnSql` path or the exact set delivered to each affected hook. The task explicitly requires these as tests, not only code inspection.

### P2 — blocking: the fault injector is public production API rather than a strictly constrained test seam

- **Code:** [startup.rs:104](../../../crates/deepchat-services/src/startup.rs#L104) through [startup.rs:134](../../../crates/deepchat-services/src/startup.rs#L134), [startup.rs:176](../../../crates/deepchat-services/src/startup.rs#L176)

`StartupFaultPoint`, `StartupFaultInjector`, `NoStartupFaults`, and `Storage::open_production_with_faults` are all public in a public module. `#[doc(hidden)]` only hides documentation; it does not constrain use. Although the normal production entry points correctly pass `NoStartupFaults`, any downstream production caller can invoke the public fault-bearing entry point and inject arbitrary recognized reasons or failures. This does not weaken password ownership, but it does enlarge the production API and makes the claimed test-only boundary unenforced. The seam should be compile-time constrained or moved behind a non-production integration-test boundary while retaining deterministic coverage.

## Resolved findings and verified behavior

- The prior `recognized.is_some() || repairable > 0` gate was removed; the immediate construction branch now uses only a recognized reason.
- Diagnosis kinds, normalization, issue ordering, repairability, dedupe, safe identifier quoting, and redacted classifier debug output remain covered.
- Exact ordered 41-entry manual and 38-entry startup catalog membership is locked, including the three legacy and four settings-owner exclusions.
- Healthy and manual-only repair paths create no backup. Checkpoint and copy failures precede mutation. Fixed timestamp destinations use overwrite-capable copy, and only the main database path is passed to the copy port.
- Missing-table, missing-column, and missing-index work plus affected hooks execute through one Rust transaction. Hook failures and a later-hook failure roll back tested schema changes while retaining the backup.
- Generated fixtures cover the four real hook success paths and the required environment, pending-input, audit, agent-memory state/index/bridge/FTS invalidation, scope, temporal, lineage/dirty, and clear artifacts.
- Startup covers successful one-shot repair/reopen, diagnosis-unavailable continuation, manual-only continuation, residual continuation, repair/open/close/reopen failures, second recognized failure refusal, and observer panic isolation.
- Explicit repair executes the ordered full 41-definition catalog to a healthy diagnosis; the normal `Storage` startup path is wired to the 38-entry startup catalog.

## Acceptance-evidence assessment

| Group | Assessment |
| --- | --- |
| 1. 41/38 catalogs, exclusions, issue ordering | proven |
| 2. normalization, quoting, diagnosis kinds, repairability, dedupe, fields | proven |
| 3. healthy/manual-only no backup | proven |
| 4. checkpoint/copy/backup naming and retention | proven, except successful phase order is primarily structural |
| 5. tables/columns/indexes, added-column sets, one transaction | incomplete: full production column/set matrix missing |
| 6. rollback of schema work and every hook failure | incomplete: SQL-failure rollback case missing |
| 7. four complete hooks and agent-memory artifacts | proven for the exercised trigger branches |
| 8. classifier reasons, adversarial text, redaction, source-free errors | proven in isolation; startup reason propagation is defective |
| 9. startup one-shot and failure/refusal matrix | incomplete/contradicted: destructive production refusal is not preserved or tested |
| 10. real 38-entry Storage and explicit 41-entry repair integration | proven |

## Checks

| Command | Result |
| --- | --- |
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test -p deepchat-services --test schema_repair` | PASS — 26 passed, 0 failed |
| `cargo test --workspace` | PASS — 87 passed, 0 failed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |
| `git diff --check` | PASS, but ineffective for the zero-tracked-file repository |
| explicit untracked-tree trailing-whitespace scan excluding `.git`/`target` | PASS — 0 matches |
| absolute-path scan | PASS for safety — 11 matches, all declared frozen-reference documentation/evidence entries |
| database/WAL/SHM/repair-backup artifact scan excluding `target` | PASS — 0 artifacts |
| frozen reference check | PASS — `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd` |

## Conclusion

**blocked.** The current suite is substantially stronger and all executed checks pass, but the production construction path erases destructive failure classification and may authorize repair from an unrelated schema probe. Public failure observations also synthesize incorrect schema reasons. Mandatory SQL-failure rollback and complete added-column-set evidence remain absent, and the fault seam is not actually constrained to tests. No production code was modified by this review. No files were staged, committed, or pushed.
