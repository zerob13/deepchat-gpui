//! Catalog-driven schema diagnosis and transactional repair.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, Transaction};
use thiserror::Error;

use crate::clock::Clock;
use crate::production_schema::{CatalogDefinition, ProductionSchemaCatalog, install_triggers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaIssueKind {
    MissingTable,
    MissingColumn,
    ColumnTypeMismatch,
    MissingIndex,
}
impl SchemaIssueKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingTable => "missing_table",
            Self::MissingColumn => "missing_column",
            Self::ColumnTypeMismatch => "column_type_mismatch",
            Self::MissingIndex => "missing_index",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaIssue {
    pub kind: SchemaIssueKind,
    pub table: String,
    pub name: String,
    pub repairable: bool,
    pub message: String,
    pub expected_type: Option<String>,
    pub actual_type: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaDiagnosis {
    pub checked_at: i64,
    pub is_healthy: bool,
    pub issues: Vec<SchemaIssue>,
    pub repairable_issues: Vec<SchemaIssue>,
    pub manual_issues: Vec<SchemaIssue>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaDiagnosisError {
    Read,
}

pub struct SchemaInspector<'a, C> {
    conn: &'a Connection,
    catalog: &'a [CatalogDefinition],
    clock: C,
}
impl<'a, C: Clock> SchemaInspector<'a, C> {
    pub fn new(conn: &'a Connection, catalog: &'a [CatalogDefinition], clock: C) -> Self {
        Self {
            conn,
            catalog,
            clock,
        }
    }
    pub fn diagnose(&self) -> Result<SchemaDiagnosis, SchemaDiagnosisError> {
        diagnose(self.conn, self.catalog, &self.clock)
    }
}

fn diagnose<C: Clock>(
    conn: &Connection,
    catalog: &[CatalogDefinition],
    clock: &C,
) -> Result<SchemaDiagnosis, SchemaDiagnosisError> {
    let snapshot = read_snapshot(conn)?;
    let mut issues = Vec::new();
    let mut emitted = BTreeSet::new();
    for table in catalog {
        let Some(actual) = snapshot.get(table.name) else {
            push_issue(
                &mut issues,
                &mut emitted,
                SchemaIssue {
                    kind: SchemaIssueKind::MissingTable,
                    table: table.name.into(),
                    name: table.name.into(),
                    repairable: true,
                    message: format!("Missing table \"{}\".", table.name),
                    expected_type: None,
                    actual_type: None,
                },
            );
            continue;
        };
        for column in &table.columns {
            match actual.columns.get(column.name) {
                None => push_issue(
                    &mut issues,
                    &mut emitted,
                    SchemaIssue {
                        kind: SchemaIssueKind::MissingColumn,
                        table: table.name.into(),
                        name: column.name.into(),
                        repairable: column.add_column_sql.is_some(),
                        message: format!("Missing column \"{}.{}\".", table.name, column.name),
                        expected_type: normalize_type(column.declared_type),
                        actual_type: None,
                    },
                ),
                Some(actual_type)
                    if column.check_type
                        && normalize_type(column.declared_type).is_some()
                        && (actual_type.is_none()
                            || actual_type != &normalize_type(column.declared_type)) =>
                {
                    let expected_type = normalize_type(column.declared_type);
                    push_issue(
                        &mut issues,
                        &mut emitted,
                        SchemaIssue {
                            kind: SchemaIssueKind::ColumnTypeMismatch,
                            table: table.name.into(),
                            name: column.name.into(),
                            repairable: false,
                            message: format!(
                                "Column \"{}.{}\" has type \"{}\", expected \"{}\".",
                                table.name,
                                column.name,
                                actual_type.as_deref().unwrap_or("null"),
                                expected_type.as_deref().unwrap_or("null")
                            ),
                            expected_type,
                            actual_type: actual_type.clone(),
                        },
                    );
                }
                _ => {}
            }
        }
        for index in &table.indexes {
            if !actual.indexes.contains(index.name) {
                push_issue(
                    &mut issues,
                    &mut emitted,
                    SchemaIssue {
                        kind: SchemaIssueKind::MissingIndex,
                        table: table.name.into(),
                        name: index.name.into(),
                        repairable: true,
                        message: format!(
                            "Missing index \"{}\" on table \"{}\".",
                            index.name, table.name
                        ),
                        expected_type: None,
                        actual_type: None,
                    },
                );
            }
        }
    }
    let repairable_issues = issues.iter().filter(|i| i.repairable).cloned().collect();
    let manual_issues = issues.iter().filter(|i| !i.repairable).cloned().collect();
    Ok(SchemaDiagnosis {
        checked_at: clock.now_millis(),
        is_healthy: issues.is_empty(),
        issues,
        repairable_issues,
        manual_issues,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseRepairStatus {
    Healthy,
    Repaired,
    ManualActionRequired,
}
impl DatabaseRepairStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Repaired => "repaired",
            Self::ManualActionRequired => "manual-action-required",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseRepairReport {
    pub started_at: i64,
    pub finished_at: i64,
    pub status: DatabaseRepairStatus,
    pub backup_path: Option<PathBuf>,
    pub diagnosis_before_repair: SchemaDiagnosis,
    pub diagnosis_after_repair: SchemaDiagnosis,
    pub repaired_issues: Vec<SchemaIssue>,
    pub remaining_issues: Vec<SchemaIssue>,
}

pub trait RepairFileSystem: Send + Sync {
    fn copy(&self, source: &Path, destination: &Path) -> Result<(), RepairFileSystemError>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepairFileSystemError;
#[derive(Debug, Default)]
pub struct StdRepairFileSystem;
impl RepairFileSystem for StdRepairFileSystem {
    fn copy(&self, source: &Path, destination: &Path) -> Result<(), RepairFileSystemError> {
        std::fs::copy(source, destination)
            .map(|_| ())
            .map_err(|_| RepairFileSystemError)
    }
}
#[derive(Error, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseRepairError {
    #[error("schema diagnosis failed")]
    Diagnosis,
    #[error("database checkpoint failed")]
    Checkpoint,
    #[error("repair backup is unavailable")]
    BackupUnavailable,
    #[error("repair backup copy failed")]
    BackupCopy,
    #[error("schema repair transaction failed")]
    Transaction,
    #[error("schema repair SQL failed")]
    Sql,
    #[error("schema repair hook failed")]
    Hook,
}
impl fmt::Debug for DatabaseRepairError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Diagnosis => "DatabaseRepairError::Diagnosis",
            Self::Checkpoint => "DatabaseRepairError::Checkpoint",
            Self::BackupUnavailable => "DatabaseRepairError::BackupUnavailable",
            Self::BackupCopy => "DatabaseRepairError::BackupCopy",
            Self::Transaction => "DatabaseRepairError::Transaction",
            Self::Sql => "DatabaseRepairError::Sql",
            Self::Hook => "DatabaseRepairError::Hook",
        })
    }
}

pub struct DatabaseRepairService<'a, C, F> {
    conn: &'a Connection,
    db_path: &'a Path,
    catalog: &'a [CatalogDefinition],
    clock: &'a C,
    file_system: &'a F,
}
impl<'a, C: Clock, F: RepairFileSystem> DatabaseRepairService<'a, C, F> {
    pub fn new(
        conn: &'a Connection,
        db_path: &'a Path,
        catalog: &'a [CatalogDefinition],
        clock: &'a C,
        file_system: &'a F,
    ) -> Self {
        Self {
            conn,
            db_path,
            catalog,
            clock,
            file_system,
        }
    }
    pub fn diagnose(&self) -> Result<SchemaDiagnosis, DatabaseRepairError> {
        diagnose(self.conn, self.catalog, self.clock).map_err(|_| DatabaseRepairError::Diagnosis)
    }
    pub fn repair(&self) -> Result<DatabaseRepairReport, DatabaseRepairError> {
        let started_at = self.clock.now_millis();
        let before = self.diagnose()?;
        if before.is_healthy {
            return Ok(self.noop(started_at, DatabaseRepairStatus::Healthy, before));
        }
        if before.repairable_issues.is_empty() {
            return Ok(self.noop(
                started_at,
                DatabaseRepairStatus::ManualActionRequired,
                before,
            ));
        }
        self.conn
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .map_err(|_| DatabaseRepairError::Checkpoint)?;
        if !self.db_path.is_file() {
            return Err(DatabaseRepairError::BackupUnavailable);
        }
        let backup_path = repair_backup_path(self.db_path, self.clock.now_millis())?;
        self.file_system
            .copy(self.db_path, &backup_path)
            .map_err(|_| DatabaseRepairError::BackupCopy)?;
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|_| DatabaseRepairError::Transaction)?;
        let repaired_keys = match apply_repairs(&tx, self.catalog, &before) {
            Ok(keys) => keys,
            Err(error) => {
                let _ = tx.rollback();
                return Err(error);
            }
        };
        tx.commit().map_err(|_| DatabaseRepairError::Transaction)?;
        let after = self.diagnose()?;
        let repaired_issues = before
            .issues
            .iter()
            .filter(|i| repaired_keys.contains(&issue_key(i)))
            .cloned()
            .collect::<Vec<_>>();
        let status = if after.is_healthy {
            if repaired_issues.is_empty() {
                DatabaseRepairStatus::Healthy
            } else {
                DatabaseRepairStatus::Repaired
            }
        } else {
            DatabaseRepairStatus::ManualActionRequired
        };
        Ok(DatabaseRepairReport {
            started_at,
            finished_at: self.clock.now_millis(),
            status,
            backup_path: Some(backup_path),
            diagnosis_before_repair: before,
            remaining_issues: after.issues.clone(),
            diagnosis_after_repair: after,
            repaired_issues,
        })
    }
    fn noop(
        &self,
        started_at: i64,
        status: DatabaseRepairStatus,
        diagnosis: SchemaDiagnosis,
    ) -> DatabaseRepairReport {
        DatabaseRepairReport {
            started_at,
            finished_at: self.clock.now_millis(),
            status,
            backup_path: None,
            diagnosis_before_repair: diagnosis.clone(),
            diagnosis_after_repair: diagnosis.clone(),
            repaired_issues: Vec::new(),
            remaining_issues: diagnosis.issues,
        }
    }
}

