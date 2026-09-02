use deepchat_services::agent_memory_fts::{
    AGENT_MEMORY_FTS_POLICY_VERSION, AGENT_MEMORY_FTS_SCHEMA_VERSION, AgentMemoryFts,
    AgentMemoryMutation, MatchMode, MemoryScope, SearchStrategy, agent_fts_scope,
};
use deepchat_services::connection::open_database;
use deepchat_services::fts::{FtsCapability, FtsTokenizer};
use deepchat_services::memory_ingestion_projection::{
    MemoryIngestionProjection, TapeEntry, append_tape_entry_with_projection,
};
use deepchat_services::production_schema::{
    EMPTY_MIGRATION_VERSIONS, EXPECTED_CATALOG_DEFINITIONS, EXPECTED_PHYSICAL_OWNERS,
    EXPECTED_RUNTIME_OWNERS, EXPECTED_STARTUP_DEFINITIONS, PRODUCTION_SCHEMA_VERSION,
    ProductionSchemaCatalog,
};
use deepchat_services::sqlite_copy::{
    SQLITE_COPY_EXCLUDED_OBJECTS, should_exclude_from_sqlite_copy,
};
use deepchat_services::tape_search_projection::{
    TAPE_SEARCH_PROJECTION_VERSION, TapeReadSource, TapeSearchInput, TapeSearchOptions,
    TapeSearchProjection,
};
use rusqlite::Connection;
use serde_json::json;

fn schema_connection() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    ProductionSchemaCatalog::frozen()
        .create_static_schema(&conn)
        .unwrap();
    conn
}

fn memory<'a>(
    id: &'a str,
    agent: &'a str,
    kind: &'a str,
    content: &'a str,
    created: i64,
) -> AgentMemoryMutation<'a> {
    AgentMemoryMutation {
        id,
        agent_id: agent,
        kind,
        content,
        importance: 0.5,
        created_at: created,
        lifecycle_state: "active",
        superseded_by: None,
        scope_type: "agent",
        scope_id: None,
    }
}

fn tape(session: &str, entry: i64, text: &str, kind: &str) -> TapeSearchInput {
    TapeSearchInput {
        session_id: session.to_owned(),
        entry_id: entry,
        kind: kind.to_owned(),
        name: Some("event/name".to_owned()),
        source_type: Some("runtime_event".to_owned()),
        source_id: Some("source".to_owned()),
        source_seq: Some(entry),
        search_text: text.to_owned(),
        summary_text: format!("summary {text}"),
        refs: json!({"entry": entry}),
        created_at: entry * 100,
    }
}

#[test]
fn dynamic_fts_is_not_static_and_global_topology_stays_frozen() {
    let catalog = ProductionSchemaCatalog::frozen();
    assert_eq!(catalog.definitions().len(), EXPECTED_CATALOG_DEFINITIONS);
    assert_eq!(catalog.physical_owners().len(), EXPECTED_PHYSICAL_OWNERS);
    assert_eq!(catalog.runtime_owners().len(), EXPECTED_RUNTIME_OWNERS);
    assert_eq!(
        catalog.startup_definitions().len(),
        EXPECTED_STARTUP_DEFINITIONS
    );
    assert_eq!(EMPTY_MIGRATION_VERSIONS.len(), 19);
    assert_eq!(PRODUCTION_SCHEMA_VERSION, 69);
    assert!(catalog.definitions().iter().all(|definition| {
        !definition.create_sql.contains("CREATE VIRTUAL TABLE")
            && !definition.create_sql.contains("agent_memory_fts USING")
            && !definition
                .create_sql
                .contains("deepchat_tape_search_fts USING")
    }));
}

