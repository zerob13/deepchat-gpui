//! Session-authorized Tape search projection and dynamic FTS lifecycle.

use std::collections::{BTreeMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde_json::Value;
use thiserror::Error;

use crate::fts::{FtsCapability, FtsTokenizer, SqliteCapabilityProbe, detect_capability};

pub const TAPE_SEARCH_PROJECTION_VERSION: i64 = 9;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum TapeSearchError {
    #[error("tape search projection storage failed")]
    Storage,
    #[error("tape search projection input is invalid")]
    InvalidInput,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TapeSearchInput {
    pub session_id: String,
    pub entry_id: i64,
    pub kind: String,
    pub name: Option<String>,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub source_seq: Option<i64>,
    pub search_text: String,
    pub summary_text: String,
    pub refs: Value,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TapeSearchRow {
    pub session_id: String,
    pub entry_id: i64,
    pub kind: String,
    pub name: Option<String>,
    pub source_type: Option<String>,
    pub source_id: Option<String>,
    pub source_seq: Option<i64>,
    pub search_text: String,
    pub summary_text: String,
    pub refs: Value,
    pub created_at: i64,
    pub score: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TapeReadSource<'a> {
    pub session_id: &'a str,
    pub max_entry_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectionMeta {
    pub projection_version: i64,
    pub max_entry_id: i64,
}

#[derive(Debug, Clone, Default)]
pub struct TapeSearchOptions {
    pub limit: Option<usize>,
    pub kinds: Vec<String>,
    pub start_created_at: Option<i64>,
    pub end_created_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MultiSourceResult {
    pub rows: Vec<TapeSearchRow>,
    pub covered_sources: Vec<(String, i64)>,
}

pub struct TapeSearchProjection<'conn> {
    conn: &'conn Connection,
    capability: FtsCapability,
    fts_ready: bool,
}

impl<'conn> TapeSearchProjection<'conn> {
    pub fn new(conn: &'conn Connection) -> Result<Self, TapeSearchError> {
        create_base_tables(conn)?;
        prune_invalid(conn)?;
        let capability = shared_tape_capability(conn)?;
        let mut owner = Self {
            conn,
            capability,
            fts_ready: false,
        };
        owner.ensure_fts();
        Ok(owner)
    }

    pub fn capability(&self) -> FtsCapability {
        self.capability
    }
    pub fn is_fts_ready(&self) -> bool {
        self.fts_ready
    }

    fn ensure_fts(&mut self) {
        let tokenizer = match self.capability {
            FtsCapability::Available(tokenizer) => tokenizer,
            FtsCapability::Unavailable => {
                self.fts_ready = false;
                return;
            }
        };
        let sql = match tokenizer {
            FtsTokenizer::Trigram => TAPE_FTS_TRIGRAM_SQL,
            FtsTokenizer::Unicode61 => TAPE_FTS_UNICODE61_SQL,
        };
        self.fts_ready = self.conn.execute_batch(sql).is_ok();
    }

    pub fn get_session_meta(
        &self,
        session_id: &str,
    ) -> Result<Option<ProjectionMeta>, TapeSearchError> {
        read_meta(
            self.conn,
            "deepchat_tape_search_projection_meta",
            session_id,
        )
    }

    pub fn is_current(&self, session_id: &str, max_entry_id: i64) -> Result<bool, TapeSearchError> {
        Ok(self.get_session_meta(session_id)?.is_some_and(|meta| {
            meta.projection_version == TAPE_SEARCH_PROJECTION_VERSION
                && meta.max_entry_id == max_entry_id
        }))
    }

    pub fn get_projected_entry_ids(&self, session_id: &str) -> Result<Vec<i64>, TapeSearchError> {
        let mut statement = self.conn.prepare(
            "SELECT entry_id FROM deepchat_tape_search_projection WHERE session_id=? ORDER BY entry_id"
        ).map_err(|_| TapeSearchError::Storage)?;
        statement
            .query_map([session_id], |row| row.get(0))
            .map_err(|_| TapeSearchError::Storage)?
            .collect::<rusqlite::Result<_>>()
            .map_err(|_| TapeSearchError::Storage)
    }

    pub fn append_session(
        &mut self,
        session_id: &str,
        rows: &[TapeSearchInput],
        max_entry_id: i64,
    ) -> Result<(), TapeSearchError> {
        validate_rows(session_id, rows, max_entry_id)?;
        self.transactional_write(false, session_id, rows, max_entry_id)
    }

    pub fn replace_session(
        &mut self,
        session_id: &str,
        rows: &[TapeSearchInput],
        max_entry_id: i64,
    ) -> Result<(), TapeSearchError> {
        validate_rows(session_id, rows, max_entry_id)?;
        self.transactional_write(true, session_id, rows, max_entry_id)
    }

    fn transactional_write(
        &mut self,
        replace: bool,
        session_id: &str,
        rows: &[TapeSearchInput],
        max_entry_id: i64,
    ) -> Result<(), TapeSearchError> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| TapeSearchError::Storage)?;
        let result = (|| {
            let previous_projection = if replace {
                None
            } else {
                read_meta_raw(
                    self.conn,
                    "deepchat_tape_search_projection_meta",
                    session_id,
                )?
            };
            if replace {
                self.conn.execute(
                    "DELETE FROM deepchat_tape_search_projection WHERE session_id=?",
                    [session_id],
                )?;
                self.conn.execute(
                    "DELETE FROM deepchat_tape_search_projection_meta WHERE session_id=?",
                    [session_id],
                )?;
                clear_session_fts(self.conn, session_id)?;
            }
            insert_projection_rows(self.conn, rows)?;
            if self.fts_ready {
                let prior = read_meta_raw(self.conn, "deepchat_tape_search_fts_meta", session_id)?;
                if !replace && prior.is_some() && prior == previous_projection {
                    insert_fts_rows(self.conn, rows)?;
                } else {
                    self.conn.execute(
                        "DELETE FROM deepchat_tape_search_fts WHERE session_id=?",
                        [session_id],
                    )?;
                    let all = projection_inputs(self.conn, session_id)?;
                    insert_fts_rows(self.conn, &all)?;
                }
                upsert_meta_raw(
                    self.conn,
                    "deepchat_tape_search_fts_meta",
                    session_id,
                    max_entry_id,
                )?;
            } else {
                clear_session_fts(self.conn, session_id)?;
            }
            upsert_meta_raw(
                self.conn,
                "deepchat_tape_search_projection_meta",
                session_id,
                max_entry_id,
            )?;
            Ok::<_, rusqlite::Error>(())
        })();
        match result {
            Ok(()) => self
                .conn
                .execute_batch("COMMIT")
                .map_err(|_| TapeSearchError::Storage),
            Err(_) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                self.fts_ready = false;
                Err(TapeSearchError::Storage)
            }
        }
    }

    pub fn get_by_entry_ids(
        &self,
        session_id: &str,
        entry_ids: &[i64],
    ) -> Result<Vec<TapeSearchRow>, TapeSearchError> {
        self.get_by_entry_ids_impl(session_id, entry_ids, None)
    }

    pub fn get_by_entry_ids_if_current(
        &self,
        session_id: &str,
        max_entry_id: i64,
        entry_ids: &[i64],
    ) -> Result<Vec<TapeSearchRow>, TapeSearchError> {
        if !self.is_current(session_id, max_entry_id)? {
            return Ok(Vec::new());
        }
        self.get_by_entry_ids_impl(session_id, entry_ids, Some(max_entry_id))
    }

    fn get_by_entry_ids_impl(
        &self,
        session_id: &str,
        ids: &[i64],
        _head: Option<i64>,
    ) -> Result<Vec<TapeSearchRow>, TapeSearchError> {
        let ids: Vec<i64> = ids
            .iter()
            .copied()
            .filter(|id| *id > 0)
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; ids.len()].join(",");
        let sql = format!(
            "SELECT session_id,entry_id,kind,name,source_type,source_id,source_seq,search_text,summary_text,refs_json,created_at,NULL FROM deepchat_tape_search_projection WHERE session_id=? AND entry_id IN ({placeholders}) ORDER BY entry_id"
        );
        let mut values: Vec<rusqlite::types::Value> = vec![session_id.to_owned().into()];
        values.extend(ids.into_iter().map(Into::into));
        query_rows(self.conn, &sql, values)
    }

    pub fn search(
        &mut self,
        session_id: &str,
        query: &str,
        options: &TapeSearchOptions,
    ) -> Result<Vec<TapeSearchRow>, TapeSearchError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        self.recover_session_fts(session_id);
        let limit = options.limit.unwrap_or(20).clamp(1, 100);
        let mut rows = Vec::new();
        if self.fts_ready {
            rows.extend(
                self.search_fts_single(session_id, query, options, limit)
                    .unwrap_or_default(),
            );
        }
        if rows.len() < limit {
            merge_unique(
                &mut rows,
                self.search_like_single(session_id, query, options, limit)?,
            );
        }
        rows.truncate(limit);
        Ok(rows)
    }

    pub fn search_sources_read_only(
        &self,
        sources: &[TapeReadSource<'_>],
        query: &str,
        options: &TapeSearchOptions,
    ) -> Result<MultiSourceResult, TapeSearchError> {
        let normalized = normalize_sources(sources);
        let covered = current_sources(
            self.conn,
            "deepchat_tape_search_projection_meta",
            &normalized,
        )?;
        let query = query.trim();
        if covered.is_empty() || query.is_empty() || covered.len() != normalized.len() {
            return Ok(MultiSourceResult {
                rows: Vec::new(),
                covered_sources: covered,
            });
        }
        let limit = options.limit.unwrap_or(20).clamp(1, 100);
        let mut rows = Vec::new();
        if self.fts_ready {
            let fts_covered =
                current_sources(self.conn, "deepchat_tape_search_fts_meta", &normalized)?;
            if fts_covered.len() == normalized.len() {
                rows.extend(
                    self.search_sources(&normalized, query, options, limit, true)
                        .unwrap_or_default(),
                );
            }
        }
        if rows.len() < limit {
            merge_unique(
                &mut rows,
                self.search_sources(&normalized, query, options, limit, false)?,
            );
        }
        rows.truncate(limit);
        Ok(MultiSourceResult {
            rows,
            covered_sources: covered,
        })
    }

    pub fn delete_by_session(&mut self, session_id: &str) -> Result<(), TapeSearchError> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| TapeSearchError::Storage)?;
        let result = (|| {
            self.conn.execute(
                "DELETE FROM deepchat_tape_search_projection WHERE session_id=?",
                [session_id],
            )?;
            self.conn.execute(
                "DELETE FROM deepchat_tape_search_projection_meta WHERE session_id=?",
                [session_id],
            )?;
            clear_session_fts(self.conn, session_id)
        })();
        finish_transaction(self.conn, result)
    }

    pub fn clear_all(&mut self) -> Result<(), TapeSearchError> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| TapeSearchError::Storage)?;
        let result = (|| {
            self.conn
                .execute("DELETE FROM deepchat_tape_search_projection", [])?;
            self.conn
                .execute("DELETE FROM deepchat_tape_search_projection_meta", [])?;
            self.conn
                .execute("DELETE FROM deepchat_tape_search_fts_meta", [])?;
            if table_exists_raw(self.conn, "deepchat_tape_search_fts")? {
                self.conn
                    .execute("DELETE FROM deepchat_tape_search_fts", [])?;
            }
            Ok::<_, rusqlite::Error>(())
        })();
        finish_transaction(self.conn, result)
    }

    fn recover_session_fts(&mut self, session_id: &str) {
        let Ok(Some(meta)) = self.get_session_meta(session_id) else {
            let _ = clear_session_fts(self.conn, session_id);
            return;
        };
        if self.fts_ready
            && read_meta_raw(self.conn, "deepchat_tape_search_fts_meta", session_id)
                .ok()
                .flatten()
                == Some(meta)
        {
            return;
        }
        if !self.fts_ready {
            self.ensure_fts();
        }
        if !self.fts_ready {
            return;
        }
        let Ok(rows) = projection_inputs(self.conn, session_id) else {
            return;
        };
        let _ = self.conn.execute_batch("BEGIN IMMEDIATE");
        let result = (|| {
            self.conn.execute(
                "DELETE FROM deepchat_tape_search_fts WHERE session_id=?",
                [session_id],
            )?;
            insert_fts_rows(self.conn, &rows)?;
            upsert_meta_raw(
                self.conn,
                "deepchat_tape_search_fts_meta",
                session_id,
                meta.max_entry_id,
            )
        })();
        if finish_transaction(self.conn, result).is_err() {
            self.fts_ready = false;
        }
    }

    fn search_fts_single(
        &mut self,
        session_id: &str,
        query: &str,
        options: &TapeSearchOptions,
        limit: usize,
    ) -> Result<Vec<TapeSearchRow>, TapeSearchError> {
        let match_query = fts_match(query);
        let (filters, mut filter_values) = filters(options, "p");
        let sql = format!(
            "SELECT p.session_id,p.entry_id,p.kind,p.name,p.source_type,p.source_id,p.source_seq,p.search_text,p.summary_text,p.refs_json,p.created_at,bm25(deepchat_tape_search_fts) FROM deepchat_tape_search_fts JOIN deepchat_tape_search_projection p ON p.session_id=deepchat_tape_search_fts.session_id AND p.entry_id=CAST(deepchat_tape_search_fts.entry_id AS INTEGER) AND p.search_text=deepchat_tape_search_fts.search_text WHERE deepchat_tape_search_fts MATCH ? AND deepchat_tape_search_fts.session_id=? AND p.session_id=? {filters} ORDER BY 12 ASC,p.entry_id DESC LIMIT ?"
        );
        let mut values = vec![
            match_query.into(),
            session_id.to_owned().into(),
            session_id.to_owned().into(),
        ];
        values.append(&mut filter_values);
        values.push((limit as i64).into());
        match query_rows(self.conn, &sql, values) {
            Ok(rows) => Ok(rows),
            Err(error) => {
                self.fts_ready = false;
                let _ = self
                    .conn
                    .execute_batch("DROP TABLE IF EXISTS deepchat_tape_search_fts");
                Err(error)
            }
        }
    }

    fn search_like_single(
        &self,
        session_id: &str,
        query: &str,
        options: &TapeSearchOptions,
        limit: usize,
    ) -> Result<Vec<TapeSearchRow>, TapeSearchError> {
        let terms: Vec<String> = query.split_whitespace().map(escape_like).collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let predicate=terms.iter().map(|_|"(search_text LIKE ? ESCAPE '\\' OR summary_text LIKE ? ESCAPE '\\' OR COALESCE(name,'') LIKE ? ESCAPE '\\')").collect::<Vec<_>>().join(" AND ");
        let (filters, mut filter_values) = filters(options, "");
        let sql = format!(
            "SELECT session_id,entry_id,kind,name,source_type,source_id,source_seq,search_text,summary_text,refs_json,created_at,NULL FROM deepchat_tape_search_projection WHERE session_id=? AND ({predicate}) {filters} ORDER BY entry_id DESC LIMIT ?"
        );
        let mut values = vec![session_id.to_owned().into()];
        for term in terms {
            let value = format!("%{term}%");
            values.extend([value.clone().into(), value.clone().into(), value.into()]);
        }
        values.append(&mut filter_values);
        values.push((limit as i64).into());
        query_rows(self.conn, &sql, values)
    }

    fn search_sources(
        &self,
        sources: &[(String, i64)],
        query: &str,
        options: &TapeSearchOptions,
        limit: usize,
        fts: bool,
    ) -> Result<Vec<TapeSearchRow>, TapeSearchError> {
        let source_json = serde_json::to_string(
            &sources
                .iter()
                .map(|(session, max)| serde_json::json!({"sessionId":session,"maxEntryId":max}))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| TapeSearchError::InvalidInput)?;
        let (filters, mut filter_values) = filters(options, "p");
        let (sql, mut values) = if fts {
            let sql = format!(
                "WITH authorized(session_id,max_entry_id) AS (SELECT json_extract(value,'$.sessionId'),CAST(json_extract(value,'$.maxEntryId') AS INTEGER) FROM json_each(?)) SELECT p.session_id,p.entry_id,p.kind,p.name,p.source_type,p.source_id,p.source_seq,p.search_text,p.summary_text,p.refs_json,p.created_at,bm25(deepchat_tape_search_fts) FROM deepchat_tape_search_fts JOIN deepchat_tape_search_projection p ON p.session_id=deepchat_tape_search_fts.session_id AND p.entry_id=CAST(deepchat_tape_search_fts.entry_id AS INTEGER) JOIN authorized a ON a.session_id=p.session_id AND p.entry_id<=a.max_entry_id WHERE deepchat_tape_search_fts MATCH ? {filters} ORDER BY 12 ASC,p.created_at DESC,p.session_id,p.entry_id DESC LIMIT ?"
            );
            (sql, vec![source_json.into(), fts_match(query).into()])
        } else {
            let terms: Vec<String> = query.split_whitespace().map(escape_like).collect();
            let predicate=terms.iter().map(|_|"(p.search_text LIKE ? ESCAPE '\\' OR p.summary_text LIKE ? ESCAPE '\\' OR COALESCE(p.name,'') LIKE ? ESCAPE '\\')").collect::<Vec<_>>().join(" AND ");
            let sql = format!(
                "WITH authorized(session_id,max_entry_id) AS (SELECT json_extract(value,'$.sessionId'),CAST(json_extract(value,'$.maxEntryId') AS INTEGER) FROM json_each(?)) SELECT p.session_id,p.entry_id,p.kind,p.name,p.source_type,p.source_id,p.source_seq,p.search_text,p.summary_text,p.refs_json,p.created_at,NULL FROM deepchat_tape_search_projection p JOIN authorized a ON a.session_id=p.session_id AND p.entry_id<=a.max_entry_id WHERE ({predicate}) {filters} ORDER BY p.created_at DESC,p.session_id,p.entry_id DESC LIMIT ?"
            );
            let mut values = vec![source_json.into()];
            for term in terms {
                let v = format!("%{term}%");
                values.extend([v.clone().into(), v.clone().into(), v.into()]);
            }
            (sql, values)
        };
        values.append(&mut filter_values);
        values.push((limit as i64).into());
        query_rows(self.conn, &sql, values)
    }
}

