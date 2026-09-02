---
task: storage-002a-2
scope: schema diagnosis, repair hooks, and startup recovery
review-kind: independent-final-gate
conclusion: blocked
---

# storage-002a-2 — independent review 03

## Scope and safety

This review inspected the current implementation against [storage-002a-2](../tasks/storage-002a-2.md), the storage contract, the previous independent review, and frozen reference commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`. It used generated temporary databases only. No real profile, database, Keychain item, credential, provider login, or provider session was accessed or modified.

## Findings

### P1 — blocking: an unclassified construction failure can still trigger schema repair

- **Contract:** [storage-002a-2.md:63](../tasks/storage-002a-2.md#L63)
- **Code:** [startup.rs:155](../../../crates/deepchat-services/src/startup.rs#L155), [startup.rs:168](../../../crates/deepchat-services/src/startup.rs#L168), [startup.rs:169](../../../crates/deepchat-services/src/startup.rs#L169)
- **Missing coverage:** [schema_repair.rs:408](../../../crates/deepchat-services/tests/schema_repair.rs#L408) through [schema_repair.rs:904](../../../crates/deepchat-services/tests/schema_repair.rs#L904)

When `initialize_before_assert` fails, `should_repair` is computed as `recognized.is_some() || repairable > 0`. Therefore an unclassified construction failure is allowed to trigger repair whenever the independent catalog diagnosis happens to find any repairable issue. The locked contract requires construction-time repair only for a recognized non-destructive schema reason; destructive and unclassified construction failures must not trigger repair. The normal post-initialization diagnosis path may repair diagnosed issues, but it cannot be used to weaken the construction-failure gate. There is no failure-injection test for an unclassified/destructive construction error combined with a repairable diagnosis, so the current suite does not catch this behavior.

### P1 — blocking verification: mandatory startup failure evidence remains absent

- **Contract:** [storage-002a-2.md:85](../tasks/storage-002a-2.md#L85)
- **Startup boundary:** [startup.rs:127](../../../crates/deepchat-services/src/startup.rs#L127) through [startup.rs:283](../../../crates/deepchat-services/src/startup.rs#L283)
- **Focused suite:** [schema_repair.rs:408](../../../crates/deepchat-services/tests/schema_repair.rs#L408) through [schema_repair.rs:904](../../../crates/deepchat-services/tests/schema_repair.rs#L904)

The focused suite now proves successful repair/reopen, manual-only continuation, residual-manual continuation, and observer panic isolation. It still does not prove diagnosis-unavailable continuation, repair failure at the `Storage` integration boundary, reopen failure, recognized construction-time classifier gating, destructive/unclassified refusal, or second-failure refusal. These are explicit mandatory acceptance items, not optional test-quality suggestions. The current API injects only `RepairFileSystem` and the observer; it has no deterministic open/reopen or initializer failure port, making several required failure paths difficult to exercise without adding a bounded test seam.

### P1 — blocking verification: complete `agent_memory` repair behavior is not covered

- **Contract:** [storage-002a-2.md:54](../tasks/storage-002a-2.md#L54), [storage-002a-2.md:57](../tasks/storage-002a-2.md#L57), [storage-002a-2.md:83](../tasks/storage-002a-2.md#L83)
- **Implementation:** [schema_repair.rs:447](../../../crates/deepchat-services/src/schema_repair.rs#L447) through [schema_repair.rs:529](../../../crates/deepchat-services/src/schema_repair.rs#L529)
- **Focused tests:** [schema_repair.rs:700](../../../crates/deepchat-services/tests/schema_repair.rs#L700), [schema_repair.rs:765](../../../crates/deepchat-services/tests/schema_repair.rs#L765)

The prior implementation omissions were substantially repaired: dirty triggers, the derivation CHECK, conflict anomaly index, temporal claim quarantine/internal normalization, and the audit predicate now exist. However, the generated-fixture suite only directly exercises the decision-revision lineage/dirty branch and the temporal branch. It does not prove lifecycle and embedding backfills, shadow reconciliation, retired/canonical index maintenance, legacy-status bridge replacement, FTS meta invalidation with the frozen key/policy behavior, scope triggers/index, conflict artifact behavior, or clear table/guards. The task explicitly requires evidence for all of those artifacts and their added-column trigger conditions. Their presence in concatenated SQL/constants is indirect evidence and does not satisfy the task's generated-fixture acceptance gate.

### P2 — blocking verification: repair ordering and rollback evidence is still incomplete

- **Contract:** [storage-002a-2.md:80](../tasks/storage-002a-2.md#L80) through [storage-002a-2.md:82](../tasks/storage-002a-2.md#L82)
- **Implementation:** [schema_repair.rs:272](../../../crates/deepchat-services/src/schema_repair.rs#L272) through [schema_repair.rs:423](../../../crates/deepchat-services/src/schema_repair.rs#L423)
- **Focused tests:** [schema_repair.rs:234](../../../crates/deepchat-services/tests/schema_repair.rs#L234), [schema_repair.rs:331](../../../crates/deepchat-services/tests/schema_repair.rs#L331), [schema_repair.rs:368](../../../crates/deepchat-services/tests/schema_repair.rs#L368), [schema_repair.rs:520](../../../crates/deepchat-services/tests/schema_repair.rs#L520)

The suite proves checkpoint failure precedes backup/mutation, copy failure precedes mutation, and an unknown-hook failure rolls back one added column while retaining the backup. It does not prove exact-destination overwrite semantics, main-file-only copying in the presence of WAL/SHM, missing-table and missing-index rollback, rollback for each real hook failure, added-column-set behavior across multiple repairs, or the empty added-column set for every missing-table hook. Those cases are explicitly enumerated acceptance evidence. The current successful missing-table environment fixture proves the environment hook receives a usable empty set indirectly, but it does not close the broader rollback and hook-input matrix.

### P2 — blocking verification: exact full-catalog/manual-repair evidence is absent

- **Contract:** [storage-002a-2.md:77](../tasks/storage-002a-2.md#L77), [storage-002a-2.md:86](../tasks/storage-002a-2.md#L86)
- **Focused test:** [schema_repair.rs:82](../../../crates/deepchat-services/tests/schema_repair.rs#L82)

The focused catalog test checks only counts and exclusions. It does not lock the complete ordered 41/38 membership, nor prove explicit/manual repair through all 41 definitions while `Storage` startup uses exactly 38. Broader production-catalog tests establish topology, but no repair integration test executes the full manual catalog boundary required by this task.

## Resolved prior findings

- Manual-only and post-repair residual schema issues now continue startup and remain observable.
- The missing dirty triggers, derivation CHECK, conflict anomaly index, temporal quarantine/internal normalization split, and audit `COALESCE(...) IS NOT NULL` predicate are present and covered at least at their focused branch boundaries.
- The explicit untracked-tree trailing-whitespace scan now passes.

## Checks

| Command | Result |
| --- | --- |
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test -p deepchat-services --test schema_repair` | PASS — 17 passed, 0 failed |
| `cargo test --workspace` | PASS — 76 passed, 0 failed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |
| `git diff --check` | PASS, but ineffective for the zero-tracked-file repository |
| explicit trailing-whitespace scan excluding `.git`/`target` | PASS |
| absolute-path scan | PASS for safety — matches are declared frozen-reference documentation/evidence entries only |
| database/sidecar/repair-backup artifact scan excluding `target` | PASS — no artifacts found |

## Conclusion

**blocked.** The previous concrete hook and continuation defects are improved, but construction-time repair still weakens the recognized/non-destructive gate, and multiple mandatory acceptance-evidence groups remain untested. Passing 17 focused tests and 76 workspace tests does not establish the task's enumerated completion boundary. No production code was modified by this review; only this review record was written. No files were staged, committed, or pushed.
