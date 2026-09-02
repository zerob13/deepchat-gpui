//! Integration/contract tests for the SQLCipher storage foundation.
//!
//! Every fixture is generated in an isolated temporary directory. No test
//! touches a real profile, database, Keychain item, or provider credential.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::Path;
use std::rc::Rc;

use deepchat_services::clock::Clock;
use deepchat_services::connection::open_database;
use deepchat_services::password::{
    PasswordCancelled, PasswordError, PasswordResolver, PasswordValidator, UnlockProvider,
    UnlockReason, ValidationOutcome,
};
use deepchat_services::schema::{
    MigrationCatalog, MigrationError, MigrationFinalizer, MigrationRunner, SCHEMA_VERSIONS_TABLE,
};
use deepchat_services::startup::{StartupError, Storage};
use secrecy::SecretString;

struct TestClock(i64);

impl Clock for TestClock {
    fn now_millis(&self) -> i64 {
        self.0
    }
}

fn create_encrypted_db(db_path: &Path, password: &str) {
    let conn = open_database(db_path, Some(password)).unwrap();
    conn.execute_batch(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t(v) VALUES ('hello');",
    )
    .unwrap();
    conn.pragma_update(None, "wal_checkpoint", "TRUNCATE")
        .unwrap();
}

struct FakeProvider {
    passwords: Vec<&'static str>,
    next: usize,
    reasons: Vec<UnlockReason>,
}

impl FakeProvider {
    fn new(passwords: Vec<&'static str>) -> Self {
        Self {
            passwords,
            next: 0,
            reasons: Vec::new(),
        }
    }
}

impl UnlockProvider for FakeProvider {
    fn provide(&mut self, reason: UnlockReason) -> Result<SecretString, PasswordCancelled> {
        self.reasons.push(reason);
        let password = self.passwords.get(self.next).ok_or(PasswordCancelled)?;
        self.next += 1;
        Ok(SecretString::from((*password).to_string()))
    }
}

struct RecordingProvider {
    candidates: VecDeque<Result<SecretString, PasswordCancelled>>,
    reasons: Rc<RefCell<Vec<UnlockReason>>>,
}

impl UnlockProvider for RecordingProvider {
    fn provide(&mut self, reason: UnlockReason) -> Result<SecretString, PasswordCancelled> {
        self.reasons.borrow_mut().push(reason);
        self.candidates
            .pop_front()
            .unwrap_or(Err(PasswordCancelled))
    }
}

struct SequenceValidator {
    outcomes: VecDeque<Result<ValidationOutcome, PasswordError>>,
}

impl PasswordValidator for SequenceValidator {
    fn validate(
        &mut self,
        _db_path: &Path,
        _password: &str,
    ) -> Result<ValidationOutcome, PasswordError> {
        self.outcomes.pop_front().expect("validator outcome")
    }
}

#[test]
fn sqlcipher_4_utf8_keying_and_wal_ordering() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");
    // Non-ASCII plus SQL metacharacters prove UTF-8 byte keying, not UTF-16.
    let password = "密码🔐pa'ss\";--";

    create_encrypted_db(&db_path, password);

    let conn = open_database(&db_path, Some(password)).unwrap();
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");

    let value: String = conn
        .query_row("SELECT v FROM t WHERE id = 1", [], |row| row.get(0))
        .unwrap();
    assert_eq!(value, "hello");

    // The password must not be stored in the main file in plaintext.
    let bytes = std::fs::read(&db_path).unwrap();
    assert!(
        !bytes
            .windows(password.len())
            .any(|window| window == password.as_bytes())
    );

    // A wrong key is rejected (proof the file is actually encrypted).
    assert!(open_database(&db_path, Some("wrong-password")).is_err());
}

#[test]
fn unencrypted_database_enables_wal_directly() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");

    let conn = open_database(&db_path, None).unwrap();
    conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY)")
        .unwrap();
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
}

#[test]
fn orphan_wal_is_rejected_without_creating_a_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");
    std::fs::write(format!("{}-wal", db_path.display()), b"wal").unwrap();

    let result = open_database(&db_path, None);
    assert!(matches!(
        result,
        Err(deepchat_services::connection::ConnectionError::OrphanWal(_))
    ));
    assert!(!db_path.exists());
}

