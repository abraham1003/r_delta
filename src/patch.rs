use std::fs::File;
use std::io::{self, Read, Write, BufReader, BufWriter, Seek, SeekFrom};
use std::path::Path;
use crate::delta::{PatchInstruction, read_patch_file};
use crate::signature::CHUNK_SIZE;

macro_rules! impl_patch_applier_accessors {
    () => {
        fn output_writer_mut(&mut self) -> &mut dyn Write {
            &mut self.output_writer
        }

        fn stats_mut(&mut self) -> &mut PatchStats {
            &mut self.stats
        }
    };
}

trait PatchApplier {
    fn process_copy(&mut self, chunk_index: u64, length: usize) -> io::Result<()>;
    fn output_writer_mut(&mut self) -> &mut dyn Write;
    fn stats_mut(&mut self) -> &mut PatchStats;
    fn process_literal(&mut self, data: &[u8]) -> io::Result<()> {
        self.output_writer_mut().write_all(data)?;

        let stats = self.stats_mut();
        stats.literal_instructions += 1;
        stats.bytes_literal += data.len();
        stats.bytes_written += data.len();

        Ok(())
    }

    fn apply_instruction(&mut self, instruction: &PatchInstruction) -> io::Result<()> {
        self.stats_mut().total_instructions += 1;

        match instruction {
            PatchInstruction::Copy(chunk_index, length) => {
                self.process_copy(*chunk_index, *length)
            }
            PatchInstruction::Literal(data) => {
                self.process_literal(data)
            }
        }
    }

    fn apply_instructions(&mut self, instructions: &[PatchInstruction]) -> io::Result<()> {
        for instruction in instructions {
            self.apply_instruction(instruction)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct PatchStats {
    pub total_instructions: usize,
    pub copy_instructions: usize,
    pub literal_instructions: usize,
    pub bytes_written: usize,
    pub bytes_copied: usize,
    pub bytes_literal: usize,
}

impl PatchStats {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            total_instructions: 0,
            copy_instructions: 0,
            literal_instructions: 0,
            bytes_written: 0,
            bytes_copied: 0,
            bytes_literal: 0,
        }
    }

    #[inline]
    fn record_copy(&mut self, bytes: usize) {
        self.copy_instructions += 1;
        self.bytes_copied += bytes;
        self.bytes_written += bytes;
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn print(&self) {
        println!("\n=== Patch Application Statistics ===");
        println!("Total instructions processed: {}", self.total_instructions);
        println!("Copy instructions: {}", self.copy_instructions);
        println!("Literal instructions: {}", self.literal_instructions);
        println!("\nData written:");
        println!("  Total bytes: {}", self.bytes_written);
        println!("  Copied from old file: {} bytes ({:.2}%)",
                 self.bytes_copied,
                 if self.bytes_written == 0 { 0.0 } else {
                     (self.bytes_copied as f64 / self.bytes_written as f64) * 100.0
                 });
        println!("  Literal data: {} bytes ({:.2}%)",
                 self.bytes_literal,
                 if self.bytes_written == 0 { 0.0 } else {
                     (self.bytes_literal as f64 / self.bytes_written as f64) * 100.0
                 });
    }
}

pub struct PatchBuilder {
    old_file_data: Vec<u8>,
    output_writer: BufWriter<File>,
    stats: PatchStats,
}

impl PatchBuilder {
    pub fn new<P: AsRef<Path>>(
        old_file_path: P,
        output_file_path: P,
    ) -> io::Result<Self> {
        let old_file = File::open(old_file_path)?;
        let mut reader = BufReader::new(old_file);
        let mut old_file_data = Vec::new();
        reader.read_to_end(&mut old_file_data)?;

        let output_file = File::create(output_file_path)?;
        let output_writer = BufWriter::new(output_file);

        Ok(Self {
            old_file_data,
            output_writer,
            stats: PatchStats::new(),
        })
    }

    pub fn apply_instruction(&mut self, instruction: &PatchInstruction) -> io::Result<()> {
        PatchApplier::apply_instruction(self, instruction)
    }

    pub fn apply_instructions(&mut self, instructions: &[PatchInstruction]) -> io::Result<()> {
        PatchApplier::apply_instructions(self, instructions)
    }

    pub fn finalize(mut self) -> io::Result<PatchStats> {
        self.output_writer.flush()?;
        Ok(self.stats)
    }

    #[must_use]
    pub const fn stats(&self) -> &PatchStats {
        &self.stats
    }
}

impl PatchApplier for PatchBuilder {
    #[allow(clippy::cast_possible_truncation)]
    fn process_copy(&mut self, chunk_index: u64, length: usize) -> io::Result<()> {
        let start = (chunk_index as usize) * CHUNK_SIZE;
        let end = start + length;

        if start >= self.old_file_data.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Copy instruction chunk index {chunk_index} is out of bounds (old file has {} chunks)",
                    self.old_file_data.len().div_ceil(CHUNK_SIZE)
                ),
            ));
        }

        if end > self.old_file_data.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Copy instruction range {start}..{end} exceeds old file size {}",
                    self.old_file_data.len()
                ),
            ));
        }

        self.output_writer.write_all(&self.old_file_data[start..end])?;
        self.stats.record_copy(length);

        Ok(())
    }

    impl_patch_applier_accessors!();
}

