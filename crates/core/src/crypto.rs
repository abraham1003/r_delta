use anyhow::{Context, Result, bail};
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng, AeadCore},
    XChaCha20Poly1305, XNonce,
};
use rand::RngCore;
use zeroize::{Zeroize, ZeroizeOnDrop};
use std::fs;
use std::path::Path;

const NONCE_SIZE: usize = 24;
const TAG_SIZE: usize = 16;
const KEY_SIZE: usize = 32;

#[derive(Zeroize, ZeroizeOnDrop)]
pub struct KeyContext {
    #[zeroize(skip)]
    cipher: XChaCha20Poly1305,
    key_bytes: [u8; KEY_SIZE],
}

impl KeyContext {
    pub fn new_random() -> Self {
        let mut key_bytes = [0u8; KEY_SIZE];
        OsRng.fill_bytes(&mut key_bytes);
        
        let cipher = XChaCha20Poly1305::new((&key_bytes).into());
        
        Self { cipher, key_bytes }
    }

    pub fn from_bytes(key: [u8; KEY_SIZE]) -> Self {
        let cipher = XChaCha20Poly1305::new((&key).into());
        Self { cipher, key_bytes: key }
    }

    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let hex_str = fs::read_to_string(path.as_ref())
            .context("Failed to read key file")?;
        
        let hex_str = hex_str.trim();
        
        if hex_str.len() != KEY_SIZE * 2 {
            bail!("Invalid key length: expected {} hex characters, got {}", 
                KEY_SIZE * 2, hex_str.len());
        }

        let mut key_bytes = [0u8; KEY_SIZE];
        hex::decode_to_slice(hex_str, &mut key_bytes)
            .context("Invalid hex encoding in key file")?;

        Ok(Self::from_bytes(key_bytes))
    }

    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let hex_str = hex::encode(&self.key_bytes);
        fs::write(path.as_ref(), hex_str)
            .context("Failed to write key file")?;
        
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(path.as_ref())?.permissions();
            perms.set_mode(0o600);
            fs::set_permissions(path.as_ref(), perms)?;
        }
        
        Ok(())
    }

    pub fn key_bytes(&self) -> &[u8; KEY_SIZE] {
        &self.key_bytes
    }
}

pub fn encrypt_chunk(data: &[u8], key: &KeyContext) -> Result<Vec<u8>> {
    if data.is_empty() {
        bail!("Cannot encrypt empty data");
    }

    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    
    let ciphertext = key.cipher
        .encrypt(&nonce, data)
        .map_err(|e| anyhow::anyhow!("Encryption failed: {}", e))?;

    let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    result.extend_from_slice(&nonce);
    result.extend_from_slice(&ciphertext);

    Ok(result)
}