const BASE_SQL: &str = "CREATE TABLE IF NOT EXISTS deepchat_tape_search_projection(session_id TEXT NOT NULL,entry_id INTEGER NOT NULL,kind TEXT NOT NULL,name TEXT,source_type TEXT,source_id TEXT,source_seq INTEGER,search_text TEXT NOT NULL,summary_text TEXT NOT NULL,refs_json TEXT NOT NULL DEFAULT '{}',created_at INTEGER NOT NULL,PRIMARY KEY(session_id,entry_id));CREATE TABLE IF NOT EXISTS deepchat_tape_search_projection_meta(session_id TEXT PRIMARY KEY,projection_version INTEGER NOT NULL,max_entry_id INTEGER NOT NULL,updated_at INTEGER NOT NULL);CREATE TABLE IF NOT EXISTS deepchat_tape_search_fts_meta(session_id TEXT PRIMARY KEY,projection_version INTEGER NOT NULL,max_entry_id INTEGER NOT NULL,updated_at INTEGER NOT NULL);CREATE INDEX IF NOT EXISTS idx_deepchat_tape_search_projection_session_kind ON deepchat_tape_search_projection(session_id,kind,entry_id);CREATE INDEX IF NOT EXISTS idx_deepchat_tape_search_projection_session_created ON deepchat_tape_search_projection(session_id,created_at,entry_id);";
const TAPE_FTS_TRIGRAM_SQL: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS deepchat_tape_search_fts USING fts5(search_text,name,session_id UNINDEXED,entry_id UNINDEXED,kind UNINDEXED,source_type UNINDEXED,source_id UNINDEXED,source_seq UNINDEXED,summary_text UNINDEXED,refs_json UNINDEXED,created_at UNINDEXED,tokenize='trigram');";
const TAPE_FTS_UNICODE61_SQL: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS deepchat_tape_search_fts USING fts5(search_text,name,session_id UNINDEXED,entry_id UNINDEXED,kind UNINDEXED,source_type UNINDEXED,source_id UNINDEXED,source_seq UNINDEXED,summary_text UNINDEXED,refs_json UNINDEXED,created_at UNINDEXED,tokenize='unicode61');";

