//! Platform secret-store boundary.

pub mod keychain;
pub mod manual;
pub mod secret_store;

pub use keychain::{KeychainClient, KeychainError, KeychainSecretStore};
pub use manual::ManualFallbackSecretStore;
pub use secret_store::{SecretStore, SecretStoreError};
