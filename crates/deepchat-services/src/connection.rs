//! SQLCipher-compatible connection opening.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use thiserror::Error;

use crate::startup_recovery::has_orphan_wal_sidecar;

/// SQLCipher compatibility version requested by the frozen reference.
pub const SQLCIPHER_COMPATIBILITY_VERSION: i64 = 4;

/// Safe public connection errors. Raw driver and I/O errors remain inside the
/// storage crate for classification and are never exposed through fields or an
/// `Error::source` chain.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConnectionError {
    #[error("orphan WAL sidecar exists")]
    OrphanWal(PathBuf),
    #[error("database open failed")]
    Sqlite,
    #[error("database file I/O failed")]
    Io,
}

pub(crate) enum ClassifiedConnectionError {
    OrphanWal(PathBuf),
    Sqlite(rusqlite::Error),
    Io,
}

fn ensure_database_directory(db_path: &Path) -> std::io::Result<()> {
    if let Some(parent) = db_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    Ok(())
}

/// Opens a database, applying SQLCipher keying and WAL mode.
///
/// For an encrypted database the ordering is `cipher='sqlcipher'`,
/// `legacy=4`, then the UTF-8 password bytes, and only then WAL mode. An
/// orphan WAL sidecar is rejected before any file is created.
pub fn open_database(
    db_path: &Path,
    password: Option<&str>,
) -> Result<Connection, ConnectionError> {
    open_database_classified(db_path, password).map_err(|error| match error {
        ClassifiedConnectionError::OrphanWal(path) => ConnectionError::OrphanWal(path),
        ClassifiedConnectionError::Sqlite(_) => ConnectionError::Sqlite,
        ClassifiedConnectionError::Io => ConnectionError::Io,
    })
}

pub(crate) fn open_database_classified(
    db_path: &Path,
    password: Option<&str>,
) -> Result<Connection, ClassifiedConnectionError> {
    ensure_database_directory(db_path).map_err(|_| ClassifiedConnectionError::Io)?;

    if has_orphan_wal_sidecar(db_path) {
        return Err(ClassifiedConnectionError::OrphanWal(db_path.to_path_buf()));
    }

    let conn = Connection::open(db_path).map_err(ClassifiedConnectionError::Sqlite)?;
    if let Err(error) = configure_connection(&conn, password) {
        let _ = conn.close();
        return Err(ClassifiedConnectionError::Sqlite(error));
    }

    Ok(conn)
}

fn configure_connection(conn: &Connection, password: Option<&str>) -> rusqlite::Result<()> {
    if let Some(password) = password {
        conn.pragma_update(None, "cipher", "sqlcipher")?;
        conn.pragma_update(None, "legacy", SQLCIPHER_COMPATIBILITY_VERSION)?;
        conn.pragma_update(None, "key", password)?;
    }
    conn.pragma_update(None, "journal_mode", "WAL")
}
