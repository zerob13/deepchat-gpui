---
task: storage-002a-2
scope: schema diagnosis, repair hooks, and startup recovery
review-kind: independent-final-gate
conclusion: pass
---

# storage-002a-2 — independent review 05

## Scope and safety

This review inspected the current implementation against [storage-002a-2](../tasks/storage-002a-2.md), reviews 01–04, the [storage contract](../../storage.md), and frozen reference commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`. It used generated temporary databases only. No real profile, database, Keychain item, credential, provider login, or provider session was accessed or modified.

## Findings

No P1, P2, or P3 findings.

## Review-04 blocker disposition

### Raw construction classification is preserved before public-error erasure

- [`schema.rs:26`](../../../crates/deepchat-services/src/schema.rs#L26) retains `Destructive`, the exact stable `Schema(SchemaErrorReason)`, or `Other` in crate-private `MigrationFailureClass`.
- [`schema.rs:39`](../../../crates/deepchat-services/src/schema.rs#L39) classifies the original `rusqlite::Error` before replacing it with the redacted public `MigrationError`; every version SQL, finalizer, marker, transaction-open, and commit failure uses that path at [`schema.rs:185`](../../../crates/deepchat-services/src/schema.rs#L185) through [`schema.rs:235`](../../../crates/deepchat-services/src/schema.rs#L235).
- Static-schema failures are classified from the original driver error at [`production_schema.rs:284`](../../../crates/deepchat-services/src/production_schema.rs#L284) through [`production_schema.rs:297`](../../../crates/deepchat-services/src/production_schema.rs#L297). Migration classes are carried into `ProductionInitializationFailure` without catalog probing at [`production_schema.rs:331`](../../../crates/deepchat-services/src/production_schema.rs#L331) through [`production_schema.rs:355`](../../../crates/deepchat-services/src/production_schema.rs#L355).
- The construction gate accepts only `ProductionInitializationClass::Schema(reason)` and refuses destructive/other classes independently of diagnosis at [`startup.rs:216`](../../../crates/deepchat-services/src/startup.rs#L216) through [`startup.rs:250`](../../../crates/deepchat-services/src/startup.rs#L250). The unrelated post-failure diagnosis can populate counts but cannot authorize repair.
- Direct classification regression coverage is at [`production_schema.rs:797`](../../../crates/deepchat-services/src/production_schema.rs#L797), while the private startup seam proves recognized one-shot repair, second-failure refusal, destructive refusal, and unclassified refusal at [`startup.rs:644`](../../../crates/deepchat-services/src/startup.rs#L644) through [`startup.rs:723`](../../../crates/deepchat-services/src/startup.rs#L723).

### Startup observations use the real classifier outcome and remain redacted

- The exact recognized reason is retained in `classified_schema_failure` at [`startup.rs:192`](../../../crates/deepchat-services/src/startup.rs#L192) and updated directly from the construction class at [`startup.rs:216`](../../../crates/deepchat-services/src/startup.rs#L216) through [`startup.rs:230`](../../../crates/deepchat-services/src/startup.rs#L230).
- Failure observation maps a production-schema error to `Schema(reason)` only when that retained reason exists; otherwise it reports persistence. Integrity errors remain integrity at [`startup.rs:476`](../../../crates/deepchat-services/src/startup.rs#L476) through [`startup.rs:489`](../../../crates/deepchat-services/src/startup.rs#L489). No wrapper variant is used to invent a schema reason.
- The second-failure test proves the exact `column-count-mismatch` reason survives rather than being replaced by a wrapper-derived reason at [`startup.rs:664`](../../../crates/deepchat-services/src/startup.rs#L664) through [`startup.rs:689`](../../../crates/deepchat-services/src/startup.rs#L689); destructive and other failures prove integrity/persistence observation at [`startup.rs:692`](../../../crates/deepchat-services/src/startup.rs#L692) through [`startup.rs:723`](../../../crates/deepchat-services/src/startup.rs#L723).
- Public startup observations contain only enums, counts, and duration fields at [`startup.rs:66`](../../../crates/deepchat-services/src/startup.rs#L66) through [`startup.rs:95`](../../../crates/deepchat-services/src/startup.rs#L95). Classifier `Debug` omits the internal identity at [`schema_error_classifier.rs:25`](../../../crates/deepchat-services/src/schema_error_classifier.rs#L25) through [`schema_error_classifier.rs:48`](../../../crates/deepchat-services/src/schema_error_classifier.rs#L48), and public startup/repair errors retain no raw driver, SQL, path, or source chain.

### Fault injection is not a production API

`StartupFaultPoint`, `StartupFaultInjector`, `NoStartupFaults`, and `Storage::open_production_with_faults` are module-private at [`startup.rs:107`](../../../crates/deepchat-services/src/startup.rs#L107) through [`startup.rs:131`](../../../crates/deepchat-services/src/startup.rs#L131) and [`startup.rs:173`](../../../crates/deepchat-services/src/startup.rs#L173). The only public injected entry point always supplies `NoStartupFaults` at [`startup.rs:156`](../../../crates/deepchat-services/src/startup.rs#L156) through [`startup.rs:170`](../../../crates/deepchat-services/src/startup.rs#L170). Test implementations are confined to the module's `#[cfg(test)]` section beginning at [`startup.rs:507`](../../../crates/deepchat-services/src/startup.rs#L507); downstream production crates cannot name or inject the seam.

