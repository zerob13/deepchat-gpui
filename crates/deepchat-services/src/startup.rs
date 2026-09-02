//! Storage owner: open, classify, migrate, and close a database.

use std::path::{Path, PathBuf};

use rusqlite::Connection;
use thiserror::Error;

use crate::clock::Clock;
use crate::connection::{ClassifiedConnectionError, open_database_classified};
use crate::error::StartupFailureKind;
use crate::password::VerifiedPassword;
use crate::production_schema::{
    ProductionInitializationClass, ProductionInitializationFailure, ProductionSchemaCatalog,
    ProductionSchemaError,
};
use crate::schema::{
    ClassifiedMigrationError, MigrationCatalog, MigrationError, MigrationFailureClass,
    MigrationRunner,
};
use crate::schema_error_classifier::SchemaErrorReason;
use crate::schema_repair::{
    DatabaseRepairError, DatabaseRepairService, RepairFileSystem, SchemaInspector,
    StdRepairFileSystem, startup_catalog,
};
use crate::startup_recovery::classify_database_startup_failure;

/// Startup errors from the storage owner.
#[derive(Error)]
pub enum StartupError {
    #[error("orphan WAL sidecar exists")]
    OrphanWal { db_path: PathBuf },
    #[error("unreadable encrypted database")]
    Unreadable,
    #[error("true database corruption")]
    TrueCorruption,
    #[error("database I/O failure")]
    Io,
    #[error("migration failed")]
    Migration(MigrationError),
    #[error("database open failed")]
    Open,
    #[error("database close failed")]
    Close,
    #[error("production schema initialization failed")]
    ProductionSchema(ProductionSchemaError),
    #[error("schema repair failed")]
    SchemaRepair(DatabaseRepairError),
}

