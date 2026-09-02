use std::collections::BTreeSet;

use deepchat_services::clock::Clock;
use deepchat_services::production_schema::{
    EMPTY_MIGRATION_VERSIONS, EXPECTED_CATALOG_DEFINITIONS, EXPECTED_PHYSICAL_OWNERS,
    EXPECTED_RUNTIME_OWNERS, EXPECTED_STARTUP_DEFINITIONS, PRODUCTION_SCHEMA_VERSION,
    ProductionSchemaCatalog, ProductionSchemaError,
};
use deepchat_services::schema::{MigrationError, MigrationRunner};
use deepchat_services::startup::Storage;
use rusqlite::Connection;

struct TestClock(i64);
impl Clock for TestClock {
    fn now_millis(&self) -> i64 {
        self.0
    }
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .unwrap()
        == 1
}

fn columns(conn: &Connection, table: &str) -> Vec<(String, i64)> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(1)?, row.get(5)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

#[test]
fn catalog_topology_and_membership_are_exact() {
    let catalog = ProductionSchemaCatalog::frozen();
    assert_eq!(catalog.definitions().len(), EXPECTED_CATALOG_DEFINITIONS);
    assert_eq!(catalog.physical_owners().len(), EXPECTED_PHYSICAL_OWNERS);
    assert_eq!(catalog.runtime_owners().len(), EXPECTED_RUNTIME_OWNERS);
    assert_eq!(
        catalog.startup_definitions().len(),
        EXPECTED_STARTUP_DEFINITIONS
    );
    assert_eq!(
        catalog.reference_commit(),
        "ca75acfdc680fa3d0a2bbde13575fa711d08a3bd"
    );

    let legacy = ["conversations", "messages", "message_attachments"];
    for name in legacy {
        let definition = catalog
            .definitions()
            .iter()
            .find(|entry| entry.name == name)
            .unwrap();
        assert!(!definition.created_on_fresh_install);
        assert!(
            !catalog
                .physical_owners()
                .iter()
                .any(|owner| owner.name == name)
        );
        assert!(
            !catalog
                .runtime_owners()
                .iter()
                .any(|owner| owner.name == name)
        );
    }
    assert!(
        catalog
            .physical_owners()
            .iter()
            .any(|owner| owner.name == "acp_turns")
    );
    assert!(
        !catalog
            .runtime_owners()
            .iter()
            .any(|owner| owner.name == "acp_turns")
    );
    for name in ["app_settings", "providers", "mcp_servers", "agent_settings"] {
        assert!(
            catalog
                .physical_owners()
                .iter()
                .any(|owner| owner.name == name)
        );
        assert!(
            catalog
                .runtime_owners()
                .iter()
                .any(|owner| owner.name == name)
        );
        assert!(!catalog.definitions().iter().any(|entry| entry.name == name));
    }
}

#[test]
fn metadata_preserves_columns_indexes_repair_hooks_and_owner_versions() {
    let catalog = ProductionSchemaCatalog::frozen();
    assert_eq!(
        catalog
            .runtime_owners()
            .iter()
            .map(|owner| owner.latest_version)
            .max(),
        Some(PRODUCTION_SCHEMA_VERSION)
    );
    assert_eq!(
        catalog.empty_versions(),
        EMPTY_MIGRATION_VERSIONS
            .into_iter()
            .collect::<BTreeSet<_>>()
    );
    for definition in catalog.definitions() {
        assert!(!definition.create_sql.trim().is_empty());
        assert!(!definition.columns.is_empty());
        for index in &definition.indexes {
            assert!(index.create_sql.contains(index.name));
        }
    }
    for (name, hook) in [
        ("new_environments", "new_environments.rebuildFromSessions"),
        (
            "deepchat_pending_inputs",
            "deepchat_pending_inputs.normalizeRetryRequiredRows",
        ),
        (
            "agent_memory",
            "agent_memory.repairCanonicalStateAfterSchemaRepair",
        ),
        (
            "agent_memory_audit",
            "agent_memory_audit.backfillMemoryRefIds",
        ),
    ] {
        assert_eq!(
            catalog
                .definitions()
                .iter()
                .find(|entry| entry.name == name)
                .unwrap()
                .after_repair,
            Some(hook)
        );
    }
    let app = catalog
        .runtime_owners()
        .into_iter()
        .find(|owner| owner.name == "app_settings")
        .unwrap();
    assert!(app.migrations[&25].contains("config_migrations"));
    assert!(!app.migrations[&26].contains("config_migrations"));
}

