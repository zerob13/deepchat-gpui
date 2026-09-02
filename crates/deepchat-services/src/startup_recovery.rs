//! Startup failure classification, orphan-WAL guard, and quarantine.

use std::io::Read;
use std::path::{Path, PathBuf};

use rusqlite::ffi::ErrorCode;
use thiserror::Error;

use crate::clock::Clock;
use crate::error::StartupFailureKind;

/// Plaintext SQLite file header used to distinguish encrypted from corrupt files.
pub const SQLITE_MAGIC_HEADER: &[u8; 16] = b"SQLite format 3\0";

/// Error raised when a WAL sidecar exists without its main database file.
#[derive(Debug, Clone, Error)]
#[error("refusing to create {db_path}: a leftover WAL sidecar exists")]
pub struct OrphanWalDatabaseError {
    pub db_path: PathBuf,
}

/// Returns the `-wal` sidecar path for a database path.
pub fn sqlite_wal_path(db_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", db_path.display()))
}

/// Returns the `-shm` sidecar path for a database path.
pub fn sqlite_shm_path(db_path: &Path) -> PathBuf {
    PathBuf::from(format!("{}-shm", db_path.display()))
}

/// True when the main file is missing but a `-wal` sidecar exists.
pub fn has_orphan_wal_sidecar(db_path: &Path) -> bool {
    !db_path.exists() && sqlite_wal_path(db_path).exists()
}

/// Rejects a leftover WAL sidecar before a replacement database can be created.
pub fn assert_no_orphan_wal_sidecar(db_path: &Path) -> Result<(), OrphanWalDatabaseError> {
    if has_orphan_wal_sidecar(db_path) {
        return Err(OrphanWalDatabaseError {
            db_path: db_path.to_path_buf(),
        });
    }
    Ok(())
}

fn error_parts(error: &rusqlite::Error) -> (Option<ErrorCode>, String) {
    match error {
        rusqlite::Error::SqliteFailure(ffi_err, message) => {
            (Some(ffi_err.code), message.clone().unwrap_or_default())
        }
        other => (None, other.to_string()),
    }
}

/// True for destructive database errors (corruption or not-a-database).
pub fn is_destructive_database_error(error: &rusqlite::Error) -> bool {
    let (code, message) = error_parts(error);
    matches!(
        code,
        Some(ErrorCode::DatabaseCorrupt) | Some(ErrorCode::NotADatabase)
    ) || message.contains("database disk image is malformed")
        || message.contains("file is not a database")
        || message.contains("SQLITE_CORRUPT")
        || message.contains("SQLITE_NOTADB")
}

/// True when the file decrypted successfully but its pages are corrupt.
///
/// This is distinct from a wrong password: a wrong key surfaces as
/// "file is not a database" (`NotADatabase`), never as corruption.
pub fn is_decrypted_database_corruption_error(error: &rusqlite::Error) -> bool {
    let (code, message) = error_parts(error);
    matches!(code, Some(ErrorCode::DatabaseCorrupt))
        || message.contains("database disk image is malformed")
        || message.contains("SQLITE_CORRUPT")
}

/// True for the classic wrong-key signal (`NotADatabase`).
pub fn is_wrong_password_error(error: &rusqlite::Error) -> bool {
    let (code, message) = error_parts(error);
    matches!(code, Some(ErrorCode::NotADatabase))
        || message.contains("file is not a database")
        || message.contains("SQLITE_NOTADB")
}

/// Classifies a startup failure into a stable kind, or `None` when the error
/// is not destructive and therefore not owned by storage classification.
pub fn classify_database_startup_failure(
    error: &rusqlite::Error,
    db_path: &Path,
) -> Option<StartupFailureKind> {
    if has_orphan_wal_sidecar(db_path) {
        return Some(StartupFailureKind::OrphanWal);
    }

    if !is_destructive_database_error(error) {
        return None;
    }

    match read_database_header(db_path) {
        None => Some(StartupFailureKind::TrueCorruption),
        Some(header) if header.len() < SQLITE_MAGIC_HEADER.len() => {
            Some(StartupFailureKind::TrueCorruption)
        }
        Some(header) if header == *SQLITE_MAGIC_HEADER => Some(StartupFailureKind::TrueCorruption),
        Some(_) => Some(StartupFailureKind::Unreadable),
    }
}

