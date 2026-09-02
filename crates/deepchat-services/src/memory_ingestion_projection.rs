//! Memory-ingestion projection derived directly from authoritative Tape entries.

use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::Value;
use thiserror::Error;

pub const MEMORY_INGESTION_PROJECTION_VERSION: i64 = 1;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum MemoryIngestionError {
    #[error("memory ingestion projection storage failed")]
    Storage,
    #[error("memory ingestion projection input is invalid")]
    InvalidInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TapeEntry {
    pub session_id: String,
    pub entry_id: i64,
    pub kind: String,
    pub name: Option<String>,
    pub payload_json: String,
    pub meta_json: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IngestionInput {
    session_id: String,
    message_id: String,
    order_seq: i64,
    entry_id: i64,
    role: String,
    content: String,
    status: String,
    had_tool_use: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestionRow {
    pub session_id: String,
    pub message_id: String,
    pub order_seq: i64,
    pub entry_id: i64,
    pub role: String,
    pub content: String,
    pub status: String,
    pub had_tool_use: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentRange {
    pub current: bool,
    pub max_entry_id: i64,
    pub rows: Vec<IngestionRow>,
}

pub struct MemoryIngestionProjection<'conn> {
    conn: &'conn Connection,
}

impl<'conn> MemoryIngestionProjection<'conn> {
    pub fn new(conn: &'conn Connection) -> Result<Self, MemoryIngestionError> {
        create_tables(conn)?;
        Ok(Self { conn })
    }

    pub fn apply_appended_entry(
        &self,
        row: &TapeEntry,
        previous_max: i64,
    ) -> Result<bool, MemoryIngestionError> {
        apply_entry(self.conn, row, previous_max)
    }

    pub fn replace_session(
        &self,
        session: &str,
        entries: &[TapeEntry],
        max: i64,
    ) -> Result<(), MemoryIngestionError> {
        if session.trim().is_empty()
            || max < 0
            || entries
                .iter()
                .any(|row| row.session_id != session || row.entry_id <= 0 || row.entry_id > max)
        {
            return Err(MemoryIngestionError::InvalidInput);
        }
        let rows = effective_ingestion_rows(session, entries)?;
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| MemoryIngestionError::Storage)?;
        let result = (|| {
            self.conn.execute(
                "DELETE FROM deepchat_memory_ingestion_projection WHERE session_id=?",
                [session],
            )?;
            for row in &rows {
                upsert_replace(self.conn, row)?;
            }
            write_meta(self.conn, session, max)
        })();
        finish(self.conn, result)
    }

    pub fn list_range(
        &self,
        session: &str,
        from: i64,
        to: i64,
    ) -> Result<Vec<IngestionRow>, MemoryIngestionError> {
        let mut stmt=self.conn.prepare("SELECT session_id,message_id,order_seq,entry_id,role,content,status,had_tool_use FROM deepchat_memory_ingestion_projection WHERE session_id=? AND order_seq>? AND order_seq<=? AND status IN ('sent','error') ORDER BY order_seq,message_id").map_err(|_|MemoryIngestionError::Storage)?;
        let rows = stmt
            .query_map(params![session, from, to], map_row)
            .map_err(|_| MemoryIngestionError::Storage)?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|_| MemoryIngestionError::Storage)
    }

    pub fn read_current_range(
        &self,
        session: &str,
        from: i64,
        to: i64,
    ) -> Result<CurrentRange, MemoryIngestionError> {
        let mut stmt=self.conn.prepare("WITH state AS(SELECT COALESCE((SELECT MAX(entry_id) FROM deepchat_tape_entries WHERE session_id=?1),0) tape_max,(SELECT max_entry_id FROM deepchat_memory_ingestion_projection_meta WHERE session_id=?1 AND projection_version=?2) projection_max) SELECT state.tape_max,state.projection_max,p.session_id,p.message_id,p.order_seq,p.entry_id,p.role,p.content,p.status,p.had_tool_use FROM state LEFT JOIN deepchat_memory_ingestion_projection p ON state.tape_max=state.projection_max AND p.session_id=?1 AND p.order_seq>?3 AND p.order_seq<=?4 AND p.status IN ('sent','error') ORDER BY p.order_seq,p.message_id").map_err(|_|MemoryIngestionError::Storage)?;
        let records = stmt
            .query_map(
                params![session, MEMORY_INGESTION_PROJECTION_VERSION, from, to],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, Option<i64>>(4)?,
                        r.get::<_, Option<i64>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                        r.get::<_, Option<String>>(8)?,
                        r.get::<_, Option<i64>>(9)?,
                    ))
                },
            )
            .map_err(|_| MemoryIngestionError::Storage)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| MemoryIngestionError::Storage)?;
        let Some(first) = records.first() else {
            return Ok(CurrentRange {
                current: false,
                max_entry_id: 0,
                rows: Vec::new(),
            });
        };
        let max_entry_id = first.0;
        let current = first.1 == Some(max_entry_id);
        if !current {
            return Ok(CurrentRange {
                current: false,
                max_entry_id,
                rows: Vec::new(),
            });
        }
        let rows = records
            .into_iter()
            .filter_map(
                |(_, _, session, message, order, entry, role, content, status, tool)| {
                    Some(IngestionRow {
                        session_id: session?,
                        message_id: message?,
                        order_seq: order?,
                        entry_id: entry?,
                        role: role?,
                        content: content?,
                        status: status?,
                        had_tool_use: tool? == 1,
                    })
                },
            )
            .collect();
        Ok(CurrentRange {
            current: true,
            max_entry_id,
            rows,
        })
    }

    pub fn invalidate_session(&self, session: &str) -> Result<(), MemoryIngestionError> {
        self.conn
            .execute(
                "DELETE FROM deepchat_memory_ingestion_projection_meta WHERE session_id=?",
                [session],
            )
            .map(|_| ())
            .map_err(|_| MemoryIngestionError::Storage)
    }
    pub fn delete_by_session(&self, session: &str) -> Result<(), MemoryIngestionError> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| MemoryIngestionError::Storage)?;
        let result = (|| {
            self.conn.execute(
                "DELETE FROM deepchat_memory_ingestion_projection WHERE session_id=?",
                [session],
            )?;
            self.conn.execute(
                "DELETE FROM deepchat_memory_ingestion_projection_meta WHERE session_id=?",
                [session],
            )?;
            Ok::<_, rusqlite::Error>(())
        })();
        finish(self.conn, result)
    }
    pub fn clear_all(&self) -> Result<(), MemoryIngestionError> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| MemoryIngestionError::Storage)?;
        let result = (|| {
            self.conn
                .execute("DELETE FROM deepchat_memory_ingestion_projection", [])?;
            self.conn
                .execute("DELETE FROM deepchat_memory_ingestion_projection_meta", [])?;
            Ok::<_, rusqlite::Error>(())
        })();
        finish(self.conn, result)
    }
}