pub fn apply_patch<P: AsRef<Path>>(
    old_file_path: P,
    patch_file_path: P,
    output_file_path: P,
) -> io::Result<PatchStats> {
    let instructions = read_patch_file(&patch_file_path)?;
    let mut builder = PatchBuilder::new(old_file_path, output_file_path)?;
    builder.apply_instructions(&instructions)?;
    builder.finalize()
}


pub struct StreamingPatchBuilder<R: Read + Seek, W: Write> {
    old_file_reader: BufReader<R>,
    output_writer: W,
    stats: PatchStats,
}

impl<R: Read + Seek, W: Write> StreamingPatchBuilder<R, W> {
    pub fn new(old_file: R, output: W) -> Self {
        Self {
            old_file_reader: BufReader::new(old_file),
            output_writer: output,
            stats: PatchStats::new(),
        }
    }

    pub fn apply_instruction(&mut self, instruction: &PatchInstruction) -> io::Result<()> {
        PatchApplier::apply_instruction(self, instruction)
    }

    pub fn apply_instructions(&mut self, instructions: &[PatchInstruction]) -> io::Result<()> {
        PatchApplier::apply_instructions(self, instructions)
    }

    pub fn finalize(mut self) -> io::Result<PatchStats> {
        self.output_writer.flush()?;
        Ok(self.stats)
    }

    pub const fn stats(&self) -> &PatchStats {
        &self.stats
    }
}

impl<R: Read + Seek, W: Write> PatchApplier for StreamingPatchBuilder<R, W> {
    #[allow(clippy::cast_possible_truncation)]
    fn process_copy(&mut self, chunk_index: u64, length: usize) -> io::Result<()> {
        let start = (chunk_index as usize) * CHUNK_SIZE;

        self.old_file_reader.seek(SeekFrom::Start(start as u64))?;
        let mut remaining = length;
        let mut buffer = vec![0u8; 8192];

        while remaining > 0 {
            let to_read = std::cmp::min(remaining, buffer.len());
            let bytes_read = self.old_file_reader.read(&mut buffer[..to_read])?;

            if bytes_read == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("Unexpected EOF while copying {length} bytes from offset {start}"),
                ));
            }

            self.output_writer.write_all(&buffer[..bytes_read])?;
            remaining -= bytes_read;
        }

        self.stats.record_copy(length);

        Ok(())
    }

    impl_patch_applier_accessors!();
}

pub fn apply_patch_streaming<P: AsRef<Path>>(
    old_file_path: P,
    patch_file_path: P,
    output_file_path: P,
) -> io::Result<PatchStats> {
    let instructions = read_patch_file(&patch_file_path)?;
    let old_file = File::open(old_file_path)?;
    let output_file = File::create(output_file_path)?;
    let output_writer = BufWriter::new(output_file);
    let mut builder = StreamingPatchBuilder::new(old_file, output_writer);
    builder.apply_instructions(&instructions)?;
    builder.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    fn create_temp_file_with_content(content: &[u8]) -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(content).unwrap();
        file.flush().unwrap();
        file
    }

    fn read_file_content(path: PathBuf) -> Vec<u8> {
        let mut result = Vec::new();
        let mut file = File::open(path).unwrap();
        file.read_to_end(&mut result).unwrap();
        result
    }

    fn setup_patch_builder(old_content: &[u8]) -> (NamedTempFile, NamedTempFile, PatchBuilder) {
        let old_file = create_temp_file_with_content(old_content);
        let output_file = NamedTempFile::new().unwrap();
        let builder = PatchBuilder::new(old_file.path(), output_file.path()).unwrap();
        (old_file, output_file, builder)
    }

    #[test]
    fn test_patch_builder_copy() {
        let (_old_file, output_file, mut builder) = setup_patch_builder(b"Hello, World!");

        builder.apply_instruction(&PatchInstruction::Copy(0, 5)).unwrap();
        let stats = builder.finalize().unwrap();

        assert_eq!(stats.copy_instructions, 1);
        assert_eq!(stats.bytes_copied, 5);
        assert_eq!(&read_file_content(output_file.path().to_owned()), b"Hello");
    }

    #[test]
    fn test_patch_builder_literal() {
        let (_old_file, output_file, mut builder) = setup_patch_builder(b"dummy");

        builder.apply_instruction(&PatchInstruction::Literal(b"New data!".to_vec())).unwrap();
        let stats = builder.finalize().unwrap();

        assert_eq!(stats.literal_instructions, 1);
        assert_eq!(stats.bytes_literal, 9);
        assert_eq!(&read_file_content(output_file.path().to_owned()), b"New data!");
    }

    #[test]
    fn test_patch_builder_mixed() {
        let (_old_file, output_file, mut builder) = setup_patch_builder(b"ABCDEFGHIJKLMNOP");

        builder.apply_instruction(&PatchInstruction::Copy(0, 4)).unwrap();
        builder.apply_instruction(&PatchInstruction::Literal(b"123".to_vec())).unwrap();
        builder.apply_instruction(&PatchInstruction::Copy(0, 4)).unwrap();
        let stats = builder.finalize().unwrap();

        assert_eq!(stats.total_instructions, 3);
        assert_eq!(stats.copy_instructions, 2);
        assert_eq!(stats.literal_instructions, 1);
        assert_eq!(stats.bytes_copied, 8);
        assert_eq!(stats.bytes_literal, 3);
        assert_eq!(&read_file_content(output_file.path().to_owned()), b"ABCD123ABCD");
    }
}

