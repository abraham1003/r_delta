use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use std::fs;
use crate::crypto::KeyContext;

const APP_NAME: &str = "r_delta";
const KEY_FILENAME: &str = "secret.key";

pub struct EncryptionManager;

impl EncryptionManager {
    pub fn get_default_key_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| anyhow::anyhow!("Unable to determine config directory"))?;
        
        let app_dir = config_dir.join(APP_NAME);
        Ok(app_dir.join(KEY_FILENAME))
    }

    pub fn get_default_key() -> Result<KeyContext> {
        let key_path = Self::get_default_key_path()?;
        
        if key_path.exists() {
            KeyContext::load_from_file(&key_path)
                .context("Failed to load existing encryption key")
        } else {
            let app_dir = key_path.parent()
                .ok_or_else(|| anyhow::anyhow!("Invalid key path"))?;
            
            fs::create_dir_all(app_dir)
                .context("Failed to create config directory")?;
            
            let key = KeyContext::new_random();
            key.save_to_file(&key_path)
                .context("Failed to save generated encryption key")?;
            
            eprintln!("Generated new encryption key at: {}", key_path.display());
            
            Ok(key)
        }
    }

    pub fn load_or_generate(path: Option<PathBuf>) -> Result<KeyContext> {
        match path {
            Some(p) => {
                if !p.exists() {
                    bail!("Specified key file does not exist: {}", p.display());
                }
                KeyContext::load_from_file(&p)
                    .context(format!("Failed to load key from {}", p.display()))
            }
            None => Self::get_default_key(),
        }
    }

    pub fn ensure_key_exists(path: Option<&Path>) -> Result<PathBuf> {
        match path {
            Some(p) => {
                if !p.exists() {
                    let key = KeyContext::new_random();
                    if let Some(parent) = p.parent() {
                        fs::create_dir_all(parent)
                            .context("Failed to create key directory")?;
                    }
                    key.save_to_file(p)
                        .context("Failed to save key")?;
                    eprintln!("Generated new encryption key at: {}", p.display());
                }
                Ok(p.to_path_buf())
            }
            None => {
                let default_path = Self::get_default_key_path()?;
                if !default_path.exists() {
                    Self::get_default_key()?;
                }
                Ok(default_path)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_get_default_key_path() {
        let path = EncryptionManager::get_default_key_path().unwrap();
        assert!(path.to_string_lossy().contains("r_delta"));
        assert!(path.to_string_lossy().contains("secret.key"));
    }

    #[test]
    fn test_load_or_generate_with_path() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("test.key");
        
        let key = KeyContext::new_random();
        key.save_to_file(&key_path).unwrap();
        
        let loaded = EncryptionManager::load_or_generate(Some(key_path.clone())).unwrap();
        assert_eq!(key.key_bytes(), loaded.key_bytes());
    }

    #[test]
    fn test_load_or_generate_nonexistent_path() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("nonexistent.key");
        
        let result = EncryptionManager::load_or_generate(Some(key_path));
        assert!(result.is_err());
        let err_msg = format!("{}", result.err().unwrap());
        assert!(err_msg.contains("does not exist"));
    }

    #[test]
    fn test_ensure_key_exists_creates_new() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("new_key.key");
        
        assert!(!key_path.exists());
        
        let result = EncryptionManager::ensure_key_exists(Some(&key_path)).unwrap();
        assert_eq!(result, key_path);
        assert!(key_path.exists());
        
        let key = KeyContext::load_from_file(&key_path).unwrap();
        assert_eq!(key.key_bytes().len(), 32);
    }

    #[test]
    fn test_ensure_key_exists_preserves_existing() {
        let temp_dir = TempDir::new().unwrap();
        let key_path = temp_dir.path().join("existing.key");
        
        let original_key = KeyContext::new_random();
        original_key.save_to_file(&key_path).unwrap();
        let original_bytes = *original_key.key_bytes();
        
        EncryptionManager::ensure_key_exists(Some(&key_path)).unwrap();
        
        let loaded_key = KeyContext::load_from_file(&key_path).unwrap();
        assert_eq!(*loaded_key.key_bytes(), original_bytes);
    }
}
