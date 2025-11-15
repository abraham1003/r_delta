use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write, BufReader, BufWriter};
use std::path::Path;
use sha2::{Sha256, Digest};
use crate::signature::{ChunkSignature, Adler32, CHUNK_SIZE};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchInstruction {

    Copy(u64, usize),
    Literal(Vec<u8>),
}

impl PatchInstruction {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            PatchInstruction::Copy(index, length) => {
                let mut bytes = Vec::new();
                bytes.push(0x01); // Copy tag
                bytes.extend_from_slice(&index.to_le_bytes());
                bytes.extend_from_slice(&(*length as u64).to_le_bytes());
                bytes
            }
            PatchInstruction::Literal(data) => {
                let mut bytes = Vec::new();
                bytes.push(0x02); // Literal tag
                bytes.extend_from_slice(&(data.len() as u64).to_le_bytes());
                bytes.extend_from_slice(data);
                bytes
            }
        }
    }

    pub fn from_bytes(tag: u8, reader: &mut dyn Read) -> io::Result<Self> {
        match tag {
            0x01 => {
                let mut index_buf = [0u8; 8];
                let mut length_buf = [0u8; 8];
                reader.read_exact(&mut index_buf)?;
                reader.read_exact(&mut length_buf)?;
                let index = u64::from_le_bytes(index_buf);
                let length = u64::from_le_bytes(length_buf) as usize;
                Ok(PatchInstruction::Copy(index, length))
            }
            0x02 => {
                let mut length_buf = [0u8; 8];
                reader.read_exact(&mut length_buf)?;
                let length = u64::from_le_bytes(length_buf) as usize;
                let mut data = vec![0u8; length];
                reader.read_exact(&mut data)?;
                Ok(PatchInstruction::Literal(data))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Unknown patch instruction tag: {}", tag),
            )),
        }
    }
}

pub struct SignatureMap {
    weak_hash_map: HashMap<u32, Vec<ChunkSignature>>,
}

impl SignatureMap {
    pub fn new(signatures: Vec<ChunkSignature>) -> Self {
        let mut weak_hash_map: HashMap<u32, Vec<ChunkSignature>> = HashMap::new();

        for sig in signatures {
            weak_hash_map
                .entry(sig.weak_hash)
                .or_insert_with(Vec::new)
                .push(sig);
        }

        Self { weak_hash_map }
    }

    pub fn lookup(&self, weak_hash: u32) -> Option<&Vec<ChunkSignature>> {
        self.weak_hash_map.get(&weak_hash)
    }

    pub fn unique_weak_hashes(&self) -> usize {
        self.weak_hash_map.len()
    }

    pub fn total_signatures(&self) -> usize {
        self.weak_hash_map.values().map(|v| v.len()).sum()
    }
}

fn reset_rolling_hasher(
    rolling_hasher: &mut Adler32,
    file_data: &[u8],
    current_pos: usize,
    chunk_size: usize,
    file_size: usize,
) {
    rolling_hasher.reset();
    let next_window_size = std::cmp::min(chunk_size, file_size - current_pos);
    if next_window_size > 0 {
        rolling_hasher.update(&file_data[current_pos..current_pos + next_window_size]);
    }
}

pub struct DeltaGenerator {
    signature_map: SignatureMap,
    chunk_size: usize,
}

impl DeltaGenerator {
    pub fn new(signatures: Vec<ChunkSignature>) -> Self {
        Self {
            signature_map: SignatureMap::new(signatures),
            chunk_size: CHUNK_SIZE,
        }
    }

    fn compute_strong_hash(&self, data: &[u8]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(data);
        let result = hasher.finalize();
        result.into()
    }

    fn find_match(&self, data: &[u8], weak_hash: u32) -> Option<(u64, usize)> {
        let candidates = self.signature_map.lookup(weak_hash)?;
        let strong_hash = self.compute_strong_hash(data);

        for candidate in candidates {
            if candidate.strong_hash == strong_hash {
                return Some((candidate.index, data.len()));
            }
        }
        None
    }

