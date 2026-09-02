---
id: storage-001
scope: storage-sqlcipher
status: done
depends-on: [foundation-001]
---

# SQLCipher storage foundation

## Objective

Deliver the first real, independently verifiable storage vertical slice. Create `crates/deepchat-services` and `crates/deepchat-platform` with non-empty production implementations. Do not create `deepchat-core` for this task and do not add a fake production schema.

Implement the SQLCipher-compatible connection and startup foundation: UTF-8 key application, SQLCipher compatibility `4`, WAL ordering, schema-version high-water mark, ordered per-version transactions including empty markers, narrow tolerated-error handling, orphan-WAL protection, typed startup classification with an in-process `VerifiedPassword` marker, quarantine of database/WAL/SHM files, and the platform secret-store port with a macOS Keychain target adapter plus manual fallback abstraction. Keep all fixtures generated and isolated.

This task does not implement the complete production schema catalog or claim historical migrations 1..69. The factual reference latest version is 69. A fresh new database records the latest marker directly; an existing database runs each version transaction from its recorded high-water mark. `storage-002` owns the full schema catalog/repair, FTS, dynamic DDL, backup/import, and migration overwrite behavior.

## Context

- `docs/INDEX.md`
- `docs/architecture.md`
- `docs/storage.md`
- `docs/plan/README.md`
- `docs/plan/analysis/porting-roadmap.md`
- `PORTING.md`
- `parity/manifest.json` (`storage-sqlcipher`)
- Frozen reference commit `ca75acfdc680fa3d0a2bbde13575fa711d08a3bd`
- Reference selectors: `src/main/data/connectionConfig.ts`, `src/main/data/databaseConnection.ts`, `src/main/data/databaseStartupRecovery.ts`, `src/main/data/mainDatabase.ts` (`schema_versions` and `migrate`), `src/main/app/databaseStartup.ts`, `src/shared/contracts/databaseSecurity.ts`, `test/main/data/databaseConnection.test.ts`, `test/main/data/databaseStartupRecovery.test.ts`, `test/main/data/mainDatabase.test.ts`, `test/main/app/databaseStartup.test.ts`

## Path

- `crates/deepchat-services/`
- `crates/deepchat-platform/`
- `tests/fixtures/` (generated-fixture helpers only)
- `docs/storage.md`
- `docs/plan/tasks/storage-001.md`
- `parity/evidence/storage-sqlcipher/`
- `parity/manifest.json`

Do not read or modify `/Users/colab/Documents/workspace/deepchat-2`; it is a read-only oracle. Do not access real profiles, databases, Keychain items, or provider credentials.

## Contracts

### Connection and migration

- Apply `cipher='sqlcipher'`, `legacy=4`, then the UTF-8 password bytes for encrypted connections; enable `journal_mode=WAL` after keying.
- Reject an orphan WAL (`dbPath-wal` exists while `dbPath` does not) without creating a replacement database or deleting the sidecar.
- Maintain `schema_versions(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL)` and use `MAX(version)` as the high-water mark.
- Fresh database: record the catalog latest version once, without replaying historical migration SQL.
- Existing database: execute versions in ascending order, one transaction per version; record empty versions; write each marker after that version succeeds; rollback the version on failure.
- Tolerate only explicitly allowlisted, statement-specific idempotency errors. Never blanket-ignore migration failures.

### Startup and recovery

- The password resolver consumes wrong-password attempts in its own retry loop. Return a typed `VerifiedPassword` marker only after validation succeeds.
- Destructive errors with no verified password classify as `Unreadable`; destructive errors after `VerifiedPassword` classify as `TrueCorruption`. Orphan WAL is always its own classification. Cancellation makes no destructive filesystem change.
- Quarantine existing DB, WAL, and SHM files into one newly allocated directory. Surface partial failure as an error and never claim the move completed.
- Log safe classifications/codes only; never log passwords, keys, SQL, credentials, or unsanitized driver messages.

### Platform secret store

- Define a typed secret-store trait/port and manual fallback abstraction.
- Provide a macOS Keychain adapter as a target implementation with injectable service/account policy. Service/account values are target policy only: there is no reference contract for them and parity evidence must not claim one.
- Unit tests use generated fakes and temporary paths; no test touches the user's Keychain or credentials.

## Verification

Run from the repository root:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
uv run python tools/parity-audit/validate.py
```

Acceptance evidence must include reproducible commands, output, and generated-fixture metadata under `parity/evidence/storage-sqlcipher/` and prove:

1. UTF-8 password bytes, SQLCipher compatibility 4, and WAL-after-key ordering.
2. Fresh latest-marker behavior and existing-database ordered per-version transactions with empty markers.
3. Rollback and narrow tolerated-error behavior.
4. Orphan WAL guard and no replacement/deletion.
5. Inner wrong-password retry, `VerifiedPassword`, `Unreadable` versus `TrueCorruption`, and cancellation.
6. Atomic quarantine of DB/WAL/SHM plus observable partial-failure behavior.
7. Keychain adapter and manual fallback through injected/generated fakes only.
8. No-secret logging assertions.

The completed slice may set `storage-sqlcipher` to `implemented` when these checks have real implementation evidence. Do not set it to `verified` or claim production migrations 1..69/full schema parity until `storage-002` and the remaining platform/runtime checks are complete.
