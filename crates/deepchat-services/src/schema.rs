//! Schema-version high-water mark and the generic migration runner.

use std::collections::BTreeMap;

use rusqlite::{Connection, Transaction, params};
use thiserror::Error;

use crate::clock::Clock;
use crate::schema_error_classifier::{SchemaErrorReason, classify_schema_error};

/// Table storing the monotonic schema high-water mark.
pub const SCHEMA_VERSIONS_TABLE: &str = "schema_versions";

/// Safe public migration error. Raw driver errors and SQL are intentionally not
/// retained, exposed through `Debug`, or returned by `Error::source`.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum MigrationError {
    #[error("migration SQL failed")]
    Sql,
    #[error("migration marker failed")]
    Marker,
    #[error("migration transaction failed")]
    Transaction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationFailureClass {
    Destructive,
    Schema(SchemaErrorReason),
    Other,
}

#[derive(Debug)]
pub(crate) struct ClassifiedMigrationError {
    pub error: MigrationError,
    pub class: MigrationFailureClass,
}

fn classified(error: rusqlite::Error, public: MigrationError) -> ClassifiedMigrationError {
    let class = if crate::startup_recovery::is_destructive_database_error(&error) {
        MigrationFailureClass::Destructive
    } else if let Some(classification) = classify_schema_error(&error.to_string()) {
        MigrationFailureClass::Schema(classification.reason)
    } else {
        MigrationFailureClass::Other
    };
    ClassifiedMigrationError {
        error: public,
        class,
    }
}

/// Production extension point for per-version catalog work that cannot be
/// represented as static SQL (for example dynamic auxiliary schema updates).
pub trait MigrationFinalizer {
    fn finalize(&self, tx: &Transaction<'_>, version: i64) -> rusqlite::Result<()>;
}

/// A migration catalog maps schema versions to SQL blocks and optional dynamic
/// finalization. `storage-001` uses generated fixture catalogs only.
#[derive(Default)]
pub struct MigrationCatalog {
    latest_version: i64,
    migrations: BTreeMap<i64, Vec<String>>,
    finalizer: Option<Box<dyn MigrationFinalizer>>,
}

impl MigrationCatalog {
    pub fn new(latest_version: i64) -> Self {
        Self {
            latest_version,
            migrations: BTreeMap::new(),
            finalizer: None,
        }
    }

    /// Registers the SQL blocks for a version. Versions without SQL are
    /// intentionally empty markers recorded by the runner.
    pub fn add(&mut self, version: i64, statements: Vec<String>) -> &mut Self {
        self.migrations.insert(version, statements);
        self
    }

    pub fn latest_version(&self) -> i64 {
        self.latest_version
    }

    pub fn statements(&self, version: i64) -> Option<&[String]> {
        self.migrations.get(&version).map(Vec::as_slice)
    }

    pub fn set_finalizer(&mut self, finalizer: impl MigrationFinalizer + 'static) -> &mut Self {
        self.finalizer = Some(Box::new(finalizer));
        self
    }
}

/// Generic runner over a [`MigrationCatalog`] with a deterministic clock.
pub struct MigrationRunner<'a, C> {
    conn: &'a Connection,
    clock: &'a C,
}

impl<'a, C: Clock> MigrationRunner<'a, C> {
    pub fn new(conn: &'a Connection, clock: &'a C) -> Self {
        Self { conn, clock }
    }

    /// Creates the `schema_versions` table if absent.
    pub fn initialize_version_table(&self) -> Result<(), MigrationError> {
        self.initialize_version_table_classified()
            .map_err(|failure| failure.error)
    }

