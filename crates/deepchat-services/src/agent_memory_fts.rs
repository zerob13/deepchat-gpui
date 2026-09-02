//! Agent-memory dynamic FTS mirror and bounded recall.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rusqlite::functions::FunctionFlags;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::fts::{FtsCapability, FtsTokenizer, SqliteCapabilityProbe, detect_capability};

pub const AGENT_MEMORY_FTS_SCHEMA_VERSION: i64 = 4;
pub const AGENT_MEMORY_FTS_POLICY_VERSION: i64 = 3;
pub const AGENT_MEMORY_FTS_RECOVERY_COOLDOWN_MS: i64 = 30_000;
const META_KEY: &str = "agent_memory_fts";
const MAX_CANDIDATES: usize = 1_000;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum AgentMemoryFtsError {
    #[error("agent memory search storage failed")]
    Storage,
    #[error("agent memory mutation input is invalid")]
    InvalidInput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    All,
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryScope {
    Agent,
    User(String),
    Project(String),
    Session(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentMemoryRecord {
    pub id: String,
    pub agent_id: String,
    pub kind: String,
    pub content: String,
    pub importance: f64,
    pub created_at: i64,
    pub scope_type: String,
    pub scope_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStrategy {
    FtsOnly,
    LikeFallback,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentMemorySearchResult {
    pub rows: Vec<AgentMemoryRecord>,
    pub strategy: SearchStrategy,
}

#[derive(Debug, Clone)]
pub struct AgentMemoryMutation<'a> {
    pub id: &'a str,
    pub agent_id: &'a str,
    pub kind: &'a str,
    pub content: &'a str,
    pub importance: f64,
    pub created_at: i64,
    pub lifecycle_state: &'a str,
    pub superseded_by: Option<&'a str>,
    pub scope_type: &'a str,
    pub scope_id: Option<&'a str>,
}

pub struct AgentMemoryFts<'conn> {
    conn: &'conn Connection,
    capability: Cell<Option<FtsCapability>>,
    ready: Cell<bool>,
    recovery_after: Cell<i64>,
    last_strategy: Cell<SearchStrategy>,
    internal_error: RefCell<Option<rusqlite::Error>>,
    now: fn() -> i64,
    #[cfg(test)]
    read_fault: Cell<Option<TestReadFault>>,
}

#[cfg(test)]
#[derive(Clone, Copy)]
enum TestReadFault {
    Transient,
    NonTransient,
}

impl<'conn> AgentMemoryFts<'conn> {
    pub fn new(conn: &'conn Connection) -> Result<Self, AgentMemoryFtsError> {
        Self::new_internal(conn, None, now_ms)
    }

    fn new_internal(
        conn: &'conn Connection,
        capability: Option<FtsCapability>,
        now: fn() -> i64,
    ) -> Result<Self, AgentMemoryFtsError> {
        let owner = Self {
            conn,
            capability: Cell::new(capability),
            ready: Cell::new(false),
            recovery_after: Cell::new(0),
            last_strategy: Cell::new(SearchStrategy::LikeFallback),
            internal_error: RefCell::new(None),
            now,
            #[cfg(test)]
            read_fault: Cell::new(None),
        };
        owner.register_scope_function()?;
        owner.ensure_index()?;
        Ok(owner)
    }

    fn register_scope_function(&self) -> Result<(), AgentMemoryFtsError> {
        self.conn
            .create_scalar_function(
                "agent_memory_fts_scope",
                1,
                FunctionFlags::SQLITE_DETERMINISTIC | FunctionFlags::SQLITE_INNOCUOUS,
                |ctx| {
                    let agent_id = ctx.get::<String>(0)?;
                    Ok(agent_fts_scope(&agent_id))
                },
            )
            .map_err(|error| self.fail(error))
    }

    fn capability(&self) -> FtsCapability {
        if let Some(capability) = self.capability.get() {
            return capability;
        }
        let capability = detect_capability(self.conn, &SqliteCapabilityProbe);
        self.capability.set(Some(capability));
        capability
    }

    pub fn capability_state(&self) -> FtsCapability {
        self.capability()
    }

    pub fn is_ready(&self) -> bool {
        self.ready.get()
    }

    pub fn last_strategy(&self) -> SearchStrategy {
        self.last_strategy.get()
    }

    pub fn ensure_index(&self) -> Result<(), AgentMemoryFtsError> {
        match self.capability() {
            FtsCapability::Available(FtsTokenizer::Trigram) => {}
            FtsCapability::Available(FtsTokenizer::Unicode61) | FtsCapability::Unavailable => {
                let _ = self.drop_index();
                self.ready.set(false);
                return Ok(());
            }
        }
        let result = self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS agent_memory_fts_meta(
               key TEXT PRIMARY KEY,
               schema_version INTEGER NOT NULL,
               policy_version INTEGER NOT NULL,
               tokenizer TEXT NOT NULL,
               mutation_generation INTEGER NOT NULL,
               indexed_generation INTEGER NOT NULL,
               updated_at INTEGER NOT NULL
             );
             DROP TRIGGER IF EXISTS agent_memory_fts_ai;
             DROP TRIGGER IF EXISTS agent_memory_fts_ad;
             DROP TRIGGER IF EXISTS agent_memory_fts_au;",
        );
        if let Err(error) = result {
            return Err(self.fail(error));
        }
        let meta = self.read_meta()?;
        let table_exists = table_exists(self.conn, "agent_memory_fts")?;
        let valid = table_exists
            && meta.as_ref().is_some_and(|meta| {
                meta.schema_version == AGENT_MEMORY_FTS_SCHEMA_VERSION
                    && meta.policy_version == AGENT_MEMORY_FTS_POLICY_VERSION
                    && meta.tokenizer == "trigram"
                    && meta.mutation_generation == meta.indexed_generation
            });
        if !valid {
            self.rebuild(meta.map_or(0, |meta| meta.mutation_generation.max(0)))?;
        }
        self.ready.set(true);
        Ok(())
    }

    fn rebuild(&self, generation: i64) -> Result<(), AgentMemoryFtsError> {
        self.conn
            .execute_batch(
                "SAVEPOINT agent_memory_fts_rebuild;
                 DROP TABLE IF EXISTS agent_memory_fts;
                 CREATE VIRTUAL TABLE agent_memory_fts USING fts5(
                   content,
                   agent_scope,
                   memory_id UNINDEXED,
                   tokenize='trigram'
                 );
                 INSERT INTO agent_memory_fts(rowid, content, agent_scope, memory_id)
                 SELECT rowid, content, agent_memory_fts_scope(CAST(agent_id AS TEXT)), id
                 FROM agent_memory
                 WHERE superseded_by IS NULL
                   AND lifecycle_state = 'active'
                   AND kind NOT IN ('persona', 'working');
                 RELEASE agent_memory_fts_rebuild;",
            )
            .map_err(|error| self.fail(error))?;
        self.write_meta(generation, generation)
    }

    fn drop_index(&self) -> Result<(), AgentMemoryFtsError> {
        self.conn
            .execute_batch(
                "DROP TRIGGER IF EXISTS agent_memory_fts_ai;
                 DROP TRIGGER IF EXISTS agent_memory_fts_ad;
                 DROP TRIGGER IF EXISTS agent_memory_fts_au;
                 DROP TABLE IF EXISTS agent_memory_fts;",
            )
            .map_err(|error| self.fail(error))
    }

    pub fn upsert(&self, row: &AgentMemoryMutation<'_>) -> Result<(), AgentMemoryFtsError> {
        validate_mutation(row)?;
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| self.fail(error))?;
        let result = (|| {
            upsert_authoritative(self.conn, row)?;
            self.maintain_after_mutation(row.id)?;
            Ok::<_, rusqlite::Error>(())
        })();
        match result {
            Ok(()) => self
                .conn
                .execute_batch("COMMIT")
                .map_err(|error| self.fail(error)),
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(self.fail(error))
            }
        }
    }

    /// Applies a set of authoritative upserts in one outer transaction while maintaining the
    /// rebuildable mirror behind nested savepoints.
    pub fn bulk_upsert(&self, rows: &[AgentMemoryMutation<'_>]) -> Result<(), AgentMemoryFtsError> {
        for row in rows {
            validate_mutation(row)?;
        }
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| self.fail(error))?;
        let result = (|| {
            for row in rows {
                upsert_authoritative(self.conn, row)?;
                self.maintain_after_mutation(row.id)?;
            }
            Ok::<_, rusqlite::Error>(())
        })();
        match result {
            Ok(()) => self
                .conn
                .execute_batch("COMMIT")
                .map_err(|error| self.fail(error)),
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(self.fail(error))
            }
        }
    }

    pub fn delete(&self, id: &str) -> Result<bool, AgentMemoryFtsError> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|error| self.fail(error))?;
        let result = self
            .conn
            .execute("DELETE FROM agent_memory WHERE id=?", [id]);
        let changes = match result {
            Ok(changes) => changes,
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                return Err(self.fail(error));
            }
        };
        if changes > 0
            && let Err(error) = self.maintain_after_mutation(id)
        {
            let _ = self.conn.execute_batch("ROLLBACK");
            return Err(self.fail(error));
        }
        self.conn
            .execute_batch("COMMIT")
            .map_err(|error| self.fail(error))?;
        Ok(changes == 1)
    }

    fn maintain_after_mutation(&self, id: &str) -> Result<(), rusqlite::Error> {
        // unicode61 and unavailable are permanent LIKE-only modes for this connection. They own no
        // derived Agent FTS generation, so authoritative mutations must not depend on FTS metadata.
        if self.capability() != FtsCapability::Available(FtsTokenizer::Trigram) {
            return Ok(());
        }
        let generation = self.mark_dirty()?;
        if !self.ready.get() {
            return Ok(());
        }
        let update = (|| {
            self.conn
                .execute_batch("SAVEPOINT agent_memory_fts_mutation")?;
            let outcome = (|| {
                self.conn
                    .execute("DELETE FROM agent_memory_fts WHERE memory_id=?", [id])?;
                self.conn.execute(
                    "INSERT INTO agent_memory_fts(rowid,content,agent_scope,memory_id)
                     SELECT rowid,content,agent_memory_fts_scope(CAST(agent_id AS TEXT)),id
                     FROM agent_memory WHERE id=? AND superseded_by IS NULL
                       AND lifecycle_state='active' AND kind NOT IN ('persona','working')",
                    [id],
                )?;
                Ok::<_, rusqlite::Error>(())
            })();
            match outcome {
                Ok(()) => self.conn.execute_batch("RELEASE agent_memory_fts_mutation"),
                Err(error) => {
                    let _ = self.conn.execute_batch(
                        "ROLLBACK TO agent_memory_fts_mutation; RELEASE agent_memory_fts_mutation",
                    );
                    Err(error)
                }
            }
        })();
        match update {
            Ok(()) => self.mark_indexed(generation),
            Err(error) => {
                self.ready.set(false);
                *self.internal_error.borrow_mut() = Some(error);
                Ok(())
            }
        }
    }

    fn mark_dirty(&self) -> rusqlite::Result<i64> {
        self.conn
            .query_row(
                "UPDATE agent_memory_fts_meta
                 SET mutation_generation=mutation_generation+1,updated_at=?1
                 WHERE key=?2 RETURNING mutation_generation",
                params![now_ms(), META_KEY],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(rusqlite::Error::QueryReturnedNoRows)
    }

    fn mark_indexed(&self, generation: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE agent_memory_fts_meta SET indexed_generation=?1,updated_at=?2
             WHERE key=?3 AND mutation_generation=?1",
            params![generation, now_ms(), META_KEY],
        )?;
        Ok(())
    }

    pub fn search(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
        match_mode: MatchMode,
        scopes: &[MemoryScope],
    ) -> Result<AgentMemorySearchResult, AgentMemoryFtsError> {
        self.recover_if_needed();
        let query = query.trim();
        if query.is_empty() || scopes.is_empty() {
            return Ok(self.finish(Vec::new(), self.strategy()));
        }
        let terms: Vec<&str> = query
            .split_whitespace()
            .filter(|term| !term.is_empty())
            .collect();
        if terms.is_empty() {
            return Ok(self.finish(Vec::new(), self.strategy()));
        }
        let limit = limit.clamp(1, MAX_CANDIDATES);
        let safe_fts = self.ready.get() && terms.iter().all(|term| term.chars().count() >= 3);
        if !safe_fts {
            let rows = self.search_like(agent_id, &terms, limit, match_mode, scopes)?;
            return Ok(self.finish(rows, SearchStrategy::LikeFallback));
        }
        #[cfg(test)]
        let forced_error = self.read_fault.take().map(|fault| match fault {
            TestReadFault::Transient => rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
                None,
            ),
            TestReadFault::NonTransient => rusqlite::Error::InvalidQuery,
        });
        #[cfg(not(test))]
        let forced_error: Option<rusqlite::Error> = None;
        let fts_result = forced_error.map_or_else(
            || self.search_fts(agent_id, &terms, limit, match_mode, scopes),
            Err,
        );
        match fts_result {
            Ok(rows) => Ok(self.finish(rows, SearchStrategy::FtsOnly)),
            Err(error) => {
                if !is_transient_fts_error(&error) {
                    self.ready.set(false);
                    self.recovery_after
                        .set((self.now)() + AGENT_MEMORY_FTS_RECOVERY_COOLDOWN_MS);
                    let _ = self.mark_dirty();
                }
                let rows = self.search_like(agent_id, &terms, limit, match_mode, scopes)?;
                Ok(self.finish(rows, SearchStrategy::LikeFallback))
            }
        }
    }

    fn finish(
        &self,
        rows: Vec<AgentMemoryRecord>,
        strategy: SearchStrategy,
    ) -> AgentMemorySearchResult {
        self.last_strategy.set(strategy);
        AgentMemorySearchResult { rows, strategy }
    }

    fn strategy(&self) -> SearchStrategy {
        if self.ready.get() {
            SearchStrategy::FtsOnly
        } else {
            SearchStrategy::LikeFallback
        }
    }

    fn search_fts(
        &self,
        agent_id: &str,
        terms: &[&str],
        limit: usize,
        match_mode: MatchMode,
        scopes: &[MemoryScope],
    ) -> rusqlite::Result<Vec<AgentMemoryRecord>> {
        let operator = if match_mode == MatchMode::Any {
            " OR "
        } else {
            " AND "
        };
        let content = terms
            .iter()
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(operator);
        let expression = format!(
            "content : ({content}) AND agent_scope : \"{}\"",
            agent_fts_scope(agent_id)
        );
        let (scope_sql, scope_params) = scope_predicate(scopes);
        let importance_candidate_limit = 800_usize.min(64_usize.max(limit.saturating_mul(8)));
        let sql = format!(
            "WITH lexical AS MATERIALIZED (
               SELECT am.rowid,bm25(agent_memory_fts,1.0,0.0) score,
                      am.importance,am.created_at,am.id
               FROM agent_memory_fts JOIN agent_memory am ON am.rowid=agent_memory_fts.rowid
               WHERE agent_memory_fts MATCH ? AND am.agent_id=?
                 AND am.superseded_by IS NULL AND am.lifecycle_state='active'
                 AND am.kind NOT IN ('persona','working') AND ({scope_sql})
               ORDER BY score ASC,am.importance DESC,am.created_at DESC,am.id ASC LIMIT ?
             ), importance_candidates AS MATERIALIZED (
               SELECT am.rowid,am.importance,am.created_at,am.id
               FROM agent_memory am
               WHERE am.agent_id=? AND am.superseded_by IS NULL AND am.lifecycle_state='active'
                 AND am.kind NOT IN ('persona','working') AND ({scope_sql})
               ORDER BY am.importance DESC,am.created_at DESC,am.id ASC LIMIT ?
             ), importance AS MATERIALIZED (
               SELECT candidate.rowid,candidate.importance,candidate.created_at,candidate.id
               FROM agent_memory_fts f JOIN importance_candidates candidate ON f.rowid=candidate.rowid
               WHERE agent_memory_fts MATCH ?
               ORDER BY candidate.importance DESC,candidate.created_at DESC,candidate.id ASC LIMIT ?
             ), combined AS (
               SELECT rowid,0 source_order,score,importance,created_at,id FROM lexical
               UNION ALL SELECT rowid,1,NULL,importance,created_at,id FROM importance
               WHERE rowid NOT IN (SELECT rowid FROM lexical)
             )
             SELECT am.id,am.agent_id,am.kind,am.content,am.importance,am.created_at,
                    am.scope_type,am.scope_id
             FROM combined JOIN agent_memory am ON am.rowid=combined.rowid
             WHERE am.agent_id=? AND am.superseded_by IS NULL AND am.lifecycle_state='active'
               AND am.kind NOT IN ('persona','working') AND ({scope_sql})
             ORDER BY source_order,score ASC,combined.importance DESC,
                      combined.created_at DESC,combined.id ASC"
        );
        let mut values: Vec<rusqlite::types::Value> =
            vec![expression.clone().into(), agent_id.to_owned().into()];
        values.extend(scope_params.iter().cloned().map(Into::into));
        values.push((limit as i64).into());
        values.push(agent_id.to_owned().into());
        values.extend(scope_params.iter().cloned().map(Into::into));
        values.push((importance_candidate_limit as i64).into());
        values.push(expression.into());
        values.push((limit as i64).into());
        values.push(agent_id.to_owned().into());
        values.extend(scope_params.iter().cloned().map(Into::into));
        query_memory_rows(self.conn, &sql, values)
    }

    fn search_like(
        &self,
        agent_id: &str,
        terms: &[&str],
        limit: usize,
        match_mode: MatchMode,
        scopes: &[MemoryScope],
    ) -> Result<Vec<AgentMemoryRecord>, AgentMemoryFtsError> {
        let operator = if match_mode == MatchMode::Any {
            " OR "
        } else {
            " AND "
        };
        let clauses = terms
            .iter()
            .map(|_| "content LIKE ? ESCAPE '\\'")
            .collect::<Vec<_>>()
            .join(operator);
        let (scope_sql, scope_params) = scope_predicate(scopes);
        let sql = format!(
            "SELECT id,agent_id,kind,content,importance,created_at,scope_type,scope_id
             FROM agent_memory WHERE agent_id=? AND superseded_by IS NULL
               AND lifecycle_state='active' AND kind NOT IN ('persona','working')
               AND ({scope_sql}) AND ({clauses})
             ORDER BY importance DESC,created_at DESC,id ASC LIMIT ?"
        );
        let mut values: Vec<rusqlite::types::Value> = vec![agent_id.to_owned().into()];
        values.extend(scope_params.into_iter().map(Into::into));
        values.extend(
            terms
                .iter()
                .map(|term| format!("%{}%", escape_like(term)).into()),
        );
        values.push((limit as i64).into());
        query_memory_rows(self.conn, &sql, values).map_err(|error| self.fail(error))
    }

    fn recover_if_needed(&self) {
        if self.ready.get()
            || self.capability.get() == Some(FtsCapability::Available(FtsTokenizer::Unicode61))
        {
            return;
        }
        let now = (self.now)();
        if now < self.recovery_after.get() {
            return;
        }
        self.recovery_after
            .set(now + AGENT_MEMORY_FTS_RECOVERY_COOLDOWN_MS);
        if self.ensure_index().is_ok() && self.ready.get() {
            self.recovery_after.set(0);
        }
    }

    fn read_meta(&self) -> Result<Option<FtsMeta>, AgentMemoryFtsError> {
        self.conn
            .query_row(
                "SELECT schema_version,policy_version,tokenizer,mutation_generation,indexed_generation
                 FROM agent_memory_fts_meta WHERE key=?",
                [META_KEY],
                |row| {
                    Ok(FtsMeta {
                        schema_version: row.get(0)?,
                        policy_version: row.get(1)?,
                        tokenizer: row.get(2)?,
                        mutation_generation: row.get(3)?,
                        indexed_generation: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(|error| self.fail(error))
    }

    fn write_meta(&self, mutation: i64, indexed: i64) -> Result<(), AgentMemoryFtsError> {
        self.conn
            .execute(
                "INSERT INTO agent_memory_fts_meta(key,schema_version,policy_version,tokenizer,
               mutation_generation,indexed_generation,updated_at)
             VALUES(?1,?2,?3,'trigram',?4,?5,?6)
             ON CONFLICT(key) DO UPDATE SET schema_version=excluded.schema_version,
               policy_version=excluded.policy_version,tokenizer=excluded.tokenizer,
               mutation_generation=excluded.mutation_generation,
               indexed_generation=excluded.indexed_generation,updated_at=excluded.updated_at",
                params![
                    META_KEY,
                    AGENT_MEMORY_FTS_SCHEMA_VERSION,
                    AGENT_MEMORY_FTS_POLICY_VERSION,
                    mutation,
                    indexed,
                    now_ms()
                ],
            )
            .map(|_| ())
            .map_err(|error| self.fail(error))
    }

    fn fail(&self, error: rusqlite::Error) -> AgentMemoryFtsError {
        *self.internal_error.borrow_mut() = Some(error);
        AgentMemoryFtsError::Storage
    }
}

#[derive(Debug)]
struct FtsMeta {
    schema_version: i64,
    policy_version: i64,
    tokenizer: String,
    mutation_generation: i64,
    indexed_generation: i64,
}

fn upsert_authoritative(conn: &Connection, row: &AgentMemoryMutation<'_>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO agent_memory(
           id, agent_id, user_scope, scope_type, scope_id, kind, content, importance,
           status, created_at, lifecycle_state, embedding_state, superseded_by
         ) VALUES(?1,?2,CASE WHEN ?9='user' THEN ?10 ELSE NULL END,?9,?10,?3,?4,?5,
                  'pending_embedding',?6,?7,'pending',?8)
         ON CONFLICT(id) DO UPDATE SET
           agent_id=excluded.agent_id, kind=excluded.kind, content=excluded.content,
           importance=excluded.importance, lifecycle_state=excluded.lifecycle_state,
           superseded_by=excluded.superseded_by, scope_type=excluded.scope_type,
           scope_id=excluded.scope_id, user_scope=excluded.user_scope",
        params![
            row.id,
            row.agent_id,
            row.kind,
            row.content,
            row.importance,
            row.created_at,
            row.lifecycle_state,
            row.superseded_by,
            row.scope_type,
            row.scope_id
        ],
    )?;
    Ok(())
}

