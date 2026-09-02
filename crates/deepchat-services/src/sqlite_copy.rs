//! Canonical dynamic-object exclusions used by SQLite copy workflows.

pub const SQLITE_COPY_EXCLUDED_OBJECTS: [&str; 6] = [
    "agent_memory_dirty",
    "agent_memory_dirty_ai",
    "agent_memory_dirty_au",
    "agent_memory_dirty_ad",
    "agent_memory_fts_meta",
    "deepchat_tape_search_fts_meta",
];

pub fn should_exclude_from_sqlite_copy(object_name: &str) -> bool {
    SQLITE_COPY_EXCLUDED_OBJECTS.contains(&object_name)
}