fn create_base_tables(conn: &Connection) -> Result<(), TapeSearchError> {
    conn.execute_batch(BASE_SQL)
        .map_err(|_| TapeSearchError::Storage)
}
fn shared_tape_capability(conn: &Connection) -> Result<FtsCapability, TapeSearchError> {
    shared_tape_capability_with(conn, &SqliteCapabilityProbe)
}

fn shared_tape_capability_with(
    conn: &Connection,
    probe: &dyn crate::fts::CapabilityProbe,
) -> Result<FtsCapability, TapeSearchError> {
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS deepchat_tape_fts_capability(tokenizer TEXT NOT NULL)",
    )
    .map_err(|_| TapeSearchError::Storage)?;
    let cached: Option<String> = conn
        .query_row(
            "SELECT tokenizer FROM temp.deepchat_tape_fts_capability LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(|_| TapeSearchError::Storage)?;
    if let Some(value) = cached {
        return Ok(match value.as_str() {
            "trigram" => FtsCapability::Available(FtsTokenizer::Trigram),
            "unicode61" => FtsCapability::Available(FtsTokenizer::Unicode61),
            _ => FtsCapability::Unavailable,
        });
    }
    let capability = detect_capability(conn, probe);
    let value = match capability {
        FtsCapability::Available(t) => t.sql_name(),
        FtsCapability::Unavailable => "unavailable",
    };
    conn.execute(
        "INSERT INTO temp.deepchat_tape_fts_capability VALUES(?)",
        [value],
    )
    .map_err(|_| TapeSearchError::Storage)?;
    Ok(capability)
}
fn validate_rows(session: &str, rows: &[TapeSearchInput], max: i64) -> Result<(), TapeSearchError> {
    if session.is_empty()
        || max < 0
        || rows
            .iter()
            .any(|r| r.session_id != session || r.entry_id <= 0 || !r.refs.is_object())
    {
        return Err(TapeSearchError::InvalidInput);
    }
    Ok(())
}
fn insert_projection_rows(conn: &Connection, rows: &[TapeSearchInput]) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO deepchat_tape_search_projection VALUES(?,?,?,?,?,?,?,?,?,?,?)",
    )?;
    for r in rows {
        stmt.execute(params![
            r.session_id,
            r.entry_id,
            r.kind,
            r.name,
            r.source_type,
            r.source_id,
            r.source_seq,
            r.search_text,
            r.summary_text,
            r.refs.to_string(),
            r.created_at
        ])?;
    }
    Ok(())
}
fn insert_fts_rows(conn: &Connection, rows: &[TapeSearchInput]) -> rusqlite::Result<()> {
    let mut stmt=conn.prepare("INSERT INTO deepchat_tape_search_fts(rowid,search_text,name,session_id,entry_id,kind,source_type,source_id,source_seq,summary_text,refs_json,created_at) VALUES((SELECT rowid FROM deepchat_tape_search_projection WHERE session_id=?1 AND entry_id=?2),?3,?4,?1,?2,?5,?6,?7,?8,?9,?10,?11)")?;
    for r in rows {
        conn.execute(
            "DELETE FROM deepchat_tape_search_fts WHERE session_id=? AND entry_id=?",
            params![r.session_id, r.entry_id],
        )?;
        stmt.execute(params![
            r.session_id,
            r.entry_id,
            r.search_text,
            r.name.as_deref().unwrap_or(""),
            r.kind,
            r.source_type,
            r.source_id,
            r.source_seq,
            r.summary_text,
            r.refs.to_string(),
            r.created_at
        ])?;
    }
    Ok(())
}
fn projection_inputs(conn: &Connection, session: &str) -> rusqlite::Result<Vec<TapeSearchInput>> {
    let mut stmt=conn.prepare("SELECT session_id,entry_id,kind,name,source_type,source_id,source_seq,search_text,summary_text,refs_json,created_at FROM deepchat_tape_search_projection WHERE session_id=? ORDER BY entry_id")?;
    let rows = stmt.query_map([session], |r| {
        Ok(TapeSearchInput {
            session_id: r.get(0)?,
            entry_id: r.get(1)?,
            kind: r.get(2)?,
            name: r.get(3)?,
            source_type: r.get(4)?,
            source_id: r.get(5)?,
            source_seq: r.get(6)?,
            search_text: r.get(7)?,
            summary_text: r.get(8)?,
            refs: parse_refs(&r.get::<_, String>(9)?),
            created_at: r.get(10)?,
        })
    })?;
    rows.collect()
}
fn parse_refs(raw: &str) -> Value {
    serde_json::from_str::<Value>(raw)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}))
}
fn upsert_meta_raw(
    conn: &Connection,
    table: &str,
    session: &str,
    max: i64,
) -> rusqlite::Result<()> {
    let sql = format!(
        "INSERT INTO {table}(session_id,projection_version,max_entry_id,updated_at) VALUES(?,?,?,?) ON CONFLICT(session_id) DO UPDATE SET projection_version=excluded.projection_version,max_entry_id=excluded.max_entry_id,updated_at=excluded.updated_at"
    );
    conn.execute(
        &sql,
        params![session, TAPE_SEARCH_PROJECTION_VERSION, max, now_ms()],
    )?;
    Ok(())
}
fn read_meta_raw(
    conn: &Connection,
    table: &str,
    session: &str,
) -> rusqlite::Result<Option<ProjectionMeta>> {
    let sql = format!("SELECT projection_version,max_entry_id FROM {table} WHERE session_id=?");
    conn.query_row(&sql, [session], |r| {
        Ok(ProjectionMeta {
            projection_version: r.get(0)?,
            max_entry_id: r.get(1)?,
        })
    })
    .optional()
}
fn read_meta(
    conn: &Connection,
    table: &str,
    session: &str,
) -> Result<Option<ProjectionMeta>, TapeSearchError> {
    read_meta_raw(conn, table, session).map_err(|_| TapeSearchError::Storage)
}
fn clear_session_fts(conn: &Connection, session: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM deepchat_tape_search_fts_meta WHERE session_id=?",
        [session],
    )?;
    if table_exists_raw(conn, "deepchat_tape_search_fts")? {
        conn.execute(
            "DELETE FROM deepchat_tape_search_fts WHERE session_id=?",
            [session],
        )?;
    }
    Ok(())
}
fn table_exists_raw(conn: &Connection, name: &str) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?)",
        [name],
        |r| r.get(0),
    )
}
fn prune_invalid(conn: &Connection) -> Result<(), TapeSearchError> {
    conn.execute_batch("BEGIN IMMEDIATE")
        .map_err(|_| TapeSearchError::Storage)?;
    let result = (|| {
        conn.execute("DELETE FROM deepchat_tape_search_projection WHERE NOT EXISTS(SELECT 1 FROM deepchat_tape_search_projection_meta m WHERE m.session_id=deepchat_tape_search_projection.session_id AND m.projection_version>=?)",[TAPE_SEARCH_PROJECTION_VERSION])?;
        conn.execute(
            "DELETE FROM deepchat_tape_search_projection_meta WHERE projection_version<?",
            [TAPE_SEARCH_PROJECTION_VERSION],
        )?;
        if table_exists_raw(conn, "deepchat_tape_search_fts")? {
            let cleanup = conn.execute("DELETE FROM deepchat_tape_search_fts WHERE NOT EXISTS(SELECT 1 FROM deepchat_tape_search_fts_meta m WHERE m.session_id=deepchat_tape_search_fts.session_id AND m.projection_version>=?) OR NOT EXISTS(SELECT 1 FROM deepchat_tape_search_projection_meta p WHERE p.session_id=deepchat_tape_search_fts.session_id AND p.projection_version>=?)",params![TAPE_SEARCH_PROJECTION_VERSION,TAPE_SEARCH_PROJECTION_VERSION]);
            if cleanup.is_err() {
                conn.execute_batch("DROP TABLE IF EXISTS deepchat_tape_search_fts")?;
                conn.execute("DELETE FROM deepchat_tape_search_fts_meta", [])?;
            }
        }
        conn.execute("DELETE FROM deepchat_tape_search_fts_meta WHERE projection_version<? OR NOT EXISTS(SELECT 1 FROM deepchat_tape_search_projection_meta p WHERE p.session_id=deepchat_tape_search_fts_meta.session_id AND p.projection_version>=?)",params![TAPE_SEARCH_PROJECTION_VERSION,TAPE_SEARCH_PROJECTION_VERSION])?;
        Ok::<_, rusqlite::Error>(())
    })();
    finish_transaction(conn, result)
}
fn finish_transaction(
    conn: &Connection,
    result: rusqlite::Result<()>,
) -> Result<(), TapeSearchError> {
    match result {
        Ok(()) => conn
            .execute_batch("COMMIT")
            .map_err(|_| TapeSearchError::Storage),
        Err(_) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(TapeSearchError::Storage)
        }
    }
}
fn query_rows(
    conn: &Connection,
    sql: &str,
    values: Vec<rusqlite::types::Value>,
) -> Result<Vec<TapeSearchRow>, TapeSearchError> {
    let mut stmt = conn.prepare(sql).map_err(|_| TapeSearchError::Storage)?;
    let rows = stmt
        .query_map(params_from_iter(values), |r| {
            Ok(TapeSearchRow {
                session_id: r.get(0)?,
                entry_id: r.get(1)?,
                kind: r.get(2)?,
                name: r.get(3)?,
                source_type: r.get(4)?,
                source_id: r.get(5)?,
                source_seq: r.get(6)?,
                search_text: r.get(7)?,
                summary_text: r.get(8)?,
                refs: parse_refs(&r.get::<_, String>(9)?),
                created_at: r.get(10)?,
                score: r.get(11)?,
            })
        })
        .map_err(|_| TapeSearchError::Storage)?;
    rows.collect::<rusqlite::Result<_>>()
        .map_err(|_| TapeSearchError::Storage)
}
fn normalize_sources(sources: &[TapeReadSource<'_>]) -> Vec<(String, i64)> {
    let mut max_by_session = BTreeMap::new();
    for source in sources {
        let session_id = source.session_id.trim();
        if session_id.is_empty() || source.max_entry_id < 0 {
            continue;
        }
        max_by_session
            .entry(session_id.to_owned())
            .and_modify(|max: &mut i64| *max = (*max).max(source.max_entry_id))
            .or_insert(source.max_entry_id);
    }
    max_by_session.into_iter().collect()
}
fn current_sources(
    conn: &Connection,
    table: &str,
    sources: &[(String, i64)],
) -> Result<Vec<(String, i64)>, TapeSearchError> {
    let mut result = Vec::new();
    for (source, max) in sources {
        if read_meta_raw(conn, table, source)
            .map_err(|_| TapeSearchError::Storage)?
            .is_some_and(|m| {
                m.projection_version == TAPE_SEARCH_PROJECTION_VERSION && m.max_entry_id == *max
            })
        {
            result.push((source.clone(), *max));
        }
    }
    Ok(result)
}
fn filters(options: &TapeSearchOptions, alias: &str) -> (String, Vec<rusqlite::types::Value>) {
    let col = |name: &str| {
        if alias.is_empty() {
            name.to_owned()
        } else {
            format!("{alias}.{name}")
        }
    };
    let mut clauses = Vec::new();
    let mut values = Vec::new();
    if !options.kinds.is_empty() {
        clauses.push(format!(
            "AND {} IN ({})",
            col("kind"),
            vec!["?"; options.kinds.len()].join(",")
        ));
        values.extend(options.kinds.iter().cloned().map(Into::into));
    }
    if let Some(v) = options.start_created_at {
        clauses.push(format!("AND {}>=?", col("created_at")));
        values.push(v.into());
    }
    if let Some(v) = options.end_created_at {
        clauses.push(format!("AND {}<=?", col("created_at")));
        values.push(v.into());
    }
    (clauses.join(" "), values)
}
fn merge_unique(target: &mut Vec<TapeSearchRow>, incoming: Vec<TapeSearchRow>) {
    let mut seen: HashSet<(String, i64)> = target
        .iter()
        .map(|r| (r.session_id.clone(), r.entry_id))
        .collect();
    for row in incoming {
        if seen.insert((row.session_id.clone(), row.entry_id)) {
            target.push(row)
        }
    }
}
fn fts_match(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
}
fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().min(i64::MAX as u128) as i64)
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    struct Probe {
        outcomes: [bool; 2],
        calls: RefCell<Vec<FtsTokenizer>>,
    }

    impl crate::fts::CapabilityProbe for Probe {
        fn probe(&self, _conn: &Connection, tokenizer: FtsTokenizer) -> bool {
            self.calls.borrow_mut().push(tokenizer);
            self.outcomes[self.calls.borrow().len() - 1]
        }
    }

    struct CountingProbe {
        calls: Cell<usize>,
    }

    impl crate::fts::CapabilityProbe for CountingProbe {
        fn probe(&self, _conn: &Connection, _tokenizer: FtsTokenizer) -> bool {
            self.calls.set(self.calls.get() + 1);
            true
        }
    }

    #[test]
    fn owner_capability_cache_is_connection_scoped_and_reopen_probes_again() {
        let conn = Connection::open_in_memory().unwrap();
        let unicode = Probe {
            outcomes: [false, true],
            calls: RefCell::new(Vec::new()),
        };
        assert_eq!(
            shared_tape_capability_with(&conn, &unicode).unwrap(),
            FtsCapability::Available(FtsTokenizer::Unicode61)
        );
        assert_eq!(
            *unicode.calls.borrow(),
            vec![FtsTokenizer::Trigram, FtsTokenizer::Unicode61]
        );
        let cached = CountingProbe {
            calls: Cell::new(0),
        };
        assert_eq!(
            shared_tape_capability_with(&conn, &cached).unwrap(),
            FtsCapability::Available(FtsTokenizer::Unicode61)
        );
        assert_eq!(cached.calls.get(), 0);
        drop(conn);

        let reopened = Connection::open_in_memory().unwrap();
        let fresh = CountingProbe {
            calls: Cell::new(0),
        };
        assert_eq!(
            shared_tape_capability_with(&reopened, &fresh).unwrap(),
            FtsCapability::Available(FtsTokenizer::Trigram)
        );
        assert_eq!(fresh.calls.get(), 1);
    }

    #[test]
    fn owner_capability_records_unavailable_after_strict_order() {
        let conn = Connection::open_in_memory().unwrap();
        let probe = Probe {
            outcomes: [false, false],
            calls: RefCell::new(Vec::new()),
        };
        assert_eq!(
            shared_tape_capability_with(&conn, &probe).unwrap(),
            FtsCapability::Unavailable
        );
        assert_eq!(
            *probe.calls.borrow(),
            vec![FtsTokenizer::Trigram, FtsTokenizer::Unicode61]
        );
    }

    #[test]
    fn owner_level_unicode61_creates_dynamic_index_and_unavailable_does_not() {
        let unicode_conn = Connection::open_in_memory().unwrap();
        create_base_tables(&unicode_conn).unwrap();
        let mut unicode = TapeSearchProjection {
            conn: &unicode_conn,
            capability: FtsCapability::Available(FtsTokenizer::Unicode61),
            fts_ready: false,
        };
        unicode.ensure_fts();
        assert!(unicode.fts_ready);
        let ddl: String = unicode_conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='deepchat_tape_search_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(ddl.contains("unicode61"));

        let unavailable_conn = Connection::open_in_memory().unwrap();
        create_base_tables(&unavailable_conn).unwrap();
        let mut unavailable = TapeSearchProjection {
            conn: &unavailable_conn,
            capability: FtsCapability::Unavailable,
            fts_ready: false,
        };
        unavailable.ensure_fts();
        assert!(!unavailable.fts_ready);
        assert!(!table_exists_raw(&unavailable_conn, "deepchat_tape_search_fts").unwrap());
    }
}
