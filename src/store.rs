/// Encrypted persistent key-value store.
///
/// Stores minimal bot settings (group model selections) encrypted at rest
/// using ChaCha20 with a key derived from the API key.
///
/// File format: 12-byte nonce + ChaCha20-encrypted UTF-8 lines of "key=value".
/// The file is rewritten atomically on every save.
use crate::crypto;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

pub struct EncryptedStore {
    path: PathBuf,
    secret: String,
    data: HashMap<String, String>,
}

impl EncryptedStore {
    /// Create or load a store. If the file exists, decrypt and load it.
    /// If it doesn't exist or can't be decrypted, start empty.
    pub fn open(path: &str, secret: &str) -> Self {
        let path = PathBuf::from(path);
        let mut store = EncryptedStore {
            path,
            secret: secret.to_string(),
            data: HashMap::new(),
        };
        store.load();
        store
    }

    /// Get a value by key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.data.get(key).map(|s| s.as_str())
    }

    /// Set a key-value pair and persist to disk.
    pub fn set(&mut self, key: &str, value: &str) {
        self.data.insert(key.to_string(), value.to_string());
        if let Err(e) = self.save() {
            eprintln!("Failed to persist store: {}", e);
        }
    }

    /// Remove a key and persist.
    #[allow(dead_code)]
    pub fn remove(&mut self, key: &str) {
        self.data.remove(key);
        if let Err(e) = self.save() {
            eprintln!("Failed to persist store: {}", e);
        }
    }

    /// Load and decrypt the store from disk.
    fn load(&mut self) {
        let raw = match fs::read(&self.path) {
            Ok(data) => data,
            Err(_) => return, // File doesn't exist yet — start empty
        };

        let plaintext = match crypto::decrypt(&self.secret, &raw) {
            Ok(pt) => pt,
            Err(e) => {
                eprintln!("Warning: could not decrypt store ({}), starting fresh", e);
                return;
            }
        };

        let text = match String::from_utf8(plaintext) {
            Ok(t) => t,
            Err(_) => {
                eprintln!("Warning: store contains invalid UTF-8 after decryption, starting fresh");
                return;
            }
        };

        for line in text.lines() {
            // Use tab as delimiter — can't appear in base64 group IDs or model names
            if let Some((key, value)) = line.split_once('\t') {
                let key = key.trim();
                let value = value.trim();
                if !key.is_empty() {
                    self.data.insert(key.to_string(), value.to_string());
                }
            }
        }
    }

    /// Encrypt and write the store to disk atomically.
    fn save(&self) -> Result<(), String> {
        // Serialize
        let mut content = String::new();
        for (key, value) in &self.data {
            content.push_str(key);
            content.push('\t');
            content.push_str(value);
            content.push('\n');
        }

        // Encrypt
        let encrypted = crypto::encrypt(&self.secret, content.as_bytes());

        // Write atomically: write to temp file, then rename
        let tmp_path = self.path.with_extension("tmp");

        // Ensure parent directory exists with restricted permissions
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create store directory: {}", e))?;
        }

        fs::write(&tmp_path, &encrypted)
            .map_err(|e| format!("Failed to write temp store: {}", e))?;

        // Set restrictive permissions before rename
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            fs::set_permissions(&tmp_path, perms)
                .map_err(|e| format!("Failed to set permissions: {}", e))?;
        }

        fs::rename(&tmp_path, &self.path).map_err(|e| format!("Failed to rename store: {}", e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_path(name: &str) -> String {
        format!("/tmp/signal-bot-test-{}-{}", name, std::process::id())
    }

    #[test]
    fn test_store_set_and_get() {
        let path = temp_path("set-get");
        let mut store = EncryptedStore::open(&path, "test-secret");
        store.set("group.abc", "dolphin-mistral");
        assert_eq!(store.get("group.abc"), Some("dolphin-mistral"));
        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_store_persistence() {
        let path = temp_path("persist");
        {
            let mut store = EncryptedStore::open(&path, "test-secret");
            store.set("group.abc", "llama3:8b");
            store.set("group.xyz", "qwen3:14b");
        }
        // Reopen — should load persisted data
        {
            let store = EncryptedStore::open(&path, "test-secret");
            assert_eq!(store.get("group.abc"), Some("llama3:8b"));
            assert_eq!(store.get("group.xyz"), Some("qwen3:14b"));
        }
        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_store_wrong_secret_starts_fresh() {
        let path = temp_path("wrong-secret");
        {
            let mut store = EncryptedStore::open(&path, "correct-secret");
            store.set("key", "value");
        }
        {
            let store = EncryptedStore::open(&path, "wrong-secret");
            // Should not be able to read the data
            assert!(store.get("key").is_none() || store.get("key") != Some("value"));
        }
        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_store_remove() {
        let path = temp_path("remove");
        let mut store = EncryptedStore::open(&path, "secret");
        store.set("a", "1");
        store.set("b", "2");
        store.remove("a");
        assert!(store.get("a").is_none());
        assert_eq!(store.get("b"), Some("2"));
        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_store_overwrite() {
        let path = temp_path("overwrite");
        let mut store = EncryptedStore::open(&path, "secret");
        store.set("key", "old");
        store.set("key", "new");
        assert_eq!(store.get("key"), Some("new"));
        fs::remove_file(&path).ok();
    }

    #[test]
    fn test_store_debug_load() {
        // Test loading the actual state file if it exists
        let path = format!(
            "{}/.config/signal-bot-crawly/state.enc",
            std::env::var("HOME").unwrap_or_default()
        );
        let secret = std::env::var("OPEN_WEBUI_API_KEY").unwrap_or_else(|_| "no-key".to_string());
        if std::fs::metadata(&path).is_ok() && secret != "no-key" {
            let store = EncryptedStore::open(&path, &secret);
            eprintln!("Store entries:");
            for (k, v) in &store.data {
                eprintln!("  '{}' = '{}'", k, v);
            }
        }
    }

    #[test]
    fn test_store_nonexistent_file_starts_empty() {
        let store = EncryptedStore::open("/tmp/signal-bot-nonexistent-file", "secret");
        assert!(store.get("anything").is_none());
    }

    #[test]
    fn test_store_file_permissions() {
        let path = temp_path("perms");
        let mut store = EncryptedStore::open(&path, "secret");
        store.set("k", "v");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs::metadata(&path).unwrap();
            let mode = metadata.permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        fs::remove_file(&path).ok();
    }
}
