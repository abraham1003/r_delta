use std::fs::File;
use std::io::{self, Read, Write, BufReader};
use std::path::Path;
use sha2::{Sha256, Digest};

pub const CHUNK_SIZE: usize = 4096; // 4KB
const ADLER_MOD: u32 = 65521; // Adler-32 modulus (largest prime less than 2^16)

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSignature{
    pub index: u64, // file chunk index
    pub weak_hash: u32, // fast, used for initial match
    pub strong_hash: [u8; 32] // slow, used for verification
}

impl ChunkSignature {
    pub fn new(index: u64, weak_hash: u32, strong_hash: [u8; 32]) -> Self {
        Self {
            index,
            weak_hash,
            strong_hash
        }
    }

    pub fn strong_hash_hex(&self) -> String {hex::encode(self.strong_hash)}
}

#[derive(Debug, Clone)]
pub struct Adler32 {
    a: u32,
    b: u32
}

impl Adler32 {
    pub fn new() -> Self {
        Self {a: 1, b: 0}
    }

    pub fn hash(data: &[u8]) -> u32 {
        let mut hasher = Self::new();
        hasher.update(data);
        hasher.finalize()
    }

    pub fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.a = (self.a + byte as u32) % ADLER_MOD;
            self.b = (self.b + self.a) % ADLER_MOD;
        }
    }

    pub fn finalize(&self) -> u32 {
        (self.b << 16) | self.a
    }

    pub fn roll(&mut self, old_byte: u8, new_byte: u8, window_size: usize) {
        let old_byte = old_byte as u32;
        let new_byte = new_byte as u32;
        let n = window_size as u32;

        self.a = (self.a + ADLER_MOD - old_byte + new_byte) % ADLER_MOD;
        let b_remove = (n * old_byte + 1) % ADLER_MOD;
        self.b = (self.b + ADLER_MOD * 2 - b_remove + self.a) % ADLER_MOD;
    }

    pub fn reset(&mut self) {
        self.a = 1;
        self.b = 0;
    }
}

impl Default for Adler32 {
    fn default() -> Self {
        Self::new()
    }
}

fn compute_strong_hash(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    result.into()
}

pub fn generate_signature<P: AsRef<Path>>(old_file_path: P, sig_file_path: P)
    -> io::Result<Vec<ChunkSignature>> {
    let old_file = File::open(old_file_path)?;
    let mut reader = BufReader::new(old_file);
    let mut signatures = Vec::new();
    let mut chunk_index: u64 = 0;
    let mut buffer = vec![0u8; CHUNK_SIZE];
    loop {
        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }

        let chunk_data = &buffer[..bytes_read];
        let weak_hash = Adler32::hash(chunk_data);
        let strong_hash = compute_strong_hash(chunk_data);
        let signature = ChunkSignature::new(chunk_index, weak_hash, strong_hash);
        signatures.push(signature);
        chunk_index += 1;
    }

    write_signature_file(&sig_file_path, &signatures)?;
    Ok(signatures)
}

fn write_signature_file<P: AsRef<Path>>(path: P, signatures: &[ChunkSignature])
    -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(&(signatures.len() as u64).to_le_bytes())?;

    for sig in signatures {
        file.write_all(&sig.index.to_le_bytes())?;
        file.write_all(&sig.weak_hash.to_le_bytes())?;
        file.write_all(&sig.strong_hash)?;
    }

    Ok(())
}

pub fn read_signature_file<P: AsRef<Path>>(path: P) -> io::Result<Vec<ChunkSignature>> {
    let mut file = File::open(path)?;
    let mut buffer = [0u8; 8];
    file.read_exact(&mut buffer)?;
    let num_chunks = u64::from_le_bytes(buffer);
    let mut signatures = Vec::with_capacity(num_chunks as usize);

    for _ in 0..num_chunks {
        // Read index
        file.read_exact(&mut buffer)?;
        let index = u64::from_le_bytes(buffer);

        // Read weak hash
        let mut weak_buffer = [0u8; 4];
        file.read_exact(&mut weak_buffer)?;
        let weak_hash = u32::from_le_bytes(weak_buffer);

        // Read strong hash
        let mut strong_hash = [0u8; 32];
        file.read_exact(&mut strong_hash)?;

        signatures.push(ChunkSignature::new(index, weak_hash, strong_hash));
    }

    Ok(signatures)
}

pub fn print_signature_stats(signatures: &[ChunkSignature]) {
    println!("===Signature Statistics===");
    println!("Total chunks: {}", signatures.len());
    println!("Chunk Size: {} bytes", CHUNK_SIZE);
    println!("Total data size: ~{} bytes", signatures.len() * CHUNK_SIZE);
    println!("Signature file size: {} bytes", 8 + signatures.len() * 44);
    println!("\nFirst 5 signatures:");
    for (i, sig) in signatures.iter().take(5).enumerate() {
        println!(
            "Chunk {}: weak=0x{:08x}, strong={}",
            i, sig.weak_hash, &sig.strong_hash_hex()[..16]
        );
    }
}