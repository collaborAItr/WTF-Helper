//! The device token lives only in the macOS Keychain or Windows Credential Manager.

use std::sync::Mutex;

use crate::flavor::FLAVOR;

const ACCOUNT: &str = "device-token";

pub trait TokenStore: Send + Sync {
    fn get(&self) -> Option<String>;
    fn set(&self, token: &str) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
}

pub struct KeychainStore;

impl KeychainStore {
    fn entry() -> Result<keyring::Entry, String> {
        keyring::Entry::new(FLAVOR.keychain_service, ACCOUNT).map_err(|e| e.to_string())
    }
}

impl TokenStore for KeychainStore {
    fn get(&self) -> Option<String> {
        Self::entry()
            .ok()?
            .get_password()
            .ok()
            .filter(|t| !t.is_empty())
    }

    fn set(&self, token: &str) -> Result<(), String> {
        Self::entry()?
            .set_password(token)
            .map_err(|e| e.to_string())
    }

    fn delete(&self) -> Result<(), String> {
        match Self::entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[derive(Default)]
pub struct MemoryStore(pub Mutex<Option<String>>);

impl TokenStore for MemoryStore {
    fn get(&self) -> Option<String> {
        self.0.lock().unwrap().clone()
    }

    fn set(&self, token: &str) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(token.to_string());
        Ok(())
    }

    fn delete(&self) -> Result<(), String> {
        *self.0.lock().unwrap() = None;
        Ok(())
    }
}
