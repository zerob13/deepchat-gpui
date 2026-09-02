# storage-001 review 01

## Findings

1. **P1 — blocking — `VerifiedPassword` is forgeable without validation.**
   - **Contract:** [`docs/plan/tasks/storage-001.md:55-56`](../tasks/storage-001.md#L55) and [`docs/storage.md:53-57`](../../storage.md#L53) require a marker that can exist *only after* successful validation; only that fact may promote a destructive error to `TrueCorruption`.
   - **Code:** [`crates/deepchat-services/src/password.rs:16-28`](../../../crates/deepchat-services/src/password.rs#L16) exposes `pub fn VerifiedPassword::new(SecretString)`. [`crates/deepchat-services/src/startup.rs:53-68`](../../../crates/deepchat-services/src/startup.rs#L53) accepts that externally constructible value and uses `password.is_some()` as the verified predicate; [`crates/deepchat-services/tests/storage_contract.rs:130-131`](../../../crates/deepchat-services/tests/storage_contract.rs#L130) itself creates it directly.
   - **Reproduction/evidence:** Any composition-root caller can create `VerifiedPassword::new` from an unvalidated candidate, pass it to `Storage::open`, and cause an otherwise destructive open failure to classify as `TrueCorruption`. The type therefore does not encode the asserted security boundary. Make construction private to the resolver/validator path (or use a sealed/private capability) and test that an unvalidated candidate cannot reach the promotion path.

2. **P1 — blocking — public errors leak migration SQL through `Debug`; the evidence claims a property it does not prove.**
   - **Contract:** [`docs/plan/tasks/storage-001.md:58`](../tasks/storage-001.md#L58), [`docs/storage.md:32`](../../storage.md#L32), and [`docs/storage.md:71-73`](../../storage.md#L71) prohibit SQL and raw driver messages in safe error/logging surfaces. Acceptance item 8 requires no-secret logging assertions.
   - **Code:** [`crates/deepchat-services/src/schema.rs:13-24`](../../../crates/deepchat-services/src/schema.rs#L13) derives `Debug` and retains `rusqlite::Error`; [`crates/deepchat-services/src/startup.rs:30`](../../../crates/deepchat-services/src/startup.rs#L30) exposes it as an error source. `rusqlite::Error::SqlInputError` contains the original SQL. [`crates/deepchat-services/tests/storage_contract.rs:210-225`](../../../crates/deepchat-services/tests/storage_contract.rs#L210) checks only a wrong-key connection error, while [`parity/evidence/storage-sqlcipher/README.md:49`](../../../parity/evidence/storage-sqlcipher/README.md#L49) says it asserts error `Display`/`Debug` generally.
   - **Reproduction:** A synthetic temporary in-memory catalog with `INVALID TOPSECRET_SQL_TOKEN` produces `MigrationError::Sql(SqlInputError { ..., sql: "INVALID TOPSECRET_SQL_TOKEN", ... })` under `Debug`; the token is exposed. This probe touched no profile, Keychain, or real database. Do not derive/expose raw driver `Debug` on public errors; retain diagnostics in a redacted/non-displayable internal form, and add tests for migration SQL, marker, transaction, and startup wrappers.

3. **P1 — blocking — quarantine accepts an existing caller-selected directory, so it does not enforce the required newly allocated evidence boundary.**
   - **Contract:** [`docs/plan/tasks/storage-001.md:57`](../tasks/storage-001.md#L57) and [`docs/storage.md:30-31`](../../storage.md#L30) require moving all sidecars into one **newly allocated** directory and never overwriting preserved evidence.
   - **Code:** [`crates/deepchat-services/src/startup_recovery.rs:143-152`](../../../crates/deepchat-services/src/startup_recovery.rs#L143) allocates a candidate, but [`crates/deepchat-services/src/startup_recovery.rs:160-185`](../../../crates/deepchat-services/src/startup_recovery.rs#L160) takes an arbitrary `directory` and calls `create_dir_all`, accepting an already-existing directory. No storage-owner recovery API ties allocation and move into one non-reusable operation.
   - **Reproduction/evidence:** The existing unit test creates the candidate only to force a partial move ([`startup_recovery.rs:381-386`](../../../crates/deepchat-services/src/startup_recovery.rs#L381)); it does not show that ordinary recovery rejects a pre-existing directory. A caller can reuse a directory that already contains preserved files, violating the one-new-directory guarantee even if a same-name destination would later fail. Make allocation + quarantine one API that creates the directory atomically/new-only, or have the mover reject pre-existing destinations and return a typed collision error. Preserve and report all moved/unmoved paths on partial failure.

4. **P2 — blocking — password retry semantics intentionally diverge from the frozen reference without an explicit contract decision or coverage.**
   - **Contract/reference:** [`docs/plan/tasks/storage-001.md:55`](../tasks/storage-001.md#L55) calls for the resolver-owned retry loop. The frozen reference retries every manual-unlock validation failure other than decrypted corruption and orphan WAL: `databaseSecurity.ts:212-232` at `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`; it sets the next reason to `invalid` rather than returning an open/I/O error.
   - **Code:** [`crates/deepchat-services/src/password.rs:98-109`](../../../crates/deepchat-services/src/password.rs#L98) returns `PasswordError::Open`/`Io` for non-corruption/non-orphan failures, and [`password.rs:140-152`](../../../crates/deepchat-services/src/password.rs#L140) propagates them instead of continuing the retry loop. The only integration coverage is wrong-password retries ([`storage_contract.rs:181-192`](../../../crates/deepchat-services/tests/storage_contract.rs#L181)).
   - **Reproduction/evidence:** A synthetic `CannotOpen`/other validation open failure exits `resolve()` immediately; the reference loop would ask again with `invalid`. If this narrower retry policy is intentional target behavior, state it as an explicit approved divergence in the task/contract/manifest and evidence. Otherwise retry it consistently and test the exact reason transitions. The current evidence calls this “reference-compatible” without addressing the distinction.

## Checks run

All runs were from the repository root and used only build outputs plus generated temporary fixtures:

| Check | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace` | PASS — 37 tests passed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |
| `git diff --check` | PASS |
| `cargo test -p deepchat-services -p deepchat-platform` | PASS — 28 tests passed |
| Focused synthetic NUL-key probe | PASS safely — rejected as `database open failed`; no injection/secret output observed |
| Focused synthetic migration-debug probe | FAILS the no-SQL-leak contract — raw migration SQL appears in `Debug` as described above |
| `cargo check -p deepchat-platform --all-features --target x86_64-apple-darwin` | Not runnable: host only has `aarch64-apple-darwin`; target standard library is not installed |

The native host `all-features` clippy/test run compiled the macOS `security-framework` adapter. Its unit tests use injected fakes and did not touch the real Keychain. The adapter is a real production implementation on macOS ([`crates/deepchat-platform/src/keychain.rs:44-83`](../../../crates/deepchat-platform/src/keychain.rs#L44)); non-macOS remains an honest unavailable fallback ([`keychain.rs:86-117`](../../../crates/deepchat-platform/src/keychain.rs#L86)), not cross-platform support.

## Judgment

**blocked**

The manifest status is validly capped at `implemented`, not `verified`; its remaining gaps correctly defer the production catalog, repair/FTS/dynamic-DDL, and backup/import. The statement “41 tables, migrations 1..69” is scoped as storage-002 work rather than claiming that the current slice has a complete physical schema. That does not clear the blocking boundary failures above.

## Remaining uncertainty

- The macOS production Keychain path was deliberately not invoked; only compile coverage and injected-fake tests were used. It cannot be certified against a real Keychain without violating the review boundary.
- x86_64 macOS and Windows/Linux target compilation was not possible from this host because those Rust targets are absent; non-macOS behavior is documented as unavailable rather than tested platform support.
- The static/source and generated-fixture checks establish SQLCipher-4 ordering, UTF-8 metacharacter safety, orphan-WAL pre-main-file guard, high-water migrations, marker rollback, and strict allowlisting. They do not cure the identified public-capability, error-sanitization, and quarantine API defects.