#[test]
fn agent_fts_meta_backfill_policy_scope_and_recall_are_exact() {
    let conn = schema_connection();
    let owner = AgentMemoryFts::new(&conn).unwrap();
    assert_eq!(
        owner.capability_state(),
        FtsCapability::Available(FtsTokenizer::Trigram)
    );
    owner
        .upsert(&memory(
            "a",
            "agent-a",
            "semantic",
            "Alpha 中文本 recall",
            1,
        ))
        .unwrap();
    owner
        .upsert(&memory("b", "agent-b", "semantic", "Alpha hidden", 2))
        .unwrap();
    owner
        .upsert(&memory("p", "agent-a", "persona", "Alpha persona", 3))
        .unwrap();
    let meta: (i64, i64, String, i64, i64) = conn.query_row(
        "SELECT schema_version,policy_version,tokenizer,mutation_generation,indexed_generation FROM agent_memory_fts_meta WHERE key='agent_memory_fts'",
        [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
    assert_eq!(meta.0, AGENT_MEMORY_FTS_SCHEMA_VERSION);
    assert_eq!(meta.1, AGENT_MEMORY_FTS_POLICY_VERSION);
    assert_eq!(meta.2, "trigram");
    assert_eq!(meta.3, meta.4);
    assert_eq!(agent_fts_scope("agent-a").len(), 4);
    let result = owner
        .search(
            "agent-a",
            "Alpha recall",
            20,
            MatchMode::All,
            &[MemoryScope::Agent],
        )
        .unwrap();
    assert_eq!(result.strategy, SearchStrategy::FtsOnly);
    assert_eq!(
        result
            .rows
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        vec!["a"]
    );
    let short = owner
        .search("agent-a", "中", 20, MatchMode::All, &[MemoryScope::Agent])
        .unwrap();
    assert_eq!(short.strategy, SearchStrategy::LikeFallback);
    assert_eq!(short.rows[0].id, "a");
    let escaped = owner
        .search(
            "agent-a",
            "Alpha\" OR hidden",
            20,
            MatchMode::Any,
            &[MemoryScope::Agent],
        )
        .unwrap();
    assert!(escaped.rows.iter().all(|row| row.agent_id == "agent-a"));
    assert!(
        owner
            .search("agent-a", "   ", 20, MatchMode::All, &[MemoryScope::Agent])
            .unwrap()
            .rows
            .is_empty()
    );
}

#[test]
fn agent_mirror_failure_commits_authoritative_row_and_rebuilds_dirty_index() {
    let conn = schema_connection();
    {
        let owner = AgentMemoryFts::new(&conn).unwrap();
        owner
            .upsert(&memory("before", "agent", "semantic", "before mirror", 1))
            .unwrap();
        conn.execute_batch("DROP TABLE agent_memory_fts").unwrap();
        owner
            .upsert(&memory("after", "agent", "semantic", "after mirror", 2))
            .unwrap();
        assert!(!owner.is_ready());
        let authoritative: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM agent_memory WHERE id='after'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(authoritative, 1);
        let generations: (i64, i64) = conn
            .query_row(
                "SELECT mutation_generation,indexed_generation FROM agent_memory_fts_meta",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!(generations.0 > generations.1);
    }
    let owner = AgentMemoryFts::new(&conn).unwrap();
    let rebuilt = owner
        .search(
            "agent",
            "after mirror",
            10,
            MatchMode::All,
            &[MemoryScope::Agent],
        )
        .unwrap();
    assert_eq!(rebuilt.rows[0].id, "after");
}

#[test]
fn agent_stale_meta_and_repair_invalidation_rebuild_on_open() {
    let conn = schema_connection();
    {
        let owner = AgentMemoryFts::new(&conn).unwrap();
        owner
            .upsert(&memory("a", "agent", "semantic", "stale rebuild", 1))
            .unwrap();
    }
    conn.execute(
        "UPDATE agent_memory_fts_meta SET policy_version=1,indexed_generation=-1",
        [],
    )
    .unwrap();
    let owner = AgentMemoryFts::new(&conn).unwrap();
    assert!(owner.is_ready());
    let meta: (i64, i64) = conn
        .query_row(
            "SELECT policy_version,indexed_generation FROM agent_memory_fts_meta",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(meta.0, AGENT_MEMORY_FTS_POLICY_VERSION);
    assert!(meta.1 >= 0);
    conn.execute(
        "DELETE FROM agent_memory_fts_meta WHERE key='agent_memory_fts'",
        [],
    )
    .unwrap();
    drop(owner);
    assert!(AgentMemoryFts::new(&conn).unwrap().is_ready());
}

#[test]
fn agent_all_stale_metadata_variants_rebuild_authoritative_mirror() {
    let conn = schema_connection();
    AgentMemoryFts::new(&conn)
        .unwrap()
        .upsert(&memory("stale", "agent", "semantic", "stale authority", 1))
        .unwrap();

    for mutation in [
        "DELETE FROM agent_memory_fts_meta WHERE key='agent_memory_fts'",
        "UPDATE agent_memory_fts_meta SET schema_version=0",
        "UPDATE agent_memory_fts_meta SET policy_version=0",
        "UPDATE agent_memory_fts_meta SET tokenizer='unicode61'",
        "UPDATE agent_memory_fts_meta SET mutation_generation=indexed_generation+1",
    ] {
        conn.execute_batch(mutation).unwrap();
        let owner = AgentMemoryFts::new(&conn).unwrap();
        let meta: (i64, i64, String, i64, i64) = conn
            .query_row(
                "SELECT schema_version,policy_version,tokenizer,mutation_generation,indexed_generation FROM agent_memory_fts_meta WHERE key='agent_memory_fts'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(meta.0, AGENT_MEMORY_FTS_SCHEMA_VERSION);
        assert_eq!(meta.1, AGENT_MEMORY_FTS_POLICY_VERSION);
        assert_eq!(meta.2, "trigram");
        assert_eq!(meta.3, meta.4);
        assert_eq!(
            owner
                .search(
                    "agent",
                    "stale authority",
                    10,
                    MatchMode::All,
                    &[MemoryScope::Agent],
                )
                .unwrap()
                .rows[0]
                .id,
            "stale"
        );
    }

    conn.execute_batch("DROP TABLE agent_memory_fts").unwrap();
    let owner = AgentMemoryFts::new(&conn).unwrap();
    assert!(owner.is_ready());
    assert_eq!(
        owner
            .search(
                "agent",
                "stale authority",
                10,
                MatchMode::All,
                &[MemoryScope::Agent],
            )
            .unwrap()
            .rows[0]
            .id,
        "stale"
    );
}

#[test]
fn tape_projection_surface_currentness_filters_and_multi_source_authority() {
    let conn = schema_connection();
    let mut owner = TapeSearchProjection::new(&conn).unwrap();
    assert!(owner.is_fts_ready());
    owner
        .replace_session(
            "s1",
            &[
                tape("s1", 1, "alpha first", "message"),
                tape("s1", 2, "alpha second", "event"),
            ],
            2,
        )
        .unwrap();
    owner
        .replace_session("s2", &[tape("s2", 1, "alpha other", "message")], 1)
        .unwrap();
    assert!(owner.is_current("s1", 2).unwrap());
    assert_eq!(owner.get_projected_entry_ids("s1").unwrap(), vec![1, 2]);
    assert_eq!(owner.get_by_entry_ids("s1", &[2, 1, 2]).unwrap().len(), 2);
    assert!(
        owner
            .get_by_entry_ids_if_current("s1", 1, &[1])
            .unwrap()
            .is_empty()
    );
    let options = TapeSearchOptions {
        limit: Some(1000),
        kinds: vec!["message".into()],
        start_created_at: None,
        end_created_at: Some(150),
    };
    let rows = owner.search("s1", "alpha", &options).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].entry_id, 1);
    conn.execute("UPDATE deepchat_tape_search_projection SET refs_json='bad' WHERE session_id='s1' AND entry_id=1",[]).unwrap();
    assert_eq!(
        owner.get_by_entry_ids("s1", &[1]).unwrap()[0].refs,
        json!({})
    );
    let sources = [
        TapeReadSource {
            session_id: "s1",
            max_entry_id: 2,
        },
        TapeReadSource {
            session_id: "s2",
            max_entry_id: 1,
        },
    ];
    let result = owner
        .search_sources_read_only(&sources, "alpha", &TapeSearchOptions::default())
        .unwrap();
    assert_eq!(result.covered_sources.len(), 2);
    assert_eq!(result.rows.len(), 3);
    let stale = [
        TapeReadSource {
            session_id: "s1",
            max_entry_id: 1,
        },
        TapeReadSource {
            session_id: "s2",
            max_entry_id: 1,
        },
    ];
    let result = owner
        .search_sources_read_only(&stale, "alpha", &TapeSearchOptions::default())
        .unwrap();
    assert!(result.rows.is_empty());
    assert_eq!(result.covered_sources.len(), 1);
    owner.delete_by_session("s2").unwrap();
    assert!(owner.get_projected_entry_ids("s2").unwrap().is_empty());
    owner.clear_all().unwrap();
    assert!(owner.get_projected_entry_ids("s1").unwrap().is_empty());
}

#[test]
fn agent_bulk_mutation_uses_one_authoritative_boundary() {
    let conn = schema_connection();
    let owner = AgentMemoryFts::new(&conn).unwrap();
    owner
        .bulk_upsert(&[
            memory("bulk-a", "agent", "semantic", "bulk alpha", 1),
            memory("bulk-b", "agent", "semantic", "bulk beta", 2),
        ])
        .unwrap();
    let rows = owner
        .search("agent", "bulk", 20, MatchMode::All, &[MemoryScope::Agent])
        .unwrap();
    assert_eq!(rows.rows.len(), 2);
    let generations: (i64, i64) = conn
        .query_row(
            "SELECT mutation_generation,indexed_generation FROM agent_memory_fts_meta",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(generations, (2, 2));
}

#[test]
fn tape_append_rebuilds_when_fts_head_is_stale() {
    let conn = schema_connection();
    let mut owner = TapeSearchProjection::new(&conn).unwrap();
    owner
        .replace_session("s", &[tape("s", 1, "first alpha", "message")], 1)
        .unwrap();
    conn.execute(
        "UPDATE deepchat_tape_search_fts_meta SET max_entry_id=0 WHERE session_id='s'",
        [],
    )
    .unwrap();
    owner
        .append_session("s", &[tape("s", 2, "second alpha", "message")], 2)
        .unwrap();
    let indexed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM deepchat_tape_search_fts WHERE session_id='s'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 2);
    let head: i64 = conn
        .query_row(
            "SELECT max_entry_id FROM deepchat_tape_search_fts_meta WHERE session_id='s'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(head, 2);
}

#[test]
fn tape_micro_version_prunes_stale_and_rebuilds_fts_meta() {
    let conn = schema_connection();
    conn.execute("INSERT INTO deepchat_tape_search_projection VALUES('s',1,'message',NULL,NULL,NULL,NULL,'old','old','{}',1)",[]).unwrap();
    conn.execute(
        "INSERT INTO deepchat_tape_search_projection_meta VALUES('s',1,1,1)",
        [],
    )
    .unwrap();
    let mut owner = TapeSearchProjection::new(&conn).unwrap();
    assert!(owner.get_projected_entry_ids("s").unwrap().is_empty());
    owner
        .replace_session("s", &[tape("s", 1, "fresh text", "message")], 1)
        .unwrap();
    conn.execute(
        "DELETE FROM deepchat_tape_search_fts_meta WHERE session_id='s'",
        [],
    )
    .unwrap();
    assert_eq!(
        owner
            .search("s", "fresh", &TapeSearchOptions::default())
            .unwrap()
            .len(),
        1
    );
    let version: i64 = conn
        .query_row(
            "SELECT projection_version FROM deepchat_tape_search_fts_meta WHERE session_id='s'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(version, TAPE_SEARCH_PROJECTION_VERSION);
}

fn entry(
    session: &str,
    id: i64,
    kind: &str,
    name: Option<&str>,
    payload: serde_json::Value,
    meta: serde_json::Value,
) -> TapeEntry {
    TapeEntry {
        session_id: session.into(),
        entry_id: id,
        kind: kind.into(),
        name: name.map(Into::into),
        payload_json: payload.to_string(),
        meta_json: meta.to_string(),
        created_at: id * 10,
    }
}
fn message_entry(
    session: &str,
    id: i64,
    message_id: &str,
    status: &str,
    content: &str,
) -> TapeEntry {
    entry(
        session,
        id,
        "message",
        None,
        json!({"record":{"id":message_id,"sessionId":session,"orderSeq":id,"role":"assistant","content":content,"status":status,"metadata":"{}"}}),
        json!({}),
    )
}

#[test]
fn ingestion_projection_effective_semantics_retraction_and_atomic_current_read() {
    let mut conn = schema_connection();
    let first = message_entry("s", 1, "m", "pending", "draft");
    assert!(append_tape_entry_with_projection(&mut conn, &first).unwrap());
    let context = entry("s", 2, "context", None, json!({}), json!({}));
    assert!(append_tape_entry_with_projection(&mut conn, &context).unwrap());
    let tool = entry(
        "s",
        3,
        "tool_call",
        None,
        json!({"messageId":"m","toolCall":{"id":"t"}}),
        json!({"status":"success"}),
    );
    assert!(append_tape_entry_with_projection(&mut conn, &tool).unwrap());
    let blocks =
        json!([{"type":"tool_call","status":"success","tool_call":{"id":"t"}}]).to_string();
    let final_message = message_entry("s", 4, "m", "sent", &blocks);
    assert!(append_tape_entry_with_projection(&mut conn, &final_message).unwrap());
    let owner = MemoryIngestionProjection::new(&conn).unwrap();
    let range = owner.read_current_range("s", 0, 10).unwrap();
    assert!(range.current);
    assert_eq!(range.max_entry_id, 4);
    assert_eq!(range.rows.len(), 1);
    assert!(range.rows[0].had_tool_use);
    let retraction = entry(
        "s",
        5,
        "event",
        Some("message/retracted"),
        json!({"data":{"messageId":"m"}}),
        json!({}),
    );
    assert!(!append_tape_entry_with_projection(&mut conn, &retraction).unwrap());
    assert!(
        !MemoryIngestionProjection::new(&conn)
            .unwrap()
            .read_current_range("s", 0, 10)
            .unwrap()
            .current
    );
}

#[test]
fn concurrent_append_never_exposes_false_current_range() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("concurrent.db");
    {
        let mut conn = open_database(&path, None).unwrap();
        ProductionSchemaCatalog::frozen()
            .create_static_schema(&conn)
            .unwrap();
        append_tape_entry_with_projection(&mut conn, &message_entry("s", 1, "m1", "sent", "first"))
            .unwrap();
    }
    let barrier = Arc::new(Barrier::new(2));
    let writer_path = path.clone();
    let writer_barrier = barrier.clone();
    let writer = thread::spawn(move || {
        let mut conn = open_database(&writer_path, None).unwrap();
        writer_barrier.wait();
        append_tape_entry_with_projection(
            &mut conn,
            &message_entry("s", 2, "m2", "sent", "second"),
        )
        .unwrap();
    });
    let reader = open_database(&path, None).unwrap();
    barrier.wait();
    for _ in 0..100 {
        let state = MemoryIngestionProjection::new(&reader)
            .unwrap()
            .read_current_range("s", 0, 10)
            .unwrap();
        if state.current {
            assert_eq!(state.rows.len() as i64, state.max_entry_id);
            assert!(matches!(state.max_entry_id, 1 | 2));
        } else {
            assert!(state.rows.is_empty());
        }
        if state.max_entry_id == 2 {
            break;
        }
        thread::yield_now();
    }
    writer.join().unwrap();
    let final_state = MemoryIngestionProjection::new(&reader)
        .unwrap()
        .read_current_range("s", 0, 10)
        .unwrap();
    assert!(final_state.current);
    assert_eq!(final_state.max_entry_id, 2);
    assert_eq!(final_state.rows.len(), 2);
}

#[test]
fn ingestion_filters_retired_workflow_and_validates_tool_identity() {
    let mut conn = schema_connection();
    let retired = entry(
        "s",
        1,
        "message",
        None,
        json!({"record":{"id":"retired","sessionId":"s","orderSeq":1,"role":"assistant","content":"done","status":"sent","metadata":"{\"messageType\":\"workflow_result\"}"}}),
        json!({}),
    );
    assert!(append_tape_entry_with_projection(&mut conn, &retired).unwrap());
    let message = message_entry("s", 2, "m", "sent", "hello");
    assert!(append_tape_entry_with_projection(&mut conn, &message).unwrap());
    let malformed_tool = entry(
        "s",
        3,
        "tool_call",
        None,
        json!({"messageId":"m","toolCall":{}}),
        json!({"status":"success"}),
    );
    assert!(append_tape_entry_with_projection(&mut conn, &malformed_tool).unwrap());
    let valid_nested_tool = entry(
        "s",
        4,
        "tool_call",
        None,
        json!({"messageId":"m","toolCall":"{\"id\":\"tool-1\"}"}),
        json!({"status":"success"}),
    );
    assert!(append_tape_entry_with_projection(&mut conn, &valid_nested_tool).unwrap());
    let rows = MemoryIngestionProjection::new(&conn)
        .unwrap()
        .read_current_range("s", 0, 10)
        .unwrap()
        .rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message_id, "m");
    assert!(rows[0].had_tool_use);
}

#[test]
fn ingestion_replace_validates_session_and_stale_append_commits_tape() {
    let mut conn = schema_connection();
    let input = message_entry("s", 1, "m", "sent", "hello");
    MemoryIngestionProjection::new(&conn)
        .unwrap()
        .replace_session("s", std::slice::from_ref(&input), 1)
        .unwrap();
    let row = entry("s", 2, "context", None, json!({}), json!({}));
    assert!(!append_tape_entry_with_projection(&mut conn, &row).unwrap());
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM deepchat_tape_entries WHERE session_id='s'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM deepchat_memory_ingestion_projection_meta WHERE session_id='s'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    let owner = MemoryIngestionProjection::new(&conn).unwrap();
    let wrong = message_entry("other", 1, "m", "sent", "hello");
    assert!(owner.replace_session("s", &[wrong], 1).is_err());
}

#[test]
fn ingestion_rebuild_derives_the_same_effective_rows_as_incremental_tape() {
    let mut incremental = schema_connection();
    let rebuild = schema_connection();
    let entries = vec![
        entry(
            "s",
            1,
            "message",
            None,
            json!({"record":{"id":"m","sessionId":"embedded-other","orderSeq":1,"role":"assistant","content":"draft","status":"pending","metadata":"{}"}}),
            json!({}),
        ),
        message_entry("s", 2, "m", "sent", "final"),
        entry(
            "s",
            3,
            "tool_result",
            None,
            json!({"messageId":"m","toolCallId":"result-only"}),
            json!({"status":"success"}),
        ),
        entry(
            "s",
            4,
            "tool_call",
            None,
            json!({"messageId":"m","toolCall":{"id":"call"}}),
            json!({"status":"success"}),
        ),
        entry(
            "s",
            5,
            "message",
            None,
            json!({"record":{"id":"retired","sessionId":"s","orderSeq":2,"role":"assistant","content":"hidden","status":"sent","metadata":"{\"messageType\":\"workflow_result\"}"}}),
            json!({}),
        ),
        entry("s", 6, "context", None, json!({}), json!({})),
    ];
    for row in &entries {
        assert!(append_tape_entry_with_projection(&mut incremental, row).unwrap());
    }
    for row in &entries {
        rebuild.execute(
            "INSERT INTO deepchat_tape_entries(session_id,entry_id,kind,name,payload_json,meta_json,created_at) VALUES(?,?,?,?,?,?,?)",
            rusqlite::params![row.session_id,row.entry_id,row.kind,row.name,row.payload_json,row.meta_json,row.created_at],
        ).unwrap();
    }
    MemoryIngestionProjection::new(&rebuild)
        .unwrap()
        .replace_session("s", &entries, 6)
        .unwrap();
    let incremental_rows = MemoryIngestionProjection::new(&incremental)
        .unwrap()
        .read_current_range("s", 0, 10)
        .unwrap();
    let rebuilt_rows = MemoryIngestionProjection::new(&rebuild)
        .unwrap()
        .read_current_range("s", 0, 10)
        .unwrap();
    assert_eq!(rebuilt_rows, incremental_rows);
    assert_eq!(rebuilt_rows.rows.len(), 1);
    assert_eq!(rebuilt_rows.rows[0].session_id, "s");
    assert!(rebuilt_rows.rows[0].had_tool_use);

    let retracted = vec![
        message_entry("s", 1, "gone", "sent", "visible"),
        entry(
            "s",
            2,
            "event",
            Some("message/retracted"),
            json!({"data":"{\"messageId\":\"gone\"}"}),
            json!({}),
        ),
    ];
    rebuild
        .execute("DELETE FROM deepchat_tape_entries WHERE session_id='s'", [])
        .unwrap();
    for row in &retracted {
        rebuild.execute(
            "INSERT INTO deepchat_tape_entries(session_id,entry_id,kind,name,payload_json,meta_json,created_at) VALUES(?,?,?,?,?,?,?)",
            rusqlite::params![row.session_id,row.entry_id,row.kind,row.name,row.payload_json,row.meta_json,row.created_at],
        ).unwrap();
    }
    MemoryIngestionProjection::new(&rebuild)
        .unwrap()
        .replace_session("s", &retracted, 2)
        .unwrap();
    let range = MemoryIngestionProjection::new(&rebuild)
        .unwrap()
        .read_current_range("s", 0, 10)
        .unwrap();
    assert!(range.current);
    assert!(range.rows.is_empty());
}

#[test]
fn ingestion_tool_result_does_not_mutate_and_tape_session_is_authoritative() {
    let mut conn = schema_connection();
    let cross_session = entry(
        "authoritative",
        1,
        "message",
        None,
        json!({"record":{"id":"m","sessionId":"embedded-other","orderSeq":1,"role":"assistant","content":"plain","status":"sent","metadata":"{}"}}),
        json!({}),
    );
    assert!(append_tape_entry_with_projection(&mut conn, &cross_session).unwrap());
    let result = entry(
        "authoritative",
        2,
        "tool_result",
        None,
        json!({"messageId":"m","toolCallId":"result"}),
        json!({"status":"success"}),
    );
    assert!(append_tape_entry_with_projection(&mut conn, &result).unwrap());
    let range = MemoryIngestionProjection::new(&conn)
        .unwrap()
        .read_current_range("authoritative", 0, 10)
        .unwrap();
    assert_eq!(range.rows.len(), 1);
    assert_eq!(range.rows[0].session_id, "authoritative");
    assert!(!range.rows[0].had_tool_use);
    assert!(
        MemoryIngestionProjection::new(&conn)
            .unwrap()
            .read_current_range("embedded-other", 0, 10)
            .unwrap()
            .rows
            .is_empty()
    );
}

#[test]
fn tape_sources_trim_and_collapse_to_the_maximum_authorized_head() {
    let conn = schema_connection();
    let mut owner = TapeSearchProjection::new(&conn).unwrap();
    owner
        .replace_session(
            "s",
            &[
                tape("s", 1, "needle one", "message"),
                tape("s", 2, "needle two", "message"),
            ],
            2,
        )
        .unwrap();
    let result = owner
        .search_sources_read_only(
            &[
                TapeReadSource {
                    session_id: " s ",
                    max_entry_id: 1,
                },
                TapeReadSource {
                    session_id: "s",
                    max_entry_id: 2,
                },
                TapeReadSource {
                    session_id: " ",
                    max_entry_id: 99,
                },
                TapeReadSource {
                    session_id: "s",
                    max_entry_id: -1,
                },
            ],
            "needle",
            &TapeSearchOptions::default(),
        )
        .unwrap();
    assert_eq!(result.covered_sources, vec![("s".to_owned(), 2)]);
    assert_eq!(
        result
            .rows
            .iter()
            .map(|row| row.entry_id)
            .collect::<Vec<_>>(),
        vec![2, 1]
    );
}

#[test]
fn tape_open_prunes_stale_dynamic_rows_and_recovers_corrupt_cleanup() {
    let conn = schema_connection();
    {
        let mut owner = TapeSearchProjection::new(&conn).unwrap();
        owner
            .replace_session("stale", &[tape("stale", 1, "old", "message")], 1)
            .unwrap();
    }
    conn.execute(
        "UPDATE deepchat_tape_search_projection_meta SET projection_version=8 WHERE session_id='stale'",
        [],
    )
    .unwrap();
    let _owner = TapeSearchProjection::new(&conn).unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM deepchat_tape_search_fts WHERE session_id='stale'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap(),
        0
    );

    conn.execute_batch(
        "DROP TABLE deepchat_tape_search_fts;
         CREATE TABLE deepchat_tape_search_fts(broken TEXT);
         INSERT INTO deepchat_tape_search_fts VALUES('stale');
         INSERT OR REPLACE INTO deepchat_tape_search_fts_meta VALUES('orphan',8,1,0);",
    )
    .unwrap();
    let owner = TapeSearchProjection::new(&conn).unwrap();
    assert!(owner.is_fts_ready());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM deepchat_tape_search_fts", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM deepchat_tape_search_fts_meta",
            [],
            |row| { row.get::<_, i64>(0) }
        )
        .unwrap(),
        0
    );
}

