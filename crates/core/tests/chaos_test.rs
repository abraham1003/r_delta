use r_delta_core::{signature, delta, patch};
use tempfile::tempdir;
use std::fs;
use proptest::prelude::*;

fn run_delta_cycle(old_data: &[u8], new_data: &[u8]) -> Result<bool, String> {
    let dir = tempdir().map_err(|e| e.to_string())?;
    
    let old_path = dir.path().join("old");
    let new_path = dir.path().join("new");
    let sig_path = dir.path().join("sig");
    let patch_path = dir.path().join("patch");
    let restored_path = dir.path().join("restored");

    fs::write(&old_path, old_data).map_err(|e| e.to_string())?;
    fs::write(&new_path, new_data).map_err(|e| e.to_string())?;

    signature::generate_signature(&old_path, &sig_path).map_err(|e| e.to_string())?;
    
    let signatures = signature::read_signature_file(&sig_path).map_err(|e| e.to_string())?;
    let generator = delta::DeltaGenerator::new(signatures);
    generator.generate_delta(&new_path, &patch_path).map_err(|e| e.to_string())?;

    patch::apply_patch(&old_path, &patch_path, &restored_path).map_err(|e| e.to_string())?;

    let restored_data = fs::read(&restored_path).map_err(|e| e.to_string())?;
    Ok(restored_data == new_data)
}

fn file_data_strategy(max_size: usize) -> impl Strategy<Value = Vec<u8>> {
    prop_oneof![
        prop::collection::vec(any::<u8>(), 0..max_size),
        (1..100usize, any::<u8>()).prop_flat_map(move |(pattern_size, _)| {
            prop::collection::vec(any::<u8>(), pattern_size)
                .prop_map(move |pattern| {
                    pattern.iter().cycle().take(max_size).copied().collect::<Vec<u8>>()
                })
        }),
        prop::collection::vec(any::<u8>(), 0..max_size / 10)
            .prop_map(move |spike_positions| {
                let mut data = vec![0u8; max_size];
                let data_len = data.len();
                for pos in spike_positions {
                    if (pos as usize) < data_len {
                        data[pos as usize % data_len] = pos;
                    }
                }
                data
            }),

        prop::collection::vec(32u8..127u8, 0..max_size),
        prop::collection::vec((any::<u8>(), 1..50usize), 0..max_size / 10)
            .prop_map(move |runs| {
                let mut data = Vec::new();
                for (byte, len) in runs {
                    data.extend(std::iter::repeat(byte).take(len.min(max_size - data.len())));
                    if data.len() >= max_size {
                        break;
                    }
                }
                data.truncate(max_size);
                data
            }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(250))]

    #[test]
    fn chaos_random_small_files(
        old_data in file_data_strategy(1024),
        new_data in file_data_strategy(1024),
    ) {
        let result = run_delta_cycle(&old_data, &new_data);
        prop_assert!(result.is_ok(), "Delta cycle failed: {:?}", result.err());
        prop_assert!(result.unwrap(), "Restored data doesn't match original");
    }

    #[test]
    fn chaos_random_medium_files(
        old_data in file_data_strategy(50 * 1024),
        new_data in file_data_strategy(50 * 1024),
    ) {
        let result = run_delta_cycle(&old_data, &new_data);
        prop_assert!(result.is_ok(), "Delta cycle failed: {:?}", result.err());
        prop_assert!(result.unwrap(), "Restored data doesn't match original");
    }

    #[test]
    fn chaos_random_large_files(
        old_data in file_data_strategy(200 * 1024),
        new_data in file_data_strategy(200 * 1024),
    ) {
        let result = run_delta_cycle(&old_data, &new_data);
        prop_assert!(result.is_ok(), "Delta cycle failed: {:?}", result.err());
        prop_assert!(result.unwrap(), "Restored data doesn't match original");
    }

    #[test]
    fn chaos_identical_files(
        data in file_data_strategy(50 * 1024),
    ) {
        let result = run_delta_cycle(&data, &data);
        prop_assert!(result.is_ok(), "Delta cycle failed: {:?}", result.err());
        prop_assert!(result.unwrap(), "Restored data doesn't match original");
    }

    #[test]
    fn chaos_append_operations(
        base_data in file_data_strategy(10 * 1024),
        append_data in prop::collection::vec(any::<u8>(), 1..5 * 1024),
    ) {
        let mut new_data = base_data.clone();
        new_data.extend_from_slice(&append_data);
        
        let result = run_delta_cycle(&base_data, &new_data);
        prop_assert!(result.is_ok(), "Delta cycle failed: {:?}", result.err());
        prop_assert!(result.unwrap(), "Restored data doesn't match original");
    }

    #[test]
    fn chaos_prepend_operations(
        base_data in file_data_strategy(10 * 1024),
        prepend_data in prop::collection::vec(any::<u8>(), 1..5 * 1024),
    ) {
        let mut new_data = prepend_data.clone();
        new_data.extend_from_slice(&base_data);
        
        let result = run_delta_cycle(&base_data, &new_data);
        prop_assert!(result.is_ok(), "Delta cycle failed: {:?}", result.err());
        prop_assert!(result.unwrap(), "Restored data doesn't match original");
    }

    #[test]
    fn chaos_insert_operations(
        base_data in file_data_strategy(10 * 1024).prop_filter("non-empty", |d| !d.is_empty()),
        insert_data in prop::collection::vec(any::<u8>(), 1..1024),
        insert_pos_ratio in 0.0..1.0f64,
    ) {
        let insert_pos = (base_data.len() as f64 * insert_pos_ratio) as usize;
        let mut new_data = base_data[..insert_pos].to_vec();
        new_data.extend_from_slice(&insert_data);
        new_data.extend_from_slice(&base_data[insert_pos..]);
        
        let result = run_delta_cycle(&base_data, &new_data);
        prop_assert!(result.is_ok(), "Delta cycle failed: {:?}", result.err());
        prop_assert!(result.unwrap(), "Restored data doesn't match original");
    }

    #[test]
    fn chaos_single_byte_changes(
        mut data in file_data_strategy(5 * 1024).prop_filter("non-empty", |d| !d.is_empty()),
        pos_ratio in 0.0..1.0f64,
        new_byte in any::<u8>(),
    ) {
        let pos = (data.len() as f64 * pos_ratio) as usize;
        let old_data = data.clone();
        data[pos] = new_byte;
        
        let result = run_delta_cycle(&old_data, &data);
        prop_assert!(result.is_ok(), "Delta cycle failed: {:?}", result.err());
        prop_assert!(result.unwrap(), "Restored data doesn't match original");
    }

    #[test]
    fn chaos_delete_operations(
        base_data in file_data_strategy(10 * 1024).prop_filter("non-empty", |d| !d.is_empty()),
        start_ratio in 0.0..1.0f64,
        end_ratio in 0.0..1.0f64,
    ) {
        let start = (base_data.len() as f64 * start_ratio.min(end_ratio)) as usize;
        let end = (base_data.len() as f64 * start_ratio.max(end_ratio)) as usize;
        
        let mut new_data = base_data[..start].to_vec();
        new_data.extend_from_slice(&base_data[end..]);
        
        let result = run_delta_cycle(&base_data, &new_data);
        prop_assert!(result.is_ok(), "Delta cycle failed: {:?}", result.err());
        prop_assert!(result.unwrap(), "Restored data doesn't match original");
    }
}