pub fn decrypt_chunk(blob: &[u8], key: &KeyContext) -> Result<Vec<u8>> {
    if blob.len() < NONCE_SIZE + TAG_SIZE {
        bail!("Invalid encrypted blob: too short (minimum {} bytes required, got {})",
            NONCE_SIZE + TAG_SIZE, blob.len());
    }

    let (nonce_bytes, ciphertext) = blob.split_at(NONCE_SIZE);
    
    let nonce = XNonce::from_slice(nonce_bytes);

    let plaintext = key.cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| anyhow::anyhow!("Decryption failed (data may be tampered or corrupted): {}", e))?;

    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    #[test]
    fn test_key_generation() {
        let key1 = KeyContext::new_random();
        let key2 = KeyContext::new_random();
        
        assert_ne!(key1.key_bytes(), key2.key_bytes(), "Random keys should be different");
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = KeyContext::new_random();
        let plaintext = b"The quick brown fox jumps over the lazy dog";

        let encrypted = encrypt_chunk(plaintext, &key).unwrap();
        
        assert_ne!(encrypted.as_slice(), plaintext, "Ciphertext should differ from plaintext");
        assert!(encrypted.len() > plaintext.len(), "Encrypted data should include nonce + tag overhead");

        let decrypted = decrypt_chunk(&encrypted, &key).unwrap();
        assert_eq!(decrypted.as_slice(), plaintext, "Decrypted data should match original");
    }

    #[test]
    fn test_nonce_uniqueness() {
        let key = KeyContext::new_random();
        let plaintext = b"same message";

        let encrypted1 = encrypt_chunk(plaintext, &key).unwrap();
        let encrypted2 = encrypt_chunk(plaintext, &key).unwrap();

        assert_ne!(encrypted1, encrypted2, "Same plaintext should produce different ciphertext with different nonces");

        let decrypted1 = decrypt_chunk(&encrypted1, &key).unwrap();
        let decrypted2 = decrypt_chunk(&encrypted2, &key).unwrap();

        assert_eq!(decrypted1, plaintext.to_vec());
        assert_eq!(decrypted2, plaintext.to_vec());
    }

    #[test]
    fn test_wrong_key_fails() {
        let key1 = KeyContext::new_random();
        let key2 = KeyContext::new_random();
        let plaintext = b"secret message";

        let encrypted = encrypt_chunk(plaintext, &key1).unwrap();
        let result = decrypt_chunk(&encrypted, &key2);

        assert!(result.is_err(), "Decryption with wrong key should fail");
    }

    #[test]
    fn test_tampered_data_fails() {
        let key = KeyContext::new_random();
        let plaintext = b"authenticated message";

        let mut encrypted = encrypt_chunk(plaintext, &key).unwrap();
        
        let tamper_index = encrypted.len() - 5;
        encrypted[tamper_index] ^= 0xFF;

        let result = decrypt_chunk(&encrypted, &key);
        assert!(result.is_err(), "Tampered ciphertext should fail authentication");
    }

    #[test]
    fn test_key_save_load() {
        let original_key = KeyContext::new_random();
        let temp_file = NamedTempFile::new().unwrap();
        
        original_key.save_to_file(temp_file.path()).unwrap();
        
        let loaded_key = KeyContext::load_from_file(temp_file.path()).unwrap();
        
        assert_eq!(original_key.key_bytes(), loaded_key.key_bytes(), "Loaded key should match original");
        
        let plaintext = b"test message for key persistence";
        let encrypted = encrypt_chunk(plaintext, &original_key).unwrap();
        let decrypted = decrypt_chunk(&encrypted, &loaded_key).unwrap();
        
        assert_eq!(decrypted.as_slice(), plaintext, "Keys should be functionally identical");
    }

    #[test]
    fn test_invalid_key_file() {
        let mut temp_file = NamedTempFile::new().unwrap();
        write!(temp_file, "invalid_hex_data").unwrap();
        temp_file.flush().unwrap();

        let result = KeyContext::load_from_file(temp_file.path());
        assert!(result.is_err(), "Invalid key file should fail to load");
    }

    #[test]
    fn test_truncated_blob_fails() {
        let key = KeyContext::new_random();
        
        let short_blob = vec![0u8; NONCE_SIZE + TAG_SIZE - 1];
        let result = decrypt_chunk(&short_blob, &key);
        assert!(result.is_err(), "Truncated blob should fail");
    }

    #[test]
    fn test_empty_data_fails() {
        let key = KeyContext::new_random();
        let result = encrypt_chunk(&[], &key);
        assert!(result.is_err(), "Empty data should not be encrypted");
    }

    #[test]
    fn test_large_data() {
        let key = KeyContext::new_random();
        let large_data = vec![0x42u8; 1024 * 1024];

        let encrypted = encrypt_chunk(&large_data, &key).unwrap();
        let decrypted = decrypt_chunk(&encrypted, &key).unwrap();

        assert_eq!(decrypted, large_data, "Large data roundtrip should succeed");
    }

    #[test]
    fn test_key_from_bytes() {
        let key_bytes = [0x42u8; KEY_SIZE];
        let key1 = KeyContext::from_bytes(key_bytes);
        let key2 = KeyContext::from_bytes(key_bytes);

        let plaintext = b"deterministic key test";
        let encrypted1 = encrypt_chunk(plaintext, &key1).unwrap();
        
        let decrypted1 = decrypt_chunk(&encrypted1, &key1).unwrap();
        let decrypted2 = decrypt_chunk(&encrypted1, &key2).unwrap();

        assert_eq!(decrypted1, plaintext.to_vec());
        assert_eq!(decrypted2, plaintext.to_vec());
    }
}