#[test]
fn copy_exclusion_is_exact() {
    assert_eq!(SQLITE_COPY_EXCLUDED_OBJECTS.len(), 6);
    for name in SQLITE_COPY_EXCLUDED_OBJECTS {
        assert!(should_exclude_from_sqlite_copy(name));
    }
    for name in [
        "agent_memory",
        "agent_memory_dirty_extra",
        "deepchat_tape_search_fts",
        "agent_memory_fts_meta_backup",
    ] {
        assert!(!should_exclude_from_sqlite_copy(name));
    }
}

#[test]
fn agent_authority_filters_scope_status_and_supersession() {
    let conn = schema_connection();
    let owner = AgentMemoryFts::new(&conn).unwrap();
    let mut user = memory("user", "agent", "semantic", "authority alpha", 1);
    user.scope_type = "user";
    user.scope_id = Some("u1");
    let mut archived = memory("archived", "agent", "semantic", "authority alpha", 2);
    archived.lifecycle_state = "archived";
    let mut working = memory("working", "agent", "working", "authority alpha", 3);
    working.scope_type = "user";
    working.scope_id = Some("u1");
    let mut superseded = memory("superseded", "agent", "semantic", "authority alpha", 4);
    superseded.scope_type = "user";
    superseded.scope_id = Some("u1");
    superseded.superseded_by = Some("replacement");
    owner
        .bulk_upsert(&[user, archived, working, superseded])
        .unwrap();
    let rows = owner
        .search(
            "agent",
            "authority",
            1000,
            MatchMode::All,
            &[MemoryScope::User("u1".into())],
        )
        .unwrap();
    assert_eq!(
        rows.rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        vec!["user"]
    );
    assert!(
        owner
            .search(
                "agent",
                "authority",
                10,
                MatchMode::All,
                &[MemoryScope::User("u2".into())]
            )
            .unwrap()
            .rows
            .is_empty()
    );
    assert_eq!(agent_fts_scope("agent-234"), agent_fts_scope("agent-4227"));
    owner
        .upsert(&memory(
            "collision",
            "agent-4227",
            "semantic",
            "authority alpha",
            5,
        ))
        .unwrap();
    assert!(
        owner
            .search(
                "agent-234",
                "authority",
                10,
                MatchMode::All,
                &[MemoryScope::Agent],
            )
            .unwrap()
            .rows
            .is_empty()
    );
}

