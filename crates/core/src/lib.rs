pub mod chunker;
pub mod signature;
pub mod delta;
pub mod patch;
pub mod verify;
pub mod protocol;
pub mod network;
pub mod manifest;
pub mod diff;
pub mod crypto;
pub mod encryption;
pub mod pipeline;

#[cfg(test)]
mod chaos_tests {
    use super::*;
    use proptest::prelude::*;
    use tempfile::NamedTempFile;
    use std::io::Write;

    fn round_trip_test(old_data: &[u8], new_data: &[u8]) -> Result<bool, Box<dyn std::error::Error>> {
        let mut old_file = NamedTempFile::new()?;
        let mut new_file = NamedTempFile::new()?;
        let sig_file = NamedTempFile::new()?;
        let patch_file = NamedTempFile::new()?;
        let restored_file = NamedTempFile::new()?;

        old_file.write_all(old_data)?;
        old_file.flush()?;
        new_file.write_all(new_data)?;
        new_file.flush()?;

        let old_path = old_file.path();
        let new_path = new_file.path();
        let sig_path = sig_file.path();
        let patch_path = patch_file.path();
        let restored_path = restored_file.path();

        signature::generate_signature(old_path, sig_path)?;
        
        let signatures = signature::read_signature_file(sig_path)?;
        let generator = delta::DeltaGenerator::new(signatures);
        generator.generate_delta(new_path, patch_path)?;

        patch::apply_patch(old_path, patch_path, restored_path)?;

        let restored_data = std::fs::read(restored_path)?;
        Ok(restored_data == new_data)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        
        #[test]
        fn chaos_empty_files(
            old_data in proptest::collection::vec(any::<u8>(), 0..1024),
        ) {
            let result = round_trip_test(&old_data, &[]);
            prop_assert!(result.is_ok());
            prop_assert!(result.unwrap());
        }

        #[test]
        fn chaos_small_files(
            old_data in proptest::collection::vec(any::<u8>(), 0..1024),
            new_data in proptest::collection::vec(any::<u8>(), 0..1024),
        ) {
            let result = round_trip_test(&old_data, &new_data);
            prop_assert!(result.is_ok());
            prop_assert!(result.unwrap());
        }

        #[test]
        fn chaos_medium_files(
            old_data in proptest::collection::vec(any::<u8>(), 1024..50*1024),
            new_data in proptest::collection::vec(any::<u8>(), 1024..50*1024),
        ) {
            let result = round_trip_test(&old_data, &new_data);
            prop_assert!(result.is_ok());
            prop_assert!(result.unwrap());
        }

        #[test]
        fn chaos_identical_files(
            data in proptest::collection::vec(any::<u8>(), 0..10*1024),
        ) {
            let result = round_trip_test(&data, &data);
            prop_assert!(result.is_ok());
            prop_assert!(result.unwrap());
        }

        #[test]
        fn chaos_append_data(
            base_data in proptest::collection::vec(any::<u8>(), 100..10*1024),
            append_data in proptest::collection::vec(any::<u8>(), 1..5*1024),
        ) {
            let mut new_data = base_data.clone();
            new_data.extend_from_slice(&append_data);
            
            let result = round_trip_test(&base_data, &new_data);
            prop_assert!(result.is_ok());
            prop_assert!(result.unwrap());
        }

        #[test]
        fn chaos_prepend_data(
            base_data in proptest::collection::vec(any::<u8>(), 100..10*1024),
            prepend_data in proptest::collection::vec(any::<u8>(), 1..5*1024),
        ) {
            let mut new_data = prepend_data.clone();
            new_data.extend_from_slice(&base_data);
            
            let result = round_trip_test(&base_data, &new_data);
            prop_assert!(result.is_ok());
            prop_assert!(result.unwrap());
        }

        #[test]
        fn chaos_insert_middle(
            base_data in proptest::collection::vec(any::<u8>(), 200..10*1024),
            insert_data in proptest::collection::vec(any::<u8>(), 1..1024),
        ) {
            let insert_pos = base_data.len() / 2;
            let mut new_data = base_data[..insert_pos].to_vec();
            new_data.extend_from_slice(&insert_data);
            new_data.extend_from_slice(&base_data[insert_pos..]);
            
            let result = round_trip_test(&base_data, &new_data);
            prop_assert!(result.is_ok());
            prop_assert!(result.unwrap());
        }

        #[test]
        fn chaos_single_byte_change(
            mut data in proptest::collection::vec(any::<u8>(), 100..5*1024),
            pos in any::<usize>(),
            new_byte in any::<u8>(),
        ) {
            if data.is_empty() {
                return Ok(());
            }
            let pos = pos % data.len();
            data[pos] = new_byte;
            
            let original = vec![0u8; data.len()];
            let result = round_trip_test(&original, &data);
            prop_assert!(result.is_ok());
            prop_assert!(result.unwrap());
        }
    }

    #[test]
    fn chaos_large_identical() {
        let data = vec![0x42u8; 100 * 1024];
        let result = round_trip_test(&data, &data);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn chaos_compressible_pattern() {
        let old_data = b"AAAA".repeat(1000);
        let new_data = b"BBBB".repeat(1000);
        let result = round_trip_test(&old_data, &new_data);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn chaos_empty_to_nonempty() {
        let old_data = vec![];
        let new_data = vec![1, 2, 3, 4, 5];
        let result = round_trip_test(&old_data, &new_data);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }

    #[test]
    fn chaos_nonempty_to_empty() {
        let old_data = vec![1, 2, 3, 4, 5];
        let new_data = vec![];
        let result = round_trip_test(&old_data, &new_data);
        assert!(result.is_ok());
        assert!(result.unwrap());
    }
}