impl std::fmt::Debug for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::OrphanWal { .. } => "StartupError::OrphanWal",
            Self::Unreadable => "StartupError::Unreadable",
            Self::TrueCorruption => "StartupError::TrueCorruption",
            Self::Io => "StartupError::Io",
            Self::Migration(_) => "StartupError::Migration",
            Self::Open => "StartupError::Open",
            Self::Close => "StartupError::Close",
            Self::ProductionSchema(_) => "StartupError::ProductionSchema",
            Self::SchemaRepair(_) => "StartupError::SchemaRepair",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaDiagnosisOutcome {
    Completed,
    Unavailable,
    NotCompleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitializationOutcome {
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitializationFailureCategory {
    Integrity,
    Persistence,
    Schema(SchemaErrorReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatabaseInitializationObservation {
    pub outcome: InitializationOutcome,
    pub duration_ms: u64,
    pub repair_attempted: bool,
    pub schema_diagnosis: SchemaDiagnosisOutcome,
    pub repairable_issue_count: usize,
    pub manual_issue_count: usize,
    pub failure: Option<InitializationFailureCategory>,
}

pub trait InitializationObserver: Send + Sync {
    fn observe(&self, observation: DatabaseInitializationObservation);
}

#[derive(Debug, Default)]
pub struct NoopInitializationObserver;
impl InitializationObserver for NoopInitializationObserver {
    fn observe(&self, _observation: DatabaseInitializationObservation) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupFaultPoint {
    Diagnose { attempt: usize },
    Repair,
    CloseBeforeRepair,
    OpenRepair,
    CloseAfterRepair,
    Reopen,
}

trait StartupFaultInjector: Send + Sync {
    fn fail(&self, point: StartupFaultPoint) -> bool;

    fn initialization_failure(&self, _attempt: usize) -> Option<ProductionInitializationFailure> {
        None
    }
}

#[derive(Debug, Default)]
struct NoStartupFaults;
impl StartupFaultInjector for NoStartupFaults {
    fn fail(&self, _point: StartupFaultPoint) -> bool {
        false
    }
}

/// Owns a single opened, migrated database connection.
pub struct Storage {
    conn: Connection,
    db_path: PathBuf,
    verified: Option<VerifiedPassword>,
}

impl Storage {
    /// Opens and initializes the complete production static schema and owner map.
    pub fn open_production<C: Clock>(
        db_path: &Path,
        password: Option<VerifiedPassword>,
        clock: &C,
    ) -> Result<Self, StartupError> {
        Self::open_production_with(
            db_path,
            password,
            clock,
            &StdRepairFileSystem,
            &NoopInitializationObserver,
        )
    }

    pub fn open_production_with<C: Clock, F: RepairFileSystem, O: InitializationObserver>(
        db_path: &Path,
        password: Option<VerifiedPassword>,
        clock: &C,
        file_system: &F,
        observer: &O,
    ) -> Result<Self, StartupError> {
        Self::open_production_with_faults(
            db_path,
            password,
            clock,
            file_system,
            observer,
            &NoStartupFaults,
        )
    }

    fn open_production_with_faults<
        C: Clock,
        F: RepairFileSystem,
        O: InitializationObserver,
        I: StartupFaultInjector,
    >(
        db_path: &Path,
        password: Option<VerifiedPassword>,
        clock: &C,
        file_system: &F,
        observer: &O,
        faults: &I,
    ) -> Result<Self, StartupError> {
        let started_at = clock.now_millis();
        let mut attempt = 0;
        let mut repair_attempted = false;
        let mut schema_outcome = SchemaDiagnosisOutcome::NotCompleted;
        let mut repairable = 0;
        let mut manual = 0;
        let mut classified_schema_failure = None;
        let result = loop {
            if attempt > 0 && faults.fail(StartupFaultPoint::Reopen) {
                break Err(StartupError::Open);
            }
            let existed_before = db_path.exists();
            let conn = match open_database_classified(
                db_path,
                password.as_ref().map(VerifiedPassword::as_str),
            ) {
                Ok(conn) => conn,
                Err(ClassifiedConnectionError::OrphanWal(path)) => {
                    break Err(StartupError::OrphanWal { db_path: path });
                }
                Err(ClassifiedConnectionError::Io) => break Err(StartupError::Io),
                Err(ClassifiedConnectionError::Sqlite(error)) => {
                    break Err(classify_open_failure(&error, db_path, password.as_ref()));
                }
            };
            let catalog = ProductionSchemaCatalog::frozen();
            let initialize = faults.initialization_failure(attempt).map_or_else(
                || catalog.initialize_before_assert(&conn, !existed_before, clock),
                Err,
            );
            if let Err(failure) = initialize {
                let ProductionInitializationFailure { error, class } = failure;
                let recognized = match class {
                    ProductionInitializationClass::Schema(reason) => Some(reason),
                    ProductionInitializationClass::Destructive => {
                        let _ = conn.close();
                        break Err(if password.is_some() {
                            StartupError::TrueCorruption
                        } else {
                            StartupError::Unreadable
                        });
                    }
                    ProductionInitializationClass::Other => None,
                };
                classified_schema_failure = recognized;
                let diagnosis = if faults.fail(StartupFaultPoint::Diagnose { attempt }) {
                    Err(crate::schema_repair::SchemaDiagnosisError::Read)
                } else {
                    SchemaInspector::new(&conn, startup_catalog(), clock).diagnose()
                };
                if let Ok(diagnosis) = &diagnosis {
                    schema_outcome = SchemaDiagnosisOutcome::Completed;
                    repairable = diagnosis.repairable_issues.len();
                    manual = diagnosis.manual_issues.len();
                } else {
                    schema_outcome = SchemaDiagnosisOutcome::Unavailable;
                }
                let diagnosed_schema_issue = diagnosis
                    .as_ref()
                    .is_ok_and(|diagnosis| !diagnosis.issues.is_empty());
                // A construction-time failure is a stricter gate than the normal
                // post-initialization diagnosis path. Diagnosis describes damage,
                // but only a recognized non-destructive schema error authorizes a
                // repair after construction itself failed.
                let should_repair = recognized.is_some();
                if should_repair && !repair_attempted {
                    repair_attempted = true;
                    if faults.fail(StartupFaultPoint::CloseBeforeRepair) || conn.close().is_err() {
                        break Err(StartupError::Close);
                    }
                    if faults.fail(StartupFaultPoint::OpenRepair) {
                        break Err(StartupError::Open);
                    }
                    let repair_conn = match open_database_classified(
                        db_path,
                        password.as_ref().map(VerifiedPassword::as_str),
                    ) {
                        Ok(conn) => conn,
                        Err(_) => break Err(StartupError::Open),
                    };
                    let repair = if faults.fail(StartupFaultPoint::Repair) {
                        Err(DatabaseRepairError::Transaction)
                    } else {
                        DatabaseRepairService::new(
                            &repair_conn,
                            db_path,
                            startup_catalog(),
                            clock,
                            file_system,
                        )
                        .repair()
                    };
                    let close = if faults.fail(StartupFaultPoint::CloseAfterRepair) {
                        drop(repair_conn);
                        Err(())
                    } else {
                        repair_conn.close().map_err(|_| ())
                    };
                    if let Err(error) = repair {
                        break Err(StartupError::SchemaRepair(error));
                    }
                    if close.is_err() {
                        break Err(StartupError::Close);
                    }
                    attempt += 1;
                    continue;
                }
                if repair_attempted && diagnosed_schema_issue {
                    break Ok(Self {
                        conn,
                        db_path: db_path.to_path_buf(),
                        verified: password,
                    });
                }
                let _ = conn.close();
                break Err(StartupError::ProductionSchema(error));
            }
            let diagnosis = if faults.fail(StartupFaultPoint::Diagnose { attempt }) {
                Err(crate::schema_repair::SchemaDiagnosisError::Read)
            } else {
                SchemaInspector::new(&conn, startup_catalog(), clock).diagnose()
            };
            let mut diagnosed_schema_issue = false;
            match diagnosis {
                Err(_) => schema_outcome = SchemaDiagnosisOutcome::Unavailable,
                Ok(diagnosis) => {
                    schema_outcome = SchemaDiagnosisOutcome::Completed;
                    repairable = diagnosis.repairable_issues.len();
                    manual = diagnosis.manual_issues.len();
                    diagnosed_schema_issue = !diagnosis.issues.is_empty();
                    if repairable > 0 && !repair_attempted {
                        repair_attempted = true;
                        if faults.fail(StartupFaultPoint::CloseBeforeRepair)
                            || conn.close().is_err()
                        {
                            break Err(StartupError::Close);
                        }
                        if faults.fail(StartupFaultPoint::OpenRepair) {
                            break Err(StartupError::Open);
                        }
                        let repair_conn = match open_database_classified(
                            db_path,
                            password.as_ref().map(VerifiedPassword::as_str),
                        ) {
                            Ok(conn) => conn,
                            Err(_) => break Err(StartupError::Open),
                        };
                        let repair = if faults.fail(StartupFaultPoint::Repair) {
                            Err(DatabaseRepairError::Transaction)
                        } else {
                            DatabaseRepairService::new(
                                &repair_conn,
                                db_path,
                                startup_catalog(),
                                clock,
                                file_system,
                            )
                            .repair()
                        };
                        let close = if faults.fail(StartupFaultPoint::CloseAfterRepair) {
                            drop(repair_conn);
                            Err(())
                        } else {
                            repair_conn.close().map_err(|_| ())
                        };
                        if let Err(error) = repair {
                            break Err(StartupError::SchemaRepair(error));
                        }
                        if close.is_err() {
                            break Err(StartupError::Close);
                        }
                        attempt += 1;
                        continue;
                    }
                }
            }
            if !diagnosed_schema_issue && let Err(error) = catalog.assert_current_schema(&conn) {
                let _ = conn.close();
                break Err(StartupError::ProductionSchema(error));
            }
            break Ok(Self {
                conn,
                db_path: db_path.to_path_buf(),
                verified: password,
            });
        };
        let duration_ms = clock.now_millis().saturating_sub(started_at).max(0) as u64;
        let observation = match &result {
            Ok(_) => DatabaseInitializationObservation {
                outcome: InitializationOutcome::Completed,
                duration_ms,
                repair_attempted,
                schema_diagnosis: schema_outcome,
                repairable_issue_count: repairable,
                manual_issue_count: manual,
                failure: None,
            },
            Err(error) => DatabaseInitializationObservation {
                outcome: InitializationOutcome::Failed,
                duration_ms,
                repair_attempted,
                schema_diagnosis: schema_outcome,
                repairable_issue_count: repairable,
                manual_issue_count: manual,
                failure: Some(classify_observation_failure(
                    error,
                    classified_schema_failure,
                )),
            },
        };
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            observer.observe(observation)
        }));
        result
    }

    /// Opens, classifies, and migrates a database.
    ///
    /// `password` is `None` for an unencrypted database or before a password
    /// has been verified. A verified password promotes destructive failures to
    /// [`StartupError::TrueCorruption`].
    pub fn open<C: Clock>(
        db_path: &Path,
        password: Option<VerifiedPassword>,
        catalog: &MigrationCatalog,
        clock: &C,
    ) -> Result<Self, StartupError> {
        let existed_before = db_path.exists();
        let conn = match open_database_classified(
            db_path,
            password.as_ref().map(VerifiedPassword::as_str),
        ) {
            Ok(conn) => conn,
            Err(ClassifiedConnectionError::OrphanWal(path)) => {
                return Err(StartupError::OrphanWal { db_path: path });
            }
            Err(ClassifiedConnectionError::Io) => return Err(StartupError::Io),
            Err(ClassifiedConnectionError::Sqlite(error)) => {
                return Err(classify_open_failure(&error, db_path, password.as_ref()));
            }
        };

        let runner = MigrationRunner::new(&conn, clock);
        if let Err(error) = runner.run_classified(catalog, !existed_before) {
            let _ = conn.close();
            return Err(classify_migration_failure(error, password.as_ref()));
        }

        Ok(Self {
            conn,
            db_path: db_path.to_path_buf(),
            verified: password,
        })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn has_verified_password(&self) -> bool {
        self.verified.is_some()
    }

    /// Closes the connection before releasing injected platform resources.
    pub fn close(self) -> Result<(), StartupError> {
        self.conn.close().map_err(|_| StartupError::Close)
    }
}

fn classify_open_failure(
    error: &rusqlite::Error,
    db_path: &Path,
    verified: Option<&VerifiedPassword>,
) -> StartupError {
    let unverified = classify_database_startup_failure(error, db_path);
    match unverified {
        Some(StartupFailureKind::OrphanWal) => StartupError::OrphanWal {
            db_path: db_path.to_path_buf(),
        },
        Some(StartupFailureKind::Unreadable) if verified.is_some() => StartupError::TrueCorruption,
        Some(StartupFailureKind::Unreadable) => StartupError::Unreadable,
        Some(StartupFailureKind::TrueCorruption) => StartupError::TrueCorruption,
        None => StartupError::Open,
    }
}

fn classify_observation_failure(
    error: &StartupError,
    schema_reason: Option<SchemaErrorReason>,
) -> InitializationFailureCategory {
    match error {
        StartupError::Unreadable | StartupError::TrueCorruption => {
            InitializationFailureCategory::Integrity
        }
        StartupError::ProductionSchema(_) => schema_reason.map_or(
            InitializationFailureCategory::Persistence,
            InitializationFailureCategory::Schema,
        ),
        _ => InitializationFailureCategory::Persistence,
    }
}

fn classify_migration_failure(
    failure: ClassifiedMigrationError,
    verified: Option<&VerifiedPassword>,
) -> StartupError {
    if failure.class == MigrationFailureClass::Destructive {
        if verified.is_some() {
            StartupError::TrueCorruption
        } else {
            StartupError::Unreadable
        }
    } else {
        StartupError::Migration(failure.error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::production_schema::{
        ProductionInitializationClass, ProductionInitializationFailure,
    };
    use crate::schema_repair::StdRepairFileSystem;

    struct TestClock(i64);
    impl Clock for TestClock {
        fn now_millis(&self) -> i64 {
            self.0
        }
    }

    #[derive(Default)]
    struct RecordingObserver(Mutex<Vec<DatabaseInitializationObservation>>);
    impl InitializationObserver for RecordingObserver {
        fn observe(&self, observation: DatabaseInitializationObservation) {
            self.0.lock().unwrap().push(observation);
        }
    }

    #[derive(Default)]
    struct TestStartupFaults {
        points: Vec<StartupFaultPoint>,
        initialization_classes: Vec<ProductionInitializationClass>,
    }
    impl TestStartupFaults {
        fn at(point: StartupFaultPoint) -> Self {
            Self {
                points: vec![point],
                initialization_classes: Vec::new(),
            }
        }

        fn initialize(classes: Vec<ProductionInitializationClass>) -> Self {
            Self {
                points: Vec::new(),
                initialization_classes: classes,
            }
        }
    }
    impl StartupFaultInjector for TestStartupFaults {
        fn fail(&self, point: StartupFaultPoint) -> bool {
            self.points.contains(&point)
        }

        fn initialization_failure(
            &self,
            attempt: usize,
        ) -> Option<ProductionInitializationFailure> {
            self.initialization_classes
                .get(attempt)
                .copied()
                .map(|class| ProductionInitializationFailure {
                    error: ProductionSchemaError::Finalize,
                    class,
                })
        }
    }

    fn remove_audit_memory_ref(path: &Path) {
        let storage = Storage::open_production(path, None, &TestClock(1)).unwrap();
        storage
            .connection()
            .execute_batch(
                "DROP TABLE agent_memory_audit;
                 CREATE TABLE agent_memory_audit(
                   id TEXT PRIMARY KEY,agent_id TEXT NOT NULL,event_type TEXT NOT NULL,
                   actor_type TEXT NOT NULL,session_id TEXT,input_refs_json TEXT NOT NULL DEFAULT '{}',
                   output_refs_json TEXT NOT NULL DEFAULT '{}',model_provider_id TEXT,model_id TEXT,
                   status TEXT NOT NULL,reason TEXT,created_at INTEGER NOT NULL
                 );",
            )
            .unwrap();
        storage.close().unwrap();
    }

    #[test]
    fn diagnosis_unavailable_continues_and_is_observable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.db");
        let initialized = Storage::open_production(&path, None, &TestClock(1)).unwrap();
        initialized.close().unwrap();
        let observer = RecordingObserver::default();
        let storage = Storage::open_production_with_faults(
            &path,
            None,
            &TestClock(2),
            &StdRepairFileSystem,
            &observer,
            &TestStartupFaults::at(StartupFaultPoint::Diagnose { attempt: 0 }),
        )
        .unwrap();
        assert_eq!(
            observer.0.lock().unwrap()[0].schema_diagnosis,
            SchemaDiagnosisOutcome::Unavailable
        );
        storage.close().unwrap();
    }

    #[test]
    fn repair_close_open_and_reopen_boundary_failures_are_persistence_failures() {
        for point in [
            StartupFaultPoint::Repair,
            StartupFaultPoint::CloseBeforeRepair,
            StartupFaultPoint::OpenRepair,
            StartupFaultPoint::CloseAfterRepair,
            StartupFaultPoint::Reopen,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("agent.db");
            remove_audit_memory_ref(&path);
            let observer = RecordingObserver::default();
            assert!(
                Storage::open_production_with_faults(
                    &path,
                    None,
                    &TestClock(2),
                    &StdRepairFileSystem,
                    &observer,
                    &TestStartupFaults::at(point),
                )
                .is_err()
            );
            let observation = observer.0.lock().unwrap()[0];
            assert_eq!(observation.outcome, InitializationOutcome::Failed);
            assert_eq!(
                observation.failure,
                Some(InitializationFailureCategory::Persistence)
            );
        }
    }

    #[test]
    fn recognized_construction_failure_repairs_once_and_preserves_reason() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.db");
        remove_audit_memory_ref(&path);
        let observer = RecordingObserver::default();
        let storage = Storage::open_production_with_faults(
            &path,
            None,
            &TestClock(2),
            &StdRepairFileSystem,
            &observer,
            &TestStartupFaults::initialize(vec![ProductionInitializationClass::Schema(
                SchemaErrorReason::MissingColumn,
            )]),
        )
        .unwrap();
        assert!(observer.0.lock().unwrap()[0].repair_attempted);
        storage.close().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.db");
        remove_audit_memory_ref(&path);
        let observer = RecordingObserver::default();
        let error = Storage::open_production_with_faults(
            &path,
            None,
            &TestClock(2),
            &StdRepairFileSystem,
            &observer,
            &TestStartupFaults::initialize(vec![
                ProductionInitializationClass::Schema(SchemaErrorReason::MissingColumn),
                ProductionInitializationClass::Schema(SchemaErrorReason::ColumnCountMismatch),
            ]),
        )
        .err()
        .expect("second construction failure must fail startup");
        assert!(matches!(error, StartupError::ProductionSchema(_)));
        let observation = observer.0.lock().unwrap()[0];
        assert!(observation.repair_attempted);
        assert_eq!(
            observation.failure,
            Some(InitializationFailureCategory::Schema(
                SchemaErrorReason::ColumnCountMismatch
            ))
        );
    }

    #[test]
    fn destructive_and_unclassified_construction_failures_refuse_repair() {
        for (class, expected) in [
            (
                ProductionInitializationClass::Destructive,
                InitializationFailureCategory::Integrity,
            ),
            (
                ProductionInitializationClass::Other,
                InitializationFailureCategory::Persistence,
            ),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("agent.db");
            remove_audit_memory_ref(&path);
            let observer = RecordingObserver::default();
            assert!(
                Storage::open_production_with_faults(
                    &path,
                    None,
                    &TestClock(2),
                    &StdRepairFileSystem,
                    &observer,
                    &TestStartupFaults::initialize(vec![class]),
                )
                .is_err()
            );
            let observation = observer.0.lock().unwrap()[0];
            assert!(!observation.repair_attempted);
            assert_eq!(observation.failure, Some(expected));
        }
    }
}