#[test]
fn tape_fts_first_like_fill_dedupes_and_corruption_falls_back() {
    let conn = schema_connection();
    let mut owner = TapeSearchProjection::new(&conn).unwrap();
    let mut lexical = tape("s", 1, "needle lexical", "message");
    lexical.summary_text = "none".into();
    let mut summary_only = tape("s", 2, "other", "message");
    summary_only.summary_text = "needle summary".into();
    owner
        .replace_session("s", &[lexical, summary_only], 2)
        .unwrap();
    let rows = owner
        .search(
            "s",
            "needle",
            &TapeSearchOptions {
                limit: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        rows.iter().map(|row| row.entry_id).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        rows.iter()
            .map(|row| (row.session_id.as_str(), row.entry_id))
            .collect::<std::collections::HashSet<_>>()
            .len(),
        2
    );
    conn.execute_batch(
        "DROP TABLE deepchat_tape_search_fts; CREATE TABLE deepchat_tape_search_fts(broken TEXT);",
    )
    .unwrap();
    let rows = owner
        .search("s", "needle", &TapeSearchOptions::default())
        .unwrap();
    assert_eq!(
        rows.iter().map(|row| row.entry_id).collect::<Vec<_>>(),
        vec![2, 1]
    );
}

#[test]
fn tape_projection_metadata_failure_rolls_back_projection_and_fts() {
    let conn = schema_connection();
    let mut owner = TapeSearchProjection::new(&conn).unwrap();
    owner
        .replace_session("s", &[tape("s", 1, "before", "message")], 1)
        .unwrap();
    conn.execute_batch("CREATE TRIGGER reject_projection_meta BEFORE UPDATE ON deepchat_tape_search_projection_meta BEGIN SELECT RAISE(ABORT, 'injected'); END;").unwrap();
    assert!(
        owner
            .append_session("s", &[tape("s", 2, "after", "message")], 2)
            .is_err()
    );
    assert_eq!(owner.get_projected_entry_ids("s").unwrap(), vec![1]);
    let fts_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM deepchat_tape_search_fts WHERE session_id='s'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fts_rows, 1);
    assert!(owner.is_current("s", 1).unwrap());
}