    pub fn generate_delta<P: AsRef<Path>>(
        &self,
        new_file_path: P,
        patch_file_path: P,
    ) -> io::Result<DeltaStats> {
        let new_file = File::open(new_file_path)?;
        let mut reader = BufReader::new(new_file);
        let mut stats = DeltaStats::new();
        let mut new_file_data = Vec::new();
        reader.read_to_end(&mut new_file_data)?;
        let file_size = new_file_data.len();

        if file_size == 0 {
            let patch_file = File::create(patch_file_path)?;
            let mut writer = BufWriter::new(patch_file);
            writer.write_all(&0u64.to_le_bytes())?;
            return Ok(stats);
        }

        let mut instructions = Vec::new();
        let mut current_pos = 0;
        let mut literal_buffer = Vec::new();
        let mut rolling_hasher = Adler32::new();
        let initial_window_size = std::cmp::min(self.chunk_size, file_size);
        rolling_hasher.update(&new_file_data[0..initial_window_size]);

        while current_pos < file_size {
            let remaining = file_size - current_pos;
            let window_size = std::cmp::min(self.chunk_size, remaining);
            let window_end = current_pos + window_size;
            let current_window = &new_file_data[current_pos..window_end];

            let weak_hash = rolling_hasher.finalize();

            if let Some((chunk_index, matched_length)) = self.find_match(current_window, weak_hash) {
                if !literal_buffer.is_empty() {
                    instructions.push(PatchInstruction::Literal(literal_buffer.clone()));
                    stats.literal_bytes += literal_buffer.len();
                    stats.literal_instructions += 1;
                    literal_buffer.clear();
                }

                instructions.push(PatchInstruction::Copy(chunk_index, matched_length));
                stats.copy_bytes += matched_length;
                stats.copy_instructions += 1;
                stats.matches_found += 1;
                current_pos += matched_length;
                if current_pos < file_size {
                    reset_rolling_hasher(&mut rolling_hasher, &new_file_data, current_pos, self.chunk_size, file_size);
                }
            } else {
                literal_buffer.push(new_file_data[current_pos]);
                current_pos += 1;

                if current_pos < file_size && window_size == self.chunk_size {
                    let old_byte = new_file_data[current_pos - 1];
                    let new_byte_pos = current_pos + self.chunk_size - 1;

                    if new_byte_pos < file_size {
                        let new_byte = new_file_data[new_byte_pos];
                        rolling_hasher.roll(old_byte, new_byte, self.chunk_size);
                    } else {
                        reset_rolling_hasher(&mut rolling_hasher, &new_file_data, current_pos, self.chunk_size, file_size);
                    }
                } else if current_pos < file_size {
                    reset_rolling_hasher(&mut rolling_hasher, &new_file_data, current_pos, self.chunk_size, file_size);
                }
            }
        }

        if !literal_buffer.is_empty() {
            instructions.push(PatchInstruction::Literal(literal_buffer.clone()));
            stats.literal_bytes += literal_buffer.len();
            stats.literal_instructions += 1;
        }

        write_patch_file(&patch_file_path, &instructions)?;
        stats.total_instructions = instructions.len();
        stats.new_file_size = file_size;
        stats.patch_file_size = std::fs::metadata(patch_file_path)?.len() as usize;

        Ok(stats)
    }
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
        println!("\nData breakdown:");
        println!("  Matched (copied) bytes: {} ({:.2}%)",
                 self.copy_bytes, self.match_percentage());
        println!("  New (literal) bytes: {} ({:.2}%)",
                 self.literal_bytes,
                 if self.new_file_size == 0 { 0.0 } else {
                     (self.literal_bytes as f64 / self.new_file_size as f64) * 100.0
                 });
        println!("  Matches found: {}", self.matches_found);
    }
}

