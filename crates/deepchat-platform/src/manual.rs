//! Manual fallback: no safe storage; the password must be entered each time.

use secrecy::SecretString;

use crate::secret_store::{SecretStore, SecretStoreError};

/// A store that never persists a secret and always requires manual entry.
#[derive(Debug, Default, Clone, Copy)]
pub struct ManualFallbackSecretStore;

impl SecretStore for ManualFallbackSecretStore {
    fn is_available(&self) -> bool {
        false
    }

    fn store(&self, _secret: &SecretString) -> Result<(), SecretStoreError> {
        Err(SecretStoreError::ManualFallbackRequired)
    }

    fn load(&self) -> Result<Option<SecretString>, SecretStoreError> {
        Ok(None)
    }

    fn clear(&self) -> Result<(), SecretStoreError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret_store::SecretStore;

    #[test]
    fn manual_fallback_never_persists() {
        let store = ManualFallbackSecretStore;
        assert!(!store.is_available());
        let secret = SecretString::from("secret".to_string());
        assert_eq!(
            store.store(&secret).unwrap_err(),
            SecretStoreError::ManualFallbackRequired
        );
        assert!(store.load().unwrap().is_none());
        assert!(store.clear().is_ok());
    }
}