#[test]
fn fresh_storage_records_latest_marker() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");

    let mut catalog = MigrationCatalog::new(2);
    catalog.add(1, vec!["CREATE TABLE a (id INTEGER PRIMARY KEY)".into()]);
    catalog.add(2, vec!["CREATE TABLE b (id INTEGER PRIMARY KEY)".into()]);

    let storage = Storage::open(&db_path, None, &catalog, &TestClock(1000)).unwrap();

    let versions: Vec<i64> = {
        let mut stmt = storage
            .connection()
            .prepare(&format!(
                "SELECT version FROM {SCHEMA_VERSIONS_TABLE} ORDER BY version"
            ))
            .unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0)).unwrap();
        rows.map(|v| v.unwrap()).collect()
    };
    assert_eq!(versions, vec![2]);
    assert!(!storage.has_verified_password());
    storage.close().unwrap();

    // A fresh database must not replay historical migration SQL.
    let conn = open_database(&db_path, None).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='a'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

struct DestructiveFinalizer;

impl MigrationFinalizer for DestructiveFinalizer {
    fn finalize(&self, _tx: &rusqlite::Transaction<'_>, _version: i64) -> rusqlite::Result<()> {
        Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::DatabaseCorrupt,
                extended_code: rusqlite::ffi::SQLITE_CORRUPT,
            },
            Some("database disk image is malformed".into()),
        ))
    }
}

#[test]
fn verified_capability_promotes_real_storage_path_destructive_failure() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");
    create_encrypted_db(&db_path, "correct-password");

    let provider = FakeProvider::new(vec!["correct-password"]);
    let mut resolver = PasswordResolver::new(db_path.clone(), provider);
    let verified = resolver.resolve().unwrap();

    let mut catalog = MigrationCatalog::new(1);
    catalog.set_finalizer(DestructiveFinalizer);
    assert!(matches!(
        Storage::open(&db_path, Some(verified), &catalog, &TestClock(1000)),
        Err(StartupError::TrueCorruption)
    ));
}

#[test]
fn destructive_storage_failure_without_capability_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");
    let conn = open_database(&db_path, None).unwrap();
    conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY)")
        .unwrap();
    drop(conn);

    let mut catalog = MigrationCatalog::new(1);
    catalog.set_finalizer(DestructiveFinalizer);
    assert!(matches!(
        Storage::open(&db_path, None, &catalog, &TestClock(1000)),
        Err(StartupError::Unreadable)
    ));
}

#[test]
fn encrypted_garbage_without_capability_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let catalog = MigrationCatalog::new(1);
    let garbage = dir.path().join("garbage.db");
    std::fs::write(&garbage, b"encrypted-or-garbage").unwrap();
    assert!(matches!(
        Storage::open(&garbage, None, &catalog, &TestClock(1000)),
        Err(StartupError::Unreadable)
    ));
}

#[test]
fn password_resolver_retries_wrong_password_internally() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");
    create_encrypted_db(&db_path, "correct-password");

    let provider = FakeProvider::new(vec!["wrong-1", "wrong-2", "correct-password"]);
    let mut resolver = PasswordResolver::new(db_path.clone(), provider);

    let verified = resolver.resolve().unwrap();
    assert_eq!(format!("{verified:?}"), "VerifiedPassword([REDACTED])");
}

#[test]
fn validator_open_io_wrong_password_then_valid_retries_with_exact_reasons() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");
    std::fs::write(&db_path, b"unchanged").unwrap();
    let reasons = Rc::new(RefCell::new(Vec::new()));
    let provider = RecordingProvider {
        candidates: ["one", "two", "three", "four"]
            .into_iter()
            .map(|value| Ok(SecretString::from(value.to_string())))
            .collect(),
        reasons: reasons.clone(),
    };
    let validator = SequenceValidator {
        outcomes: VecDeque::from([
            Err(PasswordError::Open),
            Err(PasswordError::Io),
            Ok(ValidationOutcome::WrongPassword),
            Ok(ValidationOutcome::Valid),
        ]),
    };
    let mut resolver = PasswordResolver::with_validator(db_path, provider, validator);

    let verified = resolver.resolve().unwrap();
    assert_eq!(format!("{verified:?}"), "VerifiedPassword([REDACTED])");
    assert_eq!(
        *reasons.borrow(),
        vec![
            UnlockReason::ManualRequired,
            UnlockReason::Invalid,
            UnlockReason::Invalid,
            UnlockReason::Invalid,
        ]
    );
}