/// Inserts one authoritative Tape entry and attempts its projection in the same outer transaction.
/// Projection failures invalidate derived metadata while the authoritative row still commits.
pub fn append_tape_entry_with_projection(
    conn: &mut Connection,
    row: &TapeEntry,
) -> Result<bool, MemoryIngestionError> {
    create_tables(conn)?;
    let tx = conn
        .transaction()
        .map_err(|_| MemoryIngestionError::Storage)?;
    let previous: i64 = tx
        .query_row(
            "SELECT COALESCE(MAX(entry_id),0) FROM deepchat_tape_entries WHERE session_id=?",
            [&row.session_id],
            |r| r.get(0),
        )
        .map_err(|_| MemoryIngestionError::Storage)?;
    tx.execute("INSERT INTO deepchat_tape_entries(session_id,entry_id,kind,name,payload_json,meta_json,created_at) VALUES(?,?,?,?,?,?,?)",params![row.session_id,row.entry_id,row.kind,row.name,row.payload_json,row.meta_json,row.created_at]).map_err(|_|MemoryIngestionError::Storage)?;
    let projected = match apply_entry_tx(&tx, row, previous) {
        Ok(value) => value,
        Err(_) => {
            tx.execute(
                "DELETE FROM deepchat_memory_ingestion_projection_meta WHERE session_id=?",
                [&row.session_id],
            )
            .map_err(|_| MemoryIngestionError::Storage)?;
            false
        }
    };
    tx.commit().map_err(|_| MemoryIngestionError::Storage)?;
    Ok(projected)
}

