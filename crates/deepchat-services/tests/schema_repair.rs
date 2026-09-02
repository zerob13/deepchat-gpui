use deepchat_services::clock::Clock;
use deepchat_services::production_schema::{
    CatalogColumn, CatalogDefinition, CatalogIndex, ProductionSchemaCatalog,
};
use deepchat_services::schema_error_classifier::{SchemaErrorReason, classify_schema_error};
use deepchat_services::schema_repair::{SchemaInspector, SchemaIssueKind};
use rusqlite::Connection;
use std::sync::Mutex;

use deepchat_services::schema_repair::{
    DatabaseRepairError, DatabaseRepairService, DatabaseRepairStatus, RepairFileSystem,
    RepairFileSystemError, StdRepairFileSystem,
};
use deepchat_services::startup::{
    DatabaseInitializationObservation, InitializationObserver, InitializationOutcome,
    SchemaDiagnosisOutcome, Storage,
};

struct TestClock(i64);
impl Clock for TestClock {
    fn now_millis(&self) -> i64 {
        self.0
    }
}

fn definition(
    name: &'static str,
    columns: Vec<CatalogColumn>,
    indexes: Vec<CatalogIndex>,
) -> CatalogDefinition {
    CatalogDefinition {
        name,
        create_sql: "",
        created_on_fresh_install: true,
        columns,
        indexes,
        after_repair: None,
    }
}

#[derive(Default)]
struct RecordingFileSystem {
    copies: Mutex<Vec<(std::path::PathBuf, std::path::PathBuf)>>,
    fail: bool,
}
impl RepairFileSystem for RecordingFileSystem {
    fn copy(
        &self,
        source: &std::path::Path,
        destination: &std::path::Path,
    ) -> Result<(), RepairFileSystemError> {
        self.copies
            .lock()
            .unwrap()
            .push((source.to_owned(), destination.to_owned()));
        if self.fail {
            Err(RepairFileSystemError)
        } else {
            std::fs::copy(source, destination)
                .map(|_| ())
                .map_err(|_| RepairFileSystemError)
        }
    }
}

#[derive(Default)]
struct RecordingObserver(Mutex<Vec<DatabaseInitializationObservation>>);
impl InitializationObserver for RecordingObserver {
    fn observe(&self, observation: DatabaseInitializationObservation) {
        self.0.lock().unwrap().push(observation);
    }
}

struct PanickingObserver;
impl InitializationObserver for PanickingObserver {
    fn observe(&self, _observation: DatabaseInitializationObservation) {
        panic!("observer failure");
    }
}

#[test]
fn production_catalog_selection_preserves_41_38_and_exclusions() {
    let catalog = ProductionSchemaCatalog::frozen();
    assert_eq!(catalog.definitions().len(), 41);
    assert_eq!(catalog.startup_definitions().len(), 38);
    for name in ["conversations", "messages", "message_attachments"] {
        assert!(
            !catalog
                .startup_definitions()
                .iter()
                .any(|table| table.name == name)
        );
    }
    for name in ["app_settings", "providers", "mcp_servers", "agent_settings"] {
        assert!(!catalog.definitions().iter().any(|table| table.name == name));
    }
    const MANUAL: [&str; 41] = [
        "conversations",
        "messages",
        "message_attachments",
        "acp_sessions",
        "acp_turns",
        "new_environments",
        "new_environment_preferences",
        "new_sessions",
        "new_projects",
        "deepchat_sessions",
        "deepchat_messages",
        "deepchat_user_messages",
        "deepchat_user_message_files",
        "deepchat_user_message_links",
        "deepchat_assistant_blocks",
        "deepchat_message_traces",
        "deepchat_message_search_results",
        "deepchat_search_documents",
        "deepchat_pending_inputs",
        "deepchat_usage_stats",
        "deepchat_tape_entries",
        "deepchat_memory_ingestion_projection",
        "deepchat_memory_ingestion_projection_meta",
        "deepchat_tape_search_projection",
        "deepchat_tape_search_projection_meta",
        "deepchat_tape_search_fts_meta",
        "deepchat_session_metadata",
        "legacy_import_status",
        "agents",
        "agent_memory",
        "agent_memory_audit",
        "agent_memory_directive",
        "new_session_active_skills",
        "new_session_disabled_agent_tools",
        "settings_activity",
        "cron_jobs",
        "cron_job_runs",
        "cron_job_deliveries",
        "live_delegations",
        "live_delegation_turns",
        "live_delegation_events",
    ];
    assert_eq!(
        catalog
            .definitions()
            .iter()
            .map(|item| item.name)
            .collect::<Vec<_>>(),
        MANUAL
    );
    assert_eq!(
        catalog
            .startup_definitions()
            .iter()
            .map(|item| item.name)
            .collect::<Vec<_>>(),
        MANUAL[3..]
    );
}

