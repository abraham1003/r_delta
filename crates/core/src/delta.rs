use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write, BufReader, BufWriter};
use std::path::Path;
use blake3;
use crate::signature::ChunkSignature;
use crate::chunker::Chunker;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchInstruction {
    Copy(u64, usize),
    Literal(Vec<u8>),
    CompressedLiteral {
        decompressed_len: u64,
        compressed_data: Vec<u8>,
    },
}

impl PatchInstruction {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            PatchInstruction::Copy(offset, length) => {
                let mut bytes = Vec::new();
                bytes.push(0x01);
                bytes.extend_from_slice(&offset.to_le_bytes());
                bytes.extend_from_slice(&(*length as u64).to_le_bytes());
                bytes
            }
            PatchInstruction::Literal(data) => {
                let mut bytes = Vec::new();
                bytes.push(0x02);
                bytes.extend_from_slice(&(data.len() as u64).to_le_bytes());
                bytes.extend_from_slice(data);
                bytes
            }
            PatchInstruction::CompressedLiteral { decompressed_len, compressed_data } => {
                let mut bytes = Vec::new();
                bytes.push(0x03);
                bytes.extend_from_slice(&decompressed_len.to_le_bytes());
                bytes.extend_from_slice(&(compressed_data.len() as u32).to_le_bytes());
                bytes.extend_from_slice(compressed_data);
                bytes
            }
        }
    }

    pub fn from_bytes(tag: u8, reader: &mut dyn Read) -> io::Result<Self> {
        match tag {
            0x01 => {
                let mut offset_buf = [0u8; 8];
                let mut length_buf = [0u8; 8];
                reader.read_exact(&mut offset_buf)?;
                reader.read_exact(&mut length_buf)?;
                let offset = u64::from_le_bytes(offset_buf);
                let length = u64::from_le_bytes(length_buf) as usize;
                Ok(PatchInstruction::Copy(offset, length))
            }
            0x02 => {
                let mut length_buf = [0u8; 8];
                reader.read_exact(&mut length_buf)?;
                let length = u64::from_le_bytes(length_buf) as usize;
                let mut data = vec![0u8; length];
                reader.read_exact(&mut data)?;
                Ok(PatchInstruction::Literal(data))
            }
            0x03 => {
                let mut decompressed_len_buf = [0u8; 8];
                let mut compressed_len_buf = [0u8; 4];
                reader.read_exact(&mut decompressed_len_buf)?;
                reader.read_exact(&mut compressed_len_buf)?;
                let decompressed_len = u64::from_le_bytes(decompressed_len_buf);
                let compressed_len = u32::from_le_bytes(compressed_len_buf) as usize;
                let mut compressed_data = vec![0u8; compressed_len];
                reader.read_exact(&mut compressed_data)?;
                Ok(PatchInstruction::CompressedLiteral {
                    decompressed_len,
                    compressed_data,
                })
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unknown patch instruction tag: {}", tag),
            )),
        }
    }
}

pub struct SignatureMap {
    weak_hash_map: HashMap<u64, Vec<ChunkSignature>>,
}

impl SignatureMap {
    pub fn new(signatures: Vec<ChunkSignature>) -> Self {
        let mut weak_hash_map: HashMap<u64, Vec<ChunkSignature>> = HashMap::new();

        for sig in signatures {
            weak_hash_map
                .entry(sig.weak_hash)
                .or_default()
                .push(sig);
        }

        Self { weak_hash_map }
    }

    pub fn lookup(&self, weak_hash: u64) -> Option<&Vec<ChunkSignature>> {
        self.weak_hash_map.get(&weak_hash)
    }
}

pub struct DeltaGenerator {
    signature_map: SignatureMap,
}

impl DeltaGenerator {
    pub fn new(signatures: Vec<ChunkSignature>) -> Self {
        Self {
            signature_map: SignatureMap::new(signatures),
        }
    }

    #[inline]
    fn compute_weak_hash(&self, data: &[u8]) -> u64 {
        let mut hash = 0u64;
        for &byte in data {
            hash = hash.wrapping_mul(31).wrapping_add(byte as u64);
        }
        hash
    }

    #[inline]
    fn compute_strong_hash(&self, data: &[u8]) -> [u8; 32] {
        *blake3::hash(data).as_bytes()
    }

    pub fn find_match(&self, data: &[u8]) -> Option<(u64, usize)> {
        let weak_hash = self.compute_weak_hash(data);
        let candidates = self.signature_map.lookup(weak_hash)?;
        let strong_hash = self.compute_strong_hash(data);

        for candidate in candidates {
            if candidate.strong_hash == strong_hash && candidate.length == data.len() {
                return Some((candidate.offset, candidate.length));
            }
        }
        None
    }