fn validate_mutation(row: &AgentMemoryMutation<'_>) -> Result<(), AgentMemoryFtsError> {
    let scope_valid = match row.scope_type {
        "agent" => row.scope_id.is_none(),
        "user" | "project" | "session" => row.scope_id.is_some_and(|id| !id.trim().is_empty()),
        _ => false,
    };
    if row.id.is_empty()
        || row.agent_id.is_empty()
        || row.content.is_empty()
        || !matches!(row.lifecycle_state, "active" | "archived" | "conflicted")
        || !scope_valid
    {
        return Err(AgentMemoryFtsError::InvalidInput);
    }
    Ok(())
}

fn query_memory_rows(
    conn: &Connection,
    sql: &str,
    values: Vec<rusqlite::types::Value>,
) -> rusqlite::Result<Vec<AgentMemoryRecord>> {
    let mut statement = conn.prepare(sql)?;
    let rows = statement.query_map(params_from_iter(values), |row| {
        Ok(AgentMemoryRecord {
            id: row.get(0)?,
            agent_id: row.get(1)?,
            kind: row.get(2)?,
            content: row.get(3)?,
            importance: row.get(4)?,
            created_at: row.get(5)?,
            scope_type: row.get(6)?,
            scope_id: row.get(7)?,
        })
    })?;
    rows.collect()
}