#[test]
fn diagnosis_safely_quotes_names_and_preserves_contract_order_and_filters() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE 'agent''s notes'(present TEXT, mismatch ' '); CREATE TABLE existing(id INTEGER);")
        .unwrap();
    let catalog = vec![
        definition(
            "missing",
            vec![CatalogColumn {
                name: "skipped",
                declared_type: Some("TEXT"),
                add_column_sql: Some("ALTER"),
                check_type: true,
            }],
            vec![CatalogIndex {
                name: "also_skipped",
                create_sql: "CREATE INDEX",
            }],
        ),
        definition(
            "agent's notes",
            vec![
                CatalogColumn {
                    name: "needed",
                    declared_type: Some(" TEXT "),
                    add_column_sql: Some("ALTER"),
                    check_type: true,
                },
                CatalogColumn {
                    name: "manual",
                    declared_type: Some("INTEGER"),
                    add_column_sql: None,
                    check_type: false,
                },
                CatalogColumn {
                    name: "mismatch",
                    declared_type: Some("TEXT"),
                    add_column_sql: None,
                    check_type: true,
                },
            ],
            vec![CatalogIndex {
                name: "missing_index",
                create_sql: "CREATE INDEX",
            }],
        ),
    ];
    let diagnosis = SchemaInspector::new(&conn, &catalog, TestClock(42))
        .diagnose()
        .unwrap();
    assert_eq!(diagnosis.checked_at, 42);
    assert!(!diagnosis.is_healthy);
    assert_eq!(
        diagnosis
            .issues
            .iter()
            .map(|issue| (issue.kind, issue.name.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (SchemaIssueKind::MissingTable, "missing"),
            (SchemaIssueKind::MissingColumn, "needed"),
            (SchemaIssueKind::MissingColumn, "manual"),
            (SchemaIssueKind::ColumnTypeMismatch, "mismatch"),
            (SchemaIssueKind::MissingIndex, "missing_index"),
        ]
    );
    assert_eq!(diagnosis.issues[3].actual_type, None);
    assert_eq!(diagnosis.issues[3].expected_type.as_deref(), Some("TEXT"));
    assert_eq!(
        diagnosis
            .repairable_issues
            .iter()
            .map(|issue| issue.name.as_str())
            .collect::<Vec<_>>(),
        vec!["missing", "needed", "missing_index"]
    );
    assert_eq!(
        diagnosis
            .manual_issues
            .iter()
            .map(|issue| issue.name.as_str())
            .collect::<Vec<_>>(),
        vec!["manual", "mismatch"]
    );
}

#[test]
fn classifier_handles_reference_patterns_without_exposing_raw_message() {
    for (message, reason) in [
        (
            "no such table: \"agent-notes\"",
            SchemaErrorReason::MissingTable,
        ),
        (
            "x has no column named field-name",
            SchemaErrorReason::MissingColumn,
        ),
        (
            "NO SUCH COLUMN: \"field-name\"",
            SchemaErrorReason::MissingColumn,
        ),
        (
            "table \"agent-notes\" has 1 column but 2 values were supplied",
            SchemaErrorReason::ColumnCountMismatch,
        ),
    ] {
        let classified = classify_schema_error(message).unwrap();
        let debug = format!("{classified:?}");
        assert_eq!(classified.reason, reason);
        assert!(!debug.contains(message));
        assert!(!debug.contains("agent-notes"));
        assert!(!debug.contains("field-name"));
        assert!(debug.contains(reason.as_str()));
    }

    let first = format!(
        "{:?}",
        classify_schema_error("no such table: first-table").unwrap()
    );
    let second = format!(
        "{:?}",
        classify_schema_error("no such table: second-table").unwrap()
    );
    assert_eq!(first, second);
    for message in [
        "no table: agents",
        "no such column: !bad",
        "table t has 1 columns but x values were supplied",
    ] {
        assert!(classify_schema_error(message).is_none());
    }
    assert_eq!(SchemaErrorReason::TypeMismatch.as_str(), "type-mismatch");
}

#[test]
fn repair_has_exact_backup_name_repairs_in_one_transaction_and_retains_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE sample(id INTEGER PRIMARY KEY);")
        .unwrap();
    let catalog = vec![definition(
        "sample",
        vec![
            CatalogColumn {
                name: "id",
                declared_type: Some("INTEGER"),
                add_column_sql: None,
                check_type: true,
            },
            CatalogColumn {
                name: "value",
                declared_type: Some("TEXT"),
                add_column_sql: Some("ALTER TABLE sample ADD COLUMN value TEXT;"),
                check_type: true,
            },
        ],
        vec![CatalogIndex {
            name: "idx_sample_value",
            create_sql: "CREATE INDEX idx_sample_value ON sample(value);",
        }],
    )];
    let expected_backup = dir
        .path()
        .join("agent.db.1970-01-01T00-00-00-000Z.repair.bak");
    std::fs::write(&expected_backup, b"preexisting destination").unwrap();
    let fs = RecordingFileSystem::default();
    let report = DatabaseRepairService::new(&conn, &path, &catalog, &TestClock(0), &fs)
        .repair()
        .unwrap();
    assert_eq!(report.status, DatabaseRepairStatus::Repaired);
    assert_eq!(report.repaired_issues.len(), 2);
    assert_eq!(
        report.backup_path.as_deref(),
        Some(
            dir.path()
                .join("agent.db.1970-01-01T00-00-00-000Z.repair.bak")
                .as_path()
        )
    );
    let backup_path = report.backup_path.unwrap();
    assert!(backup_path.exists());
    assert_eq!(fs.copies.lock().unwrap().len(), 1);
    assert!(!backup_path.with_extension("db-wal").exists());
    assert!(!backup_path.with_extension("db-shm").exists());
    assert!(
        SchemaInspector::new(&conn, &catalog, TestClock(1))
            .diagnose()
            .unwrap()
            .is_healthy
    );
}

#[test]
fn healthy_and_manual_only_paths_never_create_backups() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE sample(id INTEGER PRIMARY KEY);")
        .unwrap();
    let fs = RecordingFileSystem::default();
    let healthy = vec![definition(
        "sample",
        vec![CatalogColumn {
            name: "id",
            declared_type: Some("INTEGER"),
            add_column_sql: None,
            check_type: true,
        }],
        vec![],
    )];
    assert_eq!(
        DatabaseRepairService::new(&conn, &path, &healthy, &TestClock(0), &fs)
            .repair()
            .unwrap()
            .status,
        DatabaseRepairStatus::Healthy
    );
    let manual = vec![definition(
        "sample",
        vec![CatalogColumn {
            name: "required",
            declared_type: Some("TEXT"),
            add_column_sql: None,
            check_type: true,
        }],
        vec![],
    )];
    assert_eq!(
        DatabaseRepairService::new(&conn, &path, &manual, &TestClock(0), &fs)
            .repair()
            .unwrap()
            .status,
        DatabaseRepairStatus::ManualActionRequired
    );
    assert!(fs.copies.lock().unwrap().is_empty());
}