#[test]
fn keyed_sqlcipher_dynamic_owners_rebuild_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("generated.db");
    let password = "generated-test-key";
    {
        let conn = open_database(&path, Some(password)).unwrap();
        ProductionSchemaCatalog::frozen()
            .create_static_schema(&conn)
            .unwrap();
        let agent = AgentMemoryFts::new(&conn).unwrap();
        agent
            .upsert(&memory("m", "agent", "semantic", "encrypted recall", 1))
            .unwrap();
        let mut tape_owner = TapeSearchProjection::new(&conn).unwrap();
        tape_owner
            .replace_session("s", &[tape("s", 1, "encrypted tape", "message")], 1)
            .unwrap();
        conn.pragma_update(None, "wal_checkpoint", "TRUNCATE")
            .unwrap();
    }
    {
        let conn = open_database(&path, Some(password)).unwrap();
        let agent = AgentMemoryFts::new(&conn).unwrap();
        assert_eq!(
            agent
                .search(
                    "agent",
                    "encrypted",
                    10,
                    MatchMode::All,
                    &[MemoryScope::Agent],
                )
                .unwrap()
                .rows[0]
                .id,
            "m"
        );
        let mut tape_owner = TapeSearchProjection::new(&conn).unwrap();
        assert_eq!(
            tape_owner
                .search("s", "encrypted", &TapeSearchOptions::default())
                .unwrap()[0]
                .entry_id,
            1
        );
    }
    assert!(open_database(&path, Some("wrong-key")).is_err());
}

