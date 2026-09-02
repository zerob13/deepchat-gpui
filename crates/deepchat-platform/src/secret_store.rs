//! Typed secret-store port.

use secrecy::SecretString;
use thiserror::Error;

/// Secret-store errors. No variant carries a secret value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SecretStoreError {
    #[error("secret store unavailable")]
    Unavailable,

    #[error("manual fallback required")]
    ManualFallbackRequired,

    #[error("secret store access failed")]
    Access,
}

/// Typed port for platform secret storage (macOS Keychain and equivalents).
///
/// Implementations must never log, serialize, or display the stored secret.
pub trait SecretStore {
    /// Whether the backing store is available on this host.
    fn is_available(&self) -> bool;

    /// Persists `secret` under the store's configured service/account policy.
    fn store(&self, secret: &SecretString) -> Result<(), SecretStoreError>;

    /// Loads a previously stored secret, if any.
    fn load(&self) -> Result<Option<SecretString>, SecretStoreError>;

    /// Removes a previously stored secret.
    fn clear(&self) -> Result<(), SecretStoreError>;
}