#[test]
fn copy_failure_precedes_mutation_and_public_error_is_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secret-agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE sample(id INTEGER PRIMARY KEY);")
        .unwrap();
    let catalog = vec![definition(
        "sample",
        vec![CatalogColumn {
            name: "secret_column",
            declared_type: Some("TEXT"),
            add_column_sql: Some("ALTER TABLE sample ADD COLUMN secret_column TEXT;"),
            check_type: true,
        }],
        vec![],
    )];
    let fs = RecordingFileSystem {
        copies: Mutex::default(),
        fail: true,
    };
    let error = DatabaseRepairService::new(&conn, &path, &catalog, &TestClock(0), &fs)
        .repair()
        .unwrap_err();
    assert_eq!(error, DatabaseRepairError::BackupCopy);
    let debug = format!("{error:?}");
    assert!(!debug.contains("secret"));
    assert!(
        SchemaInspector::new(&conn, &catalog, TestClock(1))
            .diagnose()
            .unwrap()
            .repairable_issues
            .len()
            == 1
    );
}

#[test]
fn hook_failure_rolls_back_schema_but_keeps_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE owner(id INTEGER PRIMARY KEY);")
        .unwrap();
    let catalog = vec![CatalogDefinition {
        name: "owner",
        create_sql: "",
        created_on_fresh_install: true,
        columns: vec![CatalogColumn {
            name: "added",
            declared_type: Some("TEXT"),
            add_column_sql: Some("ALTER TABLE owner ADD COLUMN added TEXT;"),
            check_type: true,
        }],
        indexes: vec![],
        after_repair: Some("unknown.hook"),
    }];
    let fs = RecordingFileSystem::default();
    let error = DatabaseRepairService::new(&conn, &path, &catalog, &TestClock(0), &fs)
        .repair()
        .unwrap_err();
    assert_eq!(error, DatabaseRepairError::Hook);
    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(owner)")
        .unwrap()
        .query_map([], |r| r.get(1))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(columns, vec!["id"]);
    assert!(
        dir.path()
            .join("agent.db.1970-01-01T00-00-00-000Z.repair.bak")
            .exists()
    );
}

#[test]
fn production_startup_repairs_once_reopens_and_reports_observation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let initialized = Storage::open_production(&path, None, &TestClock(1)).unwrap();
    initialized
        .connection()
        .execute_batch(
            "DROP TABLE agent_memory_audit;
             CREATE TABLE agent_memory_audit(
               id TEXT PRIMARY KEY,
               agent_id TEXT NOT NULL,
               event_type TEXT NOT NULL,
               actor_type TEXT NOT NULL,
               session_id TEXT,
               input_refs_json TEXT NOT NULL DEFAULT '{}',
               output_refs_json TEXT NOT NULL DEFAULT '{}',
               model_provider_id TEXT,
               model_id TEXT,
               status TEXT NOT NULL,
               reason TEXT,
               created_at INTEGER NOT NULL
             );
             CREATE INDEX idx_agent_memory_audit_agent_created
               ON agent_memory_audit(agent_id, created_at);
             CREATE INDEX idx_agent_memory_audit_agent_event
               ON agent_memory_audit(agent_id, event_type, created_at);
             CREATE INDEX idx_agent_memory_audit_operational_retention_v2
               ON agent_memory_audit(agent_id, created_at DESC, id DESC)
               WHERE event_type IN ('memory/maintenance_llm', 'memory/reflect', 'memory/repair', 'memory/conflict_repair', 'memory/extract');",
        )
        .unwrap();
    initialized.close().unwrap();

    let observer = RecordingObserver::default();
    let storage =
        Storage::open_production_with(&path, None, &TestClock(2), &StdRepairFileSystem, &observer)
            .unwrap();
    let repaired: i64 = storage
        .connection()
        .query_row(
            "SELECT count(*) FROM pragma_table_info('agent_memory_audit') WHERE name='memory_ref_id'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(repaired, 1);
    let observations = observer.0.lock().unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].outcome, InitializationOutcome::Completed);
    assert_eq!(
        observations[0].schema_diagnosis,
        SchemaDiagnosisOutcome::Completed
    );
    assert!(observations[0].repair_attempted);
    assert_eq!(observations[0].repairable_issue_count, 0);
    assert_eq!(observations[0].manual_issue_count, 0);
    assert_eq!(observations[0].failure, None);
    drop(observations);
    assert!(
        dir.path()
            .join("agent.db.1970-01-01T00-00-00-002Z.repair.bak")
            .exists()
    );
    storage.close().unwrap();
}

#[test]
fn production_startup_continues_with_manual_only_issues_and_observes_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let initialized = Storage::open_production(&path, None, &TestClock(1)).unwrap();
    initialized
        .connection()
        .execute_batch(
            "DROP TABLE deepchat_pending_inputs;
             CREATE TABLE deepchat_pending_inputs(
               id TEXT PRIMARY KEY,
               session_id TEXT NOT NULL,
               mode TEXT NOT NULL,
               state TEXT NOT NULL DEFAULT 'pending',
               payload_json TEXT NOT NULL,
               message_ids_json TEXT NOT NULL DEFAULT '[]',
               assistant_message_id TEXT,
               blocking_json TEXT,
               retry_required_at BLOB,
               queue_order INTEGER,
               claimed_at INTEGER,
               consumed_at INTEGER,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL
             );
             CREATE INDEX idx_deepchat_pending_inputs_session ON deepchat_pending_inputs(session_id,state,mode,queue_order,created_at);",
        )
        .unwrap();
    initialized.close().unwrap();

    let observer = RecordingObserver::default();
    let storage =
        Storage::open_production_with(&path, None, &TestClock(2), &StdRepairFileSystem, &observer)
            .unwrap();
    let observations = observer.0.lock().unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].outcome, InitializationOutcome::Completed);
    assert!(!observations[0].repair_attempted);
    assert_eq!(observations[0].repairable_issue_count, 0);
    assert_eq!(observations[0].manual_issue_count, 1);
    assert_eq!(observations[0].failure, None);
    drop(observations);
    storage.close().unwrap();
}

