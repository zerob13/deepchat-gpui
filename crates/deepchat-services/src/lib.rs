//! SQLCipher storage foundation.
//!
//! This crate owns the SQLCipher-compatible connection, schema-version
//! bookkeeping, migration execution, startup classification, password
//! resolution, production schema diagnosis/repair, startup one-shot recovery,
//! the frozen production static-schema/migration-owner layer, dynamic FTS,
//! and Tape/Memory projections. Backup/import remains later work.

pub mod agent_memory_fts;
pub mod clock;
pub mod connection;
pub mod error;
pub mod fts;
pub mod memory_ingestion_projection;
pub mod password;
pub mod production_schema;
pub mod schema;
pub mod schema_error_classifier;
pub mod schema_repair;
pub mod sqlite_copy;
pub mod startup;
pub mod startup_recovery;
pub mod tape_search_projection;

pub use clock::{Clock, SystemClock};
pub use error::StartupFailureKind;
pub use startup::{StartupError, Storage};
