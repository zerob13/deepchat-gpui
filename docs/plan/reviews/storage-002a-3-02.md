---
task: storage-002a-3
review: 02
verdict: fail
reviewer: independent
---

# storage-002a-3 final implementation review

## Verdict

**FAIL.** The earlier eight findings are substantially repaired and every Rust gate passes, but the implementation still diverges from the frozen public behavior in four blocking areas. In particular, the memory-ingestion full-rebuild API still requires caller-normalized projection rows, which the task explicitly forbids, and the Rust effective semantics incorrectly treats terminal `tool_result` entries as tool-use mutations.

Frozen oracle inspected at commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`.

## Blocking findings

### 1. Memory-ingestion full rebuild is still a caller-normalized stand-in

The locked translation requires the owner to accept the minimal frozen Tape domain and derive effective projection rows itself; callers must not precompute normalized projection mutations ([storage-002a-3.md:14](../tasks/storage-002a-3.md#L14), [storage-002a-3.md:77](../tasks/storage-002a-3.md#L77)). The incremental API follows that model, but `replace_session` publicly accepts `&[IngestionInput]` and inserts those precomputed rows directly ([memory_ingestion_projection.rs:30](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L30), [memory_ingestion_projection.rs:79](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L79), [memory_ingestion_projection.rs:101](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L101)). The only rebuild test constructs `IngestionInput` itself and therefore confirms the prohibited boundary rather than effective-semantic rebuilding ([fts_projection.rs:650](../../../crates/deepchat-services/tests/fts_projection.rs#L650), [fts_projection.rs:653](../../../crates/deepchat-services/tests/fts_projection.rs#L653)).

The frozen owner derives rebuild inputs through shared Tape effective semantics; the Rust public rebuild must accept Tape entries (or an equivalent authoritative Tape-domain input) and internally apply message rank, tool rank, retraction, retired-workflow, and final-tool-use behavior. Acceptance group 11 and the completion boundary are not met.

### 2. `tool_result` incorrectly mutates `had_tool_use`

The frozen ingestion owner updates `had_tool_use` only for terminal `tool_call` rows (`row.kind === 'tool_call' && tapeToolRank(...) > 0`); `readTapeToolIdentity` supports both kinds for other consumers, but ingestion does not apply `tool_result` as a mutation. Rust instead enters the tool branch for every non-message row whose metadata is terminal and whose `tool_identity` accepts either `tool_call` or `tool_result` ([memory_ingestion_projection.rs:307](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L307), [memory_ingestion_projection.rs:411](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L411), [memory_ingestion_projection.rs:424](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L424)). A terminal `tool_result` can therefore set `had_tool_use`, unlike the frozen implementation. Existing evidence tests valid and malformed `tool_call` only ([fts_projection.rs:608](../../../crates/deepchat-services/tests/fts_projection.rs#L608)).

The same conversion also stores the embedded message record's `sessionId` in the projection ([memory_ingestion_projection.rs:320](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L320), [memory_ingestion_projection.rs:340](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L340)); the frozen owner uses authoritative `row.session_id`. A malformed embedded cross-session value can write a row under another session while advancing metadata for the Tape row's real session. This needs an authority test and correction.

### 3. Tape read-source normalization differs from the frozen authorization boundary

Frozen `normalizeDeepChatTapeReadSources` trims session IDs and collapses repeated sessions to the **maximum** requested head. Rust deduplicates only exact `(session_id, max_entry_id)` pairs, does not trim, and retains multiple heads for one session ([tape_search_projection.rs:772](../../../crates/deepchat-services/src/tape_search_projection.rs#L772)). This changes the complete supplied authorization set and can duplicate authorized rows through the JSON CTE. Existing multi-source evidence uses distinct sessions only ([fts_projection.rs:345](../../../crates/deepchat-services/tests/fts_projection.rs#L345)).

The owner must port trim + per-session maximum normalization and prove duplicate/conflicting-head authorization without row duplication or partial leakage.

### 4. Open-time Tape pruning does not prune stale dynamic FTS rows

The frozen `pruneInvalidProjectionRows` removes invalid projection rows/meta **and** deletes dynamic FTS rows lacking current FTS/projection metadata, dropping the FTS table if cleanup fails. Rust `prune_invalid` removes projection rows and both metadata kinds but never deletes stale rows from `deepchat_tape_search_fts` ([tape_search_projection.rs:721](../../../crates/deepchat-services/src/tape_search_projection.rs#L721)). Thus opening an owner with stale micro-version state leaves stale dynamic FTS content until a later per-session operation happens. The existing micro-version test checks only projected IDs and then replaces the same session; it never asserts open-time FTS pruning ([fts_projection.rs:438](../../../crates/deepchat-services/tests/fts_projection.rs#L438)). This contradicts acceptance group 10's frozen owner lifecycle.

## Prior-review finding closure

| Prior finding | Result | Evidence |
|---|---|---|
| Agent bulk/failure lifecycle | **Closed** | Public bulk boundary and nested mirror savepoint are present ([agent_memory_fts.rs:262](../../../crates/deepchat-services/src/agent_memory_fts.rs#L262), [agent_memory_fts.rs:314](../../../crates/deepchat-services/src/agent_memory_fts.rs#L314)); mirror-failure evidence proves authoritative commit and generation mismatch ([fts_projection.rs:164](../../../crates/deepchat-services/tests/fts_projection.rs#L164)). |
| Agent candidate fusion | **Closed** | Independently bounded importance candidates now use `min(800, max(64, limit*8))` and fuse after the same MATCH ([agent_memory_fts.rs:453](../../../crates/deepchat-services/src/agent_memory_fts.rs#L453), [agent_memory_fts.rs:476](../../../crates/deepchat-services/src/agent_memory_fts.rs#L476)). |
| Tape stale-head append | **Closed** | Incremental insertion requires full FTS metadata equality with previous projection metadata ([tape_search_projection.rs:181](../../../crates/deepchat-services/src/tape_search_projection.rs#L181), [tape_search_projection.rs:203](../../../crates/deepchat-services/src/tape_search_projection.rs#L203)); wrong-head rebuild is tested ([fts_projection.rs:405](../../../crates/deepchat-services/tests/fts_projection.rs#L405)). |
| Retired-workflow predicate | **Closed** | Exact `messageType == "workflow_result"` predicate and fixture exist ([memory_ingestion_projection.rs:393](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L393), [fts_projection.rs:608](../../../crates/deepchat-services/tests/fts_projection.rs#L608)). |
| Tool identity validation | **Partially closed** | Object/nested-string IDs are validated ([memory_ingestion_projection.rs:424](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L424)), but ingestion wrongly applies `tool_result`; see blocker 2. |
| Invented post-commit repair invalidation | **Closed** | No implementation diff remains in repair; existing table-specific hook invalidates Agent FTS metadata inside the repair transaction ([schema_repair.rs:353](../../../crates/deepchat-services/src/schema_repair.rs#L353), [schema_repair.rs:447](../../../crates/deepchat-services/src/schema_repair.rs#L447)). |
| Capability/fallback and cooldown evidence | **Closed** | Strict probe tests, connection-local Tape cache/reopen, Agent unicode61/unavailable owner paths, and transient/non-transient cooldown tests call production internals ([fts.rs:59](../../../crates/deepchat-services/src/fts.rs#L59), [tape_search_projection.rs:888](../../../crates/deepchat-services/src/tape_search_projection.rs#L888), [agent_memory_fts.rs:805](../../../crates/deepchat-services/src/agent_memory_fts.rs#L805), [agent_memory_fts.rs:866](../../../crates/deepchat-services/src/agent_memory_fts.rs#L866)). |
| Fault/concurrency evidence | **Closed for the originally named paths** | Tape FTS/meta rollback and a two-connection WAL fixture exist ([fts_projection.rs:829](../../../crates/deepchat-services/tests/fts_projection.rs#L829), [fts_projection.rs:904](../../../crates/deepchat-services/tests/fts_projection.rs#L904), [fts_projection.rs:553](../../../crates/deepchat-services/tests/fts_projection.rs#L553)). |

## Acceptance matrix

| Group | Result | Evidence assessment |
|---|---|---|
| 1. Probe ownership/order/reopen | **Proved** | Closed probe trait; strict order tests; Tape temp-table cache is connection-local and reopen reprobes ([fts.rs:35](../../../crates/deepchat-services/src/fts.rs#L35), [tape_search_projection.rs:550](../../../crates/deepchat-services/src/tape_search_projection.rs#L550)). |
| 2. Static topology/high-water | **Proved** | 41/39/38/38, 19 markers, v69, and no static virtual-table SQL asserted through production catalog ([fts_projection.rs:70](../../../crates/deepchat-services/tests/fts_projection.rs#L70)). |
| 3. Agent capability modes/meta | **Proved** | Trigram schema 4/policy 3 lifecycle plus unicode61/unavailable drop and LIKE-only tests ([agent_memory_fts.rs:162](../../../crates/deepchat-services/src/agent_memory_fts.rs#L162), [agent_memory_fts.rs:805](../../../crates/deepchat-services/src/agent_memory_fts.rs#L805)). |
| 4. Agent backfill/authority | **Proved** | Recallable predicate exists in rebuild and queries; scope/status/supersession and a real short-hash collision are tested ([agent_memory_fts.rs:204](../../../crates/deepchat-services/src/agent_memory_fts.rs#L204), [fts_projection.rs:711](../../../crates/deepchat-services/tests/fts_projection.rs#L711)). |
| 5. Agent recall semantics | **Proved** | Unicode gating, modes, quote escaping, capped LIKE, fusion, failure classification, and exact cooldown are exercised through owner search ([agent_memory_fts.rs:381](../../../crates/deepchat-services/src/agent_memory_fts.rs#L381), [agent_memory_fts.rs:453](../../../crates/deepchat-services/src/agent_memory_fts.rs#L453), [agent_memory_fts.rs:866](../../../crates/deepchat-services/src/agent_memory_fts.rs#L866)). |
| 6. Agent mutations/failure recovery | **Proved** | Upsert/delete/bulk and mirror failure generation/reopen behavior call production owner APIs ([agent_memory_fts.rs:238](../../../crates/deepchat-services/src/agent_memory_fts.rs#L238), [fts_projection.rs:164](../../../crates/deepchat-services/tests/fts_projection.rs#L164), [fts_projection.rs:381](../../../crates/deepchat-services/tests/fts_projection.rs#L381)). |
| 7. Agent stale/repaired rebuild | **Proved** | Missing/schema/policy/tokenizer/generation/table variants and existing repair service hook are covered ([fts_projection.rs:207](../../../crates/deepchat-services/tests/fts_projection.rs#L207), [fts_projection.rs:241](../../../crates/deepchat-services/tests/fts_projection.rs#L241), [schema_repair.rs:447](../../../crates/deepchat-services/src/schema_repair.rs#L447)). |
| 8. Tape exact surface/atomic writes | **Proved** | Public methods, full previous metadata equality, and FTS/projection-meta rollback tests are present ([tape_search_projection.rs:121](../../../crates/deepchat-services/src/tape_search_projection.rs#L121), [tape_search_projection.rs:170](../../../crates/deepchat-services/src/tape_search_projection.rs#L170)). |
| 9. Tape search authority/fallback | **Fail** | Core FTS-first/LIKE-fill/filter/dedupe paths work, but read-source normalization is not frozen-equivalent; see blocker 3. |
| 10. Tape stale/missing lifecycle | **Fail** | Metadata/head recovery works, but open-time stale dynamic FTS row pruning is missing; see blocker 4. |
| 11. Ingestion effective semantics | **Fail** | Incremental semantics improved, but full rebuild requires caller-normalized rows and `tool_result`/embedded-session behavior diverges; see blockers 1–2. |
| 12. Tape append/projection failure isolation | **Proved** | Authoritative Tape insert and projection are composed in one outer transaction; projection failure deletes metadata and commits Tape ([memory_ingestion_projection.rs:232](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L232), [fts_projection.rs:650](../../../crates/deepchat-services/tests/fts_projection.rs#L650)). |
| 13. Atomic head observation | **Proved** | Production read is one SQL statement and a generated two-connection fixture races append/read ([memory_ingestion_projection.rs:123](../../../crates/deepchat-services/src/memory_ingestion_projection.rs#L123), [fts_projection.rs:553](../../../crates/deepchat-services/tests/fts_projection.rs#L553)). |
| 14. Copy exclusions | **Proved** | Exact six-name constant and near-match tests ([sqlite_copy.rs:3](../../../crates/deepchat-services/src/sqlite_copy.rs#L3), [fts_projection.rs:695](../../../crates/deepchat-services/tests/fts_projection.rs#L695)). |
| 15. Existing repair/catalog behavior | **Proved** | Existing production-schema and 24 repair tests pass; only the established transactional Agent hook remains. |
| 16. Public redaction | **Proved** | Public enums contain stable text only and Agent/Tape/ingestion failure assertions reject content/path/SQL sentinels ([fts_projection.rs:933](../../../crates/deepchat-services/tests/fts_projection.rs#L933)). |
| 17. Fixture isolation | **Proved** | Generated tempfile SQLCipher fixture proves correct/wrong key and owner reopen; no real user state is opened ([fts_projection.rs:854](../../../crates/deepchat-services/tests/fts_projection.rs#L854)). |

## Nonblocking observations

- Fault/clock/capability seams are module-private (`fn new_internal`, crate-private probe trait, `#[cfg(test)]` fault state) and do not expose fault injection publicly ([agent_memory_fts.rs:90](../../../crates/deepchat-services/src/agent_memory_fts.rs#L90), [agent_memory_fts.rs:106](../../../crates/deepchat-services/src/agent_memory_fts.rs#L106), [fts.rs:35](../../../crates/deepchat-services/src/fts.rs#L35)).
- The required full-tree absolute-path scan reports pre-existing tracked oracle/evidence paths (including task documents and baseline metadata). No newly changed production/test file contains a developer-local absolute path; this is not introduced by this slice.

## Verification executed

```text
cargo fmt --check                                               PASS
cargo clippy --workspace --all-targets --all-features -- -D warnings  PASS
cargo test -p deepchat-services --test fts_projection           PASS (20)
cargo test --workspace                                          PASS (116)
cargo test --workspace --all-targets --no-fail-fast             PASS (116)
uv run python tools/parity-audit/validate.py                     PASS
git diff --check                                                PASS
CI workflow YAML parse                                           PASS
changed-file developer-path / database-artifact / credential-signature scans  PASS
```

## Required closure

1. Replace public `replace_session(&[IngestionInput])` with a Tape-domain rebuild boundary that derives effective rows internally, and test full rebuild against incremental results.
2. Apply tool mutations only for terminal `tool_call`; add negative `tool_result` evidence and use authoritative Tape `row.session_id` for projected messages.
3. Port read-source normalization exactly: trim IDs and retain one maximum head per session; test conflicting duplicates.
4. Port frozen open-time stale FTS-row pruning (including corrupt-cleanup degradation) and assert the dynamic table contains no stale rows immediately after owner construction.