#[test]
fn checkpoint_failure_precedes_backup_and_schema_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; CREATE TABLE sample(id INTEGER PRIMARY KEY); BEGIN IMMEDIATE;",
    )
    .unwrap();
    let catalog = vec![definition(
        "sample",
        vec![
            CatalogColumn {
                name: "id",
                declared_type: Some("INTEGER"),
                add_column_sql: None,
                check_type: true,
            },
            CatalogColumn {
                name: "added",
                declared_type: Some("TEXT"),
                add_column_sql: Some("ALTER TABLE sample ADD COLUMN added TEXT;"),
                check_type: true,
            },
        ],
        vec![],
    )];
    let fs = RecordingFileSystem::default();
    let error = DatabaseRepairService::new(&conn, &path, &catalog, &TestClock(0), &fs)
        .repair()
        .unwrap_err();
    assert_eq!(error, DatabaseRepairError::Checkpoint);
    assert!(fs.copies.lock().unwrap().is_empty());
    conn.execute_batch("ROLLBACK;").unwrap();
    assert_eq!(
        SchemaInspector::new(&conn, &catalog, TestClock(1))
            .diagnose()
            .unwrap()
            .repairable_issues
            .len(),
        1
    );
}

#[test]
fn audit_hook_backfills_only_completed_events_with_a_usable_reference() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE agent_memory_audit(
           id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, event_type TEXT NOT NULL,
           actor_type TEXT NOT NULL, session_id TEXT, input_refs_json TEXT NOT NULL DEFAULT '{}',
           output_refs_json TEXT NOT NULL DEFAULT '{}', model_provider_id TEXT, model_id TEXT,
           status TEXT NOT NULL, reason TEXT, created_at INTEGER NOT NULL
         );
         INSERT INTO agent_memory_audit VALUES
           ('usable','a','memory/restore','runtime',NULL,'{}','{\"memoryId\":\" memory-1 \"}',NULL,NULL,'completed',NULL,1),
           ('empty','a','memory/restore','runtime',NULL,'{}','{}',NULL,NULL,'completed',NULL,2);",
    )
    .unwrap();
    let definition = ProductionSchemaCatalog::frozen()
        .definitions()
        .iter()
        .find(|definition| definition.name == "agent_memory_audit")
        .unwrap()
        .clone();
    let fs = RecordingFileSystem::default();
    DatabaseRepairService::new(&conn, &path, &[definition], &TestClock(0), &fs)
        .repair()
        .unwrap();
    let usable: Option<String> = conn
        .query_row(
            "SELECT memory_ref_id FROM agent_memory_audit WHERE id='usable'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let empty: Option<String> = conn
        .query_row(
            "SELECT memory_ref_id FROM agent_memory_audit WHERE id='empty'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(usable.as_deref(), Some("memory-1"));
    assert_eq!(empty, None);
}

#[test]
fn pending_inputs_hook_normalizes_only_retry_required_rows() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE deepchat_pending_inputs(
           id TEXT PRIMARY KEY, session_id TEXT NOT NULL, mode TEXT NOT NULL,
           state TEXT NOT NULL, payload_json TEXT NOT NULL, message_ids_json TEXT,
           assistant_message_id TEXT, blocking_json TEXT, queue_order INTEGER,
           claimed_at INTEGER, consumed_at INTEGER, created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL
         );
         INSERT INTO deepchat_pending_inputs VALUES
           ('retry','s','m','retry_required','{}','[]',NULL,'blocked-by',NULL,NULL,NULL,10,20),
           ('pending','s','m','pending','{}','[]',NULL,'untouched',NULL,NULL,NULL,11,21);
         CREATE INDEX idx_deepchat_pending_inputs_session ON deepchat_pending_inputs(session_id,state,mode,queue_order,created_at);",
    )
    .unwrap();
    let definition = ProductionSchemaCatalog::frozen()
        .definitions()
        .iter()
        .find(|definition| definition.name == "deepchat_pending_inputs")
        .unwrap()
        .clone();
    let fs = RecordingFileSystem::default();
    DatabaseRepairService::new(&conn, &path, &[definition], &TestClock(0), &fs)
        .repair()
        .unwrap();
    let retry: (String, Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT state,retry_required_at,blocking_json FROM deepchat_pending_inputs WHERE id='retry'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(retry, ("blocked".to_owned(), Some(20), None));
    let pending: (String, Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT state,retry_required_at,blocking_json FROM deepchat_pending_inputs WHERE id='pending'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        pending,
        ("pending".to_owned(), None, Some("untouched".to_owned()))
    );
}

