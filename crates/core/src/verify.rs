use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Debug)]
pub struct VerificationResult {
    pub are_equal: bool,
    pub file_a_size: u64,
    pub file_b_size: u64,
    pub first_mismatch_offset: Option<u64>,
}

pub fn verify_files<P: AsRef<Path>>(file_a: P, file_b: P) -> io::Result<VerificationResult> {
    let file_a = file_a.as_ref();
    let file_b = file_b.as_ref();

    let mut reader_a = File::open(file_a)?;
    let mut reader_b = File::open(file_b)?;

    let metadata_a = reader_a.metadata()?;
    let metadata_b = reader_b.metadata()?;

    let size_a = metadata_a.len();
    let size_b = metadata_b.len();

    if size_a != size_b {
        return Ok(VerificationResult {
            are_equal: false,
            file_a_size: size_a,
            file_b_size: size_b,
            first_mismatch_offset: None,
        });
    }

    let mut buf_a = vec![0u8; BUFFER_SIZE];
    let mut buf_b = vec![0u8; BUFFER_SIZE];
    let mut offset: u64 = 0;

    loop {
        let bytes_read_a = reader_a.read(&mut buf_a)?;
        let bytes_read_b = reader_b.read(&mut buf_b)?;

        if bytes_read_a != bytes_read_b {
            return Ok(VerificationResult {
                are_equal: false,
                file_a_size: size_a,
                file_b_size: size_b,
                first_mismatch_offset: Some(offset),
            });
        }

        if bytes_read_a == 0 {
            break;
        }

        if buf_a[..bytes_read_a] != buf_b[..bytes_read_b] {
            for i in 0..bytes_read_a {
                if buf_a[i] != buf_b[i] {
                    return Ok(VerificationResult {
                        are_equal: false,
                        file_a_size: size_a,
                        file_b_size: size_b,
                        first_mismatch_offset: Some(offset + i as u64),
                    });
                }
            }
        }

        offset += bytes_read_a as u64;
    }

    Ok(VerificationResult {
        are_equal: true,
        file_a_size: size_a,
        file_b_size: size_b,
        first_mismatch_offset: None,
    })
}