fn read_database_header(db_path: &Path) -> Option<Vec<u8>> {
    let mut file = std::fs::File::open(db_path).ok()?;
    let mut buffer = vec![0u8; SQLITE_MAGIC_HEADER.len()];
    let read = file.read(&mut buffer).ok()?;
    buffer.truncate(read);
    Some(buffer)
}

/// Safe filesystem detail for an interrupted quarantine operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuarantinePartial {
    pub quarantine_directory: PathBuf,
    pub moved_paths: Vec<PathBuf>,
    pub unmoved_existing_source_paths: Vec<PathBuf>,
    pub failed_source: PathBuf,
    pub failed_target: PathBuf,
}

/// Quarantine errors. They intentionally expose no raw I/O source chain.
#[derive(Debug, Error)]
pub enum QuarantineError {
    #[error("failed to create quarantine directory")]
    CreateDirectory,
    #[error("quarantine partially completed")]
    Partial(QuarantinePartial),
    #[error("invalid database path")]
    InvalidPath,
    #[error("quarantine directory namespace exhausted")]
    NamespaceExhausted,
}

/// Minimal production filesystem port for atomic directory allocation and move.
pub trait QuarantineFileSystem {
    fn create_directory(&self, path: &Path) -> std::io::Result<()>;
    fn rename(&self, source: &Path, target: &Path) -> std::io::Result<()>;
    fn exists(&self, path: &Path) -> bool;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct StdQuarantineFileSystem;

impl QuarantineFileSystem for StdQuarantineFileSystem {
    fn create_directory(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir(path)
    }

    fn rename(&self, source: &Path, target: &Path) -> std::io::Result<()> {
        std::fs::rename(source, target)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }
}

pub fn quarantine_database_files(
    db_path: &Path,
    quarantine_root: &Path,
    clock: &dyn Clock,
) -> Result<PathBuf, QuarantineError> {
    quarantine_database_files_with(db_path, quarantine_root, clock, &StdQuarantineFileSystem)
}

/// Allocates a new directory atomically and moves main → wal → shm through an
/// injected production filesystem port.
pub fn quarantine_database_files_with(
    db_path: &Path,
    quarantine_root: &Path,
    clock: &dyn Clock,
    filesystem: &dyn QuarantineFileSystem,
) -> Result<PathBuf, QuarantineError> {
    let base = quarantine_root.join(format!(
        "{}.corrupt.{}",
        db_path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(QuarantineError::InvalidPath)?,
        clock.now_millis()
    ));
    let mut directory = base.clone();
    let mut suffix = 0_u64;
    loop {
        match filesystem.create_directory(&directory) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                suffix = suffix
                    .checked_add(1)
                    .ok_or(QuarantineError::NamespaceExhausted)?;
                directory = PathBuf::from(format!("{}.{}", base.display(), suffix));
            }
            Err(_) => return Err(QuarantineError::CreateDirectory),
        }
    }

    quarantine_database_files_in_directory(db_path, &directory, filesystem)
}