fn apply_repairs(
    tx: &Transaction<'_>,
    catalog: &[CatalogDefinition],
    diagnosis: &SchemaDiagnosis,
) -> Result<BTreeSet<String>, DatabaseRepairError> {
    let mut repaired = BTreeSet::new();
    let mut hook_tables = Vec::new();
    let mut added = BTreeMap::<&str, BTreeSet<&str>>::new();
    for table in catalog {
        let issues = diagnosis
            .issues
            .iter()
            .filter(|i| i.table == table.name)
            .collect::<Vec<_>>();
        if let Some(issue) = issues
            .iter()
            .find(|i| i.kind == SchemaIssueKind::MissingTable)
        {
            tx.execute_batch(table.create_sql)
                .map_err(|_| DatabaseRepairError::Sql)?;
            repaired.insert(issue_key(issue));
            if table.after_repair.is_some() {
                hook_tables.push(table);
            }
            continue;
        }
        for issue in &issues {
            if issue.kind != SchemaIssueKind::MissingColumn || !issue.repairable {
                continue;
            }
            let sql = table
                .columns
                .iter()
                .find(|c| c.name == issue.name)
                .and_then(|c| c.add_column_sql)
                .ok_or(DatabaseRepairError::Sql)?;
            tx.execute_batch(sql)
                .map_err(|_| DatabaseRepairError::Sql)?;
            added
                .entry(table.name)
                .or_default()
                .insert(issue.name.as_str());
            repaired.insert(issue_key(issue));
            if table.after_repair.is_some() && !hook_tables.iter().any(|t| t.name == table.name) {
                hook_tables.push(table);
            }
        }
        for issue in &issues {
            if issue.kind != SchemaIssueKind::MissingIndex {
                continue;
            }
            let sql = table
                .indexes
                .iter()
                .find(|i| i.name == issue.name)
                .ok_or(DatabaseRepairError::Sql)?
                .create_sql;
            tx.execute_batch(sql)
                .map_err(|_| DatabaseRepairError::Sql)?;
            repaired.insert(issue_key(issue));
        }
    }
    for table in hook_tables {
        run_hook(
            tx,
            table.after_repair.ok_or(DatabaseRepairError::Hook)?,
            &added.remove(table.name).unwrap_or_default(),
        )?;
    }
    Ok(repaired)
}

