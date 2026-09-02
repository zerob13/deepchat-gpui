//! SQLCipher storage foundation.
//!
//! This crate owns the SQLCipher-compatible connection, schema-version
//! bookkeeping, migration execution, startup classification, password
//! resolution, production schema diagnosis/repair, startup one-shot recovery,
//! and the frozen production static-schema/migration-owner layer. Complete
//! dynamic FTS lifecycle and backup/import remain later work.

pub mod clock;
pub mod connection;
pub mod error;
pub mod password;
pub mod production_schema;
pub mod schema;
pub mod schema_error_classifier;
pub mod schema_repair;
pub mod startup;
pub mod startup_recovery;

pub use clock::{Clock, SystemClock};
pub use error::StartupFailureKind;
pub use startup::{StartupError, Storage};
