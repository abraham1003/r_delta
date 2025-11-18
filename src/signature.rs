use std::fs::File;
use std::io::{self, Read, Write, BufWriter};
use std::path::Path;
use blake3;
use crate::chunker::Chunker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSignature {
    pub offset: u64,
    pub length: usize,
    pub weak_hash: u64,
    pub strong_hash: [u8; 32],
}

impl ChunkSignature {
    pub fn new(offset: u64, length: usize, weak_hash: u64, strong_hash: [u8; 32]) -> Self {
        Self {
            offset,
            length,
            weak_hash,
            strong_hash,
        }
    }

    pub fn strong_hash_hex(&self) -> String {
        hex::encode(self.strong_hash)
    }
}

#[inline]
fn compute_weak_hash(data: &[u8]) -> u64 {
    let mut hash = 0u64;
    for &byte in data {
        hash = hash.wrapping_mul(31).wrapping_add(byte as u64);
    }
    hash
}

#[inline]
fn compute_strong_hash(data: &[u8]) -> [u8; 32] {
    *blake3::hash(data).as_bytes()
}

pub fn generate_signature<P: AsRef<Path>>(
    old_file_path: P,
    sig_file_path: P,
) -> io::Result<Vec<ChunkSignature>> {
    let old_file = File::open(old_file_path)?;
    let mut chunker = Chunker::new(old_file);
    let mut signatures = Vec::new();

    while let Some(chunk) = chunker.next_chunk()? {
        let weak_hash = compute_weak_hash(&chunk.data);
        let strong_hash = compute_strong_hash(&chunk.data);

        let signature = ChunkSignature::new(
            chunk.offset,
            chunk.length,
            weak_hash,
            strong_hash,
        );

        signatures.push(signature);
    }

    write_signature_file(&sig_file_path, &signatures)?;
    Ok(signatures)
}

fn write_signature_file<P: AsRef<Path>>(path: P, signatures: &[ChunkSignature]) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);

    writer.write_all(&(signatures.len() as u64).to_le_bytes())?;

    for sig in signatures {
        writer.write_all(&sig.offset.to_le_bytes())?;
        writer.write_all(&(sig.length as u64).to_le_bytes())?;
        writer.write_all(&sig.weak_hash.to_le_bytes())?;
        writer.write_all(&sig.strong_hash)?;
    }

    writer.flush()?;
    Ok(())
}

pub fn read_signature_file<P: AsRef<Path>>(path: P) -> io::Result<Vec<ChunkSignature>> {
    let mut file = File::open(path)?;
    let mut buffer = [0u8; 8];
    file.read_exact(&mut buffer)?;
    let num_chunks = u64::from_le_bytes(buffer);
    let mut signatures = Vec::with_capacity(num_chunks as usize);

    for _ in 0..num_chunks {
        file.read_exact(&mut buffer)?;
        let offset = u64::from_le_bytes(buffer);

        file.read_exact(&mut buffer)?;
        let length = u64::from_le_bytes(buffer) as usize;

        file.read_exact(&mut buffer)?;
        let weak_hash = u64::from_le_bytes(buffer);

        let mut strong_hash = [0u8; 32];
        file.read_exact(&mut strong_hash)?;

        signatures.push(ChunkSignature::new(offset, length, weak_hash, strong_hash));
    }

    Ok(signatures)
}