fn run_hook(
    tx: &Transaction<'_>,
    identity: &str,
    columns: &BTreeSet<&str>,
) -> Result<(), DatabaseRepairError> {
    let result = match identity {
        "new_environments.rebuildFromSessions" => rebuild_environments(tx),
        "deepchat_pending_inputs.normalizeRetryRequiredRows" if columns.contains("retry_required_at") => tx.execute_batch(
            "UPDATE deepchat_pending_inputs SET state='blocked',retry_required_at=COALESCE(retry_required_at,updated_at,created_at),blocking_json=NULL WHERE state='retry_required';"),
        "deepchat_pending_inputs.normalizeRetryRequiredRows" => Ok(()),
        "agent_memory.repairCanonicalStateAfterSchemaRepair" => repair_agent_memory(tx, columns),
        "agent_memory_audit.backfillMemoryRefIds" => tx.execute_batch(AGENT_MEMORY_AUDIT_BACKFILL_SQL),
        _ => return Err(DatabaseRepairError::Hook),
    };
    result.map_err(|_| DatabaseRepairError::Hook)
}

fn rebuild_environments(tx: &Transaction<'_>) -> rusqlite::Result<()> {
    tx.execute_batch("DELETE FROM new_environments; INSERT INTO new_environments(path,session_count,last_used_at)
WITH environment_usage AS (SELECT id session_id,project_dir path,updated_at activity_at FROM new_sessions WHERE is_draft=0 AND project_dir IS NOT NULL AND TRIM(project_dir)<>'' UNION ALL SELECT acp.conversation_id,acp.workdir,MAX(COALESCE(ns.updated_at,0),COALESCE(acp.updated_at,0)) FROM acp_sessions acp INNER JOIN new_sessions ns ON ns.id=acp.conversation_id WHERE ns.is_draft=0 AND (ns.project_dir IS NULL OR TRIM(ns.project_dir)='') AND acp.workdir IS NOT NULL AND TRIM(acp.workdir)<>''), normalized_usage AS (SELECT session_id,path,MAX(activity_at) activity_at FROM environment_usage GROUP BY session_id,path) SELECT path,COUNT(*),MAX(activity_at) FROM normalized_usage GROUP BY path;")
}

fn repair_agent_memory(tx: &Transaction<'_>, columns: &BTreeSet<&str>) -> rusqlite::Result<()> {
    let state = columns.contains("lifecycle_state") || columns.contains("embedding_state");
    if columns.contains("lifecycle_state") {
        tx.execute_batch(AGENT_MEMORY_LIFECYCLE_BACKFILL_SQL)?;
    }
    if columns.contains("embedding_state") {
        tx.execute_batch(AGENT_MEMORY_EMBEDDING_BACKFILL_SQL)?;
    }
    if state {
        tx.execute_batch(AGENT_MEMORY_SHADOW_RECONCILE_SQL)?;
        tx.execute_batch(AGENT_MEMORY_STATE_INDEX_SQL)?;
        let exists: i64 = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='agent_memory_fts_meta')", [], |r| r.get(0))?;
        if exists == 1 {
            tx.execute(
                "DELETE FROM agent_memory_fts_meta WHERE key='agent_memory_fts'",
                [],
            )?;
        }
        install_triggers(
            tx,
            &[
                "agent_memory_legacy_status_bridge_ai",
                "agent_memory_legacy_status_bridge_au",
            ],
        )?;
    }
    if columns.contains("scope_type") || columns.contains("scope_id") {
        install_triggers(
            tx,
            &["agent_memory_scope_bi_v1", "agent_memory_scope_bu_v1"],
        )?;
        tx.execute_batch(AGENT_MEMORY_SCOPE_INDEX_SQL)?;
    }
    if [
        "temporal_kind",
        "valid_from",
        "valid_until",
        "temporal_confidence",
        "temporal_precision",
        "temporal_timezone",
    ]
    .iter()
    .any(|c| columns.contains(c))
    {
        tx.execute_batch(AGENT_MEMORY_TEMPORAL_REPAIR_SQL)?;
        install_triggers(
            tx,
            &["agent_memory_temporal_bi_v1", "agent_memory_temporal_bu_v1"],
        )?;
    }
    if columns.contains("decision_revision") {
        tx.execute_batch(AGENT_MEMORY_LINEAGE_DIRTY_SQL)?;
        install_triggers(
            tx,
            &[
                "agent_memory_dirty_ai",
                "agent_memory_dirty_au",
                "agent_memory_dirty_ad",
            ],
        )?;
    }
    tx.execute_batch(AGENT_MEMORY_CLEAR_TABLE_SQL)?;
    install_triggers(
        tx,
        &[
            "agent_memory_clear_guard_bi_v1",
            "agent_memory_clear_guard_bu_v1",
        ],
    )
}

