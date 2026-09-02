---
id: storage-002a-3
scope: storage-sqlcipher
status: ready
depends-on: [storage-002a-2]
---

# Dynamic FTS and memory/tape projections

## Objective

Complete the dynamic-search and projection slice on top of the production catalog and repair surface delivered by `storage-002a-1` and `storage-002a-2`. Implement connection-scoped FTS5 capability probing, dynamic virtual-table ownership, agent-memory FTS lifecycle, tape-search projection/FTS lifecycle, memory-ingestion projection lifecycle, and canonical copy exclusions as one independently verifiable vertical slice.

This task must port the complete frozen behavior rather than expose caller-normalized stand-ins. It therefore owns the minimal frozen Tape entry and effective-semantics types required to derive memory-ingestion rows. Later chat/session work must reuse that boundary instead of reimplementing projection semantics.

The `storage-sqlcipher` feature remains `implemented`. This task does not implement backup/import/encryption workflows, promote the feature to `verified`, change global schema version 69, alter static catalog topology, or change the frozen reference.

## Frozen oracle and required reading

Use frozen reference commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd` as a read-only oracle. Relevant selectors are:

- `src/main/memory/data/tables/agentMemory.ts`;
- `src/main/memory/data/tables/agentMemoryFtsPolicy.ts`;
- `src/main/tape/infrastructure/sqlite/tapeSearchProjectionStore.ts`;
- `src/main/memory/data/tables/deepchatMemoryIngestionProjection.ts`;
- the Tape message/tool effective-semantics helpers consumed by that projection;
- `src/main/data/sqliteCopyExclusions.ts`;
- focused FTS, projection, repair-invalidation, and copy-exclusion tests.

Read `docs/INDEX.md`, `docs/storage.md`, `docs/plan/README.md`, `docs/plan/analysis/porting-roadmap.md`, `docs/plan/tasks/storage-002.md`, `docs/plan/tasks/storage-002a-1.md`, `docs/plan/tasks/storage-002a-2.md`, and the latest accepted `storage-002a-2` review before implementation.

Do not access real profiles, databases, Keychain items, credentials, or provider sessions. Tests use generated isolated SQLCipher fixtures only.

## Scope and locked translations

### Connection-scoped FTS capability

- Probe at most once per owning subsystem and connection identity. Never cache capability by database path or across reopen.
- Probe in strict order: `trigram`, then `unicode61`, then unavailable. Probe with temporary FTS5 virtual tables and clean them up immediately.
- Keep tokenizer and dynamic object names inside closed internal enums/constants. No public API accepts arbitrary tokenizer names, virtual-table names, or SQL fragments.
- If a connection-local deterministic scalar function is required for agent scope tokens, enable only the necessary `rusqlite` feature and register it on the owning connection.
- Capability is scoped to connection identity and expires with that connection. Repeated Tape-search owner instances on one connection share their probe result; an agent-memory connection owner probes at most once for itself. Cross-subsystem sharing between agent-memory and Tape-search is not required. A process-global path-keyed map is forbidden.

### Agent-memory FTS lifecycle

- Own dynamic `agent_memory_fts` and static/meta `agent_memory_fts_meta` with `schema_version = 4` and `policy_version = 3`.
- Enable agent FTS only for `trigram`. `unicode61` and unavailable capability are permanent LIKE-only states for that connection and must drop/disable an existing agent FTS rather than create a downgraded index.
- Mirror only rows where `superseded_by IS NULL`, `lifecycle_state = 'active'`, and `kind NOT IN ('persona', 'working')`.
- Derive the scope token from the first four base64url characters of `SHA-256(agent_id)`. Search candidates must still be authority-checked against the real `agent_id`; the short token is not an authorization boundary.
- Rebuild when tokenizer, schema version, policy version, indexed generation, or mutation generation is missing or stale. A generation mismatch is a recoverable dirty state.
- Authoritative agent-memory create/update/delete/bulk mutation and mirror maintenance share one outer transaction. FTS work runs behind a nested savepoint so FTS failure leaves the authoritative mutation committed while marking/recovering derived state as dirty.
- Remove legacy FTS triggers idempotently. Trigger-owned mirror mutation must never be restored.
- Consume the existing `storage-002a-2` repair-hook invalidation of FTS metadata. Do not introduce a parallel repair flag or version source.
- Trim recall queries, split on Unicode whitespace, and remove empty terms. An empty query or empty term set returns no results. Use trigram only when every non-empty term has at least three Unicode code points; if any non-empty term is shorter or trigram is unavailable, perform one bounded LIKE query.
- Support frozen `all` and `any` match modes, escape FTS5 quotes/operators, enforce scope, and cap limits before querying.
- Fuse BM25 lexical candidates with importance/recency candidates using the frozen ranking and dedupe behavior. Recheck each result against the real `agent_id`, recallable predicate, and scope before returning it.
- Only non-transient FTS failures mark the index dirty. Fall back to bounded LIKE and use the frozen 30-second recovery cooldown before attempting automatic recovery again.

### Tape-search projection and FTS

- Own projection micro-version `9`, independently of global `schema_versions`.
- Own `deepchat_tape_search_projection`, `deepchat_tape_search_projection_meta`, `deepchat_tape_search_fts_meta`, and dynamic `deepchat_tape_search_fts`.
- Implement the frozen public surface: `appendSession`, `replaceSession`, `getSessionMeta`/`isCurrent`, `getProjectedEntryIds`, `getByEntryIds`, `getByEntryIdsIfCurrent`, single-session `search`, multi-source read-only search, `deleteBySession`, and `clearAll`. Do not invent an independent invalidation or current-range API for this owner.
- Projection metadata is current only when both `projection_version` and `max_entry_id` match the authoritative Tape head. Opening an owner prunes stale projection/meta/FTS state according to the frozen lifecycle.
- Prefer `trigram`; use `unicode61` as a valid tape-search FTS fallback. If capability is unavailable or read-time FTS state is stale/corrupt, search through the base projection using LIKE.
- Projection rows, FTS rows, FTS metadata, and projection metadata are one transaction for append/replace. An FTS write failure rolls back the entire Tape-search projection mutation.
- Rebuild FTS per session when FTS metadata and projection head differ.
- Single-session search is authorized by `session_id`. Multi-source read-only search is authorized by the complete supplied `(session_id, max_entry_id)` set; every source must be current or the operation returns no partial result. FTS metadata must match every authorized head.
- Support the frozen optional `kinds`, `startCreatedAt`, and `endCreatedAt` filters. `source_type` and `source_id` are result fields, not query filters. Apply FTS-first then LIKE-fill behavior, frozen dedupe keys, and stable result ordering. Normalize limit to `1..100`.
- Multi-source authority and projection-row source fields are distinct concepts. No row may cross its authorized session set.
- Malformed refs JSON degrades to an empty object without failing the projection/query path.

### Memory-ingestion projection

- Own projection micro-version `1`, independently of global `schema_versions`.
- Own `deepchat_memory_ingestion_projection` and `deepchat_memory_ingestion_projection_meta`.
- Introduce the minimal frozen Tape entry/effective-semantics domain needed by this owner, including message retraction identity, tool identity/rank, message rank, effective message conversion, and retired-workflow-result filtering. Do not make callers precompute normalized projection mutations.
- Incremental append is valid only when the previous projection head is contiguous with the authoritative Tape head.
- Context entries advance the head without creating a message row.
- Retraction removes the referenced message row and makes the session stale.
- Accept only final effective user/assistant messages with `sent|error` semantics. Retired workflow-result messages never enter the projection.
- A final tool-call updates `had_tool_use` only when the projected message row already exists. A later final assistant message derives `had_tool_use` from its own effective message blocks; the projection does not persist a separate pre-message tool fact.
- Session replace validates that every supplied row belongs to the target session and replaces rows plus metadata in one transaction.
- The production composition point invokes projection append inside the authoritative Tape append transaction. Projection failure is caught, its metadata is deleted to mark the session stale, and the authoritative Tape append still commits.
- A current-range read observes authoritative Tape head and projection head in one SQL statement so concurrent append cannot create a false-current window.
- Invalidation deletes metadata; stale rows must never be reported as current.

### Static catalog and copy boundary

- Preserve exactly 41 catalog definitions, 39 physical create owners, 38 runtime migration owners, 38 startup catalog entries, 19 empty global markers, and global high-water mark 69.
- FTS virtual tables remain dynamic and never enter static catalog SQL or global migrations.
- Existing projection/base/meta table owners remain in their current catalog positions; fix catalog SQL only if frozen-oracle comparison proves it incorrect.
- Export one canonical copy-exclusion decision for later `storage-002b` reuse. It contains exactly:
  - `agent_memory_dirty`;
  - `agent_memory_dirty_ai`;
  - `agent_memory_dirty_au`;
  - `agent_memory_dirty_ad`;
  - `agent_memory_fts_meta`;
  - `deepchat_tape_search_fts_meta`.
- Near-match names are not excluded. This task does not perform a database copy.

### Safety and error boundary

- Public errors and observations expose stable categories only. They must not include raw SQL, database paths, query text, object identifiers not already part of a safe public domain, rusqlite text/source chains, agent content, or session content.
- FTS degradation is explicit and testable; do not silently claim indexed search when LIKE fallback ran.
- Fail fast on invalid session ownership, invalid replace input, or impossible projection ordering.

## Implementation paths

Expected ownership is under `crates/deepchat-services`:

- per-subsystem, connection-scoped capability ownership;
- agent-memory FTS owner;
- tape-search projection/FTS owner;
- memory-ingestion projection owner and minimal Tape effective-semantics types;
- canonical SQLite copy-exclusion decision;
- generated-fixture integration tests.

Update the crate exports, `rusqlite` features, startup/storage composition point, and existing repair invalidation wiring only where the real owner requires them. Do not create a speculative `deepchat-core` crate or unrelated abstractions.

## Acceptance evidence

Use generated isolated SQLCipher fixtures and adversarial fault injection. Evidence must prove:

1. Repeated Tape-search owners on one connection share one probe; an agent-memory connection owner probes at most once for itself; reopen probes again; both follow strict `trigram` → `unicode61` ordering.
2. No dynamic FTS table in static catalog SQL; catalog counts remain 41/39/38/38, empty markers remain 19, and global high-water mark remains 69 after every micro-version rebuild.
3. Agent FTS creates only under `trigram`; metadata is exactly schema 4/policy 3/tokenizer; `unicode61` and unavailable drop/disable it and use LIKE.
4. Agent backfill excludes persona, working, superseded, and non-active rows; scope hashing plus real-agent authority recheck prevents cross-agent results.
5. Agent recall proves Unicode term gating, `all`/`any`, escaping, bounded LIKE fallback, capped limits, BM25/importance/recency fusion, authority recheck, transient/non-transient failure handling, and recovery cooldown.
6. Agent create/update/delete/bulk mutation maintains the mirror. Injected FTS failure commits the authoritative row, records dirty metadata, and permits later rebuild.
7. Stale agent schema/policy/tokenizer/generation and repair-hook invalidation each cause a real rebuild on next owner open.
8. Tape projection v9 proves the exact public surface, exact head/version matching, and append/replace atomicity across base rows, FTS rows, and both metadata tables; injected FTS write failure rolls back the whole projection mutation.
9. Tape `trigram` and `unicode61` paths create dynamic FTS; unavailable/corrupt/stale read paths use LIKE. Single-session and complete multi-source authorization, all-or-nothing currentness, optional filters, FTS-first/LIKE-fill, dedupe, and stable ordering never leak rows.
10. Stale Tape FTS metadata, stale projection metadata, and missing tables prune/rebuild/fallback exactly according to the frozen owner lifecycle.
11. Memory-ingestion v1 covers ordered append, context, message replacement, tool-before/after-message effective semantics, retraction, retired-workflow filtering, stale invalidation, and full rebuild.
12. Authoritative Tape append plus ingestion projection failure commits the Tape row, removes projection metadata, and cannot report the stale session as current.
13. A one-statement Tape-head/projection-head read cannot report false-current under a concurrent-append fixture.
14. The canonical exclusion set has exactly six names and rejects near matches.
15. Existing startup/manual repair behavior and all catalog topology tests remain unchanged.
16. Public errors/observations contain no raw SQL, paths, query text, agent/session content, or unsafe source chains.
17. Fixtures are generated and isolated and do not access real profiles, Keychain, credentials, providers, or sessions.

## Required verification commands

Run from the repository root:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p deepchat-services --test fts_projection
cargo test --workspace
uv run python tools/parity-audit/validate.py
git diff --check
```

Also parse the CI workflow, scan the full tracked tree for trailing whitespace and developer-local absolute paths, and scan the worktree for database/WAL/SHM/backup artifacts and credential-like signatures. Evidence logs must contain no secrets, real profile data, or developer-local paths.

## Completion boundary

This task is complete only when all dynamic FTS owners and both projection lifecycles are connected to real generated SQLCipher fixtures, the repair invalidation is consumed, copy exclusions are canonical, and every acceptance group has independently reviewed evidence. Leave backup/import/encryption to `storage-002b`; leave the end-to-end migrate → repair → backup → restore → encryption-change proof to `storage-002` integration. Keep `storage-sqlcipher` at `implemented` and preserve all unrelated remaining gaps.
