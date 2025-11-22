use anyhow::Result;
use std::borrow::Cow;
use crate::crypto::{self, KeyContext};

pub trait DataPipeline {
    fn process<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>>;
    fn reverse<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>>;
}

pub struct PlainPipeline;

impl DataPipeline for PlainPipeline {
    fn process<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        Ok(Cow::Borrowed(data))
    }

    fn reverse<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        Ok(Cow::Borrowed(data))
    }
}

pub struct CompressedPipeline {
    compression_level: i32,
}

impl CompressedPipeline {
    pub fn new(compression_level: i32) -> Self {
        Self { compression_level }
    }
}

impl DataPipeline for CompressedPipeline {
    fn process<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        let compressed = zstd::encode_all(data, self.compression_level)?;
        Ok(Cow::Owned(compressed))
    }

    fn reverse<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        let decompressed = zstd::decode_all(data)?;
        Ok(Cow::Owned(decompressed))
    }
}

pub struct SecurePipeline {
    key: KeyContext,
    compression_level: i32,
}

impl SecurePipeline {
    pub fn new(key: KeyContext, compression_level: i32) -> Self {
        Self { key, compression_level }
    }

    pub fn with_default_compression(key: KeyContext) -> Self {
        Self::new(key, 3)
    }
}

impl DataPipeline for SecurePipeline {
    fn process<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        let compressed = zstd::encode_all(data, self.compression_level)?;
        let encrypted = crypto::encrypt_chunk(&compressed, &self.key)?;
        Ok(Cow::Owned(encrypted))
    }

    fn reverse<'a>(&self, data: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        let decrypted = crypto::decrypt_chunk(data, &self.key)?;
        let decompressed = zstd::decode_all(decrypted.as_slice())?;
        Ok(Cow::Owned(decompressed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plain_pipeline() {
        let pipeline = PlainPipeline;
        let data = b"test data";
        
        let processed = pipeline.process(data).unwrap();
        assert_eq!(processed.as_ref(), data);
        
        let reversed = pipeline.reverse(&processed).unwrap();
        assert_eq!(reversed.as_ref(), data);
    }

    #[test]
    fn test_compressed_pipeline() {
        let pipeline = CompressedPipeline::new(3);
        let data = b"test data test data test data test data test data";
        
        let compressed = pipeline.process(data).unwrap();
        assert!(compressed.len() < data.len());
        
        let decompressed = pipeline.reverse(&compressed).unwrap();
        assert_eq!(decompressed.as_ref(), data);
    }

    #[test]
    fn test_secure_pipeline_roundtrip() {
        let key = KeyContext::new_random();
        let pipeline = SecurePipeline::with_default_compression(key);
        
        let plaintext = b"The quick brown fox jumps over the lazy dog";
        
        let encrypted = pipeline.process(plaintext).unwrap();
        assert_ne!(encrypted.as_ref(), plaintext);
        assert!(encrypted.len() > plaintext.len());
        
        let decrypted = pipeline.reverse(&encrypted).unwrap();
        assert_eq!(decrypted.as_ref(), plaintext);
    }

    #[test]
    fn test_secure_pipeline_compresses_before_encrypt() {
        let key = KeyContext::new_random();
        let pipeline = SecurePipeline::new(key, 9);
        
        let repetitive_data = vec![b'A'; 1024];
        
        let encrypted = pipeline.process(&repetitive_data).unwrap();
        
        let overhead = 24 + 16;
        assert!(encrypted.len() < repetitive_data.len() + overhead);
        
        let decrypted = pipeline.reverse(&encrypted).unwrap();
        assert_eq!(decrypted.as_ref(), repetitive_data.as_slice());
    }

    #[test]
    fn test_secure_pipeline_wrong_key_fails() {
        let key1 = KeyContext::new_random();
        let key2 = KeyContext::new_random();
        
        let pipeline1 = SecurePipeline::with_default_compression(key1);
        let pipeline2 = SecurePipeline::with_default_compression(key2);
        
        let data = b"secret message";
        let encrypted = pipeline1.process(data).unwrap();
        
        let result = pipeline2.reverse(&encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn test_secure_pipeline_large_data() {
        let key = KeyContext::new_random();
        let pipeline = SecurePipeline::with_default_compression(key);
        
        let large_data = vec![0x42u8; 1024 * 1024];
        
        let encrypted = pipeline.process(&large_data).unwrap();
        let decrypted = pipeline.reverse(&encrypted).unwrap();
        
        assert_eq!(decrypted.as_ref(), large_data.as_slice());
    }
}