fn apply_entry(
    conn: &Connection,
    row: &TapeEntry,
    previous: i64,
) -> Result<bool, MemoryIngestionError> {
    apply_entry_db(conn, row, previous).map_err(|_| MemoryIngestionError::Storage)
}
fn apply_entry_tx(tx: &Transaction<'_>, row: &TapeEntry, previous: i64) -> rusqlite::Result<bool> {
    apply_entry_db(tx, row, previous)
}
fn apply_entry_db(conn: &Connection, row: &TapeEntry, previous: i64) -> rusqlite::Result<bool> {
    let meta:Option<(i64,i64)>=conn.query_row("SELECT projection_version,max_entry_id FROM deepchat_memory_ingestion_projection_meta WHERE session_id=?",[&row.session_id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let sequential = meta == Some((MEMORY_INGESTION_PROJECTION_VERSION, previous));
    let initialize = meta.is_none() && previous == 0;
    if !sequential && !initialize {
        conn.execute(
            "DELETE FROM deepchat_memory_ingestion_projection_meta WHERE session_id=?",
            [&row.session_id],
        )?;
        return Ok(false);
    }
    if row.kind == "context" {
        write_meta(conn, &row.session_id, row.entry_id)?;
        return Ok(true);
    }
    if let Some(message) = retraction_id(row) {
        conn.execute(
            "DELETE FROM deepchat_memory_ingestion_projection WHERE session_id=? AND message_id=?",
            params![row.session_id, message],
        )?;
        conn.execute(
            "DELETE FROM deepchat_memory_ingestion_projection_meta WHERE session_id=?",
            [&row.session_id],
        )?;
        return Ok(false);
    }
    if row.kind == "message" {
        if let Some(input) = message_input(row)
            && !retired_workflow(&input.1)
        {
            upsert_message(conn, &input.0)?;
        }
    } else if row.kind == "tool_call"
        && tool_terminal(row)
        && let Some(message) = tool_identity(row).map(|identity| identity.message_id)
    {
        conn.execute("UPDATE deepchat_memory_ingestion_projection SET had_tool_use=1 WHERE session_id=? AND message_id=?",params![row.session_id,message])?;
    }
    write_meta(conn, &row.session_id, row.entry_id)?;
    Ok(true)
}

fn message_input(row: &TapeEntry) -> Option<(IngestionInput, String)> {
    let payload = parse_object(&row.payload_json);
    let record = payload.get("record")?.as_object()?;
    let id = record.get("id")?.as_str()?.to_owned();
    record.get("sessionId")?.as_str()?;
    let order = record.get("orderSeq")?.as_i64()?;
    let role = record.get("role")?.as_str()?;
    if role != "user" && role != "assistant" {
        return None;
    }
    let content = record.get("content")?.as_str()?.to_owned();
    let status = record
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("sent");
    if status != "sent" && status != "error" {
        return None;
    }
    let metadata = record
        .get("metadata")
        .and_then(Value::as_str)
        .unwrap_or("{}")
        .to_owned();
    Some((
        IngestionInput {
            session_id: row.session_id.clone(),
            message_id: id,
            order_seq: order,
            entry_id: row.entry_id,
            role: role.to_owned(),
            content: content.clone(),
            status: status.to_owned(),
            had_tool_use: has_final_tool_use(role, status, &content),
        },
        metadata,
    ))
}
fn has_final_tool_use(role: &str, status: &str, content: &str) -> bool {
    if role != "assistant" || !(status == "sent" || status == "error") {
        return false;
    }
    let Ok(Value::Array(blocks)) = serde_json::from_str(content) else {
        return false;
    };
    let pending: std::collections::HashSet<String> = blocks
        .iter()
        .filter_map(|b| {
            let o = b.as_object()?;
            if o.get("type")?.as_str()? == "action"
                && matches!(
                    o.get("action_type")?.as_str()?,
                    "tool_call_permission" | "question_request"
                )
                && o.get("status")?.as_str()? == "pending"
            {
                o.get("tool_call")?
                    .get("id")?
                    .as_str()
                    .map(ToOwned::to_owned)
            } else {
                None
            }
        })
        .collect();
    blocks.iter().any(|b| {
        let Some(o) = b.as_object() else { return false };
        o.get("type").and_then(Value::as_str) == Some("tool_call")
            && matches!(
                o.get("status").and_then(Value::as_str),
                Some("success" | "error")
            )
            && o.get("tool_call")
                .and_then(|v| v.get("id"))
                .and_then(Value::as_str)
                .is_some_and(|id| !pending.contains(id))
    })
}
fn retired_workflow(metadata: &str) -> bool {
    parse_object(metadata)
        .get("messageType")
        .and_then(Value::as_str)
        == Some("workflow_result")
}
fn retraction_id(row: &TapeEntry) -> Option<String> {
    if row.kind != "event" || row.name.as_deref() != Some("message/retracted") {
        return None;
    }
    let p = parse_object(&row.payload_json);
    let data = match p.get("data")? {
        Value::String(s) => parse_object(s),
        Value::Object(o) => Value::Object(o.clone()),
        _ => return None,
    };
    data.get("messageId")?.as_str().map(ToOwned::to_owned)
}
fn tool_terminal(row: &TapeEntry) -> bool {
    matches!(
        parse_object(&row.meta_json)
            .get("status")
            .and_then(Value::as_str),
        Some("success" | "error")
    )
}
struct ToolIdentity {
    message_id: String,
    key: String,
}

fn tool_identity(row: &TapeEntry) -> Option<ToolIdentity> {
    if row.kind != "tool_call" && row.kind != "tool_result" {
        return None;
    }
    let payload = parse_object(&row.payload_json);
    let message_id = payload.get("messageId")?.as_str()?;
    if message_id.is_empty() {
        return None;
    }
    let tool_call_id = if row.kind == "tool_call" {
        parse_nested_object(payload.get("toolCall")?)
            .get("id")?
            .as_str()?
            .to_owned()
    } else {
        payload.get("toolCallId")?.as_str()?.to_owned()
    };
    if tool_call_id.is_empty() {
        return None;
    }
    Some(ToolIdentity {
        message_id: message_id.to_owned(),
        key: format!("{}:{message_id}:{tool_call_id}", row.kind),
    })
}

fn parse_nested_object(value: &Value) -> Value {
    match value {
        Value::String(raw) => parse_object(raw),
        Value::Object(object) => Value::Object(object.clone()),
        _ => serde_json::json!({}),
    }
}
fn parse_object(raw: &str) -> Value {
    serde_json::from_str::<Value>(raw)
        .ok()
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}))
}
fn effective_ingestion_rows(
    session: &str,
    entries: &[TapeEntry],
) -> Result<Vec<IngestionInput>, MemoryIngestionError> {
    let mut ordered: Vec<&TapeEntry> = entries.iter().collect();
    ordered.sort_by_key(|row| row.entry_id);
    let mut messages: HashMap<String, (IngestionInput, String)> = HashMap::new();
    let mut retracted = HashSet::new();
    let mut tools: HashMap<String, (i64, String)> = HashMap::new();

    for row in ordered {
        if let Some(message_id) = retraction_id(row) {
            messages.remove(&message_id);
            retracted.insert(message_id);
            continue;
        }
        if row.kind == "message" {
            let Some((input, metadata)) = message_input(row) else {
                continue;
            };
            let replace = messages
                .get(&input.message_id)
                .is_none_or(|(current, _)| input.entry_id > current.entry_id);
            if replace {
                retracted.remove(&input.message_id);
                messages.insert(input.message_id.clone(), (input, metadata));
            }
            continue;
        }
        if row.kind == "tool_call"
            && tool_terminal(row)
            && let Some(identity) = tool_identity(row)
        {
            let replace = tools
                .get(&identity.key)
                .is_none_or(|(entry_id, _)| row.entry_id > *entry_id);
            if replace {
                tools.insert(identity.key, (row.entry_id, identity.message_id));
            }
        }
    }

    let tool_messages: HashSet<String> = tools.into_values().map(|(_, id)| id).collect();
    let mut result: Vec<IngestionInput> = messages
        .into_values()
        .filter(|(row, metadata)| {
            !retracted.contains(&row.message_id) && !retired_workflow(metadata)
        })
        .map(|(mut row, _)| {
            row.session_id = session.to_owned();
            row.had_tool_use = tool_messages.contains(&row.message_id);
            row
        })
        .collect();
    result.sort_by(|left, right| {
        left.order_seq
            .cmp(&right.order_seq)
            .then_with(|| left.message_id.as_bytes().cmp(right.message_id.as_bytes()))
    });
    Ok(result)
}