const AGENT_MEMORY_LIFECYCLE_BACKFILL_SQL: &str = "UPDATE agent_memory SET lifecycle_state=CASE WHEN status='archived' THEN 'archived' WHEN status='conflicted' THEN 'conflicted' ELSE 'active' END;";
const AGENT_MEMORY_EMBEDDING_BACKFILL_SQL: &str = "UPDATE agent_memory SET embedding_state=CASE WHEN kind IN ('persona','working') THEN 'not_applicable' WHEN status='embedded' THEN 'ready' WHEN status='error' THEN 'error' WHEN status='fts_only' THEN 'fts_only' WHEN status='pending_embedding' THEN 'pending' WHEN embedding_id IS NOT NULL AND embedding_dim IS NOT NULL AND embedding_dim>0 AND embedding_model IS NOT NULL AND length(embedding_model)>0 THEN 'ready' ELSE 'pending' END;";
const AGENT_MEMORY_SHADOW_RECONCILE_SQL: &str = "UPDATE agent_memory SET status=CASE WHEN lifecycle_state='archived' THEN 'archived' WHEN lifecycle_state='conflicted' THEN 'conflicted' WHEN embedding_state='ready' THEN 'embedded' WHEN embedding_state='error' THEN 'error' WHEN embedding_state IN ('fts_only','not_applicable') THEN 'fts_only' ELSE 'pending_embedding' END;";
const AGENT_MEMORY_STATE_INDEX_SQL: &str = "CREATE INDEX IF NOT EXISTS idx_agent_memory_agent_kind ON agent_memory(agent_id,kind,status); CREATE INDEX IF NOT EXISTS idx_agent_memory_agent_active ON agent_memory(agent_id,superseded_by); CREATE UNIQUE INDEX IF NOT EXISTS idx_agent_memory_provenance ON agent_memory(agent_id,provenance_key) WHERE provenance_key IS NOT NULL; DROP INDEX IF EXISTS idx_agent_memory_pending_embedding_v1; DROP INDEX IF EXISTS idx_agent_memory_management_page; DROP INDEX IF EXISTS idx_agent_memory_management_page_v2; DROP INDEX IF EXISTS idx_agent_memory_cognitive_top; DROP INDEX IF EXISTS idx_agent_memory_cognitive_top_v2; DROP INDEX IF EXISTS idx_agent_memory_recall_importance_v4; DROP INDEX IF EXISTS idx_agent_memory_recent_activity; DROP INDEX IF EXISTS idx_agent_memory_recent_activity_v2; DROP INDEX IF EXISTS idx_agent_memory_archive_eligible; DROP INDEX IF EXISTS idx_agent_memory_archive_eligible_v2; DROP INDEX IF EXISTS idx_agent_memory_conflict_fairness; DROP INDEX IF EXISTS idx_agent_memory_conflict_fairness_v2; DROP INDEX IF EXISTS idx_agent_memory_conflict_target; DROP INDEX IF EXISTS idx_agent_memory_conflict_link_anomaly_v2; DROP INDEX IF EXISTS idx_agent_memory_recall_importance_v5; CREATE INDEX IF NOT EXISTS idx_agent_memory_conflict_state_anomaly_v2 ON agent_memory(agent_id,conflict_state,id) WHERE conflict_state IS NOT NULL; DROP INDEX IF EXISTS idx_agent_memory_embedding_queue; DROP INDEX IF EXISTS idx_agent_memory_lifecycle_maintenance;
CREATE INDEX IF NOT EXISTS idx_agent_memory_active_recall ON agent_memory(agent_id,lifecycle_state,superseded_by,kind,created_at); CREATE INDEX IF NOT EXISTS idx_agent_memory_management_page_v3 ON agent_memory(agent_id,created_at DESC,id DESC) WHERE lifecycle_state!='conflicted' AND superseded_by IS NULL AND kind NOT IN ('persona','working'); CREATE INDEX IF NOT EXISTS idx_agent_memory_archive_eligible_v3 ON agent_memory(agent_id,COALESCE(last_accessed,created_at),created_at,id) WHERE lifecycle_state='active' AND superseded_by IS NULL AND conflict_state IS NULL AND is_anchor=0 AND kind NOT IN ('persona','working'); CREATE INDEX IF NOT EXISTS idx_agent_memory_cognitive_top_v3 ON agent_memory(agent_id,importance DESC,created_at DESC,id DESC) WHERE lifecycle_state='active' AND superseded_by IS NULL AND kind IN ('episodic','semantic','reflection'); CREATE INDEX IF NOT EXISTS idx_agent_memory_conflict_fairness_v3 ON agent_memory(agent_id,COALESCE(last_consolidated_at,0),created_at,id) WHERE lifecycle_state='conflicted' AND superseded_by IS NULL; CREATE INDEX IF NOT EXISTS idx_agent_memory_recent_activity_v3 ON agent_memory(agent_id,COALESCE(last_accessed,created_at) DESC) WHERE lifecycle_state!='archived'; CREATE INDEX IF NOT EXISTS idx_agent_memory_embedding_pending_agent_v2 ON agent_memory(agent_id,created_at,id) WHERE lifecycle_state='active' AND embedding_state='pending' AND superseded_by IS NULL AND kind NOT IN ('persona','working'); CREATE INDEX IF NOT EXISTS idx_agent_memory_embedding_pending_global_v2 ON agent_memory(created_at,id,agent_id) WHERE lifecycle_state='active' AND embedding_state='pending' AND superseded_by IS NULL AND kind NOT IN ('persona','working'); CREATE INDEX IF NOT EXISTS idx_agent_memory_conflict_target_v2 ON agent_memory(agent_id,lifecycle_state,conflict_with,id);";
const AGENT_MEMORY_SCOPE_INDEX_SQL: &str = "DROP INDEX IF EXISTS idx_agent_memory_recall_importance_v5; CREATE INDEX IF NOT EXISTS idx_agent_memory_recall_scope_v6 ON agent_memory(agent_id,scope_type,scope_id,importance DESC,created_at DESC,id ASC) WHERE lifecycle_state='active' AND superseded_by IS NULL AND kind NOT IN ('persona','working');";
const AGENT_MEMORY_TEMPORAL_REPAIR_SQL: &str = "DROP TRIGGER IF EXISTS agent_memory_temporal_bi_v1; DROP TRIGGER IF EXISTS agent_memory_temporal_bu_v1;
UPDATE agent_memory SET temporal_kind='atemporal',valid_from=NULL,valid_until=NULL,temporal_confidence=NULL,temporal_precision=NULL,temporal_timezone=NULL,lifecycle_state='archived',status='archived' WHERE ((valid_from IS NOT NULL AND valid_until IS NOT NULL AND valid_from>=valid_until) OR temporal_kind IS NULL OR temporal_kind NOT IN ('atemporal','state','event','plan','recurring') OR (temporal_confidence IS NOT NULL AND (temporal_confidence<0 OR temporal_confidence>1)) OR (temporal_precision IS NOT NULL AND temporal_precision NOT IN ('exact','day','week','month','quarter','year','unknown')) OR (temporal_timezone IS NOT NULL AND (length(temporal_timezone) NOT BETWEEN 1 AND 128 OR temporal_timezone!=trim(temporal_timezone))) OR (temporal_kind='atemporal' AND (valid_from IS NOT NULL OR valid_until IS NOT NULL OR temporal_confidence IS NOT NULL OR temporal_precision IS NOT NULL OR temporal_timezone IS NOT NULL)) OR (temporal_kind!='atemporal' AND (temporal_confidence IS NULL OR temporal_precision IS NULL OR temporal_timezone IS NULL))) AND kind NOT IN ('persona','working');
UPDATE agent_memory SET temporal_kind='atemporal',valid_from=NULL,valid_until=NULL,temporal_confidence=NULL,temporal_precision=NULL,temporal_timezone=NULL WHERE ((valid_from IS NOT NULL AND valid_until IS NOT NULL AND valid_from>=valid_until) OR temporal_kind IS NULL OR temporal_kind NOT IN ('atemporal','state','event','plan','recurring') OR (temporal_confidence IS NOT NULL AND (temporal_confidence<0 OR temporal_confidence>1)) OR (temporal_precision IS NOT NULL AND temporal_precision NOT IN ('exact','day','week','month','quarter','year','unknown')) OR (temporal_timezone IS NOT NULL AND (length(temporal_timezone) NOT BETWEEN 1 AND 128 OR temporal_timezone!=trim(temporal_timezone))) OR (temporal_kind='atemporal' AND (valid_from IS NOT NULL OR valid_until IS NOT NULL OR temporal_confidence IS NOT NULL OR temporal_precision IS NOT NULL OR temporal_timezone IS NOT NULL)) OR (temporal_kind!='atemporal' AND (temporal_confidence IS NULL OR temporal_precision IS NULL OR temporal_timezone IS NULL))) AND kind IN ('persona','working');";
const AGENT_MEMORY_LINEAGE_DIRTY_SQL: &str = "CREATE TABLE IF NOT EXISTS agent_memory_derivation(agent_id TEXT NOT NULL,parent_memory_id TEXT NOT NULL,child_memory_id TEXT NOT NULL,derivation_kind TEXT NOT NULL CHECK(derivation_kind IN ('merge','reflection','supersede','manual_edit')),created_at INTEGER NOT NULL,PRIMARY KEY(agent_id,parent_memory_id,child_memory_id,derivation_kind)) WITHOUT ROWID; CREATE INDEX IF NOT EXISTS idx_agent_memory_derivation_child_v1 ON agent_memory_derivation(agent_id,child_memory_id,created_at,parent_memory_id); DELETE FROM agent_memory_derivation WHERE parent_memory_id=child_memory_id; CREATE TABLE IF NOT EXISTS agent_memory_dirty(agent_id TEXT NOT NULL,memory_id TEXT NOT NULL,generation INTEGER NOT NULL CHECK(generation>=1),claim_revision INTEGER NOT NULL CHECK(claim_revision>=1),enqueued_at INTEGER NOT NULL CHECK(enqueued_at>=0),PRIMARY KEY(agent_id,memory_id)) WITHOUT ROWID; CREATE INDEX IF NOT EXISTS idx_agent_memory_dirty_order_v1 ON agent_memory_dirty(agent_id,enqueued_at,memory_id); INSERT INTO agent_memory_dirty(agent_id,memory_id,generation,claim_revision,enqueued_at) SELECT agent_id,id,1,max(1,decision_revision),max(0,COALESCE(last_accessed,created_at)) FROM agent_memory WHERE kind IN ('episodic','semantic','reflection') ON CONFLICT(agent_id,memory_id) DO NOTHING;";
const AGENT_MEMORY_CLEAR_TABLE_SQL: &str = "CREATE TABLE IF NOT EXISTS agent_memory_clear_job(agent_id TEXT PRIMARY KEY,cutoff_rowid INTEGER NOT NULL CHECK(cutoff_rowid>=0),created_at INTEGER NOT NULL CHECK(created_at>=0),removed_count INTEGER NOT NULL DEFAULT 0 CHECK(removed_count>=0),phase TEXT NOT NULL DEFAULT 'claims' CHECK(phase IN ('claims','vectors'))) WITHOUT ROWID;";
const AGENT_MEMORY_AUDIT_BACKFILL_SQL: &str = "UPDATE agent_memory_audit SET memory_ref_id=COALESCE(CASE WHEN json_valid(output_refs_json) AND json_type(output_refs_json,'$.memoryId')='text' THEN NULLIF(TRIM(CAST(json_extract(output_refs_json,'$.memoryId') AS TEXT)),'') END,CASE WHEN json_valid(input_refs_json) AND json_type(input_refs_json,'$.memoryId')='text' THEN NULLIF(TRIM(CAST(json_extract(input_refs_json,'$.memoryId') AS TEXT)),'') END) WHERE memory_ref_id IS NULL AND status='completed' AND ((event_type='memory/forget' AND actor_type='runtime') OR (event_type='memory/archive' AND actor_type='user') OR event_type='memory/restore') AND COALESCE(CASE WHEN json_valid(output_refs_json) AND json_type(output_refs_json,'$.memoryId')='text' THEN NULLIF(TRIM(CAST(json_extract(output_refs_json,'$.memoryId') AS TEXT)),'') END,CASE WHEN json_valid(input_refs_json) AND json_type(input_refs_json,'$.memoryId')='text' THEN NULLIF(TRIM(CAST(json_extract(input_refs_json,'$.memoryId') AS TEXT)),'') END) IS NOT NULL;";