    fn initialize_version_table_classified(&self) -> Result<(), ClassifiedMigrationError> {
        self.conn
            .execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {SCHEMA_VERSIONS_TABLE} (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                )"
            ))
            .map_err(|error| classified(error, MigrationError::Transaction))
    }

    /// Reads `MAX(version)` from the high-water mark.
    pub fn current_version(&self) -> Result<i64, MigrationError> {
        self.current_version_classified()
            .map_err(|failure| failure.error)
    }

    fn current_version_classified(&self) -> Result<i64, ClassifiedMigrationError> {
        let version: Option<i64> = self
            .conn
            .query_row(
                &format!("SELECT MAX(version) FROM {SCHEMA_VERSIONS_TABLE}"),
                [],
                |row| row.get(0),
            )
            .map_err(|error| classified(error, MigrationError::Transaction))?;
        Ok(version.unwrap_or(0))
    }

    /// Runs migrations from the recorded high-water mark to the catalog latest.
    ///
    /// A fresh database records the latest version once; an existing database
    /// runs every version from `current + 1` through the latest in ascending
    /// order, one transaction per version, including empty markers.
    pub fn run(
        &self,
        catalog: &MigrationCatalog,
        fresh_database: bool,
    ) -> Result<(), MigrationError> {
        self.run_classified(catalog, fresh_database)
            .map_err(|failure| failure.error)
    }

    pub(crate) fn run_classified(
        &self,
        catalog: &MigrationCatalog,
        fresh_database: bool,
    ) -> Result<(), ClassifiedMigrationError> {
        self.initialize_version_table_classified()?;
        let current = self.current_version_classified()?;
        let latest = catalog.latest_version();

        if fresh_database && current == 0 && latest > 0 {
            self.conn
                .execute(
                    &format!(
                        "INSERT INTO {SCHEMA_VERSIONS_TABLE} (version, applied_at) VALUES (?1, ?2)"
                    ),
                    params![latest, self.clock.now_millis()],
                )
                .map_err(|error| classified(error, MigrationError::Marker))?;
            return Ok(());
        }

        for version in (current + 1)..=latest {
            self.run_version(catalog, version)?;
        }
        Ok(())
    }

    fn run_version(
        &self,
        catalog: &MigrationCatalog,
        version: i64,
    ) -> Result<(), ClassifiedMigrationError> {
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|error| classified(error, MigrationError::Transaction))?;

        match self.apply_version(&tx, catalog, version) {
            Ok(()) => tx
                .commit()
                .map_err(|error| classified(error, MigrationError::Transaction)),
            Err(error) => {
                let _ = tx.rollback();
                Err(error)
            }
        }
    }

    fn apply_version(
        &self,
        tx: &Transaction<'_>,
        catalog: &MigrationCatalog,
        version: i64,
    ) -> Result<(), ClassifiedMigrationError> {
        if let Some(statements) = catalog.statements(version) {
            for block in statements {
                for statement in split_sql_statements(block) {
                    if let Err(error) = tx.execute_batch(&statement) {
                        if should_ignore_migration_statement_error(&statement, &error) {
                            continue;
                        }
                        return Err(classified(error, MigrationError::Sql));
                    }
                }
            }
        }

        if let Some(finalizer) = &catalog.finalizer {
            finalizer
                .finalize(tx, version)
                .map_err(|error| classified(error, MigrationError::Sql))?;
        }

        tx.execute(
            &format!("INSERT INTO {SCHEMA_VERSIONS_TABLE} (version, applied_at) VALUES (?1, ?2)"),
            params![version, self.clock.now_millis()],
        )
        .map_err(|error| classified(error, MigrationError::Marker))?;

        Ok(())
    }
}

fn strip_leading_sql_comments(statement: &str) -> String {
    let mut rest = statement;
    loop {
        let trimmed = rest.trim_start();
        match trimmed.strip_prefix("--") {
            Some(after_dashes) => match after_dashes.find(['\n', '\r']) {
                Some(index) => rest = &after_dashes[index..],
                None => return String::new(),
            },
            None => return trimmed.to_string(),
        }
    }
}