fn scope_predicate(scopes: &[MemoryScope]) -> (String, Vec<String>) {
    let mut clauses = Vec::new();
    let mut params = Vec::new();
    for scope in scopes {
        match scope {
            MemoryScope::Agent => {
                clauses.push("(scope_type='agent' AND scope_id IS NULL)".to_owned())
            }
            MemoryScope::User(id) => {
                clauses.push("(scope_type='user' AND scope_id=?)".to_owned());
                params.push(id.clone());
            }
            MemoryScope::Project(id) => {
                clauses.push("(scope_type='project' AND scope_id=?)".to_owned());
                params.push(id.clone());
            }
            MemoryScope::Session(id) => {
                clauses.push("(scope_type='session' AND scope_id=?)".to_owned());
                params.push(id.clone());
            }
        }
    }
    (clauses.join(" OR "), params)
}

pub fn agent_fts_scope(agent_id: &str) -> String {
    let digest = Sha256::digest(agent_id.as_bytes());
    URL_SAFE_NO_PAD.encode(digest).chars().take(4).collect()
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn table_exists(conn: &Connection, name: &str) -> Result<bool, AgentMemoryFtsError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
        [name],
        |row| row.get(0),
    )
    .map_err(|_| AgentMemoryFtsError::Storage)
}

fn is_transient_fts_error(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(code.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}

pub fn recallable_ids(conn: &Connection) -> Result<HashSet<String>, AgentMemoryFtsError> {
    let mut statement = conn
        .prepare(
            "SELECT id FROM agent_memory WHERE superseded_by IS NULL AND lifecycle_state='active'
             AND kind NOT IN ('persona','working')",
        )
        .map_err(|_| AgentMemoryFtsError::Storage)?;
    statement
        .query_map([], |row| row.get(0))
        .map_err(|_| AgentMemoryFtsError::Storage)?
        .collect::<rusqlite::Result<HashSet<_>>>()
        .map_err(|_| AgentMemoryFtsError::Storage)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI64, Ordering};

    use super::*;

    static TEST_NOW: AtomicI64 = AtomicI64::new(1_000);

    fn test_now() -> i64 {
        TEST_NOW.load(Ordering::SeqCst)
    }

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE agent_memory(
               id TEXT PRIMARY KEY, agent_id TEXT NOT NULL, user_scope TEXT,
               scope_type TEXT NOT NULL, scope_id TEXT, kind TEXT NOT NULL,
               content TEXT NOT NULL, importance REAL NOT NULL, status TEXT NOT NULL,
               created_at INTEGER NOT NULL, lifecycle_state TEXT NOT NULL,
               embedding_state TEXT NOT NULL, superseded_by TEXT
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn unicode61_and_unavailable_owners_are_permanent_like_only() {
        for capability in [
            FtsCapability::Available(FtsTokenizer::Unicode61),
            FtsCapability::Unavailable,
        ] {
            let conn = connection();
            conn.execute_batch(
                "CREATE VIRTUAL TABLE agent_memory_fts USING fts5(content,agent_scope,memory_id UNINDEXED,tokenize='trigram');",
            )
            .unwrap();
            let owner = AgentMemoryFts::new_internal(&conn, Some(capability), test_now).unwrap();
            assert_eq!(owner.capability_state(), capability);
            assert!(!owner.is_ready());
            assert!(!table_exists(&conn, "agent_memory_fts").unwrap());
            owner
                .upsert(&AgentMemoryMutation {
                    id: "m",
                    agent_id: "a",
                    kind: "semantic",
                    content: "alpha recall",
                    importance: 0.5,
                    created_at: 1,
                    lifecycle_state: "active",
                    superseded_by: None,
                    scope_type: "agent",
                    scope_id: None,
                })
                .unwrap();
            assert_eq!(
                owner
                    .search("a", "alpha", 10, MatchMode::All, &[MemoryScope::Agent])
                    .unwrap(),
                AgentMemorySearchResult {
                    rows: vec![AgentMemoryRecord {
                        id: "m".into(),
                        agent_id: "a".into(),
                        kind: "semantic".into(),
                        content: "alpha recall".into(),
                        importance: 0.5,
                        created_at: 1,
                        scope_type: "agent".into(),
                        scope_id: None,
                    }],
                    strategy: SearchStrategy::LikeFallback,
                }
            );
            assert!(!table_exists(&conn, "agent_memory_fts").unwrap());
            assert_eq!(
                conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_memory_fts_meta'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
                0
            );
            assert!(owner.delete("m").unwrap());
        }
    }

    #[test]
    fn transient_read_failure_falls_back_without_dirtying_index() {
        let conn = connection();
        let owner = AgentMemoryFts::new_internal(
            &conn,
            Some(FtsCapability::Available(FtsTokenizer::Trigram)),
            test_now,
        )
        .unwrap();
        owner
            .upsert(&AgentMemoryMutation {
                id: "m",
                agent_id: "a",
                kind: "semantic",
                content: "alpha recall",
                importance: 0.5,
                created_at: 1,
                lifecycle_state: "active",
                superseded_by: None,
                scope_type: "agent",
                scope_id: None,
            })
            .unwrap();
        let before: (i64, i64) = conn
            .query_row(
                "SELECT mutation_generation,indexed_generation FROM agent_memory_fts_meta",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        owner.read_fault.set(Some(TestReadFault::Transient));
        let result = owner
            .search("a", "alpha", 10, MatchMode::All, &[MemoryScope::Agent])
            .unwrap();
        assert_eq!(result.strategy, SearchStrategy::LikeFallback);
        assert!(owner.is_ready());
        let after: (i64, i64) = conn
            .query_row(
                "SELECT mutation_generation,indexed_generation FROM agent_memory_fts_meta",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn non_transient_read_failure_obeys_recovery_cooldown() {
        TEST_NOW.store(10_000, Ordering::SeqCst);
        let conn = connection();
        let owner = AgentMemoryFts::new_internal(
            &conn,
            Some(FtsCapability::Available(FtsTokenizer::Trigram)),
            test_now,
        )
        .unwrap();
        owner
            .upsert(&AgentMemoryMutation {
                id: "m",
                agent_id: "a",
                kind: "semantic",
                content: "alpha recall",
                importance: 0.5,
                created_at: 1,
                lifecycle_state: "active",
                superseded_by: None,
                scope_type: "agent",
                scope_id: None,
            })
            .unwrap();
        owner.read_fault.set(Some(TestReadFault::NonTransient));
        assert_eq!(
            owner
                .search("a", "alpha", 10, MatchMode::All, &[MemoryScope::Agent])
                .unwrap()
                .strategy,
            SearchStrategy::LikeFallback
        );
        assert!(!owner.is_ready());
        assert_eq!(owner.recovery_after.get(), 40_000);
        TEST_NOW.store(39_999, Ordering::SeqCst);
        assert_eq!(
            owner
                .search("a", "alpha", 10, MatchMode::All, &[MemoryScope::Agent])
                .unwrap()
                .strategy,
            SearchStrategy::LikeFallback
        );
        assert!(!owner.is_ready());
        TEST_NOW.store(40_000, Ordering::SeqCst);
        assert_eq!(
            owner
                .search("a", "alpha", 10, MatchMode::All, &[MemoryScope::Agent])
                .unwrap()
                .strategy,
            SearchStrategy::FtsOnly
        );
        assert!(owner.is_ready());
    }
}
