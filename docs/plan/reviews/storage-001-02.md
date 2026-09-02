# storage-001 review 02

## Findings

No P1/P2/P3 findings. No blocking or non-blocking contract/code mismatch was found in the current `storage-001` scope.

### Closure of review 01 blockers

1. **P1 closed — opaque verified-password capability and production-path classification.**
   - **Contract:** [`docs/plan/tasks/storage-001.md:53-58`](../tasks/storage-001.md#L53) and [`docs/storage.md:48-57`](../../storage.md#L48) require resolver-owned validation, a non-persistent typed fact, and `Unreadable` versus `TrueCorruption` classification based on that fact.
   - **Code:** [`crates/deepchat-services/src/password.rs:17-29`](../../../crates/deepchat-services/src/password.rs#L17) has private fields, private construction, crate-private secret access, no `Clone`, and no serde implementation. [`password.rs:81-100`](../../../crates/deepchat-services/src/password.rs#L81) defines a validator that returns facts, not a capability; its default implementation calls the real `validate_password`. Only [`password.rs:165-183`](../../../crates/deepchat-services/src/password.rs#L165) issues the capability after `Valid` or `DecryptedCorruption`. [`crates/deepchat-services/src/startup.rs:64-95`](../../../crates/deepchat-services/src/startup.rs#L64) consumes `Option<VerifiedPassword>`; classification uses the opaque value rather than caller-provided boolean input. `has_verified_password()` at [`startup.rs:106-108`](../../../crates/deepchat-services/src/startup.rs#L106) is a read-only owner state query, not a promotion input.
   - **Reproduction:** an external temporary crate calling `VerifiedPassword::validated(...)` fails to compile with `E0624: associated function 'validated' is private`. `storage_contract::verified_capability_promotes_real_storage_path_destructive_failure` performs temp SQLCipher resolver → `VerifiedPassword` → `Storage::open` and injects a real migration-finalizer `SQLITE_CORRUPT`, observing `TrueCorruption`; `destructive_storage_failure_without_capability_is_unreadable` exercises the same finalizer class without the capability and observes `Unreadable`. These are production `Storage`/migration paths, not a standalone classifier assertion.

2. **P1 closed — safe public error rendering and source chains.**
   - **Contract:** [`docs/plan/tasks/storage-001.md:58`](../tasks/storage-001.md#L58) and [`docs/storage.md:71-73`](../../storage.md#L71) require stable safe public errors without raw SQL, passwords, driver messages, or I/O messages.
   - **Code:** public connection errors are payload-free with respect to driver/I/O errors at [`crates/deepchat-services/src/connection.rs:13-29`](../../../crates/deepchat-services/src/connection.rs#L13); `MigrationError` is a closed, `Copy` public enum at [`schema.rs:13-46`](../../../crates/deepchat-services/src/schema.rs#L13); `StartupError` has explicit redacted `Debug` at [`startup.rs:19-48`](../../../crates/deepchat-services/src/startup.rs#L19). `PasswordError`, `KeychainError`, `SecretStoreError`, `QuarantineError`, and `OrphanWalDatabaseError` have no raw error fields or `#[source]` fields ([`password.rs:44-58`](../../../crates/deepchat-services/src/password.rs#L44), [`startup_recovery.rs:16-20`](../../../crates/deepchat-services/src/startup_recovery.rs#L16), [`startup_recovery.rs:129-140`](../../../crates/deepchat-services/src/startup_recovery.rs#L129), [`keychain.rs:16-22`](../../../crates/deepchat-platform/src/keychain.rs#L16), [`secret_store.rs:7-17`](../../../crates/deepchat-platform/src/secret_store.rs#L7)). Raw `rusqlite::Error` is confined to crate-private classification paths.
   - **Reproduction:** `storage_contract::public_migration_and_startup_errors_hide_sql_driver_sources_and_passwords` creates a real `SqlInputError` with unique invalid SQL, then checks `MigrationError` and wrapping `StartupError` `Display`, `Debug`, and `Error::source()` for the token, SQL text, and password. The test passes. The test also checks marker and transaction variants; quarantine partial explicitly checks an empty source chain at [`startup_recovery.rs:447-496`](../../../crates/deepchat-services/src/startup_recovery.rs#L447).

3. **P1 closed — new-only quarantine allocation and truthful partial state.**
   - **Contract:** [`docs/plan/tasks/storage-001.md:56-58`](../tasks/storage-001.md#L56) and [`docs/storage.md:29-32`](../../storage.md#L29) require a newly allocated directory, main → WAL → SHM order, no overwrite of preserved evidence, and observable partial failure.
   - **Code:** [`crates/deepchat-services/src/startup_recovery.rs:142-206`](../../../crates/deepchat-services/src/startup_recovery.rs#L142) uses the production `create_dir` port, not `create_dir_all`, and atomically allocates the directory before moving files. `AlreadyExists` moves to a numeric suffix; `checked_add` returns `NamespaceExhausted` on numeric overflow. The mover processes main, WAL, then SHM at [`startup_recovery.rs:208-243`](../../../crates/deepchat-services/src/startup_recovery.rs#L208). On failure it derives moved/unmoved/failed information from filesystem state and returns `QuarantineError::Partial` rather than success.
   - **Reproduction:** `quarantine_moves_main_wal_and_shm_into_new_directory` verifies the ordered full move; `partial_quarantine_reports_real_state_and_preserves_collision_evidence` deterministically pre-creates the base directory, forces a WAL target collision in the newly allocated `.1` directory through the production port, and verifies the original directory remains unchanged, the main is moved, WAL/SHM remain source-side, and every reported path equals the actual filesystem state. The injected filesystem is a test double at the port boundary; production code contains no test-only allocation weakening.

4. **P2 closed — retry transitions now align with the reference loop.**
   - **Contract/reference:** [`docs/plan/tasks/storage-001.md:55-56`](../tasks/storage-001.md#L55), [`docs/storage.md:53-57`](../../storage.md#L53), and frozen `src/main/app/databaseSecurity.ts:212-233` at `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd` require every non-terminal manual validation failure to transition to `invalid`; only orphan WAL and cancellation are terminal.
   - **Code:** [`crates/deepchat-services/src/password.rs:102-132`](../../../crates/deepchat-services/src/password.rs#L102) uses the real SQLCipher open/query validator; [`password.rs:165-183`](../../../crates/deepchat-services/src/password.rs#L165) retries `WrongPassword`, `Open`, and `Io` with `Invalid`, while preserving `OrphanWal` and `Cancelled` as terminal outcomes.
   - **Reproduction:** `validator_open_io_wrong_password_then_valid_retries_with_exact_reasons` verifies `ManualRequired → Invalid → Invalid → Invalid`; `injected_validator_orphan_wal_is_terminal` and `password_resolver_cancellation_makes_no_change` verify both terminal cases.

### Additional scope checks

- SQLCipher uses `cipher='sqlcipher'` → `legacy=4` → parameterized UTF-8 password → WAL at [`connection.rs:41-82`](../../../crates/deepchat-services/src/connection.rs#L41). The generated SQLCipher fixture uses non-ASCII and SQL metacharacters; an independent temporary runtime probe passed a NUL-containing password to `open_database` and received the safe `ConnectionError::Sqlite` result, with no interpolation or message leak.
- Orphan WAL is checked before opening/creating the database at [`connection.rs:57-70`](../../../crates/deepchat-services/src/connection.rs#L57); orphan WAL and cancellation are terminal in the resolver. Existing/fresh high-water migration behavior, empty markers, per-version rollback, and statement-specific allowlisting are covered by [`schema.rs:140-234`](../../../crates/deepchat-services/src/schema.rs#L140) and its generated-fixture tests.
- The macOS target uses a real `security-framework` adapter at [`crates/deepchat-platform/src/keychain.rs:44-83`](../../../crates/deepchat-platform/src/keychain.rs#L44); tests only inject fakes. Manual fallback is deliberately non-persistent at [`manual.rs:7-27`](../../../crates/deepchat-platform/src/manual.rs#L7). `Storage::close` closes the connection and maps close failure without exposing a driver source ([`startup.rs:110-113`](../../../crates/deepchat-services/src/startup.rs#L110)).
- [`parity/manifest.json:275-368`](../../../parity/manifest.json#L275) remains `implemented`, not `verified`. Its remaining gaps correctly defer the full catalog, dynamic schema, repair/FTS, and backup/import. It states global version 69 as deferred work; [`docs/storage.md:34-46`](../../storage.md#L34) and the evidence correctly distinguish 41 catalog definitions from 41 physical tables. [`parity/evidence/storage-sqlcipher/README.md`](../../../parity/evidence/storage-sqlcipher/README.md) accurately records the current 42 workspace tests and 33 services/platform tests.

## Checks run

All commands ran from the repository root against generated fixtures/build artifacts only; no real profile, Keychain item, or credential was accessed.

| Check | Result |
|---|---|
| `cargo fmt --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `cargo test --workspace` | PASS — 42 passed, 0 failed |
| `cargo test -p deepchat-services -p deepchat-platform` | PASS — 33 passed, 0 failed |
| `uv run python tools/parity-audit/validate.py` | PASS — `parity contract: PASS` |
| `git diff --check` | PASS |
| External-crate `VerifiedPassword::validated` probe | PASS — correctly rejected as private (`E0624`) |
| Runtime NUL-password probe | PASS — safely returned `Some(ConnectionError::Sqlite)` with no raw message |

## Judgment

**pass**

The four prior blockers are closed. The current implementation meets the bounded `storage-001` foundation contract and its evidence/manifest claims. This judgment does not upgrade the feature beyond `implemented` and does not assert complete production schema, all historical migrations, backup/import, or cross-platform Keychain validation.

## Remaining uncertainty

- The production macOS Keychain client was compiled but deliberately not invoked against a real Keychain; only injected fake coverage was used.
- Windows/Linux and macOS x86_64 target builds were not run on this arm64 host. Non-macOS behavior is explicitly unavailable rather than misrepresented as working support.
- The generated-fixture suite proves the stated SQLCipher/keying/recovery contract; it does not establish real-user database/profile interoperability beyond this intentionally limited foundation slice.