#[test]
fn tape_projection_write_failure_rolls_back_all_four_owners() {
    let conn = schema_connection();
    let mut owner = TapeSearchProjection::new(&conn).unwrap();
    owner
        .replace_session("s", &[tape("s", 1, "before alpha", "message")], 1)
        .unwrap();
    conn.execute_batch(
        "DROP TABLE deepchat_tape_search_fts;
         CREATE TABLE deepchat_tape_search_fts(blocked TEXT);",
    )
    .unwrap();
    assert!(
        owner
            .append_session("s", &[tape("s", 2, "after alpha", "message")], 2)
            .is_err()
    );
    assert_eq!(owner.get_projected_entry_ids("s").unwrap(), vec![1]);
    assert!(owner.is_current("s", 1).unwrap());
    let fts_meta: i64 = conn
        .query_row(
            "SELECT max_entry_id FROM deepchat_tape_search_fts_meta WHERE session_id='s'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(fts_meta, 1);
}

#[test]
fn public_errors_are_stable_and_redacted() {
    fn assert_redacted(text: &str, category: &str) {
        assert_eq!(text, category);
        for sentinel in [
            "SQL",
            "sqlite",
            "SELECT",
            "INSERT",
            "agent_memory",
            "deepchat_tape",
            "/private/generated.db",
            "secret-query",
            "secret-agent",
            "secret-session",
            "credential-sentinel",
        ] {
            assert!(
                !text.contains(sentinel),
                "public error leaked {sentinel}: {text}"
            );
        }
    }

    let agent_conn = Connection::open_in_memory().unwrap();
    let error = AgentMemoryFts::new(&agent_conn)
        .err()
        .expect("missing agent table must fail");
    assert_redacted(&error.to_string(), "agent memory search storage failed");

    let tape_conn = schema_connection();
    let mut tape_owner = TapeSearchProjection::new(&tape_conn).unwrap();
    let mut invalid_tape = tape("secret-session", 1, "credential-sentinel", "message");
    invalid_tape.session_id = "different-session".into();
    let error = tape_owner
        .replace_session("secret-session", &[invalid_tape], 1)
        .unwrap_err();
    assert_redacted(
        &error.to_string(),
        "tape search projection input is invalid",
    );
    tape_conn
        .execute_batch("DROP TABLE deepchat_tape_search_projection")
        .unwrap();
    let error = tape_owner
        .search(
            "secret-session",
            "secret-query",
            &TapeSearchOptions::default(),
        )
        .unwrap_err();
    assert_redacted(&error.to_string(), "tape search projection storage failed");

    let ingestion_conn = schema_connection();
    let ingestion = MemoryIngestionProjection::new(&ingestion_conn).unwrap();
    let invalid = message_entry(
        "different-session",
        1,
        "credential-sentinel",
        "sent",
        "secret-query",
    );
    let error = ingestion
        .replace_session("secret-session", &[invalid], 1)
        .unwrap_err();
    assert_redacted(
        &error.to_string(),
        "memory ingestion projection input is invalid",
    );
    ingestion_conn
        .execute_batch("DROP TABLE deepchat_memory_ingestion_projection")
        .unwrap();
    let error = ingestion.list_range("secret-session", 0, 10).unwrap_err();
    assert_redacted(
        &error.to_string(),
        "memory ingestion projection storage failed",
    );
}
