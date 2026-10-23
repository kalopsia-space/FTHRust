//! Saved SFTP passwords.

use std::collections::HashMap;
use std::sync::Mutex;

/// A password store keyed by service and user.
pub trait Vault: Send + Sync {
    /// The saved password.
    fn get(&self, service: &str, user: &str) -> Result<Option<String>, String>;
    /// Saves a password.
    fn set(&self, service: &str, user: &str, password: &str) -> Result<(), String>;
    /// Removes a password (absent is fine).
    fn delete(&self, service: &str, user: &str) -> Result<(), String>;
}

/// The desktop keyring.
pub struct SecretService;

impl Vault for SecretService {
    fn get(&self, _: &str, _: &str) -> Result<Option<String>, String> {
        Err("Could not read the desktop keyring.".into())
    }
    fn set(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
        Err("Could not save the password in the desktop keyring.".into())
    }
    fn delete(&self, _: &str, _: &str) -> Result<(), String> {
        Err("Could not remove the saved SFTP password.".into())
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