#[test]
fn environment_missing_table_hook_rebuilds_from_real_session_sources() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE new_sessions(id TEXT PRIMARY KEY,project_dir TEXT,updated_at INTEGER,is_draft INTEGER);
         CREATE TABLE acp_sessions(conversation_id TEXT PRIMARY KEY,workdir TEXT,updated_at INTEGER);
         INSERT INTO new_sessions VALUES ('direct','/direct',10,0),('acp',NULL,20,0),('draft','/draft',30,1);
         INSERT INTO acp_sessions VALUES ('acp','/acp',25);",
    )
    .unwrap();
    let definition = ProductionSchemaCatalog::frozen()
        .definitions()
        .iter()
        .find(|definition| definition.name == "new_environments")
        .unwrap()
        .clone();
    let fs = RecordingFileSystem::default();
    DatabaseRepairService::new(&conn, &path, &[definition], &TestClock(0), &fs)
        .repair()
        .unwrap();
    let rows = conn
        .prepare("SELECT path,session_count,last_used_at FROM new_environments ORDER BY path")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        rows,
        vec![("/acp".to_owned(), 1, 25), ("/direct".to_owned(), 1, 10)]
    );
}

#[test]
fn agent_memory_lineage_hook_installs_checked_artifacts_and_dirty_triggers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE agent_memory(
           id TEXT PRIMARY KEY,agent_id TEXT NOT NULL,kind TEXT NOT NULL,
           embedding_state TEXT NOT NULL DEFAULT 'pending',embedding_dim INTEGER,
           embedding_model TEXT,last_accessed INTEGER,created_at INTEGER NOT NULL
         );
         INSERT INTO agent_memory VALUES ('m','a','semantic','pending',NULL,NULL,NULL,7);",
    )
    .unwrap();
    let definition = CatalogDefinition {
        name: "agent_memory",
        create_sql: "",
        created_on_fresh_install: true,
        columns: vec![CatalogColumn {
            name: "decision_revision",
            declared_type: Some("INTEGER"),
            add_column_sql: Some(
                "ALTER TABLE agent_memory ADD COLUMN decision_revision INTEGER NOT NULL DEFAULT 1;",
            ),
            check_type: false,
        }],
        indexes: vec![],
        after_repair: Some("agent_memory.repairCanonicalStateAfterSchemaRepair"),
    };
    let fs = RecordingFileSystem::default();
    DatabaseRepairService::new(&conn, &path, &[definition], &TestClock(0), &fs)
        .repair()
        .unwrap();
    for name in [
        "agent_memory_dirty_ai",
        "agent_memory_dirty_au",
        "agent_memory_dirty_ad",
    ] {
        let present: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND name=?1)",
                [name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(present, 1, "{name}");
    }
    let derivation_sql: String = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='agent_memory_derivation'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(derivation_sql.contains("'manual_edit'"));
    let dirty: i64 = conn
        .query_row(
            "SELECT count(*) FROM agent_memory_dirty WHERE memory_id='m'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(dirty, 1);
    for (kind, name) in [
        ("table", "agent_memory_clear_job"),
        ("trigger", "agent_memory_clear_guard_bi_v1"),
        ("trigger", "agent_memory_clear_guard_bu_v1"),
    ] {
        let present: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type=?1 AND name=?2)",
                [kind, name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(present, 1, "missing {kind} {name}");
    }
}

#[test]
fn temporal_hook_archives_invalid_claims_normalizes_internal_rows_and_reinstalls_guards() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE agent_memory(
           id TEXT PRIMARY KEY,kind TEXT NOT NULL,status TEXT NOT NULL,lifecycle_state TEXT NOT NULL,
           valid_from INTEGER,valid_until INTEGER,temporal_confidence REAL,
           temporal_precision TEXT,temporal_timezone TEXT
         );
         INSERT INTO agent_memory VALUES
           ('claim','semantic','embedded','active',10,5,NULL,NULL,NULL),
           ('internal','working','fts_only','active',10,5,NULL,NULL,NULL);",
    )
    .unwrap();
    let definition = CatalogDefinition {
        name: "agent_memory",
        create_sql: "",
        created_on_fresh_install: true,
        columns: vec![CatalogColumn {
            name: "temporal_kind",
            declared_type: Some("TEXT"),
            add_column_sql: Some(
                "ALTER TABLE agent_memory ADD COLUMN temporal_kind TEXT NOT NULL DEFAULT 'atemporal';",
            ),
            check_type: false,
        }],
        indexes: vec![],
        after_repair: Some("agent_memory.repairCanonicalStateAfterSchemaRepair"),
    };
    let fs = RecordingFileSystem::default();
    DatabaseRepairService::new(&conn, &path, &[definition], &TestClock(0), &fs)
        .repair()
        .unwrap();
    let claim: (String, String, String, Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT temporal_kind,status,lifecycle_state,valid_from,valid_until FROM agent_memory WHERE id='claim'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        claim,
        (
            "atemporal".to_owned(),
            "archived".to_owned(),
            "archived".to_owned(),
            None,
            None
        )
    );
    let internal: (String, String, String, Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT temporal_kind,status,lifecycle_state,valid_from,valid_until FROM agent_memory WHERE id='internal'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        internal,
        (
            "atemporal".to_owned(),
            "fts_only".to_owned(),
            "active".to_owned(),
            None,
            None
        )
    );
    assert!(
        conn.execute(
            "UPDATE agent_memory SET temporal_kind='event' WHERE id='internal'",
            []
        )
        .is_err()
    );
}

