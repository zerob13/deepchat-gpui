---
task: storage-002a-3
review: 01
verdict: fail
reviewer: independent
---

# storage-002a-3 implementation review

## Verdict

**FAIL.** The implementation compiles and its nine focused tests pass, but it does not satisfy the frozen behavior or the task's 17-group acceptance matrix. The blockers below are production-semantic gaps, not documentation-only or test-count concerns.

## Blocking findings

### 1. Agent-memory mutation and failure lifecycle is incomplete

The task requires create/update/delete/**bulk** mutation, nested-savepoint FTS isolation, injected FTS failure, durable dirty metadata, and later rebuild. The public owner exposes only `upsert` and `delete`; no bulk mutation boundary exists ([agent_memory_fts.rs:217](../../../crates/deepchat-services/src/agent_memory_fts.rs#L217), [agent_memory_fts.rs:262](../../../crates/deepchat-services/src/agent_memory_fts.rs#L262)). The focused tests do not inject a mirror failure or prove authoritative mutation commit plus generation mismatch/recovery ([fts_projection.rs:90](../../../crates/deepchat-services/tests/fts_projection.rs#L90), [fts_projection.rs:163](../../../crates/deepchat-services/tests/fts_projection.rs#L163)).

`maintain_after_mutation` also silently converts a failed/missing `mark_dirty` into generation `-1` and returns success without establishing durable dirty evidence ([agent_memory_fts.rs:287](../../../crates/deepchat-services/src/agent_memory_fts.rs#L287)). That cannot prove the required recovery invariant when metadata mutation itself fails.

### 2. Agent-memory candidate fusion is not the frozen ranking algorithm

The frozen implementation creates a separately bounded importance/recency candidate population (`min(800, max(64, limit * 8))`) before applying the same MATCH and fuses it with bounded lexical hits ([agentMemory.ts:2739](/Users/colab/Documents/workspace/deepchat-2/src/main/memory/data/tables/agentMemory.ts#L2739), [agentMemory.ts:2780](/Users/colab/Documents/workspace/deepchat-2/src/main/memory/data/tables/agentMemory.ts#L2780)).

The Rust `importance` CTE instead searches all MATCH rows directly and limits them with the caller's result limit, without the frozen scoped importance-candidate stage ([agent_memory_fts.rs:436](../../../crates/deepchat-services/src/agent_memory_fts.rs#L436), [agent_memory_fts.rs:453](../../../crates/deepchat-services/src/agent_memory_fts.rs#L453)). This can return a different candidate set and ordering. The one recall test does not exercise fusion boundaries or ordering ([fts_projection.rs:121](../../../crates/deepchat-services/tests/fts_projection.rs#L121)).

### 3. Tape append accepts stale FTS metadata as current

Before incremental FTS insertion, frozen behavior requires both projection version and head to match the previous projection metadata ([tapeSearchProjectionStore.ts:171](/Users/colab/Documents/workspace/deepchat-2/src/main/tape/infrastructure/sqlite/tapeSearchProjectionStore.ts#L171), [tapeSearchProjectionStore.ts:179](/Users/colab/Documents/workspace/deepchat-2/src/main/tape/infrastructure/sqlite/tapeSearchProjectionStore.ts#L179), [tapeSearchProjectionStore.ts:308](/Users/colab/Documents/workspace/deepchat-2/src/main/tape/infrastructure/sqlite/tapeSearchProjectionStore.ts#L308)).

The Rust append path checks only `projection_version`, ignoring `max_entry_id` ([tape_search_projection.rs:194](../../../crates/deepchat-services/src/tape_search_projection.rs#L194)). A stale FTS head is therefore incrementally appended and then relabeled current, leaving missing FTS rows. No test supplies a same-version/wrong-head FTS meta before append.

### 4. Memory-ingestion retired-workflow filtering does not match the frozen predicate

The frozen predicate excludes metadata only when parsed `messageType === 'workflow_result'` ([retiredWorkflowData.ts:1](/Users/colab/Documents/workspace/deepchat-2/src/shared/orchestration/retiredWorkflowData.ts#L1)). Rust instead looks for keys named `workflowResult` or `workflow_result` ([memory_ingestion_projection.rs:394](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L394)). A real retired workflow-result message is consequently projected, while unrelated metadata containing either invented key is incorrectly excluded. The focused ingestion tests contain no retired-workflow case ([fts_projection.rs:345](../../../crates/deepchat-services/tests/fts_projection.rs#L345)).

### 5. Memory-ingestion tool identity validation is weaker than frozen behavior

Frozen effective semantics require a terminal `tool_call` to have a non-empty `messageId` and a valid tool-call id parsed from either an object or nested JSON string before it can affect projection state ([effectiveSemantics.ts:135](/Users/colab/Documents/workspace/deepchat-2/src/main/tape/domain/effectiveSemantics.ts#L135), [effectiveSemantics.ts:146](/Users/colab/Documents/workspace/deepchat-2/src/main/tape/domain/effectiveSemantics.ts#L146)). Rust's update path only extracts `messageId`; it never validates or parses the tool-call id ([memory_ingestion_projection.rs:307](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L307), [memory_ingestion_projection.rs:418](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L418)). Malformed terminal tool entries can therefore set `had_tool_use`.

### 6. Repair invalidation is globally invented and occurs after repair commit

The change adds unconditional deletion of Agent FTS, Tape FTS, and memory-ingestion metadata after every successful repair transaction ([schema_repair.rs:304](../../../crates/deepchat-services/src/schema_repair.rs#L304), [schema_repair.rs:354](../../../crates/deepchat-services/src/schema_repair.rs#L354)). The frozen repair owner runs table-specific `afterRepair` hooks inside the repair transaction ([schemaRepair.ts:223](/Users/colab/Documents/workspace/deepchat-2/src/main/data/schemaRepair.ts#L223), [schemaRepair.ts:268](/Users/colab/Documents/workspace/deepchat-2/src/main/data/schemaRepair.ts#L268)); the existing Agent hook already owns its FTS invalidation.

This new post-commit operation has two defects:

1. unrelated repairs make every ingestion session stale and discard all Tape FTS heads without frozen evidence;
2. an invalidation error returns `DatabaseRepairError::Hook` after schema repair has already committed, so the caller receives failure although the repair mutation cannot roll back.

No test proves the new global invalidation set or this post-commit failure boundary.

### 7. Capability/fallback and fault paths required by the contract are unproved

The production probe is closed and has no test seam; all focused tests assume bundled FTS reports `trigram` ([fts_projection.rs:93](../../../crates/deepchat-services/tests/fts_projection.rs#L93)). There is no evidence for:

- strict failed-trigram then unicode61 probe order;
- unicode61 Agent LIKE-only/drop behavior;
- unavailable capability behavior;
- repeated Tape owners sharing a probe and reopen probing again;
- Agent non-transient versus transient failure handling and 30-second cooldown;
- Tape unicode61 creation;
- Tape FTS write rollback of the complete projection transaction.

These are acceptance groups 1, 3, 5, 6, 8, and 9, and the current nine tests do not cover them.

### 8. Required adversarial concurrency and transaction evidence is missing

The test named `ingestion_projection_effective_semantics_retraction_and_atomic_current_read` performs only sequential calls ([fts_projection.rs:345](../../../crates/deepchat-services/tests/fts_projection.rs#L345)); it does not use two connections, a hook, or any concurrent append fixture. It therefore does not prove that the one-statement current-range read prevents a false-current window (acceptance group 13).

Likewise, no test injects an Agent FTS savepoint failure, a Tape FTS write failure, missing/corrupt Tape FTS table behavior, or projection metadata failure. Passing happy-path transactions do not prove the required rollback/degradation boundaries.

## Acceptance matrix

| Group | Result | Evidence assessment |
|---|---|---|
| 1. Probe ownership/order/reopen | **Missing** | Only a real trigram success is observed; no ordered failure/fallback or cache/reopen proof. |
| 2. Static topology/high-water | **Proved** | Catalog 41/39/38/38, 19 markers, v69, and no static virtual-table DDL are asserted ([fts_projection.rs:69](../../../crates/deepchat-services/tests/fts_projection.rs#L69)). |
| 3. Agent capability modes/meta | **Partial** | Trigram metadata is proved; unicode61 and unavailable behavior are not. |
| 4. Agent backfill/authority | **Partial** | Persona and cross-agent behavior are sampled; working, superseded, non-active, scope variants, and hash-collision authority are not all proved. |
| 5. Agent recall semantics | **Partial/incorrect** | Basic FTS, short LIKE, escaping, and empty query are sampled; frozen candidate fusion is implemented differently and failure/cooldown is absent. |
| 6. Agent mutations/failure recovery | **Fail** | No bulk API or injected mirror-failure evidence; dirty metadata can fail silently. |
| 7. Agent stale/repaired rebuild | **Partial** | Policy/indexed stale and direct meta deletion rebuild are covered; schema/tokenizer/mutation-generation variants and real repair-service invalidation are not. |
| 8. Tape exact surface/atomic writes | **Partial/fail** | Surface happy paths exist; stale-head append is incorrect and FTS write rollback is untested. |
| 9. Tape search authority/fallback | **Partial** | Single/multi-source and one filter case are sampled; unicode61, unavailable/corrupt fallback, FTS-first fill ordering/dedupe adversaries are not fully proved. |
| 10. Tape stale/missing lifecycle | **Partial** | Old projection version and missing FTS meta rebuild are covered; missing/corrupt table and same-version wrong-head paths are not. |
| 11. Ingestion effective semantics | **Fail** | Retired workflow and malformed tool identity differ from frozen behavior. |
| 12. Tape append/projection failure isolation | **Partial** | A deliberately non-sequential projection is made stale while Tape commits; an actual projection exception inside the composition point is not injected. |
| 13. Atomic head observation | **Missing** | SQL shape is one statement, but required concurrent-append fixture is absent. |
| 14. Copy exclusions | **Proved** | Exact six names and near misses are asserted ([fts_projection.rs:434](../../../crates/deepchat-services/tests/fts_projection.rs#L434)). |
| 15. Existing repair/catalog behavior | **Contradicted** | Existing tests pass, but unconditional post-commit global invalidation changes repair semantics without focused evidence. |
| 16. Public redaction | **Partial** | One Agent constructor error is checked; Tape/ingestion errors and all failure observations are not exercised. |
| 17. Fixture isolation | **Partial** | Tests use generated in-memory databases and no real user state, but no generated keyed SQLCipher file fixture exercises dynamic owners/reopen. |

## Verification executed

The following commands passed in the reviewed worktree:

```text
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p deepchat-services --test fts_projection   # 9 passed
cargo test --workspace                                 # 97 passed total
uv run python tools/parity-audit/validate.py            # PASS
git diff --check                                       # PASS
```

The local frozen repository exists and resolves to `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`; no override is required. The earlier claim that the reference path was absent is contradicted by direct inspection and the successful parity audit.

## Required closure

Before another review:

1. implement the missing Agent bulk/failure lifecycle and exact candidate fusion;
2. require full Tape FTS metadata equality before incremental append;
3. port the exact retired-workflow and tool-identity helpers;
4. remove the unconditional post-commit repair invalidation and connect only the frozen table-specific invalidation inside the repair transaction;
5. add deterministic probe/fault/clock seams kept non-public in production;
6. add adversarial evidence for every capability, stale-meta, rollback, fallback, authority, and concurrent-currentness path listed in groups 1–17.