fn quarantine_database_files_in_directory(
    db_path: &Path,
    directory: &Path,
    filesystem: &dyn QuarantineFileSystem,
) -> Result<PathBuf, QuarantineError> {
    let sources = [
        db_path.to_path_buf(),
        sqlite_wal_path(db_path),
        sqlite_shm_path(db_path),
    ];
    let mut moved_paths = Vec::new();

    for source in &sources {
        if !filesystem.exists(source) {
            continue;
        }
        let file_name = source.file_name().ok_or(QuarantineError::InvalidPath)?;
        let target = directory.join(file_name);
        if filesystem.rename(source, &target).is_err() {
            let unmoved_existing_source_paths = sources
                .iter()
                .filter(|candidate| filesystem.exists(candidate))
                .cloned()
                .collect();
            return Err(QuarantineError::Partial(QuarantinePartial {
                quarantine_directory: directory.to_path_buf(),
                moved_paths,
                unmoved_existing_source_paths,
                failed_source: source.clone(),
                failed_target: target,
            }));
        }
        moved_paths.push(target);
    }

    Ok(directory.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_error(code: ErrorCode, message: &str) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code,
                extended_code: 0,
            },
            Some(message.to_string()),
        )
    }

    #[test]
    fn destructive_error_detection_matches_reference_patterns() {
        assert!(is_destructive_database_error(&fake_error(
            ErrorCode::DatabaseCorrupt,
            "database disk image is malformed"
        )));
        assert!(is_destructive_database_error(&fake_error(
            ErrorCode::NotADatabase,
            "file is not a database"
        )));
        assert!(!is_destructive_database_error(&fake_error(
            ErrorCode::CannotOpen,
            "unable to open database file"
        )));
    }

    #[test]
    fn decrypted_corruption_is_distinct_from_wrong_password() {
        assert!(is_decrypted_database_corruption_error(&fake_error(
            ErrorCode::DatabaseCorrupt,
            "SQLITE_CORRUPT: malformed page"
        )));
        assert!(is_decrypted_database_corruption_error(&fake_error(
            ErrorCode::DatabaseCorrupt,
            "database disk image is malformed"
        )));
        assert!(!is_decrypted_database_corruption_error(&fake_error(
            ErrorCode::NotADatabase,
            "file is not a database"
        )));
        assert!(!is_decrypted_database_corruption_error(&fake_error(
            ErrorCode::NotADatabase,
            "SQLITE_NOTADB: invalid header"
        )));
    }

    #[test]
    fn sql_split_handles_quotes_comments_and_semicolons() {
        let sql = "-- leading comment\nCREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); /* inline */ INSERT INTO t(v) VALUES ('a;b'); SELECT 'x''y';\n";
        let statements = crate::schema::split_sql_statements(sql);
        assert_eq!(statements.len(), 3);
        assert_eq!(
            statements[0],
            "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)"
        );
        assert_eq!(statements[1], "INSERT INTO t(v) VALUES ('a;b')");
        assert_eq!(statements[2], "SELECT 'x''y'");
    }

    struct TestClock(i64);

    impl Clock for TestClock {
        fn now_millis(&self) -> i64 {
            self.0
        }
    }

    fn corrupt_error() -> rusqlite::Error {
        fake_error(
            ErrorCode::DatabaseCorrupt,
            "database disk image is malformed",
        )
    }

    fn notadb_error() -> rusqlite::Error {
        fake_error(ErrorCode::NotADatabase, "file is not a database")
    }

    #[test]
    fn classifies_orphan_wal_before_other_rules() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        std::fs::write(sqlite_wal_path(&db_path), b"wal").unwrap();

        let kind = classify_database_startup_failure(&notadb_error(), &db_path);
        assert_eq!(kind, Some(StartupFailureKind::OrphanWal));
        assert!(has_orphan_wal_sidecar(&db_path));
        assert!(!db_path.exists());
    }

    #[test]
    fn public_classifier_cannot_promote_non_magic_file() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        std::fs::write(&db_path, b"not-a-sqlite-header!!").unwrap();

        let kind = classify_database_startup_failure(&corrupt_error(), &db_path);
        assert_eq!(kind, Some(StartupFailureKind::Unreadable));
    }

    #[test]
    fn classifies_plaintext_header_and_truncated_header_as_true_corruption() {
        let dir = tempfile::tempdir().unwrap();

        let plaintext = dir.path().join("plain.db");
        std::fs::write(&plaintext, SQLITE_MAGIC_HEADER).unwrap();
        assert_eq!(
            classify_database_startup_failure(&corrupt_error(), &plaintext),
            Some(StartupFailureKind::TrueCorruption)
        );

        let truncated = dir.path().join("trunc.db");
        std::fs::write(&truncated, b"SQLite").unwrap();
        assert_eq!(
            classify_database_startup_failure(&notadb_error(), &truncated),
            Some(StartupFailureKind::TrueCorruption)
        );
    }

    #[test]
    fn classifies_non_magic_header_without_password_as_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        std::fs::write(&db_path, b"encrypted-or-garbage").unwrap();

        let kind = classify_database_startup_failure(&notadb_error(), &db_path);
        assert_eq!(kind, Some(StartupFailureKind::Unreadable));
    }

    #[test]
    fn ignores_orphan_shm_without_wal_and_non_destructive_errors() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        std::fs::write(sqlite_shm_path(&db_path), b"shm").unwrap();

        assert!(!has_orphan_wal_sidecar(&db_path));
        let cantopen = fake_error(ErrorCode::CannotOpen, "unable to open database file");
        assert_eq!(classify_database_startup_failure(&cantopen, &db_path), None);
    }

    #[test]
    fn orphan_guard_throws_before_replacement_file_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        std::fs::write(sqlite_wal_path(&db_path), b"wal").unwrap();

        assert!(assert_no_orphan_wal_sidecar(&db_path).is_err());
        assert!(!db_path.exists());
    }

    #[test]
    fn quarantine_moves_main_wal_and_shm_into_new_directory() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        std::fs::write(&db_path, b"main").unwrap();
        std::fs::write(sqlite_wal_path(&db_path), b"wal").unwrap();
        std::fs::write(sqlite_shm_path(&db_path), b"shm").unwrap();

        let quarantine_root = dir.path().join("evidence");
        std::fs::create_dir(&quarantine_root).unwrap();
        let result =
            quarantine_database_files(&db_path, &quarantine_root, &TestClock(1700000000000))
                .unwrap();
        let quarantine = result.clone();
        assert_eq!(result, quarantine);

        assert!(!db_path.exists());
        assert!(!sqlite_wal_path(&db_path).exists());
        assert!(!sqlite_shm_path(&db_path).exists());
        assert_eq!(std::fs::read(quarantine.join("agent.db")).unwrap(), b"main");
        assert_eq!(
            std::fs::read(quarantine.join("agent.db-wal")).unwrap(),
            b"wal"
        );
        assert_eq!(
            std::fs::read(quarantine.join("agent.db-shm")).unwrap(),
            b"shm"
        );
    }

    struct WalCollisionFileSystem;

    impl QuarantineFileSystem for WalCollisionFileSystem {
        fn create_directory(&self, path: &Path) -> std::io::Result<()> {
            std::fs::create_dir(path)?;
            std::fs::create_dir(path.join("agent.db-wal"))
        }

        fn rename(&self, source: &Path, target: &Path) -> std::io::Result<()> {
            std::fs::rename(source, target)
        }

        fn exists(&self, path: &Path) -> bool {
            path.exists()
        }
    }

    #[test]
    fn partial_quarantine_reports_real_state_and_preserves_collision_evidence() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("agent.db");
        let wal_path = sqlite_wal_path(&db_path);
        let shm_path = sqlite_shm_path(&db_path);
        std::fs::write(&db_path, b"main").unwrap();
        std::fs::write(&wal_path, b"wal").unwrap();
        std::fs::write(&shm_path, b"shm").unwrap();

        let quarantine_root = dir.path().join("evidence");
        std::fs::create_dir(&quarantine_root).unwrap();
        let collision = quarantine_root.join("agent.db.corrupt.1700000000000");
        std::fs::create_dir(&collision).unwrap();
        std::fs::write(collision.join("preserved.txt"), b"preserved").unwrap();

        let err = quarantine_database_files_with(
            &db_path,
            &quarantine_root,
            &TestClock(1700000000000),
            &WalCollisionFileSystem,
        )
        .unwrap_err();
        let QuarantineError::Partial(partial) = err else {
            panic!("expected partial quarantine");
        };

        assert_eq!(
            partial.quarantine_directory,
            PathBuf::from(format!("{}.1", collision.display()))
        );
        assert_ne!(partial.quarantine_directory, collision);
        assert!(collision.join("preserved.txt").exists());
        assert_eq!(
            partial.moved_paths,
            vec![partial.quarantine_directory.join("agent.db")]
        );
        assert_eq!(
            partial.unmoved_existing_source_paths,
            vec![wal_path.clone(), shm_path.clone()]
        );
        assert_eq!(partial.failed_source, wal_path);
        assert_eq!(
            partial.failed_target,
            partial.quarantine_directory.join("agent.db-wal")
        );
        assert!(!db_path.exists());
        assert!(shm_path.exists());
        assert!(std::error::Error::source(&QuarantineError::Partial(partial)).is_none());
    }
}