/// Splits a SQL block into individual statements, respecting single/double
/// quotes and `--`/`/* */` comments.
pub fn split_sql_statements(sql: &str) -> Vec<String> {
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    let bytes: Vec<char> = sql.chars().collect();
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i];

        if !in_single && !in_double {
            if c == '-' && bytes.get(i + 1) == Some(&'-') {
                while i + 1 < bytes.len() && bytes[i + 1] != '\n' && bytes[i + 1] != '\r' {
                    i += 1;
                }
                i += 1;
                continue;
            }
            if c == '/' && bytes.get(i + 1) == Some(&'*') {
                let ends_with_whitespace = current
                    .chars()
                    .next_back()
                    .is_some_and(|last| last.is_whitespace());
                if !current.is_empty() && !ends_with_whitespace {
                    current.push(' ');
                }
                i += 2;
                while i < bytes.len() && !(bytes[i] == '*' && bytes.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                if i >= bytes.len() {
                    break;
                }
                i += 2;
                continue;
            }
        }

        if c == '\'' && !in_double {
            current.push(c);
            if in_single && bytes.get(i + 1) == Some(&'\'') {
                current.push('\'');
                i += 2;
                continue;
            }
            in_single = !in_single;
            i += 1;
            continue;
        }

        if c == '"' && !in_single {
            in_double = !in_double;
            current.push(c);
            i += 1;
            continue;
        }

        if c == ';' && !in_single && !in_double {
            let trimmed = current.trim();
            if !trimmed.is_empty() {
                statements.push(trimmed.to_string());
            }
            current.clear();
            i += 1;
            continue;
        }

        current.push(c);
        i += 1;
    }

    let trailing = current.trim();
    if !trailing.is_empty() {
        statements.push(trailing.to_string());
    }

    statements
}