#[test]
fn chaos_edge_cases() {
    let test_cases = vec![
        ("empty_to_empty", vec![], vec![]),
        ("empty_to_data", vec![], vec![1, 2, 3, 4, 5]),
        ("data_to_empty", vec![1, 2, 3, 4, 5], vec![]),
        ("single_byte", vec![42], vec![43]),
        ("identical_large", vec![0xAA; 50000], vec![0xAA; 50000]),
        ("all_zeros", vec![0; 10000], vec![0; 10000]),
        ("all_ones", vec![0xFF; 10000], vec![0xFF; 10000]),
    ];

    for (name, old_data, new_data) in test_cases {
        match run_delta_cycle(&old_data, &new_data) {
            Ok(true) => {},
            Ok(false) => panic!("Test '{}' failed: data mismatch", name),
            Err(e) => panic!("Test '{}' failed: {}", name, e),
        }
    }
}

#[test]
fn chaos_pathological_patterns() {
    let patterns = vec![
        (0..10000).map(|i| if i % 2 == 0 { 0xAA } else { 0x55 }).collect::<Vec<u8>>(),
        (0..10000).map(|i| (i % 256) as u8).collect::<Vec<u8>>(),
        {
            let mut v = vec![0u8; 10000];
            for i in 0..14 {
                let pos = 2usize.pow(i);
                if pos < v.len() {
                    v[pos] = 0xFF;
                }
            }
            v
        },
    ];

    for (idx, old_data) in patterns.iter().enumerate() {
        match run_delta_cycle(old_data, old_data) {
            Ok(true) => {},
            Ok(false) => panic!("Pathological pattern {} (identity) failed: data mismatch", idx),
            Err(e) => panic!("Pathological pattern {} (identity) failed: {}", idx, e),
        }
        
        let mut new_data = old_data.clone();
        if !new_data.is_empty() {
            let mid_idx = new_data.len() / 2;
            new_data[mid_idx] ^= 0xFF;
        }
        
        match run_delta_cycle(old_data, &new_data) {
            Ok(true) => {},
            Ok(false) => panic!("Pathological pattern {} (modified) failed: data mismatch", idx),
            Err(e) => panic!("Pathological pattern {} (modified) failed: {}", idx, e),
        }
    }
}

#[test]
fn chaos_boundary_sizes() {
    let sizes = vec![
        0, 1, 2, 63, 64, 65,        
        8191, 8192, 8193,           
        16383, 16384, 16385,     
        32767, 32768, 32769,         
        65536,                     
    ];

    for size in sizes {
        let old_data = vec![0xAA; size];
        let new_data = vec![0x55; size];
        
        match run_delta_cycle(&old_data, &new_data) {
            Ok(true) => {},
            Ok(false) => panic!("Boundary size {} failed: data mismatch", size),
            Err(e) => panic!("Boundary size {} failed: {}", size, e),
        }
    }
}

#[test]
fn chaos_compression_patterns() {
    let old_repeating = vec![b'A'; 10000];
    let new_repeating = vec![b'B'; 10000];
    match run_delta_cycle(&old_repeating, &new_repeating) {
        Ok(true) => {},
        Ok(false) => panic!("Compression test 'repeating' failed: data mismatch"),
        Err(e) => panic!("Compression test 'repeating' failed: {}", e),
    }

    let old_random: Vec<u8> = (0..10000).map(|i| ((i * 31337) % 256) as u8).collect();
    let new_random: Vec<u8> = (0..10000).map(|i| ((i * 48271) % 256) as u8).collect();
    
    match run_delta_cycle(&old_random, &new_random) {
        Ok(true) => {},
        Ok(false) => panic!("Compression test 'random' failed: data mismatch"),
        Err(e) => panic!("Compression test 'random' failed: {}", e),
    }
}