pub fn apply_patch<P: AsRef<Path>>(
    old_file_path: P,
    patch_file_path: P,
    output_file_path: P,
) -> io::Result<()> {
    let old_file = File::open(old_file_path)?;
    let mut reader = BufReader::new(old_file);
    let mut old_file_data = Vec::new();
    reader.read_to_end(&mut old_file_data)?;
    let instructions = read_patch_file(patch_file_path)?;
    let output_file = File::create(output_file_path)?;
    let mut writer = BufWriter::new(output_file);

    for instruction in instructions {
        match instruction {
            PatchInstruction::Copy(chunk_index, length) => {
                let start = (chunk_index as usize) * CHUNK_SIZE;
                let end = start + length;

                if end > old_file_data.len() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Copy instruction references data beyond old file: chunk {} length {}",
                                chunk_index, length),
                    ));
                }

                writer.write_all(&old_file_data[start..end])?;
            }
            PatchInstruction::Literal(data) => {
                writer.write_all(&data)?;
            }
        }
    }
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signature::generate_signature;

    #[test]
    fn test_patch_instruction_serialization() {
        let copy_inst = PatchInstruction::Copy(5, 4096);
        let bytes = copy_inst.to_bytes();
        assert_eq!(bytes[0], 0x01); // Copy tag

        let literal_inst = PatchInstruction::Literal(vec![1, 2, 3, 4]);
        let bytes = literal_inst.to_bytes();
        assert_eq!(bytes[0], 0x02); // Literal tag
    }

    #[test]
    fn test_signature_map() {
        let sig1 = ChunkSignature::new(0, 0x1234, [0u8; 32]);
        let sig2 = ChunkSignature::new(1, 0x5678, [1u8; 32]);
        let sig3 = ChunkSignature::new(2, 0x1234, [2u8; 32]);

        let map = SignatureMap::new(vec![sig1, sig2, sig3]);

        assert_eq!(map.unique_weak_hashes(), 2);
        assert_eq!(map.total_signatures(), 3);

        let matches = map.lookup(0x1234).unwrap();
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn test_delta_generation_identical_files() -> io::Result<()> {
        let test_dir = std::env::temp_dir();
        let old_file = test_dir.join("test_old_identical.txt");
        let new_file = test_dir.join("test_new_identical.txt");
        let sig_file = test_dir.join("test_sig_identical.sig");
        let patch_file = test_dir.join("test_patch_identical.patch");

        let content = b"Hello, World! ".repeat(1000);
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

    #[test]
    fn test_apply_patch() -> io::Result<()> {
        let test_dir = std::env::temp_dir();
        let old_file = test_dir.join("test_old_apply.txt");
        let new_file = test_dir.join("test_new_apply.txt");
        let sig_file = test_dir.join("test_sig_apply.sig");
        let patch_file = test_dir.join("test_patch_apply.patch");
        let output_file = test_dir.join("test_output_apply.txt");

        let old_content = b"Hello, World! ".repeat(500);
        let new_content = b"Hello, World! Goodbye, World! ".repeat(300);
        std::fs::write(&old_file, &old_content)?;
        std::fs::write(&new_file, &new_content)?;
        let signatures = generate_signature(&old_file, &sig_file)?;
        let generator = DeltaGenerator::new(signatures);
        generator.generate_delta(&new_file, &patch_file)?;
        apply_patch(&old_file, &patch_file, &output_file)?;
        let output_content = std::fs::read(&output_file)?;
        let new_content_check = std::fs::read(&new_file)?;
        assert_eq!(output_content, new_content_check);

        let _ = std::fs::remove_file(old_file);
        let _ = std::fs::remove_file(new_file);
        let _ = std::fs::remove_file(sig_file);
        let _ = std::fs::remove_file(patch_file);
        let _ = std::fs::remove_file(output_file);

        Ok(())
    }
}