#[test]
fn injected_validator_orphan_wal_is_terminal() {
    let reasons = Rc::new(RefCell::new(Vec::new()));
    let db_path = Path::new("synthetic.db").to_path_buf();
    let provider = RecordingProvider {
        candidates: VecDeque::from([Ok(SecretString::from("one".to_string()))]),
        reasons: reasons.clone(),
    };
    let validator = SequenceValidator {
        outcomes: VecDeque::from([Err(PasswordError::OrphanWal(db_path.clone()))]),
    };
    let mut resolver = PasswordResolver::with_validator(db_path.clone(), provider, validator);

    assert!(matches!(resolver.resolve(), Err(PasswordError::OrphanWal(path)) if path == db_path));
    assert_eq!(*reasons.borrow(), vec![UnlockReason::ManualRequired]);
}

#[test]
fn password_resolver_cancellation_makes_no_change() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");
    create_encrypted_db(&db_path, "correct-password");

    let provider = FakeProvider::new(Vec::new());
    let mut resolver = PasswordResolver::new(db_path.clone(), provider);

    let err = resolver.resolve().unwrap_err();
    assert!(matches!(err, PasswordError::Cancelled));
    // Cancellation must not destroy the existing database file.
    assert!(db_path.exists());
}

#[test]
fn public_migration_and_startup_errors_hide_sql_driver_sources_and_passwords() {
    const TOKEN: &str = "UNIQUE_SQL_TOKEN_7f73d8";
    const PASSWORD: &str = "unique-password-9c42";
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    let runner = MigrationRunner::new(&conn, &TestClock(1000));
    runner.initialize_version_table().unwrap();
    let mut catalog = MigrationCatalog::new(1);
    catalog.add(1, vec![format!("INVALID {TOKEN}")]);
    let migration = runner.run(&catalog, false).unwrap_err();
    assert_eq!(migration, MigrationError::Sql);
    let migration_rendered = format!("{migration} {migration:?}");
    assert!(!migration_rendered.contains(TOKEN));
    assert!(!migration_rendered.contains("INVALID"));
    assert!(!migration_rendered.contains(PASSWORD));
    assert!(std::error::Error::source(&migration).is_none());

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");
    let initial = open_database(&db_path, None).unwrap();
    initial
        .execute_batch("CREATE TABLE t (id INTEGER)")
        .unwrap();
    drop(initial);
    let startup = match Storage::open(&db_path, None, &catalog, &TestClock(1000)) {
        Err(error) => error,
        Ok(_) => panic!("expected migration failure"),
    };
    assert!(matches!(
        startup,
        StartupError::Migration(MigrationError::Sql)
    ));
    let startup_rendered = format!("{startup} {startup:?}");
    assert!(!startup_rendered.contains(TOKEN));
    assert!(!startup_rendered.contains("INVALID"));
    assert!(!startup_rendered.contains(PASSWORD));
    assert!(std::error::Error::source(&startup).is_none());

    for error in [MigrationError::Marker, MigrationError::Transaction] {
        let rendered = format!("{error} {error:?}");
        assert!(!rendered.contains(TOKEN));
        assert!(!rendered.contains(PASSWORD));
        assert!(std::error::Error::source(&error).is_none());
    }
}

#[test]
fn errors_never_embed_the_password() {
    let password = "super-secret-password-123";
    let resolver_debug = format!("{:?}", PasswordError::Open);
    assert!(!resolver_debug.contains(password));

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("agent.db");
    create_encrypted_db(&db_path, password);

    // A wrong-key open error must not echo the wrong password either.
    let wrong = "attacker-wrong-password";
    let err = open_database(&db_path, Some(wrong)).unwrap_err();
    let rendered = format!("{err} {err:?}");
    assert!(!rendered.contains(wrong));
    assert!(!rendered.contains(password));
}