#[test]
fn fresh_production_storage_creates_static_schema_before_only_v69_marker() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent.db");
    let storage = Storage::open_production(&path, None, &TestClock(7)).unwrap();
    let conn = storage.connection();
    for name in [
        "acp_sessions",
        "acp_turns",
        "new_sessions",
        "deepchat_usage_stats",
        "agent_memory",
        "agent_memory_directive",
        "app_settings",
        "config_migrations",
        "providers",
        "mcp_servers",
        "agent_settings",
        "live_delegations",
    ] {
        assert!(table_exists(conn, name), "missing {name}");
    }
    let versions = conn
        .prepare("SELECT version FROM schema_versions ORDER BY version")
        .unwrap()
        .query_map([], |row| row.get::<_, i64>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert_eq!(versions, vec![69]);
    let virtual_tables: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE sql LIKE 'CREATE VIRTUAL TABLE%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(virtual_tables, 0);
}

const HISTORICAL_V10_SQL: &str = include_str!("fixtures/production_v10.sql");

fn schema_object_signature(conn: &Connection) -> Vec<(String, String)> {
    conn.prepare(
        "SELECT type, name FROM sqlite_master
         WHERE name NOT LIKE 'sqlite_%' AND name != 'schema_versions'
         ORDER BY type, name",
    )
    .unwrap()
    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn table_info_signature(
    conn: &Connection,
    table: &str,
) -> Vec<(String, String, i64, Option<String>, i64)> {
    let mut signature = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    signature.sort();
    signature
}

#[test]
fn historical_v10_file_runs_real_transformations_through_production_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("historical-v10.db");
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(HISTORICAL_V10_SQL).unwrap();
        assert!(
            !columns(&conn, "new_sessions")
                .iter()
                .any(|(name, _)| name == "is_draft")
        );
        assert!(
            !columns(&conn, "deepchat_sessions")
                .iter()
                .any(|(name, _)| name == "system_prompt")
        );
        assert!(
            !columns(&conn, "deepchat_message_traces")
                .iter()
                .any(|(name, _)| name == "logical_round")
        );
    }

    let storage = Storage::open_production(&path, None, &TestClock(11)).unwrap();
    let conn = storage.connection();
    assert_eq!(
        conn.query_row(
            "SELECT title FROM new_sessions WHERE id='historical-session'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "preserved"
    );
    assert!(
        columns(conn, "new_sessions")
            .iter()
            .any(|(name, _)| name == "is_draft")
    );
    assert!(
        columns(conn, "deepchat_sessions")
            .iter()
            .any(|(name, _)| name == "system_prompt")
    );
    assert!(
        columns(conn, "deepchat_message_traces")
            .iter()
            .any(|(name, _)| name == "logical_round")
    );
    let versions: Vec<i64> = conn
        .prepare("SELECT version FROM schema_versions ORDER BY version")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(versions, (10..=69).collect::<Vec<_>>());
    assert_eq!(
        ProductionSchemaCatalog::frozen().empty_versions(),
        EMPTY_MIGRATION_VERSIONS.into_iter().collect()
    );

    let fresh_path = dir.path().join("fresh.db");
    let fresh = Storage::open_production(&fresh_path, None, &TestClock(12)).unwrap();
    assert_eq!(
        schema_object_signature(conn),
        schema_object_signature(fresh.connection())
    );
    for table in [
        "new_sessions",
        "deepchat_sessions",
        "deepchat_message_traces",
        "acp_sessions",
        "deepchat_usage_stats",
        "agent_memory",
        "agent_memory_directive",
    ] {
        assert_eq!(
            table_info_signature(conn, table),
            table_info_signature(fresh.connection(), table),
            "schema mismatch for {table}"
        );
    }
}

