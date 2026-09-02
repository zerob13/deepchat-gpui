//! Frozen production schema catalog and migration-owner topology.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use rusqlite::{Connection, Transaction};
use serde::Deserialize;
use thiserror::Error;

use crate::clock::Clock;
use crate::schema::{
    MigrationCatalog, MigrationError, MigrationFailureClass, MigrationFinalizer, MigrationRunner,
};
use crate::schema_error_classifier::{SchemaErrorReason, classify_schema_error};

pub const PRODUCTION_SCHEMA_VERSION: i64 = 69;
pub const EXPECTED_CATALOG_DEFINITIONS: usize = 41;
pub const EXPECTED_PHYSICAL_OWNERS: usize = 39;
pub const EXPECTED_RUNTIME_OWNERS: usize = 38;
pub const EXPECTED_STARTUP_DEFINITIONS: usize = 38;
pub const EMPTY_MIGRATION_VERSIONS: [i64; 19] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 39, 40, 53, 54, 55, 56, 57, 58, 63,
];

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ProductionSchemaError {
    #[error("production schema metadata is invalid")]
    Metadata,
    #[error("production schema creation failed")]
    Create,
    #[error("production schema migration failed")]
    Migration,
    #[error("production schema finalization failed")]
    Finalize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProductionInitializationClass {
    Destructive,
    Schema(SchemaErrorReason),
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProductionInitializationFailure {
    pub error: ProductionSchemaError,
    pub class: ProductionInitializationClass,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrozenCatalog {
    reference_commit: String,
    catalog: Vec<FrozenDefinition>,
    owners: Vec<FrozenOwner>,
    finalizer_sql: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrozenDefinition {
    name: String,
    create_sql: String,
    created_on_fresh_install: bool,
    columns: Vec<FrozenColumn>,
    indexes: Vec<FrozenIndex>,
    after_repair: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrozenColumn {
    name: String,
    declared_type: Option<String>,
    add_column_sql: Option<String>,
    check_type: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrozenIndex {
    name: String,
    create_sql: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FrozenOwner {
    name: String,
    latest_version: i64,
    create_sql: String,
    migrations: BTreeMap<String, String>,
    finalizers: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogColumn {
    pub name: &'static str,
    pub declared_type: Option<&'static str>,
    pub add_column_sql: Option<&'static str>,
    pub check_type: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogIndex {
    pub name: &'static str,
    pub create_sql: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogDefinition {
    pub name: &'static str,
    pub create_sql: &'static str,
    pub created_on_fresh_install: bool,
    pub columns: Vec<CatalogColumn>,
    pub indexes: Vec<CatalogIndex>,
    pub after_repair: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationOwner {
    pub name: &'static str,
    pub latest_version: i64,
    pub create_sql: &'static str,
    pub migrations: BTreeMap<i64, &'static str>,
    pub finalizers: BTreeMap<i64, &'static str>,
}

#[derive(Debug)]
pub struct ProductionSchemaCatalog {
    reference_commit: &'static str,
    definitions: Vec<CatalogDefinition>,
    physical_owners: Vec<MigrationOwner>,
}

impl ProductionSchemaCatalog {
    pub fn frozen() -> &'static Self {
        static CATALOG: OnceLock<ProductionSchemaCatalog> = OnceLock::new();
        CATALOG.get_or_init(|| {
            let frozen: FrozenCatalog =
                serde_json::from_str(include_str!("production_catalog.json"))
                    .expect("checked-in production catalog must be valid JSON");
            Self::from_frozen(frozen).expect("checked-in production catalog must satisfy topology")
        })
    }

    fn from_frozen(frozen: FrozenCatalog) -> Result<Self, ProductionSchemaError> {
        fn leak(value: String) -> &'static str {
            Box::leak(value.into_boxed_str())
        }
        let definitions = frozen
            .catalog
            .into_iter()
            .map(|definition| CatalogDefinition {
                name: leak(definition.name),
                create_sql: leak(definition.create_sql),
                created_on_fresh_install: definition.created_on_fresh_install,
                columns: definition
                    .columns
                    .into_iter()
                    .map(|column| CatalogColumn {
                        name: leak(column.name),
                        declared_type: column.declared_type.map(leak),
                        add_column_sql: column.add_column_sql.map(leak),
                        check_type: column.check_type,
                    })
                    .collect(),
                indexes: definition
                    .indexes
                    .into_iter()
                    .map(|index| CatalogIndex {
                        name: leak(index.name),
                        create_sql: leak(index.create_sql),
                    })
                    .collect(),
                after_repair: definition.after_repair.map(leak),
            })
            .collect();
        let physical_owners = frozen
            .owners
            .into_iter()
            .map(|owner| {
                let migrations = owner
                    .migrations
                    .into_iter()
                    .map(|(version, sql)| {
                        version
                            .parse::<i64>()
                            .map(|version| (version, leak(sql)))
                            .map_err(|_| ProductionSchemaError::Metadata)
                    })
                    .collect::<Result<_, _>>()?;
                let finalizers = owner
                    .finalizers
                    .into_iter()
                    .map(|(version, identity)| {
                        version
                            .parse::<i64>()
                            .map(|version| (version, leak(identity)))
                            .map_err(|_| ProductionSchemaError::Metadata)
                    })
                    .collect::<Result<_, _>>()?;
                Ok(MigrationOwner {
                    name: leak(owner.name),
                    latest_version: owner.latest_version,
                    create_sql: leak(owner.create_sql),
                    migrations,
                    finalizers,
                })
            })
            .collect::<Result<Vec<_>, ProductionSchemaError>>()?;
        let catalog = Self {
            reference_commit: leak(frozen.reference_commit),
            definitions,
            physical_owners,
        };
        catalog.validate(&frozen.finalizer_sql)?;
        Ok(catalog)
    }

    fn validate(
        &self,
        finalizer_sql: &BTreeMap<String, String>,
    ) -> Result<(), ProductionSchemaError> {
        let runtime = self.runtime_owners();
        if self.definitions.len() != EXPECTED_CATALOG_DEFINITIONS
            || self.physical_owners.len() != EXPECTED_PHYSICAL_OWNERS
            || runtime.len() != EXPECTED_RUNTIME_OWNERS
            || self.startup_definitions().len() != EXPECTED_STARTUP_DEFINITIONS
            || runtime.iter().map(|owner| owner.latest_version).max()
                != Some(PRODUCTION_SCHEMA_VERSION)
            || finalizer_sql.is_empty()
        {
            return Err(ProductionSchemaError::Metadata);
        }
        Ok(())
    }

    pub fn reference_commit(&self) -> &str {
        self.reference_commit
    }

    pub fn definitions(&self) -> &[CatalogDefinition] {
        &self.definitions
    }

    pub fn startup_definitions(&self) -> Vec<&CatalogDefinition> {
        self.definitions
            .iter()
            .filter(|definition| definition.created_on_fresh_install)
            .collect()
    }

    pub fn physical_owners(&self) -> &[MigrationOwner] {
        &self.physical_owners
    }

    pub fn runtime_owners(&self) -> Vec<&MigrationOwner> {
        self.physical_owners
            .iter()
            .filter(|owner| owner.name != "acp_turns")
            .collect()
    }

    pub fn empty_versions(&self) -> BTreeSet<i64> {
        let populated = self
            .runtime_owners()
            .into_iter()
            .flat_map(|owner| owner.migrations.keys().copied())
            .collect::<BTreeSet<_>>();
        let conditional_versions = [23].into_iter().collect::<BTreeSet<_>>();
        (1..=PRODUCTION_SCHEMA_VERSION)
            .filter(|version| {
                !populated.contains(version) && !conditional_versions.contains(version)
            })
            .collect()
    }

    pub fn create_static_schema(&self, conn: &Connection) -> Result<(), ProductionSchemaError> {
        self.create_static_schema_classified(conn)
            .map_err(|failure| failure.error)
    }

    fn create_static_schema_classified(
        &self,
        conn: &Connection,
    ) -> Result<(), ProductionInitializationFailure> {
        let tx = conn
            .unchecked_transaction()
            .map_err(|error| classify_initialization_error(error, ProductionSchemaError::Create))?;
        for owner in &self.physical_owners {
            tx.execute_batch(owner.create_sql).map_err(|error| {
                classify_initialization_error(error, ProductionSchemaError::Create)
            })?;
        }
        tx.commit()
            .map_err(|error| classify_initialization_error(error, ProductionSchemaError::Create))
    }

    pub fn migration_catalog(&'static self) -> MigrationCatalog {
        let mut catalog = MigrationCatalog::new(PRODUCTION_SCHEMA_VERSION);
        for version in 1..=PRODUCTION_SCHEMA_VERSION {
            let statements = self
                .runtime_owners()
                .into_iter()
                .filter_map(|owner| {
                    if owner.name == "deepchat_sessions" && version == 23 {
                        None
                    } else {
                        owner.migrations.get(&version).map(|sql| (*sql).to_owned())
                    }
                })
                .collect();
            catalog.add(version, statements);
        }
        catalog.set_finalizer(ProductionFinalizer { catalog: self });
        catalog
    }

    pub fn initialize<C: Clock>(
        &'static self,
        conn: &Connection,
        fresh_database: bool,
        clock: &C,
    ) -> Result<(), ProductionSchemaError> {
        self.initialize_before_assert(conn, fresh_database, clock)
            .map_err(|failure| failure.error)?;
        self.assert_current_schema(conn)
    }

    pub(crate) fn initialize_before_assert<C: Clock>(
        &'static self,
        conn: &Connection,
        fresh_database: bool,
        clock: &C,
    ) -> Result<(), ProductionInitializationFailure> {
        // Reference startup always runs every `CREATE ... IF NOT EXISTS` owner before
        // migration. Existing old-shape tables survive while owners introduced later
        // are created at their current static shape. This also closes the intentional
        // crash gap between static creation and the fresh v69 marker.
        self.create_static_schema_classified(conn)?;
        MigrationRunner::new(conn, clock)
            .run_classified(&self.migration_catalog(), fresh_database)
            .map_err(|failure| ProductionInitializationFailure {
                error: ProductionSchemaError::Migration,
                class: match failure.class {
                    MigrationFailureClass::Destructive => {
                        ProductionInitializationClass::Destructive
                    }
                    MigrationFailureClass::Schema(reason) => {
                        ProductionInitializationClass::Schema(reason)
                    }
                    MigrationFailureClass::Other => ProductionInitializationClass::Other,
                },
            })
    }

    pub(crate) fn assert_current_schema(
        &self,
        conn: &Connection,
    ) -> Result<(), ProductionSchemaError> {
        let memory_columns = table_column_constraints(conn, "agent_memory")?;
        for name in [
            "decision_revision",
            "lifecycle_state",
            "embedding_state",
            "temporal_kind",
            "valid_from",
            "valid_until",
            "temporal_confidence",
            "temporal_precision",
            "temporal_timezone",
            "scope_type",
            "scope_id",
        ] {
            if !memory_columns.contains_key(name) {
                return Err(ProductionSchemaError::Finalize);
            }
        }
        let memory_sql = normalized_table_sql(conn, "agent_memory")?;
        require_column_constraint(&memory_columns, "lifecycle_state", "'active'")?;
        require_column_constraint(&memory_columns, "embedding_state", "'pending'")?;
        require_column_constraint(&memory_columns, "temporal_kind", "'atemporal'")?;
        require_column_constraint(&memory_columns, "scope_type", "'agent'")?;
        for fragment in [
            "CHECK (lifecycle_state IN ('active', 'archived', 'conflicted'))",
            "CHECK (embedding_state IN ('pending', 'ready', 'error', 'fts_only', 'not_applicable'))",
            "CHECK (temporal_kind IN ('atemporal', 'state', 'event', 'plan', 'recurring'))",
            "CHECK (temporal_confidence IS NULL OR (temporal_confidence >= 0 AND temporal_confidence <= 1))",
            "CHECK (temporal_precision IS NULL OR temporal_precision IN ('exact', 'day', 'week', 'month', 'quarter', 'year', 'unknown'))",
            "CHECK (temporal_timezone IS NULL OR (length(temporal_timezone) BETWEEN 1 AND 128 AND temporal_timezone = trim(temporal_timezone)))",
            "CHECK (scope_type IN ('agent', 'user', 'project', 'session'))",
            "CHECK (scope_id IS NULL OR (length(scope_id) BETWEEN 1 AND 256 AND scope_id = trim(scope_id)))",
        ] {
            if !memory_sql.contains(fragment) {
                return Err(ProductionSchemaError::Finalize);
            }
        }
        require_zero_rows(conn, AGENT_MEMORY_TEMPORAL_INVALID_ROW_SQL, &[])?;
        require_zero_rows(conn, AGENT_MEMORY_SCOPE_INVALID_ROW_SQL, &[256])?;
        maintain_agent_memory_indexes(conn)?;
        // Connection-scoped tokenizer selection, FTS virtual tables, projection
        // rebuilds, and their triggers remain explicitly deferred to storage-002a-3.
        // This finalizer owns only deterministic static schema, persisted rows,
        // and ordinary indexes from the frozen assertCurrentSchema contract.
        for name in [
            "agent_memory_tombstone",
            "agent_memory_clear_job",
            "agent_memory_derivation",
            "agent_memory_dirty",
            "agent_memory_directive",
        ] {
            require_schema_object(conn, "table", name)?;
        }
        for name in [
            "idx_agent_memory_derivation_child_v1",
            "idx_agent_memory_dirty_order_v1",
            "idx_agent_memory_recall_scope_v6",
            "idx_agent_memory_directive_management_v1",
            "idx_agent_memory_directive_status_v1",
            "idx_agent_memory_directive_active_kind_v1",
        ] {
            require_schema_object(conn, "index", name)?;
        }
        for name in [
            "agent_memory_clear_guard_bi_v1",
            "agent_memory_clear_guard_bu_v1",
            "agent_memory_dirty_ai",
            "agent_memory_dirty_au",
            "agent_memory_dirty_ad",
            "agent_memory_legacy_status_bridge_ai",
            "agent_memory_legacy_status_bridge_au",
            "agent_memory_scope_bi_v1",
            "agent_memory_scope_bu_v1",
            "agent_memory_temporal_bi_v1",
            "agent_memory_temporal_bu_v1",
        ] {
            require_schema_object(conn, "trigger", name)?;
        }
        let directive_sql = normalized_table_sql(conn, "agent_memory_directive")?;
        for fragment in [
            "kind IN ('instruction', 'suppress_topic')",
            "status IN ('draft', 'active', 'rejected')",
            "source IN ('explicit_user', 'manual', 'derived_suggestion')",
            "UNIQUE (agent_id, kind, identity_hash)",
            "WITHOUT ROWID",
        ] {
            if !directive_sql.contains(fragment) {
                return Err(ProductionSchemaError::Finalize);
            }
        }
        Ok(())
    }
}

fn classify_initialization_error(
    error: rusqlite::Error,
    public: ProductionSchemaError,
) -> ProductionInitializationFailure {
    let class = if crate::startup_recovery::is_destructive_database_error(&error) {
        ProductionInitializationClass::Destructive
    } else if let Some(classification) = classify_schema_error(&error.to_string()) {
        ProductionInitializationClass::Schema(classification.reason)
    } else {
        ProductionInitializationClass::Other
    };
    ProductionInitializationFailure {
        error: public,
        class,
    }
}

const AGENT_MEMORY_TEMPORAL_INVALID_ROW_SQL: &str = "SELECT COUNT(*) FROM agent_memory WHERE
  (valid_from IS NOT NULL AND valid_until IS NOT NULL AND valid_from >= valid_until)
  OR temporal_kind IS NULL
  OR temporal_kind NOT IN ('atemporal', 'state', 'event', 'plan', 'recurring')
  OR (temporal_confidence IS NOT NULL AND (temporal_confidence < 0 OR temporal_confidence > 1))
  OR (temporal_precision IS NOT NULL AND temporal_precision NOT IN ('exact', 'day', 'week', 'month', 'quarter', 'year', 'unknown'))
  OR (temporal_timezone IS NOT NULL AND (length(temporal_timezone) NOT BETWEEN 1 AND 128 OR temporal_timezone != trim(temporal_timezone)))
  OR (temporal_kind = 'atemporal' AND (valid_from IS NOT NULL OR valid_until IS NOT NULL OR temporal_confidence IS NOT NULL OR temporal_precision IS NOT NULL OR temporal_timezone IS NOT NULL))
  OR (temporal_kind != 'atemporal' AND (temporal_confidence IS NULL OR temporal_precision IS NULL OR temporal_timezone IS NULL))";

const AGENT_MEMORY_SCOPE_INVALID_ROW_SQL: &str = "SELECT COUNT(*) FROM agent_memory WHERE
  scope_type NOT IN ('agent', 'user', 'project', 'session')
  OR (scope_type = 'agent' AND scope_id IS NOT NULL)
  OR (scope_type != 'agent' AND (scope_id IS NULL OR length(scope_id) NOT BETWEEN 1 AND ?1 OR scope_id != trim(scope_id)))
  OR (scope_type = 'user' AND user_scope IS NOT scope_id)
  OR (scope_type IN ('project', 'session') AND user_scope IS NOT NULL)";

const AGENT_MEMORY_INDEX_MAINTENANCE_SQL: &str = "
  CREATE INDEX IF NOT EXISTS idx_agent_memory_agent_kind ON agent_memory(agent_id, kind, status);
  CREATE INDEX IF NOT EXISTS idx_agent_memory_agent_active ON agent_memory(agent_id, superseded_by);
  CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_memory_provenance ON agent_memory(agent_id, provenance_key) WHERE provenance_key IS NOT NULL;
  DROP INDEX IF EXISTS idx_agent_memory_pending_embedding_v1;
  DROP INDEX IF EXISTS idx_agent_memory_management_page;
  DROP INDEX IF EXISTS idx_agent_memory_management_page_v2;
  DROP INDEX IF EXISTS idx_agent_memory_cognitive_top;
  DROP INDEX IF EXISTS idx_agent_memory_cognitive_top_v2;
  DROP INDEX IF EXISTS idx_agent_memory_recall_importance_v4;
  DROP INDEX IF EXISTS idx_agent_memory_recent_activity;
  DROP INDEX IF EXISTS idx_agent_memory_recent_activity_v2;
  DROP INDEX IF EXISTS idx_agent_memory_archive_eligible;
  DROP INDEX IF EXISTS idx_agent_memory_archive_eligible_v2;
  DROP INDEX IF EXISTS idx_agent_memory_conflict_fairness;
  DROP INDEX IF EXISTS idx_agent_memory_conflict_fairness_v2;
  DROP INDEX IF EXISTS idx_agent_memory_conflict_target;
  DROP INDEX IF EXISTS idx_agent_memory_conflict_link_anomaly_v2;
  DROP INDEX IF EXISTS idx_agent_memory_recall_importance_v5;
  CREATE INDEX IF NOT EXISTS idx_agent_memory_conflict_state_anomaly_v2 ON agent_memory(agent_id, conflict_state, id) WHERE conflict_state IS NOT NULL;
  DROP INDEX IF EXISTS idx_agent_memory_embedding_queue;
  DROP INDEX IF EXISTS idx_agent_memory_lifecycle_maintenance;
  CREATE INDEX IF NOT EXISTS idx_agent_memory_active_recall ON agent_memory(agent_id, lifecycle_state, superseded_by, kind, created_at);
  CREATE INDEX IF NOT EXISTS idx_agent_memory_management_page_v3 ON agent_memory(agent_id, created_at DESC, id DESC) WHERE lifecycle_state != 'conflicted' AND superseded_by IS NULL AND kind NOT IN ('persona', 'working');
  CREATE INDEX IF NOT EXISTS idx_agent_memory_archive_eligible_v3 ON agent_memory(agent_id, COALESCE(last_accessed, created_at), created_at, id) WHERE lifecycle_state = 'active' AND superseded_by IS NULL AND conflict_state IS NULL AND is_anchor = 0 AND kind NOT IN ('persona', 'working');
  CREATE INDEX IF NOT EXISTS idx_agent_memory_cognitive_top_v3 ON agent_memory(agent_id, importance DESC, created_at DESC, id DESC) WHERE lifecycle_state = 'active' AND superseded_by IS NULL AND kind IN ('episodic', 'semantic', 'reflection');
  CREATE INDEX IF NOT EXISTS idx_agent_memory_conflict_fairness_v3 ON agent_memory(agent_id, COALESCE(last_consolidated_at, 0), created_at, id) WHERE lifecycle_state = 'conflicted' AND superseded_by IS NULL;
  CREATE INDEX IF NOT EXISTS idx_agent_memory_recent_activity_v3 ON agent_memory(agent_id, COALESCE(last_accessed, created_at) DESC) WHERE lifecycle_state != 'archived';
  CREATE INDEX IF NOT EXISTS idx_agent_memory_embedding_pending_agent_v2 ON agent_memory(agent_id, created_at, id) WHERE lifecycle_state = 'active' AND embedding_state = 'pending' AND superseded_by IS NULL AND kind NOT IN ('persona', 'working');
  CREATE INDEX IF NOT EXISTS idx_agent_memory_embedding_pending_global_v2 ON agent_memory(created_at, id, agent_id) WHERE lifecycle_state = 'active' AND embedding_state = 'pending' AND superseded_by IS NULL AND kind NOT IN ('persona', 'working');
  CREATE INDEX IF NOT EXISTS idx_agent_memory_conflict_target_v2 ON agent_memory(agent_id, lifecycle_state, conflict_with, id);
";

type ColumnConstraints = BTreeMap<String, (i64, Option<String>)>;

fn table_column_constraints(
    conn: &Connection,
    table: &str,
) -> Result<ColumnConstraints, ProductionSchemaError> {
    let mut statement = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|_| ProductionSchemaError::Finalize)?;
    statement
        .query_map([], |row| Ok((row.get(1)?, (row.get(3)?, row.get(4)?))))
        .map_err(|_| ProductionSchemaError::Finalize)?
        .collect::<rusqlite::Result<ColumnConstraints>>()
        .map_err(|_| ProductionSchemaError::Finalize)
}

fn require_column_constraint(
    columns: &ColumnConstraints,
    name: &str,
    default: &str,
) -> Result<(), ProductionSchemaError> {
    match columns.get(name) {
        Some((1, Some(value))) if value == default => Ok(()),
        _ => Err(ProductionSchemaError::Finalize),
    }
}

fn normalized_table_sql(conn: &Connection, table: &str) -> Result<String, ProductionSchemaError> {
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name=?1",
        [table],
        |row| row.get::<_, String>(0),
    )
    .map(|sql| sql.split_whitespace().collect::<Vec<_>>().join(" "))
    .map_err(|_| ProductionSchemaError::Finalize)
}

fn require_zero_rows(
    conn: &Connection,
    sql: &str,
    params: &[i64],
) -> Result<(), ProductionSchemaError> {
    let count: i64 = conn
        .query_row(sql, rusqlite::params_from_iter(params.iter()), |row| {
            row.get(0)
        })
        .map_err(|_| ProductionSchemaError::Finalize)?;
    if count == 0 {
        Ok(())
    } else {
        Err(ProductionSchemaError::Finalize)
    }
}

fn maintain_agent_memory_indexes(conn: &Connection) -> Result<(), ProductionSchemaError> {
    conn.execute_batch(AGENT_MEMORY_INDEX_MAINTENANCE_SQL)
        .map_err(|_| ProductionSchemaError::Finalize)
}

fn require_schema_object(
    conn: &Connection,
    object_type: &str,
    name: &str,
) -> Result<(), ProductionSchemaError> {
    let present: i64 = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type=?1 AND name=?2)",
            [object_type, name],
            |row| row.get(0),
        )
        .map_err(|_| ProductionSchemaError::Finalize)?;
    if present == 1 {
        Ok(())
    } else {
        Err(ProductionSchemaError::Finalize)
    }
}

struct ProductionFinalizer {
    catalog: &'static ProductionSchemaCatalog,
}

impl MigrationFinalizer for ProductionFinalizer {
    fn finalize(&self, tx: &Transaction<'_>, version: i64) -> rusqlite::Result<()> {
        if version == 23 {
            finalize_deepchat_sessions_v23(tx)?;
        }
        for owner in self.catalog.runtime_owners() {
            if owner.finalizers.contains_key(&version) {
                run_owner_finalizer(tx, owner.name, version)?;
            }
        }
        Ok(())
    }
}

fn trigger_sql(name: &str) -> Option<&'static str> {
    let frozen: &'static FrozenCatalog = {
        static FROZEN: OnceLock<FrozenCatalog> = OnceLock::new();
        FROZEN
            .get_or_init(|| serde_json::from_str(include_str!("production_catalog.json")).unwrap())
    };
    frozen.finalizer_sql.get(name).map(String::as_str)
}

pub(crate) fn install_triggers(tx: &Transaction<'_>, names: &[&str]) -> rusqlite::Result<()> {
    for name in names {
        if let Some(sql) = trigger_sql(name) {
            tx.execute_batch(&format!("DROP TRIGGER IF EXISTS {name};\n{sql}"))?;
        }
    }
    Ok(())
}

fn run_owner_finalizer(tx: &Transaction<'_>, owner: &str, version: i64) -> rusqlite::Result<()> {
    match (owner, version) {
        ("deepchat_pending_inputs", 67) => tx.execute_batch(
            "UPDATE deepchat_pending_inputs SET state='blocked', retry_required_at=COALESCE(retry_required_at, updated_at, created_at), blocking_json=NULL WHERE state='retry_required';",
        ),
        ("deepchat_usage_stats", 69) => finalize_usage_stats_v69(tx),
        ("live_delegations", 60) => install_triggers(
            tx,
            &[
                "trg_live_delegations_parent_insert",
                "trg_live_delegations_delete_children",
                "trg_live_delegation_sessions_delete_references",
                "trg_live_delegations_child_bind",
                "trg_live_delegations_child_rebind",
            ],
        ),
        ("live_delegation_turns", 60) => {
            install_triggers(tx, &["trg_live_delegation_turns_parent_insert"])
        }
        ("live_delegation_events", 60) => {
            install_triggers(tx, &["trg_live_delegation_events_parent_insert"])
        }
        ("agent_memory", 42) => {
            install_triggers(
                tx,
                &[
                    "agent_memory_legacy_status_bridge_ai",
                    "agent_memory_legacy_status_bridge_au",
                ],
            )?;
            let marker_exists: i64 = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_temp_master WHERE type='table' AND name='agent_memory_v42_added_columns')",
                [],
                |row| row.get(0),
            )?;
            if marker_exists == 0 {
                return Ok(());
            }
            let added_column_count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM agent_memory_v42_added_columns",
                [],
                |row| row.get(0),
            )?;
            let fts_meta_exists: i64 = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agent_memory_fts_meta')",
                [],
                |row| row.get(0),
            )?;
            if added_column_count == 2 && fts_meta_exists == 1 {
                tx.execute(
                    "UPDATE agent_memory_fts_meta
                     SET policy_version=3
                     WHERE key='agent_memory_fts' AND policy_version=2
                       AND mutation_generation=indexed_generation",
                    [],
                )?;
            }
            tx.execute_batch("DROP TABLE agent_memory_v42_migration_stats; DROP TABLE agent_memory_v42_added_columns;")
        }
        ("agent_memory", 46) => install_triggers(
            tx,
            &["agent_memory_temporal_bi_v1", "agent_memory_temporal_bu_v1"],
        ),
        ("agent_memory", 48 | 49) => install_triggers(
            tx,
            &["agent_memory_dirty_ai", "agent_memory_dirty_au", "agent_memory_dirty_ad"],
        ),
        ("agent_memory", 51) => install_triggers(
            tx,
            &["agent_memory_scope_bi_v1", "agent_memory_scope_bu_v1"],
        ),
        ("agent_memory", 52) => install_triggers(
            tx,
            &["agent_memory_clear_guard_bi_v1", "agent_memory_clear_guard_bu_v1"],
        ),
        _ => Ok(()),
    }
}

