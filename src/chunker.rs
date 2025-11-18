use std::io::{self, Read};

const MIN_CHUNK_SIZE: usize = 2048;
const AVG_CHUNK_SIZE: usize = 8192;
const MAX_CHUNK_SIZE: usize = 65536;

const GEAR_TABLE: [u64; 256] = generate_gear_table();

const fn generate_gear_table() -> [u64; 256] {
    let mut table = [0u64; 256];
    let mut i = 0;
    while i < 256 {
        let mut hash = i as u64;
        hash ^= hash << 13;
        hash ^= hash >> 7;
        hash ^= hash << 17;
        hash = hash.wrapping_mul(0x27d4_eb2d_1655_67c5);
        table[i] = hash;
        i += 1;
    }
    table
}

#[inline]
fn compute_mask(avg_size: usize) -> u64 {
    (1u64 << (avg_size.trailing_zeros())) - 1
}

pub struct Chunk {
    pub offset: u64,
    pub length: usize,
    pub data: Vec<u8>,
}

pub struct Chunker<R: Read> {
    reader: R,
    buffer: Vec<u8>,
    current_offset: u64,
    mask: u64,
    min_size: usize,
    max_size: usize,
    eof: bool,
}

impl<R: Read> Chunker<R> {
    pub fn new(reader: R) -> Self {
        Self::with_sizes(reader, MIN_CHUNK_SIZE, AVG_CHUNK_SIZE, MAX_CHUNK_SIZE)
    }

    pub fn with_sizes(reader: R, min_size: usize, avg_size: usize, max_size: usize) -> Self {
        let mask = compute_mask(avg_size);
        Self {
            reader,
            buffer: Vec::with_capacity(max_size * 2),
            current_offset: 0,
            mask,
            min_size,
            max_size,
            eof: false,
        }
    }

    fn fill_buffer(&mut self) -> io::Result<()> {
        if self.eof {
            return Ok(());
        }

        let mut temp = vec![0u8; 65536];
        match self.reader.read(&mut temp)? {
            0 => {
                self.eof = true;
            }
            n => {
                self.buffer.extend_from_slice(&temp[..n]);
            }
        }
        Ok(())
    }

    #[inline]
    fn gear_hash(data: &[u8]) -> u64 {
        let mut hash = 0u64;
        for &byte in data {
            hash = (hash << 1).wrapping_add(GEAR_TABLE[byte as usize]);
        }
        hash
    }

    pub fn next_chunk(&mut self) -> io::Result<Option<Chunk>> {
        if self.buffer.is_empty() && self.eof {
            return Ok(None);
        }

        while self.buffer.len() < self.max_size && !self.eof {
            self.fill_buffer()?;
        }

        if self.buffer.is_empty() {
            return Ok(None);
        }

        let available = self.buffer.len();
        let mut cut_point = available.min(self.max_size);

        if available >= self.min_size {
            let search_start = self.min_size;
            let search_end = available.min(self.max_size);

            if search_end > search_start {
                let window_size = 64.min(search_start);
                let mut hash = Self::gear_hash(&self.buffer[search_start.saturating_sub(window_size)..search_start]);

                for pos in search_start..search_end {
                    if pos >= window_size {
                        let old_byte = self.buffer[pos - window_size];
                        hash = hash.wrapping_sub(GEAR_TABLE[old_byte as usize]);
                    }

                    hash = (hash << 1).wrapping_add(GEAR_TABLE[self.buffer[pos] as usize]);

                    if (hash & self.mask) == 0 {
                        cut_point = pos + 1;
                        break;
                    }
                }
            }
        }

        let chunk_data = self.buffer.drain(..cut_point).collect::<Vec<u8>>();
        let chunk = Chunk {
            offset: self.current_offset,
            length: chunk_data.len(),
            data: chunk_data,
        };

        self.current_offset += chunk.length as u64;

        Ok(Some(chunk))
    }
}

impl<R: Read> Iterator for Chunker<R> {
    type Item = io::Result<Chunk>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_chunk() {
            Ok(Some(chunk)) => Some(Ok(chunk)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_chunker_empty() {
        let data = vec![];
        let mut chunker = Chunker::new(Cursor::new(data));
        assert!(chunker.next_chunk().unwrap().is_none());
    }

    #[test]
    fn test_chunker_small() {
        let data = vec![0u8; 1024];
        let mut chunker = Chunker::new(Cursor::new(data));
        let chunk = chunker.next_chunk().unwrap();
        assert!(chunk.is_some());
        let chunk = chunk.unwrap();
        assert_eq!(chunk.length, 1024);
        assert_eq!(chunk.offset, 0);
    }

    #[test]
    fn test_chunker_respects_max_size() {
        let data = vec![0xFFu8; 100000];
        let chunker = Chunker::new(Cursor::new(data));

        for chunk_result in chunker {
            let chunk = chunk_result.unwrap();
            assert!(chunk.length <= MAX_CHUNK_SIZE);
            assert!(chunk.length >= MIN_CHUNK_SIZE || chunk.length < MAX_CHUNK_SIZE);
        }
    }

    #[test]
    fn test_chunker_deterministic() {
        let data = (0..50000).map(|i| (i % 256) as u8).collect::<Vec<u8>>();

        let chunker1 = Chunker::new(Cursor::new(data.clone()));
        let chunks1: Vec<_> = chunker1.collect::<Result<Vec<_>, _>>().unwrap();

        let chunker2 = Chunker::new(Cursor::new(data));
        let chunks2: Vec<_> = chunker2.collect::<Result<Vec<_>, _>>().unwrap();

        assert_eq!(chunks1.len(), chunks2.len());
        for (c1, c2) in chunks1.iter().zip(chunks2.iter()) {
            assert_eq!(c1.offset, c2.offset);
            assert_eq!(c1.length, c2.length);
            assert_eq!(c1.data, c2.data);
        }
    }

    #[test]
    fn test_chunker_shift_resistance() {
        let base_data: Vec<u8> = (0..20000).map(|i| ((i / 100) % 256) as u8).collect();
        let mut shifted_data = vec![0xAB; 500];
        shifted_data.extend_from_slice(&base_data);

        let chunker_base = Chunker::new(Cursor::new(base_data.clone()));
        let chunks_base: Vec<_> = chunker_base.collect::<Result<Vec<_>, _>>().unwrap();

        let chunker_shifted = Chunker::new(Cursor::new(shifted_data));
        let chunks_shifted: Vec<_> = chunker_shifted.collect::<Result<Vec<_>, _>>().unwrap();

        let mut matched = 0;
        for chunk_shifted in &chunks_shifted {
            for chunk_base in &chunks_base {
                if chunk_shifted.data == chunk_base.data {
                    matched += chunk_shifted.length;
                    break;
                }
            }
        }

        let match_ratio = matched as f64 / base_data.len() as f64;
        assert!(match_ratio > 0.5, "Match ratio too low: {:.2}%", match_ratio * 100.0);
    }
}