#[test]
fn agent_memory_scope_hook_installs_index_and_validation_guards() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE agent_memory(
           id TEXT PRIMARY KEY,agent_id TEXT NOT NULL,user_scope TEXT,kind TEXT NOT NULL,
           content TEXT NOT NULL,importance REAL NOT NULL,status TEXT NOT NULL,
           lifecycle_state TEXT NOT NULL DEFAULT 'active',superseded_by TEXT,created_at INTEGER NOT NULL
         );",
    )
    .unwrap();
    let definition = CatalogDefinition {
        name: "agent_memory",
        create_sql: "",
        created_on_fresh_install: true,
        columns: vec![
            CatalogColumn {
                name: "scope_type",
                declared_type: Some("TEXT"),
                add_column_sql: Some(
                    "ALTER TABLE agent_memory ADD COLUMN scope_type TEXT NOT NULL DEFAULT 'agent';",
                ),
                check_type: false,
            },
            CatalogColumn {
                name: "scope_id",
                declared_type: Some("TEXT"),
                add_column_sql: Some("ALTER TABLE agent_memory ADD COLUMN scope_id TEXT;"),
                check_type: false,
            },
        ],
        indexes: vec![],
        after_repair: Some("agent_memory.repairCanonicalStateAfterSchemaRepair"),
    };
    DatabaseRepairService::new(
        &conn,
        &path,
        &[definition],
        &TestClock(0),
        &StdRepairFileSystem,
    )
    .repair()
    .unwrap();
    for (kind, name) in [
        ("index", "idx_agent_memory_recall_scope_v6"),
        ("trigger", "agent_memory_scope_bi_v1"),
        ("trigger", "agent_memory_scope_bu_v1"),
    ] {
        let present: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type=?1 AND name=?2)",
                [kind, name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(present, 1, "missing {kind} {name}");
    }
    assert!(
        conn.execute(
            "INSERT INTO agent_memory(id,agent_id,user_scope,kind,content,importance,status,created_at,scope_type,scope_id)
             VALUES('bad','a',NULL,'semantic','x',1,'pending_embedding',1,'project',NULL)",
            [],
        )
        .is_err()
    );
}

#[test]
fn production_startup_repairs_once_then_continues_with_residual_manual_issue() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let initialized = Storage::open_production(&path, None, &TestClock(1)).unwrap();
    initialized
        .connection()
        .execute_batch(
            "DROP INDEX idx_deepchat_pending_inputs_session;
             DROP TABLE deepchat_pending_inputs;
             CREATE TABLE deepchat_pending_inputs(
               id TEXT PRIMARY KEY,session_id TEXT NOT NULL,mode TEXT NOT NULL,
               state TEXT NOT NULL DEFAULT 'pending',payload_json TEXT NOT NULL,
               message_ids_json TEXT NOT NULL DEFAULT '[]',assistant_message_id TEXT,
               retry_required_at BLOB,queue_order INTEGER,claimed_at INTEGER,
               consumed_at INTEGER,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL
             );",
        )
        .unwrap();
    initialized.close().unwrap();
    let observer = RecordingObserver::default();
    let storage =
        Storage::open_production_with(&path, None, &TestClock(2), &StdRepairFileSystem, &observer)
            .unwrap();
    let observations = observer.0.lock().unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].outcome, InitializationOutcome::Completed);
    assert!(observations[0].repair_attempted);
    assert_eq!(observations[0].repairable_issue_count, 0);
    assert_eq!(observations[0].manual_issue_count, 1);
    drop(observations);
    storage.close().unwrap();
}

#[test]
fn agent_memory_state_hook_repairs_indexes_bridge_and_fts_invalidation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let initialized = Storage::open_production(&path, None, &TestClock(1)).unwrap();
    initialized
        .connection()
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS agent_memory_fts_meta(
               key TEXT PRIMARY KEY,schema_version INTEGER NOT NULL,policy_version INTEGER NOT NULL,
               tokenizer TEXT NOT NULL,mutation_generation INTEGER NOT NULL,
               indexed_generation INTEGER NOT NULL,updated_at INTEGER NOT NULL
             ) WITHOUT ROWID;
             INSERT INTO agent_memory_fts_meta(key,schema_version,policy_version,tokenizer,mutation_generation,indexed_generation,updated_at)
             VALUES('agent_memory_fts',1,1,'unicode61',0,0,1)
             ON CONFLICT(key) DO UPDATE SET schema_version=1;
             DROP TRIGGER agent_memory_legacy_status_bridge_ai;
             DROP TRIGGER agent_memory_legacy_status_bridge_au;
             INSERT INTO agent_memory(
               id,agent_id,user_scope,scope_type,scope_id,kind,category,content,importance,status,
               embedding_id,embedding_dim,embedding_model,source_session,provenance_key,is_anchor,
               superseded_by,created_at,last_accessed,access_count,decay_score,source_entry_ids,
               confidence,temporal_kind,valid_from,valid_until,temporal_confidence,
               temporal_precision,temporal_timezone,last_consolidated_at,conflict_state,
               conflict_with,persona_state,decision_revision,lifecycle_state,embedding_state
             ) VALUES
               ('archived','a',NULL,'agent',NULL,'semantic',NULL,'archived',1,'archived',
                NULL,NULL,NULL,NULL,NULL,0,NULL,1,NULL,0,1,NULL,NULL,'atemporal',NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,1,'active','pending'),
               ('ready','a',NULL,'agent',NULL,'semantic',NULL,'ready',1,'embedded',
                'embedding-ref',3,'model',NULL,NULL,0,NULL,2,NULL,0,1,NULL,NULL,'atemporal',NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,NULL,1,'active','pending');
             DROP INDEX idx_agent_memory_active_recall;
             DROP INDEX idx_agent_memory_recall_scope_v6;
             DROP INDEX idx_agent_memory_management_page_v3;
             DROP INDEX idx_agent_memory_archive_eligible_v3;
             DROP INDEX idx_agent_memory_cognitive_top_v3;
             DROP INDEX idx_agent_memory_conflict_fairness_v3;
             DROP INDEX idx_agent_memory_recent_activity_v3;
             DROP INDEX idx_agent_memory_embedding_pending_agent_v2;
             DROP INDEX idx_agent_memory_embedding_pending_global_v2;
             DROP INDEX idx_agent_memory_conflict_target_v2;
             DROP INDEX idx_agent_memory_conflict_state_anomaly_v2;
             ALTER TABLE agent_memory DROP COLUMN lifecycle_state;
             ALTER TABLE agent_memory DROP COLUMN embedding_state;",
        )
        .unwrap();
    initialized.close().unwrap();

    let conn = Connection::open(&path).unwrap();
    let definition = ProductionSchemaCatalog::frozen()
        .definitions()
        .iter()
        .find(|definition| definition.name == "agent_memory")
        .unwrap()
        .clone();
    DatabaseRepairService::new(
        &conn,
        &path,
        &[definition],
        &TestClock(2),
        &StdRepairFileSystem,
    )
    .repair()
    .unwrap();
    let fts_meta: i64 = conn
        .query_row(
            "SELECT count(*) FROM agent_memory_fts_meta WHERE key='agent_memory_fts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fts_meta, 0);
    let states = conn
        .prepare("SELECT id,status,lifecycle_state,embedding_state FROM agent_memory WHERE id IN ('archived','ready') ORDER BY id")
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        states,
        vec![
            (
                "archived".into(),
                "archived".into(),
                "archived".into(),
                "pending".into()
            ),
            (
                "ready".into(),
                "embedded".into(),
                "active".into(),
                "ready".into()
            ),
        ]
    );
    for (kind, name) in [
        ("trigger", "agent_memory_legacy_status_bridge_ai"),
        ("trigger", "agent_memory_legacy_status_bridge_au"),
        ("index", "idx_agent_memory_active_recall"),
        ("index", "idx_agent_memory_embedding_pending_agent_v2"),
        ("index", "idx_agent_memory_embedding_pending_global_v2"),
        ("index", "idx_agent_memory_conflict_state_anomaly_v2"),
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type=?1 AND name=?2)",
                [kind, name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing {kind} {name}");
    }
}

