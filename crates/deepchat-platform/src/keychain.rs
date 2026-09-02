//! macOS Keychain target adapter with an injectable service/account policy.

use std::sync::Arc;

use secrecy::{ExposeSecret, SecretString};
use thiserror::Error;

use crate::secret_store::{SecretStore, SecretStoreError};

/// Security framework status code for "item not found" (`errSecItemNotFound`).
#[cfg(target_os = "macos")]
const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

/// Low-level Keychain access, injected so tests use fakes and never touch the
/// user's real Keychain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum KeychainError {
    #[error("keychain item not found")]
    NotFound,
    #[error("keychain access failed")]
    Other,
}

/// Low-level Keychain client. Service/account values are target policy only.
pub trait KeychainClient: Send + Sync {
    fn is_available(&self) -> bool;

    fn add_generic_password(
        &self,
        service: &str,
        account: &str,
        password: &[u8],
    ) -> Result<(), KeychainError>;

    fn find_generic_password(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainError>;

    fn delete_generic_password(&self, service: &str, account: &str) -> Result<(), KeychainError>;
}

/// Production Keychain client backed by the macOS Security framework.
#[cfg(target_os = "macos")]
#[derive(Debug, Default, Clone, Copy)]
pub struct SecurityFrameworkKeychainClient;

#[cfg(target_os = "macos")]
impl KeychainClient for SecurityFrameworkKeychainClient {
    fn is_available(&self) -> bool {
        true
    }

    fn add_generic_password(
        &self,
        service: &str,
        account: &str,
        password: &[u8],
    ) -> Result<(), KeychainError> {
        security_framework::passwords::set_generic_password(service, account, password)
            .map_err(|_| KeychainError::Other)
    }

    fn find_generic_password(
        &self,
        service: &str,
        account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainError> {
        match security_framework::passwords::get_generic_password(service, account) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(_) => Err(KeychainError::Other),
        }
    }

    fn delete_generic_password(&self, service: &str, account: &str) -> Result<(), KeychainError> {
        match security_framework::passwords::delete_generic_password(service, account) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(_) => Err(KeychainError::Other),
        }
    }
}

/// Non-macOS fallback client: never available and never persists.
#[cfg(not(target_os = "macos"))]
#[derive(Debug, Default, Clone, Copy)]
pub struct UnavailableKeychainClient;

#[cfg(not(target_os = "macos"))]
impl KeychainClient for UnavailableKeychainClient {
    fn is_available(&self) -> bool {
        false
    }

    fn add_generic_password(
        &self,
        _service: &str,
        _account: &str,
        _password: &[u8],
    ) -> Result<(), KeychainError> {
        Err(KeychainError::Other)
    }

    fn find_generic_password(
        &self,
        _service: &str,
        _account: &str,
    ) -> Result<Option<Vec<u8>>, KeychainError> {
        Err(KeychainError::Other)
    }

    fn delete_generic_password(&self, _service: &str, _account: &str) -> Result<(), KeychainError> {
        Ok(())
    }
}

/// Returns a production Keychain client for the current host.
pub fn default_keychain_client() -> Arc<dyn KeychainClient> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(SecurityFrameworkKeychainClient)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(UnavailableKeychainClient)
    }
}

/// macOS Keychain adapter with an injectable service/account policy.
///
/// The service and account names are target policy only: there is no frozen
/// reference contract for them, and evidence must never claim one.
pub struct KeychainSecretStore {
    client: Arc<dyn KeychainClient>,
    service: String,
    account: String,
}

impl KeychainSecretStore {
    pub fn new(client: Arc<dyn KeychainClient>, service: String, account: String) -> Self {
        Self {
            client,
            service,
            account,
        }
    }

    /// Constructs the adapter with the host's production Keychain client.
    pub fn with_default_client(service: String, account: String) -> Self {
        Self::new(default_keychain_client(), service, account)
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub fn account(&self) -> &str {
        &self.account
    }
}

impl SecretStore for KeychainSecretStore {
    fn is_available(&self) -> bool {
        self.client.is_available()
    }

    fn store(&self, secret: &SecretString) -> Result<(), SecretStoreError> {
        if !self.client.is_available() {
            return Err(SecretStoreError::Unavailable);
        }
        self.client
            .add_generic_password(
                &self.service,
                &self.account,
                secret.expose_secret().as_bytes(),
            )
            .map_err(|_| SecretStoreError::Access)
    }

    fn load(&self) -> Result<Option<SecretString>, SecretStoreError> {
        if !self.client.is_available() {
            return Ok(None);
        }
        match self
            .client
            .find_generic_password(&self.service, &self.account)
        {
            Ok(Some(bytes)) => {
                let text = String::from_utf8(bytes).map_err(|_| SecretStoreError::Access)?;
                Ok(Some(SecretString::from(text)))
            }
            Ok(None) => Ok(None),
            Err(_) => Err(SecretStoreError::Access),
        }
    }

    fn clear(&self) -> Result<(), SecretStoreError> {
        if !self.client.is_available() {
            return Ok(());
        }
        self.client
            .delete_generic_password(&self.service, &self.account)
            .map_err(|_| SecretStoreError::Access)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeKeychainClient {
        available: bool,
        entries: Mutex<Vec<(String, String, Vec<u8>)>>,
    }

    impl FakeKeychainClient {
        fn new(available: bool) -> Self {
            Self {
                available,
                entries: Mutex::new(Vec::new()),
            }
        }
    }

    impl KeychainClient for FakeKeychainClient {
        fn is_available(&self) -> bool {
            self.available
        }

        fn add_generic_password(
            &self,
            service: &str,
            account: &str,
            password: &[u8],
        ) -> Result<(), KeychainError> {
            if !self.available {
                return Err(KeychainError::Other);
            }
            let mut entries = self.entries.lock().unwrap();
            entries.retain(|(s, a, _)| s != service || a != account);
            entries.push((service.to_string(), account.to_string(), password.to_vec()));
            Ok(())
        }

        fn find_generic_password(
            &self,
            service: &str,
            account: &str,
        ) -> Result<Option<Vec<u8>>, KeychainError> {
            if !self.available {
                return Err(KeychainError::Other);
            }
            let entries = self.entries.lock().unwrap();
            Ok(entries
                .iter()
                .find(|(s, a, _)| s == service && a == account)
                .map(|(_, _, bytes)| bytes.clone()))
        }

        fn delete_generic_password(
            &self,
            service: &str,
            account: &str,
        ) -> Result<(), KeychainError> {
            if !self.available {
                return Err(KeychainError::Other);
            }
            let mut entries = self.entries.lock().unwrap();
            entries.retain(|(s, a, _)| s != service || a != account);
            Ok(())
        }
    }

    #[test]
    fn keychain_store_round_trips_through_injected_fake() {
        let client = Arc::new(FakeKeychainClient::new(true));
        let store = KeychainSecretStore::new(client, "svc".into(), "acct".into());
        assert!(store.is_available());

        let secret = SecretString::from("pässwörd".to_string());
        store.store(&secret).unwrap();
        let loaded = store.load().unwrap().expect("stored secret");
        assert_eq!(loaded.expose_secret(), "pässwörd");

        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn unavailable_keychain_reports_unavailable_and_never_persists() {
        let client = Arc::new(FakeKeychainClient::new(false));
        let store = KeychainSecretStore::new(client, "svc".into(), "acct".into());
        assert!(!store.is_available());

        let secret = SecretString::from("secret".to_string());
        assert_eq!(
            store.store(&secret).unwrap_err(),
            SecretStoreError::Unavailable
        );
        assert!(store.load().unwrap().is_none());
    }
}