#[test]
fn static_create_marker_crash_gap_recovers_through_production_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("interrupted.db");
    {
        let conn = Connection::open(&path).unwrap();
        ProductionSchemaCatalog::frozen()
            .create_static_schema(&conn)
            .unwrap();
        conn.execute(
            "INSERT INTO new_projects(path,name,last_accessed_at) VALUES('/kept','kept',1)",
            [],
        )
        .unwrap();
        assert!(!table_exists(&conn, "schema_versions"));
    }
    let storage = Storage::open_production(&path, None, &TestClock(13)).unwrap();
    let conn = storage.connection();
    assert_eq!(
        conn.query_row(
            "SELECT name FROM new_projects WHERE path='/kept'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "kept"
    );
    assert_eq!(
        conn.query_row("SELECT MAX(version) FROM schema_versions", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        69
    );
}

#[test]
fn owner_sql_finalizer_and_marker_roll_back_together() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE deepchat_pending_inputs (
           id TEXT PRIMARY KEY, session_id TEXT NOT NULL, mode TEXT NOT NULL,
           state TEXT NOT NULL, payload_json TEXT NOT NULL, blocking_json TEXT,
           queue_order INTEGER, claimed_at INTEGER, consumed_at INTEGER,
           created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
         );
         INSERT INTO deepchat_pending_inputs VALUES
           ('p','s','queue','retry_required','{}','blocked',NULL,NULL,NULL,10,20);
         CREATE TABLE schema_versions(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);
         INSERT INTO schema_versions VALUES(66,0);
         CREATE TRIGGER reject_v67 BEFORE INSERT ON schema_versions WHEN NEW.version=67
         BEGIN SELECT RAISE(ABORT, 'reject marker'); END;",
    )
    .unwrap();
    let error = MigrationRunner::new(&conn, &TestClock(1))
        .run(
            &ProductionSchemaCatalog::frozen().migration_catalog(),
            false,
        )
        .unwrap_err();
    assert_eq!(error, MigrationError::Marker);
    assert!(
        !columns(&conn, "deepchat_pending_inputs")
            .iter()
            .any(|(name, _)| name == "retry_required_at")
    );
    let state: (String, Option<String>) = conn
        .query_row(
            "SELECT state, blocking_json FROM deepchat_pending_inputs WHERE id='p'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(state, ("retry_required".into(), Some("blocked".into())));
    assert_eq!(
        conn.query_row("SELECT MAX(version) FROM schema_versions", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        66
    );
}

#[test]
fn v23_recovers_only_missing_deepchat_session_columns() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE deepchat_sessions (
           id TEXT PRIMARY KEY, provider_id TEXT NOT NULL, model_id TEXT NOT NULL,
           permission_mode TEXT NOT NULL DEFAULT 'full_access', system_prompt TEXT
         );
         CREATE TABLE schema_versions(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);
         INSERT INTO schema_versions VALUES(22,0);",
    )
    .unwrap();
    let catalog = ProductionSchemaCatalog::frozen().migration_catalog();
    MigrationRunner::new(&conn, &TestClock(1))
        .run(&catalog, false)
        .unwrap_err();
    let names = columns(&conn, "deepchat_sessions")
        .into_iter()
        .map(|(name, _)| name)
        .collect::<BTreeSet<_>>();
    assert!(names.contains("system_prompt"));
    assert!(names.contains("temperature"));
    assert!(names.contains("top_p"));
}

#[test]
fn v26_normalization_creates_all_normalized_tables_without_recreating_config_migrations() {
    let catalog = ProductionSchemaCatalog::frozen();
    let version_26 = catalog
        .runtime_owners()
        .into_iter()
        .filter_map(|owner| owner.migrations.get(&26).map(|sql| (owner.name, *sql)))
        .collect::<Vec<_>>();
    assert_eq!(version_26.len(), 8);
    let names = version_26
        .iter()
        .map(|(name, _)| *name)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        names,
        [
            "app_settings",
            "deepchat_user_messages",
            "deepchat_user_message_files",
            "deepchat_user_message_links",
            "deepchat_assistant_blocks",
            "deepchat_search_documents",
            "new_session_active_skills",
            "new_session_disabled_agent_tools",
        ]
        .into_iter()
        .collect()
    );
    assert!(
        version_26
            .iter()
            .all(|(_, sql)| !sql.contains("config_migrations"))
    );
}

#[test]
fn acp_v30_rebuild_preserves_rows_and_replaces_schema() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE acp_sessions (
           id INTEGER PRIMARY KEY, conversation_id TEXT NOT NULL, agent_id TEXT NOT NULL,
           session_id TEXT, workdir TEXT, status TEXT NOT NULL,
           created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, metadata TEXT
         );
         INSERT INTO acp_sessions VALUES(1,'c','a','remote','/tmp','idle',1,2,NULL);",
    )
    .unwrap();
    let owner = ProductionSchemaCatalog::frozen()
        .runtime_owners()
        .into_iter()
        .find(|owner| owner.name == "acp_sessions")
        .unwrap();
    conn.execute_batch(owner.migrations[&30]).unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM acp_sessions", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    let indexes: BTreeSet<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name='acp_sessions' AND sql IS NOT NULL")
        .unwrap()
        .query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
    assert!(!indexes.is_empty());
    assert!(!table_exists(&conn, "acp_sessions_v30"));
}