    pub fn generate_delta<P: AsRef<Path>>(
        &self,
        new_file_path: P,
        patch_file_path: P,
    ) -> io::Result<DeltaStats> {
        const MAX_LITERAL_BUFFER: usize = 8 * 1024 * 1024;
        const COMPRESSION_THRESHOLD: usize = 64;

        let new_file = File::open(&new_file_path)?;
        let file_size = new_file.metadata()?.len() as usize;

        if file_size == 0 {
            let patch_file = File::create(&patch_file_path)?;
            let mut writer = BufWriter::new(patch_file);
            writer.write_all(&0u64.to_le_bytes())?;
            writer.flush()?;
            return Ok(DeltaStats::new());
        }

        let mut chunker = Chunker::new(new_file);
        let mut instructions = Vec::new();
        let mut stats = DeltaStats::new();
        let mut literal_buffer = Vec::new();

        let flush_literal_buffer = |buffer: &mut Vec<u8>,
                                      instructions: &mut Vec<PatchInstruction>,
                                      stats: &mut DeltaStats| {
            if buffer.is_empty() {
                return;
            }

            let literal_len = buffer.len();

            if literal_len < COMPRESSION_THRESHOLD {
                instructions.push(PatchInstruction::Literal(buffer.clone()));
                stats.uncompressed_literal_bytes += literal_len;
            } else {
                match zstd::encode_all(buffer.as_slice(), 3) {
                    Ok(compressed_data) => {
                        if compressed_data.len() < literal_len {
                            instructions.push(PatchInstruction::CompressedLiteral {
                                decompressed_len: literal_len as u64,
                                compressed_data,
                            });
                            stats.compressed_instructions += 1;
                            stats.compressed_literal_bytes += literal_len;
                        } else {
                            instructions.push(PatchInstruction::Literal(buffer.clone()));
                            stats.uncompressed_literal_bytes += literal_len;
                        }
                    }
                    Err(_) => {
                        instructions.push(PatchInstruction::Literal(buffer.clone()));
                        stats.uncompressed_literal_bytes += literal_len;
                    }
                }
            }

            stats.literal_bytes += literal_len;
            stats.literal_instructions += 1;
            buffer.clear();
        };

        while let Some(chunk) = chunker.next_chunk()? {
            if let Some((offset, length)) = self.find_match(&chunk.data) {
                flush_literal_buffer(&mut literal_buffer, &mut instructions, &mut stats);

                instructions.push(PatchInstruction::Copy(offset, length));
                stats.copy_bytes += length;
                stats.copy_instructions += 1;
                stats.matches_found += 1;
            } else {
                literal_buffer.extend_from_slice(&chunk.data);

                if literal_buffer.len() >= MAX_LITERAL_BUFFER {
                    flush_literal_buffer(&mut literal_buffer, &mut instructions, &mut stats);
                }
            }
        }

        flush_literal_buffer(&mut literal_buffer, &mut instructions, &mut stats);

        let optimized_instructions = optimize_instructions(instructions);
        write_patch_file(&patch_file_path, &optimized_instructions)?;

        stats.total_instructions = optimized_instructions.len();
        stats.new_file_size = file_size;
        stats.patch_file_size = std::fs::metadata(patch_file_path)?.len() as usize;

        Ok(stats)
    }
}

fn optimize_instructions(instructions: Vec<PatchInstruction>) -> Vec<PatchInstruction> {
    const MIN_MERGE_THRESHOLD: usize = 3;

    let mut optimized = Vec::new();
    let mut i = 0;

    while i < instructions.len() {
        if let PatchInstruction::Copy(offset, length) = instructions[i] {
            let mut consecutive_copies = vec![(offset, length)];
            let mut j = i + 1;

            while j < instructions.len() {
                if let PatchInstruction::Copy(next_offset, next_length) = instructions[j] {
                    let last = consecutive_copies.last().unwrap();
                    if next_offset == last.0 + last.1 as u64 {
                        consecutive_copies.push((next_offset, next_length));
                        j += 1;
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }

            if consecutive_copies.len() >= MIN_MERGE_THRESHOLD {
                let first_offset = consecutive_copies[0].0;
                let total_length: usize = consecutive_copies.iter().map(|(_, len)| *len).sum();
                optimized.push(PatchInstruction::Copy(first_offset, total_length));
                i = j;
            } else {
                optimized.push(instructions[i].clone());
                i += 1;
            }
        } else {
            optimized.push(instructions[i].clone());
            i += 1;
        }
    }

    optimized
}

fn write_patch_file<P: AsRef<Path>>(path: P, instructions: &[PatchInstruction]) -> io::Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(&(instructions.len() as u64).to_le_bytes())?;

    for instruction in instructions {
        let bytes = instruction.to_bytes();
        writer.write_all(&bytes)?;
    }

    writer.flush()?;
    Ok(())
}

pub fn read_patch_file<P: AsRef<Path>>(path: P) -> io::Result<Vec<PatchInstruction>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut count_buf = [0u8; 8];
    reader.read_exact(&mut count_buf)?;
    let instruction_count = u64::from_le_bytes(count_buf) as usize;
    let mut instructions = Vec::with_capacity(instruction_count);

    for _ in 0..instruction_count {
        let mut tag_buf = [0u8; 1];
        reader.read_exact(&mut tag_buf)?;
        let tag = tag_buf[0];

        let instruction = PatchInstruction::from_bytes(tag, &mut reader)?;
        instructions.push(instruction);
    }

    Ok(instructions)
}