#[test]
fn production_startup_observes_real_38_entry_catalog_and_isolates_observer_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let observer = RecordingObserver::default();
    let storage =
        Storage::open_production_with(&path, None, &TestClock(7), &StdRepairFileSystem, &observer)
            .unwrap();
    let observations = observer.0.lock().unwrap();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0].outcome, InitializationOutcome::Completed);
    assert_eq!(
        observations[0].schema_diagnosis,
        SchemaDiagnosisOutcome::Completed
    );
    assert!(!observations[0].repair_attempted);
    assert_eq!(observations[0].repairable_issue_count, 0);
    drop(observations);
    storage.close().unwrap();

    let reopened = Storage::open_production_with(
        &path,
        None,
        &TestClock(8),
        &StdRepairFileSystem,
        &PanickingObserver,
    )
    .unwrap();
    reopened.close().unwrap();
}

#[test]
fn every_real_hook_failure_rolls_back_its_schema_change_and_keeps_backup() {
    struct Case {
        table: &'static str,
        create_sql: &'static str,
        column: CatalogColumn,
        hook: &'static str,
    }

    let cases = [
        Case {
            table: "new_environments",
            create_sql: "CREATE TABLE new_environments(path TEXT PRIMARY KEY,session_count INTEGER NOT NULL,last_used_at INTEGER NOT NULL);",
            column: CatalogColumn {
                name: "marker",
                declared_type: Some("TEXT"),
                add_column_sql: Some("ALTER TABLE new_environments ADD COLUMN marker TEXT;"),
                check_type: false,
            },
            hook: "new_environments.rebuildFromSessions",
        },
        Case {
            table: "deepchat_pending_inputs",
            create_sql: "CREATE TABLE deepchat_pending_inputs(id TEXT PRIMARY KEY);",
            column: CatalogColumn {
                name: "retry_required_at",
                declared_type: Some("INTEGER"),
                add_column_sql: Some(
                    "ALTER TABLE deepchat_pending_inputs ADD COLUMN retry_required_at INTEGER;",
                ),
                check_type: false,
            },
            hook: "deepchat_pending_inputs.normalizeRetryRequiredRows",
        },
        Case {
            table: "agent_memory",
            create_sql: "CREATE TABLE agent_memory(id TEXT PRIMARY KEY);",
            column: CatalogColumn {
                name: "lifecycle_state",
                declared_type: Some("TEXT"),
                add_column_sql: Some("ALTER TABLE agent_memory ADD COLUMN lifecycle_state TEXT;"),
                check_type: false,
            },
            hook: "agent_memory.repairCanonicalStateAfterSchemaRepair",
        },
        Case {
            table: "agent_memory_audit",
            create_sql: "CREATE TABLE agent_memory_audit(id TEXT PRIMARY KEY);",
            column: CatalogColumn {
                name: "memory_ref_id",
                declared_type: Some("TEXT"),
                add_column_sql: Some(
                    "ALTER TABLE agent_memory_audit ADD COLUMN memory_ref_id TEXT;",
                ),
                check_type: false,
            },
            hook: "agent_memory_audit.backfillMemoryRefIds",
        },
    ];

    for case in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(case.create_sql).unwrap();
        let catalog = [CatalogDefinition {
            name: case.table,
            create_sql: case.create_sql,
            created_on_fresh_install: true,
            columns: vec![case.column.clone()],
            indexes: vec![],
            after_repair: Some(case.hook),
        }];
        let error =
            DatabaseRepairService::new(&conn, &path, &catalog, &TestClock(0), &StdRepairFileSystem)
                .repair()
                .unwrap_err();
        assert_eq!(error, DatabaseRepairError::Hook, "{}", case.hook);
        let column_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info(?1) WHERE name=?2",
                [case.table, case.column.name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(column_count, 0, "{}", case.hook);
        assert!(
            dir.path()
                .join("agent.db.1970-01-01T00-00-00-000Z.repair.bak")
                .exists(),
            "{}",
            case.hook
        );
    }
}

