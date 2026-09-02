-- Historical static owner shapes reconstructed from frozen-reference history.
-- Provenance: deepchat-2 commit f9adbcb6b7807c91e544b0e7fd24d46df53d4fd3^
-- (`src/main/presenter/sqlitePresenter/tables/{acpSessions,newSessions,
-- newProjects,deepchatSessions,deepchatMessages,deepchatMessageTraces,
-- deepchatMessageSearchResults,legacyImportStatus}.ts`). That revision's owner
-- high-water mark was v10; current startup `CREATE IF NOT EXISTS` semantics
-- preserve these tables while creating owners that did not yet exist.
CREATE TABLE acp_sessions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  conversation_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  session_id TEXT UNIQUE,
  workdir TEXT,
  status TEXT NOT NULL DEFAULT 'idle' CHECK(status IN ('idle', 'active', 'error')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  metadata TEXT,
  UNIQUE(conversation_id, agent_id)
);
CREATE INDEX idx_acp_sessions_session_id ON acp_sessions(session_id);
CREATE INDEX idx_acp_sessions_agent ON acp_sessions(agent_id);
CREATE TABLE new_sessions (
  id TEXT PRIMARY KEY,
  agent_id TEXT NOT NULL,
  title TEXT NOT NULL,
  project_dir TEXT,
  is_pinned INTEGER DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_new_sessions_agent ON new_sessions(agent_id);
CREATE INDEX idx_new_sessions_updated ON new_sessions(updated_at DESC);
CREATE TABLE new_projects (
  path TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  icon TEXT DEFAULT NULL,
  last_accessed_at INTEGER NOT NULL
);
CREATE TABLE deepchat_sessions (
  id TEXT PRIMARY KEY,
  provider_id TEXT NOT NULL,
  model_id TEXT NOT NULL,
  permission_mode TEXT NOT NULL DEFAULT 'full_access'
);
CREATE TABLE deepchat_messages (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  order_seq INTEGER NOT NULL,
  role TEXT NOT NULL,
  content TEXT NOT NULL,
  status TEXT DEFAULT 'pending',
  is_context_edge INTEGER DEFAULT 0,
  metadata TEXT DEFAULT '{}',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX idx_deepchat_messages_session ON deepchat_messages(session_id, order_seq);
CREATE TABLE deepchat_message_traces (
  id TEXT PRIMARY KEY,
  message_id TEXT NOT NULL,
  session_id TEXT NOT NULL,
  provider_id TEXT NOT NULL,
  model_id TEXT NOT NULL,
  request_seq INTEGER NOT NULL,
  endpoint TEXT NOT NULL,
  headers_json TEXT NOT NULL,
  body_json TEXT NOT NULL,
  truncated INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_trace_message_seq ON deepchat_message_traces(message_id, request_seq DESC);
CREATE INDEX idx_trace_session_time ON deepchat_message_traces(session_id, created_at DESC);
CREATE TABLE deepchat_message_search_results (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL,
  message_id TEXT NOT NULL,
  search_id TEXT DEFAULT NULL,
  rank INTEGER DEFAULT NULL,
  content TEXT NOT NULL,
  dedupe_key TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_search_results_message ON deepchat_message_search_results(message_id, created_at ASC);
CREATE INDEX idx_search_results_message_search ON deepchat_message_search_results(message_id, search_id, rank);
CREATE INDEX idx_search_results_session ON deepchat_message_search_results(session_id, created_at DESC);
CREATE TABLE legacy_import_status (
  import_key TEXT PRIMARY KEY,
  status TEXT NOT NULL CHECK(status IN ('idle', 'running', 'completed', 'failed', 'skipped')),
  source_db_path TEXT NOT NULL,
  started_at INTEGER DEFAULT NULL,
  finished_at INTEGER DEFAULT NULL,
  imported_sessions INTEGER NOT NULL DEFAULT 0,
  imported_messages INTEGER NOT NULL DEFAULT 0,
  imported_search_results INTEGER NOT NULL DEFAULT 0,
  error TEXT DEFAULT NULL,
  updated_at INTEGER NOT NULL
);
INSERT INTO new_sessions(id,agent_id,title,project_dir,is_pinned,created_at,updated_at)
VALUES('historical-session','agent','preserved',NULL,0,1,1);
INSERT INTO deepchat_sessions(id,provider_id,model_id,permission_mode)
VALUES('historical-session','provider','model','full_access');
CREATE TABLE schema_versions(version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL);
INSERT INTO schema_versions VALUES(10,0);
