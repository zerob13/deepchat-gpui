---
task: storage-002a-2
scope: schema diagnosis + schema-error classifier
review-kind: partial-gate
conclusion: blocked
---

# storage-002a-2 — partial gate review 01

## Boundary

This is a **partial gate** only. It reviews the current schema diagnosis and schema-error classifier implementation against the frozen oracle commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`. It does not approve the whole `storage-002a-2` task: repair/backup ordering, transaction-scoped hooks, and startup one-shot recovery remain outside this review boundary.

No real profiles, databases, Keychain items, credentials, or provider sessions were accessed.

## Findings

### P1 — blocking: classifier `Debug` output leaks the supposedly private identity/dedupe key

- **Code:** [`crates/deepchat-services/src/schema_error_classifier.rs:27`](../../../crates/deepchat-services/src/schema_error_classifier.rs#L27), [`crates/deepchat-services/src/schema_error_classifier.rs:67`](../../../crates/deepchat-services/src/schema_error_classifier.rs#L67)
- **Test that codifies the leak:** [`crates/deepchat-services/tests/schema_repair.rs:161`](../../../crates/deepchat-services/tests/schema_repair.rs#L161)
- **Contract:** [`docs/plan/tasks/storage-002a-2.md:36`](../tasks/storage-002a-2.md#L36), [`docs/plan/tasks/storage-002a-2.md:61`](../tasks/storage-002a-2.md#L61), [`docs/plan/tasks/storage-002a-2.md:64`](../tasks/storage-002a-2.md#L64)

`SchemaErrorClassification` exposes a derived public `Debug` implementation. Although `dedupe_key` is a private field and its accessor is crate-private, the derived formatter prints `dedupe_key`, which contains the parsed table/column identity from the raw SQLite message. The integration test explicitly requires that identity to appear in debug output. Any caller that logs `{:?}` therefore leaks the identifier, defeating the stated redaction boundary for classifier identity. Do not derive public `Debug` for this payload, or implement a redacted formatter that renders only the stable reason.

## Verified behavior within this partial boundary

- Public diagnosis types retain all required diagnosis fields and use the four stable issue strings: [`schema_repair.rs:10`](../../../crates/deepchat-services/src/schema_repair.rs#L10) through [`schema_repair.rs:47`](../../../crates/deepchat-services/src/schema_repair.rs#L47). `SchemaDiagnosisError::Read` is typed and contains no raw driver source chain.
- The diagnosis loop preserves catalog-table order and, for an existing table, column-before-index order; repairability and repairable/manual projections follow the specified rules: [`schema_repair.rs:79`](../../../crates/deepchat-services/src/schema_repair.rs#L79) through [`schema_repair.rs:166`](../../../crates/deepchat-services/src/schema_repair.rs#L166).
- The catalog assertions verify 41 manual definitions, 38 startup definitions, the three legacy startup exclusions, and the four settings exclusions: [`tests/schema_repair.rs:31`](../../../crates/deepchat-services/tests/schema_repair.rs#L31) through [`tests/schema_repair.rs:47`](../../../crates/deepchat-services/tests/schema_repair.rs#L47).
- Snapshot reading uses `sqlite_master`, excludes `sqlite_%` tables and indexes, binds the index owning-table predicate, and quotes apostrophes before interpolation into `PRAGMA table_info(...)`: [`schema_repair.rs:175`](../../../crates/deepchat-services/src/schema_repair.rs#L175) through [`schema_repair.rs:215`](../../../crates/deepchat-services/src/schema_repair.rs#L215). The live SQLite test covers an apostrophe-bearing identifier rather than only a mock.
- Type normalization trims and ASCII-uppercases both declared and observed types and maps empty values to `None`; absent actual types produce a type mismatch only for checked, declared columns: [`schema_repair.rs:99`](../../../crates/deepchat-services/src/schema_repair.rs#L99) through [`schema_repair.rs:125`](../../../crates/deepchat-services/src/schema_repair.rs#L125), [`schema_repair.rs:217`](../../../crates/deepchat-services/src/schema_repair.rs#L217).
- Classifier priority matches the frozen pattern order, recognizes quoted/unquoted ASCII word/hyphen identities case-insensitively, checks numeric column-count grammar, and keeps the dedupe accessor crate-private: [`schema_error_classifier.rs:41`](../../../crates/deepchat-services/src/schema_error_classifier.rs#L41) through [`schema_error_classifier.rs:110`](../../../crates/deepchat-services/src/schema_error_classifier.rs#L110). The `type-mismatch` reason remains in the stable domain and is not raw-message classified.
- `schema_repair` and `schema_error_classifier` are public modules, while the dedupe-key method is `pub(crate)`, which is sufficient for subsequent crate-local repair/startup integration without adding a public accessor: [`lib.rs:13`](../../../crates/deepchat-services/src/lib.rs#L13) through [`lib.rs:16`](../../../crates/deepchat-services/src/lib.rs#L16), [`schema_error_classifier.rs:33`](../../../crates/deepchat-services/src/schema_error_classifier.rs#L33) through [`schema_error_classifier.rs:37`](../../../crates/deepchat-services/src/schema_error_classifier.rs#L37).

## Coverage gaps (non-blocking except where covered by P1)

- The diagnosis test proves apostrophe quoting, ordering, filtering, and empty observed-type normalization, but does not directly assert exclusion of a real SQLite internal table/index or an adversarial identifier containing quote-plus-statement text. The current escaping is structurally safe for the PRAGMA call, but explicit adversarial regression coverage is still warranted.
- The classifier tests cover representative quoted/unquoted hyphen names, case-insensitivity, and a few near misses. They do not separately exercise all quoted/unquoted variants for every frozen pattern, malformed closing quotes, singular/plural permutations, or identities that should be rejected after a valid-looking prefix.
- The 41/38 test checks counts and exclusions, not complete ordered membership. Existing production-catalog tests provide broader topology coverage, but this focused suite does not independently lock all catalog names and order.

## Checks

| Command | Result |
| --- | --- |
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace` | PASS — 62 tests passed |
| `cargo test -p deepchat-services --test schema_repair` | PASS — 3 tests passed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |
| `git diff --check` | PASS |

## Conclusion

**blocked** for this partial gate until the P1 debug-format identity leak is removed and its test no longer asserts leakage. Passing checks do not close the wider `storage-002a-2` task, which still requires review of repair, backups, hooks, and startup recovery.