#[test]
fn repair_sql_failure_rolls_back_earlier_schema_work_and_keeps_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE first(id INTEGER PRIMARY KEY);
         CREATE TABLE second(id INTEGER PRIMARY KEY);",
    )
    .unwrap();
    let catalog = [
        definition(
            "first",
            vec![CatalogColumn {
                name: "added",
                declared_type: Some("TEXT"),
                add_column_sql: Some("ALTER TABLE first ADD COLUMN added TEXT;"),
                check_type: false,
            }],
            vec![],
        ),
        definition(
            "second",
            vec![CatalogColumn {
                name: "broken",
                declared_type: Some("TEXT"),
                add_column_sql: Some("ALTER TABLE second ADD COLUMN broken TEXT; invalid SQL;"),
                check_type: false,
            }],
            vec![],
        ),
    ];

    let error =
        DatabaseRepairService::new(&conn, &path, &catalog, &TestClock(0), &StdRepairFileSystem)
            .repair()
            .unwrap_err();
    assert_eq!(error, DatabaseRepairError::Sql);
    for (table, column) in [("first", "added"), ("second", "broken")] {
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info(?1) WHERE name=?2",
                [table, column],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "{table}.{column}");
    }
    assert!(
        dir.path()
            .join("agent.db.1970-01-01T00-00-00-000Z.repair.bak")
            .exists()
    );
}

#[test]
fn multiple_repairable_columns_report_the_complete_added_column_set() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE deepchat_pending_inputs(
           id TEXT PRIMARY KEY, session_id TEXT NOT NULL, mode TEXT NOT NULL,
           state TEXT NOT NULL, payload_json TEXT NOT NULL, queue_order INTEGER,
           claimed_at INTEGER, consumed_at INTEGER, created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL
         );
         INSERT INTO deepchat_pending_inputs(
           id,session_id,mode,state,payload_json,created_at,updated_at
         ) VALUES('retry','s','m','retry_required','{}',10,20);",
    )
    .unwrap();
    let definition = ProductionSchemaCatalog::frozen()
        .definitions()
        .iter()
        .find(|definition| definition.name == "deepchat_pending_inputs")
        .unwrap()
        .clone();

    let report = DatabaseRepairService::new(
        &conn,
        &path,
        &[definition],
        &TestClock(0),
        &StdRepairFileSystem,
    )
    .repair()
    .unwrap();
    let expected = [
        "assistant_message_id",
        "blocking_json",
        "message_ids_json",
        "retry_required_at",
    ]
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();
    let actual = report
        .repaired_issues
        .iter()
        .filter(|issue| issue.kind == SchemaIssueKind::MissingColumn)
        .map(|issue| issue.name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected);
    let installed = conn
        .prepare("SELECT name FROM pragma_table_info('deepchat_pending_inputs')")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<std::collections::BTreeSet<_>, _>>()
        .unwrap();
    for column in expected {
        assert!(installed.contains(column), "{column}");
    }
    let retry: (String, Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT state,retry_required_at,blocking_json FROM deepchat_pending_inputs WHERE id='retry'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(retry, ("blocked".to_owned(), Some(20), None));
}

#[test]
fn missing_table_and_index_repairs_roll_back_with_later_hook_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TABLE indexed(id INTEGER PRIMARY KEY, value TEXT); CREATE TABLE failing(id INTEGER PRIMARY KEY);")
        .unwrap();
    let catalog = [
        CatalogDefinition {
            name: "created_then_rolled_back",
            create_sql: "CREATE TABLE created_then_rolled_back(id INTEGER PRIMARY KEY);",
            created_on_fresh_install: true,
            columns: vec![],
            indexes: vec![],
            after_repair: None,
        },
        CatalogDefinition {
            name: "indexed",
            create_sql: "",
            created_on_fresh_install: true,
            columns: vec![],
            indexes: vec![CatalogIndex {
                name: "idx_indexed_value",
                create_sql: "CREATE INDEX idx_indexed_value ON indexed(value);",
            }],
            after_repair: None,
        },
        CatalogDefinition {
            name: "failing",
            create_sql: "",
            created_on_fresh_install: true,
            columns: vec![CatalogColumn {
                name: "added",
                declared_type: Some("TEXT"),
                add_column_sql: Some("ALTER TABLE failing ADD COLUMN added TEXT;"),
                check_type: false,
            }],
            indexes: vec![],
            after_repair: Some("unknown.hook"),
        },
    ];
    let error =
        DatabaseRepairService::new(&conn, &path, &catalog, &TestClock(0), &StdRepairFileSystem)
            .repair()
            .unwrap_err();
    assert_eq!(error, DatabaseRepairError::Hook);
    for (kind, name) in [
        ("table", "created_then_rolled_back"),
        ("index", "idx_indexed_value"),
    ] {
        let exists: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type=?1 AND name=?2)",
                [kind, name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(exists, 0, "{kind} {name}");
    }
    assert!(
        dir.path()
            .join("agent.db.1970-01-01T00-00-00-000Z.repair.bak")
            .exists()
    );
}

#[test]
fn explicit_manual_repair_executes_the_complete_ordered_41_definition_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
    let catalog = ProductionSchemaCatalog::frozen();
    let report = DatabaseRepairService::new(
        &conn,
        &path,
        catalog.definitions(),
        &TestClock(0),
        &StdRepairFileSystem,
    )
    .repair()
    .unwrap();
    assert_eq!(report.status, DatabaseRepairStatus::Repaired);
    assert_eq!(report.diagnosis_before_repair.issues.len(), 41);
    assert_eq!(report.repaired_issues.len(), 41);
    assert!(report.diagnosis_after_repair.is_healthy);
    assert!(report.remaining_issues.is_empty());
    let actual = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<std::collections::BTreeSet<_>, _>>()
        .unwrap();
    for definition in catalog.definitions() {
        assert!(actual.contains(definition.name), "{}", definition.name);
    }
}