struct SnapshotTable {
    columns: BTreeMap<String, Option<String>>,
    indexes: BTreeSet<String>,
}
fn read_snapshot(
    conn: &Connection,
) -> Result<BTreeMap<String, SnapshotTable>, SchemaDiagnosisError> {
    let tables = conn
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .map_err(|_| SchemaDiagnosisError::Read)?
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|_| SchemaDiagnosisError::Read)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| SchemaDiagnosisError::Read)?;
    let mut snapshot = BTreeMap::new();
    for table in tables {
        let mut columns = BTreeMap::new();
        let mut statement = conn
            .prepare(&format!("PRAGMA table_info({})", sqlite_quote(&table)))
            .map_err(|_| SchemaDiagnosisError::Read)?;
        for row in statement
            .query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, String>(2)?)))
            .map_err(|_| SchemaDiagnosisError::Read)?
        {
            let (name, ty) = row.map_err(|_| SchemaDiagnosisError::Read)?;
            columns.insert(name, normalize_type(Some(&ty)));
        }
        let indexes = conn.prepare("SELECT name FROM sqlite_master WHERE type='index' AND tbl_name=?1 AND name NOT LIKE 'sqlite_%'").map_err(|_| SchemaDiagnosisError::Read)?
            .query_map([&table], |r| r.get::<_, String>(0)).map_err(|_| SchemaDiagnosisError::Read)?.collect::<rusqlite::Result<BTreeSet<_>>>().map_err(|_| SchemaDiagnosisError::Read)?;
        snapshot.insert(table, SnapshotTable { columns, indexes });
    }
    Ok(snapshot)
}
fn sqlite_quote(identifier: &str) -> String {
    format!("'{}'", identifier.replace('\'', "''"))
}
fn normalize_type(value: Option<&str>) -> Option<String> {
    value
        .map(|v| v.trim().to_ascii_uppercase())
        .filter(|v| !v.is_empty())
}
fn issue_key(issue: &SchemaIssue) -> String {
    format!("{}:{}:{}", issue.kind.as_str(), issue.table, issue.name)
}
fn push_issue(issues: &mut Vec<SchemaIssue>, emitted: &mut BTreeSet<String>, issue: SchemaIssue) {
    if emitted.insert(issue_key(&issue)) {
        issues.push(issue);
    }
}
fn repair_backup_path(path: &Path, millis: i64) -> Result<PathBuf, DatabaseRepairError> {
    let stamp = utc_iso_millis(millis)
        .ok_or(DatabaseRepairError::BackupUnavailable)?
        .replace([':', '.'], "-");
    Ok(PathBuf::from(format!(
        "{}.{stamp}.repair.bak",
        path.display()
    )))
}
fn utc_iso_millis(millis: i64) -> Option<String> {
    if millis < 0 {
        return None;
    }
    let seconds = millis / 1000;
    let days = seconds / 86400;
    let sod = seconds % 86400;
    let z = days.checked_add(719468)?;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60,
        millis % 1000
    ))
}

pub fn startup_catalog() -> &'static [CatalogDefinition] {
    static STARTUP: std::sync::OnceLock<Vec<CatalogDefinition>> = std::sync::OnceLock::new();
    STARTUP.get_or_init(|| {
        ProductionSchemaCatalog::frozen()
            .startup_definitions()
            .into_iter()
            .cloned()
            .collect()
    })
}