fn usage_stats_v17_fixture(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE deepchat_usage_stats (
           message_id TEXT PRIMARY KEY, session_id TEXT NOT NULL, provider_id TEXT NOT NULL,
           model_id TEXT NOT NULL, input_tokens INTEGER, output_tokens INTEGER,
           total_tokens INTEGER, cached_input_tokens INTEGER, cache_write_input_tokens INTEGER,
           created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
         );
         INSERT INTO deepchat_usage_stats VALUES('m','s','p','model',1,2,3,4,5,6,7);",
    )
    .unwrap();
}

#[test]
fn usage_stats_v32_and_v68_are_copy_drop_rename_rebuilds() {
    let owner = ProductionSchemaCatalog::frozen()
        .runtime_owners()
        .into_iter()
        .find(|owner| owner.name == "deepchat_usage_stats")
        .unwrap();
    let conn = Connection::open_in_memory().unwrap();
    usage_stats_v17_fixture(&conn);
    conn.execute_batch("ALTER TABLE deepchat_usage_stats ADD COLUMN usage_date TEXT; ALTER TABLE deepchat_usage_stats ADD COLUMN source TEXT;").unwrap();
    conn.execute(
        "UPDATE deepchat_usage_stats SET usage_date='2026-01-01', source='live'",
        [],
    )
    .unwrap();
    conn.execute_batch(owner.migrations[&32]).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT source FROM deepchat_usage_stats WHERE message_id='m'",
            [],
            |row| row.get::<_, String>(0)
        )
        .unwrap(),
        "live"
    );
    conn.execute_batch(owner.migrations[&68]).unwrap();
    let row: (String, String, String) = conn
        .query_row(
            "SELECT usage_id,message_id,usage_category FROM deepchat_usage_stats",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(row, ("m".into(), "m".into(), "chat".into()));
    let info = columns(&conn, "deepchat_usage_stats");
    assert!(info.iter().any(|(name, pk)| name == "usage_id" && *pk == 1));
    assert!(
        info.iter()
            .any(|(name, pk)| name == "message_id" && *pk == 0)
    );
}

#[test]
fn v69_repairs_bad_v68_shape_and_leaves_correct_table_unchanged() {
    let owner = ProductionSchemaCatalog::frozen()
        .runtime_owners()
        .into_iter()
        .find(|owner| owner.name == "deepchat_usage_stats")
        .unwrap();
    let bad = Connection::open_in_memory().unwrap();
    usage_stats_v17_fixture(&bad);
    bad.execute_batch("ALTER TABLE deepchat_usage_stats ADD COLUMN usage_date TEXT; ALTER TABLE deepchat_usage_stats ADD COLUMN source TEXT;").unwrap();
    bad.execute(
        "UPDATE deepchat_usage_stats SET usage_date='2026-01-01', source='live'",
        [],
    )
    .unwrap();
    bad.execute_batch("ALTER TABLE deepchat_usage_stats ADD COLUMN usage_id TEXT; ALTER TABLE deepchat_usage_stats ADD COLUMN usage_category TEXT;").unwrap();
    bad.execute(
        "UPDATE deepchat_usage_stats SET usage_id=message_id, usage_category='chat'",
        [],
    )
    .unwrap();
    bad.execute_batch("CREATE TABLE schema_versions(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL); INSERT INTO schema_versions VALUES(68,0);").unwrap();
    MigrationRunner::new(&bad, &TestClock(1))
        .run(
            &ProductionSchemaCatalog::frozen().migration_catalog(),
            false,
        )
        .unwrap();
    assert!(
        columns(&bad, "deepchat_usage_stats")
            .iter()
            .any(|(name, pk)| name == "usage_id" && *pk == 1)
    );

    let good = Connection::open_in_memory().unwrap();
    good.execute_batch(owner.create_sql).unwrap();
    good.execute_batch("CREATE TABLE schema_versions(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL); INSERT INTO schema_versions VALUES(68,0); CREATE TRIGGER preserved_usage_trigger AFTER INSERT ON deepchat_usage_stats BEGIN SELECT 1; END;").unwrap();
    MigrationRunner::new(&good, &TestClock(1))
        .run(
            &ProductionSchemaCatalog::frozen().migration_catalog(),
            false,
        )
        .unwrap();
    let preserved: i64 = good.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND name='preserved_usage_trigger')", [], |row| row.get(0)).unwrap();
    assert_eq!(preserved, 1);
}