/// Narrow, statement-specific idempotency allowlist. Only these cases are
/// tolerated; migration failures are never blanket-ignored.
pub fn should_ignore_migration_statement_error(statement: &str, error: &rusqlite::Error) -> bool {
    let normalized = strip_leading_sql_comments(statement).to_uppercase();
    let message = match error {
        rusqlite::Error::SqliteFailure(_, Some(message)) => message.to_ascii_lowercase(),
        other => other.to_string().to_ascii_lowercase(),
    };

    let is_alter_table_add_column = normalized.starts_with("ALTER TABLE")
        && normalized.contains("ADD COLUMN")
        && message.contains("duplicate column name");

    let is_create_index = (normalized.starts_with("CREATE INDEX")
        || normalized.starts_with("CREATE UNIQUE INDEX"))
        && message.contains("already exists");

    let is_alter_table_drop_column = normalized.starts_with("ALTER TABLE")
        && normalized.contains("DROP COLUMN")
        && message.contains("no such column");

    is_alter_table_add_column || is_create_index || is_alter_table_drop_column
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignore_allowlist_is_narrow() {
        let duplicate = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::Unknown,
                extended_code: 0,
            },
            Some("duplicate column name: x".to_string()),
        );
        assert!(should_ignore_migration_statement_error(
            "ALTER TABLE t ADD COLUMN x INTEGER",
            &duplicate
        ));
        assert!(!should_ignore_migration_statement_error(
            "ALTER TABLE t ADD COLUMN x INTEGER",
            &rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ffi::ErrorCode::Unknown,
                    extended_code: 0,
                },
                Some("something else".to_string()),
            )
        ));

        let exists = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::Unknown,
                extended_code: 0,
            },
            Some("index idx already exists".to_string()),
        );
        assert!(should_ignore_migration_statement_error(
            "CREATE UNIQUE INDEX idx ON t(x)",
            &exists
        ));

        let no_column = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ffi::ErrorCode::Unknown,
                extended_code: 0,
            },
            Some("no such column: y".to_string()),
        );
        assert!(should_ignore_migration_statement_error(
            "ALTER TABLE t DROP COLUMN y",
            &no_column
        ));
    }

    #[test]
    fn split_sql_statements_strips_comments_and_splits() {
        let sql = "-- header\nCREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT); INSERT INTO t(v) VALUES ('a;b');";
        let statements = split_sql_statements(sql);
        assert_eq!(statements.len(), 2);
        assert_eq!(
            statements[0],
            "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)"
        );
        assert_eq!(statements[1], "INSERT INTO t(v) VALUES ('a;b')");
    }

    struct TestClock(i64);

    impl Clock for TestClock {
        fn now_millis(&self) -> i64 {
            self.0
        }
    }

    fn versions(conn: &Connection) -> Vec<i64> {
        let mut stmt = conn
            .prepare("SELECT version FROM schema_versions ORDER BY version")
            .unwrap();
        let rows = stmt.query_map([], |row| row.get::<_, i64>(0)).unwrap();
        rows.map(|v| v.unwrap()).collect()
    }

    #[test]
    fn fresh_database_records_latest_marker_once_without_replaying() {
        let conn = Connection::open_in_memory().unwrap();
        let runner = MigrationRunner::new(&conn, &TestClock(1000));

        let mut catalog = MigrationCatalog::new(3);
        catalog.add(1, vec!["CREATE TABLE a (id INTEGER PRIMARY KEY)".into()]);
        catalog.add(2, vec!["CREATE TABLE b (id INTEGER PRIMARY KEY)".into()]);
        catalog.add(3, Vec::new());

        runner.run(&catalog, true).unwrap();

        assert_eq!(versions(&conn), vec![3]);
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('a','b')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 0);
    }

    #[test]
    fn existing_database_runs_ordered_versions_including_empty_markers() {
        let conn = Connection::open_in_memory().unwrap();
        let runner = MigrationRunner::new(&conn, &TestClock(1000));
        runner.initialize_version_table().unwrap();
        conn.execute(
            "INSERT INTO schema_versions (version, applied_at) VALUES (1, 0)",
            [],
        )
        .unwrap();

        let mut catalog = MigrationCatalog::new(4);
        catalog.add(2, vec!["CREATE TABLE a (id INTEGER PRIMARY KEY)".into()]);
        catalog.add(4, vec!["CREATE TABLE b (id INTEGER PRIMARY KEY)".into()]);

        runner.run(&catalog, false).unwrap();

        assert_eq!(versions(&conn), vec![1, 2, 3, 4]);
        for table in ["a", "b"] {
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
        }
    }

    #[test]
    fn failed_version_rolls_back_sql_and_marker() {
        let conn = Connection::open_in_memory().unwrap();
        let runner = MigrationRunner::new(&conn, &TestClock(1000));
        runner.initialize_version_table().unwrap();
        conn.execute(
            "INSERT INTO schema_versions (version, applied_at) VALUES (1, 0)",
            [],
        )
        .unwrap();

        let mut catalog = MigrationCatalog::new(3);
        catalog.add(
            2,
            vec![
                "CREATE TABLE a (id INTEGER PRIMARY KEY); CREATE TABLE a (id INTEGER PRIMARY KEY)"
                    .into(),
            ],
        );

        let err = runner.run(&catalog, false).unwrap_err();
        assert_eq!(err, MigrationError::Sql);

        assert_eq!(versions(&conn), vec![1]);
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='a'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 0);
    }

    #[test]
    fn tolerated_idempotency_errors_are_skipped() {
        let conn = Connection::open_in_memory().unwrap();
        let runner = MigrationRunner::new(&conn, &TestClock(1000));
        runner.initialize_version_table().unwrap();
        conn.execute(
            "INSERT INTO schema_versions (version, applied_at) VALUES (1, 0)",
            [],
        )
        .unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)")
            .unwrap();

        let mut catalog = MigrationCatalog::new(2);
        catalog.add(
            2,
            vec![
                "ALTER TABLE t ADD COLUMN v TEXT".into(),
                "ALTER TABLE t ADD COLUMN w TEXT".into(),
            ],
        );

        runner.run(&catalog, false).unwrap();

        let columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(t)").unwrap();
            let rows = stmt.query_map([], |row| row.get::<_, String>(1)).unwrap();
            rows.map(|c| c.unwrap()).collect()
        };
        assert!(columns.iter().any(|c| c == "w"));
        assert_eq!(versions(&conn), vec![1, 2]);
    }
}