fn finalize_deepchat_sessions_v23(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    let present: i64 = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='deepchat_sessions')",
        [],
        |row| row.get(0),
    )?;
    if present == 0 {
        return Ok(());
    }
    let mut statement = tx.prepare("PRAGMA table_info(deepchat_sessions)")?;
    let existing = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    let columns = [
        ("system_prompt", "TEXT"),
        ("temperature", "REAL"),
        ("context_length", "INTEGER"),
        ("top_p", "REAL"),
        ("max_tokens", "INTEGER"),
        ("thinking_budget", "INTEGER"),
        ("reasoning_effort", "TEXT"),
        ("verbosity", "TEXT"),
        ("summary_text", "TEXT"),
        ("summary_cursor_order_seq", "INTEGER NOT NULL DEFAULT 1"),
        ("summary_updated_at", "INTEGER"),
        ("timeout_ms", "INTEGER"),
        ("force_interleaved_thinking_compat", "INTEGER"),
        ("reasoning_visibility", "TEXT"),
        ("image_generation_options_json", "TEXT"),
        ("video_generation_options_json", "TEXT"),
        ("memory_cursor_order_seq", "INTEGER"),
    ];
    for (name, declaration) in columns {
        if !existing.contains(name) {
            tx.execute_batch(&format!(
                "ALTER TABLE deepchat_sessions ADD COLUMN {name} {declaration};"
            ))?;
        }
    }
    Ok(())
}

