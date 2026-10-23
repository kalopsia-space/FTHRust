//! Saved SFTP passwords: the desktop keyring (Secret Service) in the real
//! program, memory in tests. The reference only supported Windows
//! Credential Manager.

use std::collections::HashMap;
use std::sync::Mutex;

use sha2::Digest;

/// A password store keyed by service and user.
pub trait Vault: Send + Sync {
    /// The saved password, if any.
    fn get(&self, service: &str, user: &str) -> Result<Option<String>, String>;
    /// Saves a password.
    fn set(&self, service: &str, user: &str, password: &str) -> Result<(), String>;
    /// Removes a password (absent is fine).
    fn delete(&self, service: &str, user: &str) -> Result<(), String>;
}

/// The keyring service name for one host, port and user:
/// `FTHR Clips/SFTP/<sha256 of "host\0port\0user">`.
pub fn service_name(host: &str, port: u16, user: &str) -> String {
    let digest = sha2::Sha256::digest(format!("{host}\0{port}\0{user}").as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("FTHR Clips/SFTP/{hex}")
}

/// The desktop keyring through the Secret Service.
pub struct SecretService;

fn entry(service: &str, user: &str) -> Result<keyring_core::Entry, keyring_core::Error> {
    if keyring_core::get_default_store().is_none() {
        let store = zbus_secret_service_keyring_store::Store::new()?;
        keyring_core::set_default_store(store);
    }
    keyring_core::Entry::new(service, user)
}

impl Vault for SecretService {
    fn get(&self, service: &str, user: &str) -> Result<Option<String>, String> {
        let read = || "Could not read the desktop keyring.".to_owned();
        match entry(service, user).and_then(|e| e.get_password()) {
            Ok(p) => Ok(Some(p)),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(_) => Err(read()),
        }
    }

    fn set(&self, service: &str, user: &str, password: &str) -> Result<(), String> {
        entry(service, user)
            .and_then(|e| e.set_password(password))
            .map_err(|_| "Could not save the password in the desktop keyring.".to_owned())
    }

    fn delete(&self, service: &str, user: &str) -> Result<(), String> {
        match entry(service, user).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(_) => Err("Could not remove the saved SFTP password.".to_owned()),
        }
    }
}

/// An in-memory store for tests.
#[derive(Default)]
pub struct Memory(Mutex<HashMap<(String, String), String>>);

impl Vault for Memory {
    fn get(&self, service: &str, user: &str) -> Result<Option<String>, String> {
        let m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        Ok(m.get(&(service.to_owned(), user.to_owned())).cloned())
    }

    fn set(&self, service: &str, user: &str, password: &str) -> Result<(), String> {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.insert((service.to_owned(), user.to_owned()), password.to_owned());
        Ok(())
    }

    fn delete(&self, service: &str, user: &str) -> Result<(), String> {
        let mut m = self.0.lock().unwrap_or_else(|e| e.into_inner());
        m.remove(&(service.to_owned(), user.to_owned()));
        Ok(())
    }
}