### SQL rollback and complete added-column evidence are present

- [`schema_repair.rs:1273`](../../../crates/deepchat-services/tests/schema_repair.rs#L1273) injects a real later repair-SQL failure, verifies the earlier and failing additions are both absent after rollback, verifies the typed `Sql` result, and verifies the already-created repair backup remains.
- [`schema_repair.rs:1328`](../../../crates/deepchat-services/tests/schema_repair.rs#L1328) repairs a real production `deepchat_pending_inputs` definition and compares the complete exact set of repaired columns: `assistant_message_id`, `blocking_json`, `message_ids_json`, and `retry_required_at`. It also verifies all four physical columns and the real retry normalization hook effect.

## Acceptance-evidence assessment

| Group | Assessment | Evidence |
| --- | --- | --- |
| 1. exact 41/38 catalogs, exclusions, issue ordering | proven | Complete ordered names and exact exclusions at [`schema_repair.rs:81`](../../../crates/deepchat-services/tests/schema_repair.rs#L81) through [`schema_repair.rs:156`](../../../crates/deepchat-services/tests/schema_repair.rs#L156); diagnosis ordering at [`schema_repair.rs:158`](../../../crates/deepchat-services/tests/schema_repair.rs#L158). |
| 2. normalization, safe quoting, kinds, repairability, dedupe, exact fields | proven | Inspector implementation at [`schema_repair.rs:72`](../../../crates/deepchat-services/src/schema_repair.rs#L72) through [`schema_repair.rs:171`](../../../crates/deepchat-services/src/schema_repair.rs#L171), bound index ownership and escaped PRAGMA identifier at [`schema_repair.rs:535`](../../../crates/deepchat-services/src/schema_repair.rs#L535) through [`schema_repair.rs:571`](../../../crates/deepchat-services/src/schema_repair.rs#L571), adversarial fixture at [`schema_repair.rs:158`](../../../crates/deepchat-services/tests/schema_repair.rs#L158). |
| 3. healthy/manual-only no backup | proven | [`schema_repair.rs:351`](../../../crates/deepchat-services/tests/schema_repair.rs#L351). |
| 4. checkpoint/copy ordering, UTC naming, main file only, overwrite, failures, retention | proven | Production ordering at [`schema_repair.rs:272`](../../../crates/deepchat-services/src/schema_repair.rs#L272) through [`schema_repair.rs:306`](../../../crates/deepchat-services/src/schema_repair.rs#L306); success/overwrite/main-only fixture at [`schema_repair.rs:292`](../../../crates/deepchat-services/tests/schema_repair.rs#L292); copy and checkpoint failures at [`schema_repair.rs:396`](../../../crates/deepchat-services/tests/schema_repair.rs#L396) and [`schema_repair.rs:585`](../../../crates/deepchat-services/tests/schema_repair.rs#L585); post-failure backup retention in hook and SQL rollback fixtures. |
| 5. missing tables/columns/indexes, exact added sets, empty missing-table set, one transaction | proven | One transaction and catalog-order application at [`schema_repair.rs:295`](../../../crates/deepchat-services/src/schema_repair.rs#L295) through [`schema_repair.rs:421`](../../../crates/deepchat-services/src/schema_repair.rs#L421); exact multi-column set at [`schema_repair.rs:1328`](../../../crates/deepchat-services/tests/schema_repair.rs#L1328); missing-table environment hook proves the empty-set path at [`schema_repair.rs:724`](../../../crates/deepchat-services/tests/schema_repair.rs#L724). |
| 6. SQL/schema/hook rollback with retained backup | proven | Generic hook rollback at [`schema_repair.rs:433`](../../../crates/deepchat-services/tests/schema_repair.rs#L433), every real hook at [`schema_repair.rs:1177`](../../../crates/deepchat-services/tests/schema_repair.rs#L1177), SQL rollback at [`schema_repair.rs:1273`](../../../crates/deepchat-services/tests/schema_repair.rs#L1273), and missing-table/index rollback at [`schema_repair.rs:1396`](../../../crates/deepchat-services/tests/schema_repair.rs#L1396). |
| 7. all four real hooks and complete required artifacts | proven | Environment, pending-input, and audit fixtures at [`schema_repair.rs:629`](../../../crates/deepchat-services/tests/schema_repair.rs#L629) through [`schema_repair.rs:763`](../../../crates/deepchat-services/tests/schema_repair.rs#L763); agent-memory lineage/dirty/clear, temporal, scope, lifecycle/embedding/shadow/index/legacy bridge/FTS invalidation fixtures at [`schema_repair.rs:765`](../../../crates/deepchat-services/tests/schema_repair.rs#L765) through [`schema_repair.rs:1144`](../../../crates/deepchat-services/tests/schema_repair.rs#L1144). |
| 8. classifier patterns, stable reasons, adversarial text, redaction, no sources | proven | [`schema_error_classifier.rs:51`](../../../crates/deepchat-services/src/schema_error_classifier.rs#L51) through [`schema_error_classifier.rs:121`](../../../crates/deepchat-services/src/schema_error_classifier.rs#L121) and [`schema_repair.rs:244`](../../../crates/deepchat-services/tests/schema_repair.rs#L244); construction propagation is covered by the private startup tests cited above. |
| 9. one-shot startup and complete continuation/failure/refusal matrix | proven | Real successful repair/reopen and normal continuation integration at [`schema_repair.rs:473`](../../../crates/deepchat-services/tests/schema_repair.rs#L473), [`schema_repair.rs:540`](../../../crates/deepchat-services/tests/schema_repair.rs#L540), [`schema_repair.rs:991`](../../../crates/deepchat-services/tests/schema_repair.rs#L991), and [`schema_repair.rs:1146`](../../../crates/deepchat-services/tests/schema_repair.rs#L1146); deterministic private failure matrix at [`startup.rs:588`](../../../crates/deepchat-services/src/startup.rs#L588) through [`startup.rs:723`](../../../crates/deepchat-services/src/startup.rs#L723). Moving these failure controls into module-private unit tests explains the focused integration count changing from 26 to 24 without losing the mandatory evidence. |
| 10. real Storage 38-entry path and explicit 41-definition repair | proven | Ordered catalog selection at [`schema_repair.rs:81`](../../../crates/deepchat-services/tests/schema_repair.rs#L81), real Storage startup at [`schema_repair.rs:1146`](../../../crates/deepchat-services/tests/schema_repair.rs#L1146), and full manual repair at [`schema_repair.rs:1462`](../../../crates/deepchat-services/tests/schema_repair.rs#L1462). |

The explicitly deferred connection-scoped tokenizer probing, dynamic FTS virtual-table creation, and tape/memory projection lifecycle remain outside this task. The manifest keeps `storage-sqlcipher` at `implemented`.

## Checks

| Command | Result |
| --- | --- |
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test -p deepchat-services --test schema_repair` | PASS — 24 passed, 0 failed |
| `cargo test -p deepchat-services startup::tests -- --nocapture` | PASS — 4 startup unit tests passed; other binaries reported 0 selected tests |
| `cargo test --workspace` | PASS — 98 passed, 0 failed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |
| `git diff --check` | PASS, but ineffective alone because the repository has zero tracked files |
| explicit untracked-tree trailing-whitespace scan excluding `.git`/`target` | PASS — 0 matches |
| absolute local-path scan | PASS for safety — 11 matches, all deliberate frozen-reference documentation/evidence declarations |
| database/WAL/SHM/repair-backup artifact scan excluding `target` | PASS — 0 artifacts |
| frozen reference check | PASS — `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd` |

## Conclusion

**pass.** The review-04 production-classification, observation, rollback-evidence, added-column-set, and public fault-seam blockers are closed. Current code and generated-fixture evidence satisfy all ten mandatory acceptance groups without promoting `storage-sqlcipher` beyond `implemented`. No production code was modified by this review. No files were staged, committed, or pushed.