fn valid_input(r: &IngestionInput) -> bool {
    !r.session_id.is_empty()
        && !r.message_id.is_empty()
        && r.entry_id > 0
        && matches!(r.role.as_str(), "user" | "assistant")
        && matches!(r.status.as_str(), "sent" | "error")
}
fn upsert_message(conn: &Connection, r: &IngestionInput) -> rusqlite::Result<()> {
    if !valid_input(r) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    conn.execute("INSERT INTO deepchat_memory_ingestion_projection VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(session_id,message_id) DO UPDATE SET order_seq=excluded.order_seq,entry_id=excluded.entry_id,role=excluded.role,content=excluded.content,status=excluded.status,had_tool_use=deepchat_memory_ingestion_projection.had_tool_use WHERE excluded.entry_id>deepchat_memory_ingestion_projection.entry_id",params![r.session_id,r.message_id,r.order_seq,r.entry_id,r.role,r.content,r.status,r.had_tool_use as i64])?;
    Ok(())
}
fn upsert_replace(conn: &Connection, r: &IngestionInput) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO deepchat_memory_ingestion_projection VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(session_id,message_id) DO UPDATE SET order_seq=excluded.order_seq,entry_id=excluded.entry_id,role=excluded.role,content=excluded.content,status=excluded.status,had_tool_use=excluded.had_tool_use",params![r.session_id,r.message_id,r.order_seq,r.entry_id,r.role,r.content,r.status,r.had_tool_use as i64])?;
    Ok(())
}
fn write_meta(conn: &Connection, session: &str, max: i64) -> rusqlite::Result<()> {
    conn.execute("INSERT INTO deepchat_memory_ingestion_projection_meta VALUES(?,?,?,?) ON CONFLICT(session_id) DO UPDATE SET projection_version=excluded.projection_version,max_entry_id=excluded.max_entry_id,updated_at=excluded.updated_at",params![session,MEMORY_INGESTION_PROJECTION_VERSION,max,now_ms()])?;
    Ok(())
}
fn create_tables(conn: &Connection) -> Result<(), MemoryIngestionError> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS deepchat_memory_ingestion_projection(session_id TEXT NOT NULL,message_id TEXT NOT NULL,order_seq INTEGER NOT NULL,entry_id INTEGER NOT NULL,role TEXT NOT NULL CHECK(role IN('user','assistant')),content TEXT NOT NULL,status TEXT NOT NULL CHECK(status IN('sent','error')),had_tool_use INTEGER NOT NULL DEFAULT 0 CHECK(had_tool_use IN(0,1)),PRIMARY KEY(session_id,message_id));CREATE INDEX IF NOT EXISTS idx_memory_ingestion_projection_range ON deepchat_memory_ingestion_projection(session_id,order_seq,message_id);CREATE TABLE IF NOT EXISTS deepchat_memory_ingestion_projection_meta(session_id TEXT PRIMARY KEY,projection_version INTEGER NOT NULL,max_entry_id INTEGER NOT NULL,updated_at INTEGER NOT NULL);").map_err(|_|MemoryIngestionError::Storage)
}
fn finish(conn: &Connection, result: rusqlite::Result<()>) -> Result<(), MemoryIngestionError> {
    match result {
        Ok(()) => conn
            .execute_batch("COMMIT")
            .map_err(|_| MemoryIngestionError::Storage),
        Err(_) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(MemoryIngestionError::Storage)
        }
    }
}
fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<IngestionRow> {
    Ok(IngestionRow {
        session_id: r.get(0)?,
        message_id: r.get(1)?,
        order_seq: r.get(2)?,
        entry_id: r.get(3)?,
        role: r.get(4)?,
        content: r.get(5)?,
        status: r.get(6)?,
        had_tool_use: r.get::<_, i64>(7)? == 1,
    })
}
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis().min(i64::MAX as u128) as i64)
}