fn has_category_aware_primary_key(tx: &Transaction<'_>) -> rusqlite::Result<bool> {
    let mut statement = tx.prepare("PRAGMA table_info(deepchat_usage_stats)")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
    })?;
    let columns = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(columns
        .iter()
        .any(|(name, pk)| name == "usage_id" && *pk == 1)
        && columns
            .iter()
            .any(|(name, pk)| name == "message_id" && *pk == 0)
        && columns.iter().any(|(name, _)| name == "usage_category"))
}

fn finalize_usage_stats_v69(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    if has_category_aware_primary_key(tx)? {
        return Ok(());
    }
    let sql = ProductionSchemaCatalog::frozen()
        .runtime_owners()
        .into_iter()
        .find(|owner| owner.name == "deepchat_usage_stats")
        .and_then(|owner| owner.migrations.get(&68))
        .expect("validated usage stats v68 migration")
        .replace("deepchat_usage_stats_v68", "deepchat_usage_stats_v69");
    tx.execute_batch(&sql)
}

impl From<MigrationError> for ProductionSchemaError {
    fn from(_: MigrationError) -> Self {
        Self::Migration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_error_classification_preserves_destructive_and_schema_reasons() {
        let destructive = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
            Some("database disk image is malformed".to_owned()),
        );
        assert_eq!(
            classify_initialization_error(destructive, ProductionSchemaError::Migration).class,
            ProductionInitializationClass::Destructive
        );

        let schema = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
            Some("no such column: hidden_identity".to_owned()),
        );
        assert_eq!(
            classify_initialization_error(schema, ProductionSchemaError::Migration).class,
            ProductionInitializationClass::Schema(SchemaErrorReason::MissingColumn)
        );
    }

    #[test]
    fn v42_finalizer_returns_safely_without_temp_marker() {
        let conn = Connection::open_in_memory().unwrap();
        ProductionSchemaCatalog::frozen()
            .create_static_schema(&conn)
            .unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        run_owner_finalizer(&tx, "agent_memory", 42).unwrap();
        tx.commit().unwrap();
    }

    #[test]
    fn v42_finalizer_uses_marker_then_cleans_temp_tables() {
        let conn = Connection::open_in_memory().unwrap();
        ProductionSchemaCatalog::frozen()
            .create_static_schema(&conn)
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE agent_memory_fts_meta(
               key TEXT PRIMARY KEY, policy_version INTEGER,
               mutation_generation INTEGER, indexed_generation INTEGER
             );
             INSERT INTO agent_memory_fts_meta VALUES('agent_memory_fts',2,4,4);
             CREATE TEMP TABLE agent_memory_v42_added_columns(name TEXT PRIMARY KEY);
             INSERT INTO agent_memory_v42_added_columns VALUES('lifecycle_state'),('embedding_state');
             CREATE TEMP TABLE agent_memory_v42_migration_stats(normalized_legacy_status_count INTEGER);
             INSERT INTO agent_memory_v42_migration_stats VALUES(0);",
        )
        .unwrap();
        let tx = conn.unchecked_transaction().unwrap();
        run_owner_finalizer(&tx, "agent_memory", 42).unwrap();
        tx.commit().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT policy_version FROM agent_memory_fts_meta WHERE key='agent_memory_fts'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            3
        );
        for name in [
            "agent_memory_v42_added_columns",
            "agent_memory_v42_migration_stats",
        ] {
            assert_eq!(
                conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_temp_master WHERE type='table' AND name=?1)",
                    [name],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                0
            );
        }
    }
}
