//! Stable, non-secret startup classifications.

/// Startup failure classification. These values are stable and safe to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupFailureKind {
    /// A leftover WAL sidecar exists without its main database file.
    OrphanWal,
    /// The encrypted file could not be read and no password was verified.
    Unreadable,
    /// The database decrypted (or is plaintext) but is structurally corrupt.
    TrueCorruption,
}

impl std::fmt::Display for StartupFailureKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::OrphanWal => "orphaned-sidecar",
            Self::Unreadable => "unreadable",
            Self::TrueCorruption => "true-corruption",
        };
        f.write_str(label)
    }
}