fn schema_failure_after(conn: &Connection, corruption: &str) -> ProductionSchemaError {
    conn.execute_batch(corruption).unwrap();
    if !table_exists(conn, "schema_versions") {
        conn.execute_batch("CREATE TABLE schema_versions(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL); INSERT INTO schema_versions VALUES(69,0);").unwrap();
    }
    ProductionSchemaCatalog::frozen()
        .initialize(conn, false, &TestClock(1))
        .unwrap_err()
}

#[test]
fn schema_finalizer_rejects_corrupted_constraints_and_rows() {
    for (original, replacement) in [
        (
            "lifecycle_state TEXT NOT NULL DEFAULT 'active'\n          CHECK (lifecycle_state IN ('active', 'archived', 'conflicted'))",
            "lifecycle_state TEXT DEFAULT 'active' CHECK (lifecycle_state IN ('active', 'archived', 'conflicted'))",
        ),
        (
            "embedding_state TEXT NOT NULL DEFAULT 'pending'\n          CHECK (embedding_state IN ('pending', 'ready', 'error', 'fts_only', 'not_applicable'))",
            "embedding_state TEXT NOT NULL DEFAULT 'wrong' CHECK (embedding_state IN ('pending', 'ready', 'error', 'fts_only', 'not_applicable'))",
        ),
        (
            "temporal_kind TEXT NOT NULL DEFAULT 'atemporal'\n          CHECK (temporal_kind IN ('atemporal', 'state', 'event', 'plan', 'recurring'))",
            "temporal_kind TEXT NOT NULL DEFAULT 'atemporal'",
        ),
        (
            "scope_type TEXT NOT NULL DEFAULT 'agent'\n          CHECK (scope_type IN ('agent', 'user', 'project', 'session'))",
            "scope_type TEXT NOT NULL DEFAULT 'agent'",
        ),
    ] {
        let conn = Connection::open_in_memory().unwrap();
        ProductionSchemaCatalog::frozen()
            .create_static_schema(&conn)
            .unwrap();
        let sql = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='agent_memory'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap();
        let corrupt = sql.replace(original, replacement);
        assert_ne!(corrupt, sql);
        conn.execute_batch("PRAGMA foreign_keys=OFF; DROP TABLE agent_memory;")
            .unwrap();
        conn.execute_batch(&corrupt).unwrap();
        assert_eq!(
            schema_failure_after(&conn, ""),
            ProductionSchemaError::Finalize
        );
    }

    for row in [
        "DROP TRIGGER agent_memory_temporal_bi_v1; PRAGMA ignore_check_constraints=ON; INSERT INTO agent_memory(id,agent_id,kind,content,provenance_key,importance,status,created_at,decision_revision,lifecycle_state,embedding_state,temporal_kind,valid_from,scope_type) VALUES('bad','a','semantic','c','p',1,'pending_embedding',1,1,'active','pending','atemporal',1,'agent'); PRAGMA ignore_check_constraints=OFF;",
        "DROP TRIGGER agent_memory_scope_bi_v1; PRAGMA ignore_check_constraints=ON; INSERT INTO agent_memory(id,agent_id,kind,content,provenance_key,importance,status,created_at,decision_revision,lifecycle_state,embedding_state,temporal_kind,scope_type) VALUES('bad','a','semantic','c','p',1,'pending_embedding',1,1,'active','pending','atemporal','project'); PRAGMA ignore_check_constraints=OFF;",
    ] {
        let conn = Connection::open_in_memory().unwrap();
        ProductionSchemaCatalog::frozen()
            .create_static_schema(&conn)
            .unwrap();
        assert_eq!(
            schema_failure_after(&conn, row),
            ProductionSchemaError::Finalize
        );
    }
}