#[derive(Debug, Clone, Default)]
pub struct DeltaStats {
    pub new_file_size: usize,
    pub patch_file_size: usize,
    pub total_instructions: usize,
    pub copy_instructions: usize,
    pub literal_instructions: usize,
    pub copy_bytes: usize,
    pub literal_bytes: usize,
    pub matches_found: usize,
    pub compressed_instructions: usize,
    pub uncompressed_literal_bytes: usize,
    pub compressed_literal_bytes: usize,
}

impl DeltaStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn compression_ratio(&self) -> f64 {
        if self.new_file_size == 0 {
            return 0.0;
        }
        (self.patch_file_size as f64 / self.new_file_size as f64) * 100.0
    }

    pub fn match_percentage(&self) -> f64 {
        if self.new_file_size == 0 {
            return 0.0;
        }
        (self.copy_bytes as f64 / self.new_file_size as f64) * 100.0
    }

    pub fn print(&self) {
        println!("\n=== Delta Generation Statistics ===");
        println!("New file size: {} bytes", self.new_file_size);
        println!("Patch file size: {} bytes", self.patch_file_size);
        println!("Compression ratio: {:.2}%", self.compression_ratio());
        println!("\nInstructions:");
        println!("  Total: {}", self.total_instructions);
        println!("  Copy instructions: {}", self.copy_instructions);
        println!("  Literal instructions: {}", self.literal_instructions);
        if self.compressed_instructions > 0 {
            println!("  Compressed literal instructions: {}", self.compressed_instructions);
        }
        println!("\nData breakdown:");
        println!("  Matched (copied) bytes: {} ({:.2}%)",
                 self.copy_bytes, self.match_percentage());
        println!("  New (literal) bytes: {} ({:.2}%)",
                 self.literal_bytes,
                 if self.new_file_size == 0 { 0.0 } else {
                     (self.literal_bytes as f64 / self.new_file_size as f64) * 100.0
                 });
        if self.compressed_literal_bytes > 0 {
            println!("    Compressed: {} bytes", self.compressed_literal_bytes);
            println!("    Uncompressed: {} bytes", self.uncompressed_literal_bytes);
        }
        println!("  Matches found: {}", self.matches_found);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signature::generate_signature;

    #[test]
    fn test_patch_instruction_serialization() {
        let copy_inst = PatchInstruction::Copy(5, 4096);
        let bytes = copy_inst.to_bytes();
        assert_eq!(bytes[0], 0x01);

        let literal_inst = PatchInstruction::Literal(vec![1, 2, 3, 4]);
        let bytes = literal_inst.to_bytes();
        assert_eq!(bytes[0], 0x02);
    }

    #[test]
    fn test_signature_map() {
        let sig1 = ChunkSignature::new(0, 1000, 0x1234, [0u8; 32]);
        let sig2 = ChunkSignature::new(1000, 2000, 0x5678, [1u8; 32]);
        let sig3 = ChunkSignature::new(3000, 1500, 0x1234, [2u8; 32]);

        let map = SignatureMap::new(vec![sig1, sig2, sig3]);

        let matches = map.lookup(0x1234).unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_delta_generation_identical_files() -> io::Result<()> {
        let test_dir = std::env::temp_dir();
        let old_file = test_dir.join("test_old_identical_cdc.txt");
        let new_file = test_dir.join("test_new_identical_cdc.txt");
        let sig_file = test_dir.join("test_sig_identical_cdc.sig");
        let patch_file = test_dir.join("test_patch_identical_cdc.patch");

        let content = b"Hello, World! This is a test for content-defined chunking. ".repeat(200);
        std::fs::write(&old_file, &content)?;
        std::fs::write(&new_file, &content)?;
        let signatures = generate_signature(&old_file, &sig_file)?;
        let generator = DeltaGenerator::new(signatures);
        let stats = generator.generate_delta(&new_file, &patch_file)?;
        assert!(stats.match_percentage() > 90.0);

        let _ = std::fs::remove_file(old_file);
        let _ = std::fs::remove_file(new_file);
        let _ = std::fs::remove_file(sig_file);
        let _ = std::fs::remove_file(patch_file);

        Ok(())
    }

    #[test]
    fn test_delta_generation_completely_different() -> io::Result<()> {
        let test_dir = std::env::temp_dir();
        let old_file = test_dir.join("test_old_diff.txt");
        let new_file = test_dir.join("test_new_diff.txt");
        let sig_file = test_dir.join("test_sig_diff.sig");
        let patch_file = test_dir.join("test_patch_diff.patch");

        std::fs::write(&old_file, b"AAAA".repeat(1000))?;
        std::fs::write(&new_file, b"BBBB".repeat(1000))?;
        let signatures = generate_signature(&old_file, &sig_file)?;
        let generator = DeltaGenerator::new(signatures);
        let stats = generator.generate_delta(&new_file, &patch_file)?;
        assert!(stats.literal_bytes > 0);

        let _ = std::fs::remove_file(old_file);
        let _ = std::fs::remove_file(new_file);
        let _ = std::fs::remove_file(sig_file);
        let _ = std::fs::remove_file(patch_file);

        Ok(())
    }

}