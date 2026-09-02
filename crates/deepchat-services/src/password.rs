//! Password resolution with an inner validation retry loop.

use std::path::{Path, PathBuf};

use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

use crate::connection::{ClassifiedConnectionError, open_database_classified};
use crate::startup_recovery::{is_decrypted_database_corruption_error, is_wrong_password_error};

/// Opaque in-process capability proving that the enclosed password successfully
/// opened the configured database.
///
/// Construction and secret access are crate-private. The capability is neither
/// cloneable nor serializable, so callers can only move a resolver-issued value
/// into [`crate::startup::Storage::open`].
pub struct VerifiedPassword {
    secret: SecretString,
}

impl VerifiedPassword {
    fn validated(secret: SecretString) -> Self {
        Self { secret }
    }

    pub(crate) fn as_str(&self) -> &str {
        self.secret.expose_secret()
    }
}

impl std::fmt::Debug for VerifiedPassword {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VerifiedPassword([REDACTED])")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnlockReason {
    ManualRequired,
    Invalid,
    SystemKeyMissing,
}

#[derive(Debug, Clone, Copy, Error)]
#[error("database unlock cancelled")]
pub struct PasswordCancelled;

#[derive(Debug, Error)]
pub enum PasswordError {
    #[error("password entry cancelled")]
    Cancelled,
    #[error("orphan WAL sidecar exists")]
    OrphanWal(PathBuf),
    #[error("database open failed")]
    Open,
    #[error("database file I/O failed")]
    Io,
}

impl From<PasswordCancelled> for PasswordError {
    fn from(_: PasswordCancelled) -> Self {
        PasswordError::Cancelled
    }
}

pub trait UnlockProvider {
    fn provide(&mut self, reason: UnlockReason) -> Result<SecretString, PasswordCancelled>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationOutcome {
    Valid,
    DecryptedCorruption,
    WrongPassword,
}

/// Minimal production port for validating a password candidate.
///
/// Validators report facts only. They cannot construct [`VerifiedPassword`];
/// the resolver alone seals the current candidate after a successful outcome.
pub trait PasswordValidator {
    fn validate(
        &mut self,
        db_path: &Path,
        password: &str,
    ) -> Result<ValidationOutcome, PasswordError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct DatabasePasswordValidator;

impl PasswordValidator for DatabasePasswordValidator {
    fn validate(
        &mut self,
        db_path: &Path,
        password: &str,
    ) -> Result<ValidationOutcome, PasswordError> {
        validate_password(db_path, password)
    }
}

pub fn validate_password(
    db_path: &Path,
    password: &str,
) -> Result<ValidationOutcome, PasswordError> {
    let conn = match open_database_classified(db_path, Some(password)) {
        Ok(conn) => conn,
        Err(ClassifiedConnectionError::OrphanWal(path)) => {
            return Err(PasswordError::OrphanWal(path));
        }
        Err(ClassifiedConnectionError::Io) => return Err(PasswordError::Io),
        Err(ClassifiedConnectionError::Sqlite(error)) => {
            if is_decrypted_database_corruption_error(&error) {
                return Ok(ValidationOutcome::DecryptedCorruption);
            }
            if is_wrong_password_error(&error) {
                return Ok(ValidationOutcome::WrongPassword);
            }
            return Err(PasswordError::Open);
        }
    };

    let probe = conn.query_row("SELECT name FROM sqlite_master LIMIT 1", [], |row| {
        row.get::<_, String>(0)
    });
    match probe {
        Ok(_) => Ok(ValidationOutcome::Valid),
        Err(error) if is_decrypted_database_corruption_error(&error) => {
            Ok(ValidationOutcome::DecryptedCorruption)
        }
        Err(_) => Err(PasswordError::Open),
    }
}

pub struct PasswordResolver<P, V = DatabasePasswordValidator> {
    db_path: PathBuf,
    provider: P,
    validator: V,
}

impl<P: UnlockProvider> PasswordResolver<P, DatabasePasswordValidator> {
    /// Uses the production SQLCipher validator.
    pub fn new(db_path: PathBuf, provider: P) -> Self {
        Self {
            db_path,
            provider,
            validator: DatabasePasswordValidator,
        }
    }
}

impl<P: UnlockProvider, V: PasswordValidator> PasswordResolver<P, V> {
    /// Injects a validator implementation while preserving resolver ownership of
    /// capability issuance and retry policy.
    pub fn with_validator(db_path: PathBuf, provider: P, validator: V) -> Self {
        Self {
            db_path,
            provider,
            validator,
        }
    }

    /// Owns the complete retry loop. Every non-terminal validation failure is
    /// reported to the provider as `Invalid`.
    pub fn resolve(&mut self) -> Result<VerifiedPassword, PasswordError> {
        let mut reason = UnlockReason::ManualRequired;
        loop {
            let password = self.provider.provide(reason)?;
            match self
                .validator
                .validate(&self.db_path, password.expose_secret())
            {
                Ok(ValidationOutcome::Valid | ValidationOutcome::DecryptedCorruption) => {
                    return Ok(VerifiedPassword::validated(password));
                }
                Ok(ValidationOutcome::WrongPassword)
                | Err(PasswordError::Open | PasswordError::Io) => {
                    reason = UnlockReason::Invalid;
                }
                Err(error @ PasswordError::OrphanWal(_))
                | Err(error @ PasswordError::Cancelled) => return Err(error),
            }
        }
    }
}