#[test]
fn schema_finalizer_maintains_memory_and_directive_indexes() {
    let conn = Connection::open_in_memory().unwrap();
    let catalog = ProductionSchemaCatalog::frozen();
    catalog.create_static_schema(&conn).unwrap();
    conn.execute_batch("CREATE INDEX idx_agent_memory_management_page ON agent_memory(agent_id); CREATE INDEX idx_agent_memory_embedding_queue ON agent_memory(agent_id); CREATE INDEX idx_agent_memory_lifecycle_maintenance ON agent_memory(agent_id); DROP INDEX idx_agent_memory_agent_kind; DROP INDEX idx_agent_memory_conflict_state_anomaly_v2; DROP INDEX idx_agent_memory_active_recall; DROP INDEX idx_agent_memory_management_page_v3; DROP INDEX idx_agent_memory_archive_eligible_v3; DROP INDEX idx_agent_memory_cognitive_top_v3; DROP INDEX idx_agent_memory_conflict_fairness_v3; DROP INDEX idx_agent_memory_recent_activity_v3; DROP INDEX idx_agent_memory_embedding_pending_agent_v2; DROP INDEX idx_agent_memory_embedding_pending_global_v2; DROP INDEX idx_agent_memory_conflict_target_v2; CREATE TABLE schema_versions(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL); INSERT INTO schema_versions VALUES(69,0);").unwrap();
    catalog.initialize(&conn, false, &TestClock(1)).unwrap();
    for name in [
        "idx_agent_memory_agent_kind",
        "idx_agent_memory_conflict_state_anomaly_v2",
        "idx_agent_memory_active_recall",
        "idx_agent_memory_management_page_v3",
        "idx_agent_memory_archive_eligible_v3",
        "idx_agent_memory_cognitive_top_v3",
        "idx_agent_memory_conflict_fairness_v3",
        "idx_agent_memory_recent_activity_v3",
        "idx_agent_memory_embedding_pending_agent_v2",
        "idx_agent_memory_embedding_pending_global_v2",
        "idx_agent_memory_conflict_target_v2",
        "idx_agent_memory_directive_management_v1",
        "idx_agent_memory_directive_status_v1",
        "idx_agent_memory_directive_active_kind_v1",
    ] {
        assert!(
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1)",
                [name],
                |row| row.get::<_, i64>(0)
            )
            .unwrap()
                == 1,
            "missing {name}"
        );
    }
    for name in [
        "idx_agent_memory_management_page",
        "idx_agent_memory_embedding_queue",
        "idx_agent_memory_lifecycle_maintenance",
    ] {
        assert_eq!(
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1)",
                [name],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0,
            "retired canonical conflict index survived: {name}"
        );
    }
}

#[test]
fn required_dynamic_finalizers_install_real_triggers_and_errors_are_redacted() {
    let catalog = ProductionSchemaCatalog::frozen();
    for owner_name in [
        "live_delegations",
        "live_delegation_turns",
        "live_delegation_events",
    ] {
        let owner = catalog
            .runtime_owners()
            .into_iter()
            .find(|owner| owner.name == owner_name)
            .unwrap();
        assert!(owner.finalizers.contains_key(&60));
    }
    let conn = Connection::open_in_memory().unwrap();
    catalog.create_static_schema(&conn).unwrap();
    for name in [
        "trg_live_delegations_parent_insert",
        "trg_live_delegation_turns_parent_insert",
        "trg_live_delegation_events_parent_insert",
        "agent_memory_dirty_ai",
        "agent_memory_scope_bi_v1",
        "agent_memory_temporal_bi_v1",
        "agent_memory_clear_guard_bi_v1",
    ] {
        let found: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND name=?1)",
                [name],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(found, 1, "missing finalizer artifact {name}");
    }

    let broken = Connection::open_in_memory().unwrap();
    catalog.create_static_schema(&broken).unwrap();
    broken.execute_batch("DROP TRIGGER agent_memory_scope_bi_v1; PRAGMA ignore_check_constraints=ON; INSERT INTO agent_memory(id,agent_id,kind,content,provenance_key,importance,status,created_at,decision_revision,lifecycle_state,embedding_state,temporal_kind,scope_type) VALUES('bad','a','semantic','c','p',1,'pending_embedding',1,1,'active','pending','atemporal','project'); PRAGMA ignore_check_constraints=OFF; CREATE TABLE schema_versions(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL); INSERT INTO schema_versions VALUES(69,0);").unwrap();
    let error = catalog
        .initialize(&broken, false, &TestClock(1))
        .unwrap_err();
    assert_eq!(error, ProductionSchemaError::Finalize);
    let rendered = format!("{error} {error:?}");
    assert!(!rendered.contains("deepchat_usage_stats"));
    assert!(!rendered.contains("no such table"));
    assert!(std::error::Error::source(&error).is_none());
}
