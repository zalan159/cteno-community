//! Host-side OpenRouter subkey cache.
//!
//! The desktop daemon fetches a per-user subkey from `/v1/openrouter/subkey/issue`
//! on boot (or on 401/402 from openrouter.ai) and caches it here so every
//! cteno-agent session reads the same plaintext key.
//!
//! Pattern mirrors `AuthStore`: JSON-on-disk, in-memory snapshot, atomic
//! writes (tmp + rename), default empty on missing/corrupt.
//!
//! SECURITY NOTE: The subkey is stored as plaintext in `llm_key.json`, same
//! as `access_token` / `refresh_token` in `auth.json`. OS-level file
//! permissions on the app data dir are the security boundary. OpenRouter
//! subkeys have bounded spend (limit matches user balance), so leakage
//! damage is scoped.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

pub const LLM_KEY_STORE_FILE: &str = "llm_key.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LlmKeyRecord {
    /// OpenRouter subkey plaintext (e.g. "sk-or-v1-...").
    #[serde(default)]
    pub subkey: Option<String>,
    /// OpenRouter-assigned stable hash for this key.
    #[serde(default)]
    pub hash: Option<String>,
    /// UNIX millis when this record was written.
    #[serde(default)]
    pub fetched_at_ms: u64,
    /// Monotonic counter bumped on every rotate; useful for debug/telemetry.
    #[serde(default)]
    pub rotation_counter: u64,
}

impl LlmKeyRecord {
    pub fn is_present(&self) -> bool {
        self.subkey.is_some() && self.hash.is_some()
    }
}

/// Concurrent-safe cache of the current OpenRouter subkey.
pub struct LlmKeyStore {
    inner: Arc<RwLock<LlmKeyRecord>>,
    path: PathBuf,
}

impl LlmKeyStore {
    /// Load from disk. Missing file → empty record (Ok). Corrupt file → empty
    /// record + warning log (also Ok; never abort boot for a cache file).
    pub fn load(app_data_dir: &Path) -> std::io::Result<Self> {
        let path = app_data_dir.join(LLM_KEY_STORE_FILE);
        let snapshot = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<LlmKeyRecord>(&bytes).unwrap_or_else(|err| {
                eprintln!(
                    "[cteno-host-runtime] llm_key.json unreadable ({}); resetting cache",
                    err
                );
                LlmKeyRecord::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => LlmKeyRecord::default(),
            Err(e) => return Err(e),
        };
        Ok(Self {
            inner: Arc::new(RwLock::new(snapshot)),
            path,
        })
    }

    /// Cheap snapshot for the current session to read the key.
    pub fn current(&self) -> LlmKeyRecord {
        self.inner.read().expect("llm_key lock poisoned").clone()
    }

    /// Read just the subkey plaintext if present. Returned as an owned String
    /// so callers hold no lock while performing network I/O.
    pub fn subkey(&self) -> Option<String> {
        self.inner
            .read()
            .expect("llm_key lock poisoned")
            .subkey
            .clone()
    }

    /// Atomic replace: update in-memory snapshot + persist to disk.
    pub fn replace(&self, mut record: LlmKeyRecord) -> std::io::Result<()> {
        // Bump rotation counter so subscribers can detect change cheaply.
        let prev = self.inner.read().expect("llm_key lock poisoned").clone();
        if record.rotation_counter <= prev.rotation_counter {
            record.rotation_counter = prev.rotation_counter + 1;
        }
        persist(&self.path, &record)?;
        *self.inner.write().expect("llm_key lock poisoned") = record;
        Ok(())
    }

    /// Forget everything. Deletes the on-disk file; memory resets to empty.
    pub fn clear(&self) -> std::io::Result<()> {
        *self.inner.write().expect("llm_key lock poisoned") = LlmKeyRecord::default();
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn persist(path: &Path, record: &LlmKeyRecord) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = tempdir().unwrap();
        let store = LlmKeyStore::load(dir.path()).unwrap();
        assert!(!store.current().is_present());
        assert!(store.subkey().is_none());
    }

    #[test]
    fn replace_then_reload_round_trip() {
        let dir = tempdir().unwrap();
        let store = LlmKeyStore::load(dir.path()).unwrap();
        store
            .replace(LlmKeyRecord {
                subkey: Some("sk-or-v1-test".to_string()),
                hash: Some("h1".to_string()),
                fetched_at_ms: 1700000000000,
                rotation_counter: 0,
            })
            .unwrap();

        let reloaded = LlmKeyStore::load(dir.path()).unwrap();
        let rec = reloaded.current();
        assert_eq!(rec.subkey.as_deref(), Some("sk-or-v1-test"));
        assert_eq!(rec.hash.as_deref(), Some("h1"));
        assert_eq!(rec.rotation_counter, 1); // bumped on replace
    }

    #[test]
    fn corrupt_file_resets_to_empty() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(LLM_KEY_STORE_FILE), b"{ not json").unwrap();
        let store = LlmKeyStore::load(dir.path()).unwrap();
        assert!(!store.current().is_present());
    }

    #[test]
    fn clear_removes_file_and_state() {
        let dir = tempdir().unwrap();
        let store = LlmKeyStore::load(dir.path()).unwrap();
        store
            .replace(LlmKeyRecord {
                subkey: Some("k".into()),
                hash: Some("h".into()),
                ..Default::default()
            })
            .unwrap();
        assert!(dir.path().join(LLM_KEY_STORE_FILE).exists());
        store.clear().unwrap();
        assert!(!dir.path().join(LLM_KEY_STORE_FILE).exists());
        assert!(!store.current().is_present());
    }
}
