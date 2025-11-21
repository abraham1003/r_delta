use std::path::Path;
use std::io;
use std::time::SystemTime;
use serde::{Deserialize, Serialize};
use bincode::{Encode, Decode};
use ignore::WalkBuilder;
use std::fs::File;
use std::io::Read;

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub modified: u64,
    pub is_dir: bool,
    pub checksum: Option<[u8; 32]>,
}

impl FileEntry {
    pub fn new(path: String, size: u64, modified: u64, is_dir: bool) -> Self {
        Self {
            path,
            size,
            modified,
            is_dir,
            checksum: None,
        }
    }

    pub fn from_metadata(relative_path: String, metadata: &std::fs::Metadata) -> io::Result<Self> {
        let size = metadata.len();
        let modified = metadata.modified()?
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let is_dir = metadata.is_dir();

        Ok(Self {
            path: relative_path,
            size,
            modified,
            is_dir,
            checksum: None,
        })
    }

    pub fn with_checksum(mut self, file_path: &Path) -> io::Result<Self> {
        if !self.is_dir && self.size > 0 {
            let mut file = File::open(file_path)?;
            let mut hasher = blake3::Hasher::new();
            let mut buffer = vec![0u8; 64 * 1024];
            
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
            }
            
            self.checksum = Some(*hasher.finalize().as_bytes());
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode)]
pub struct Manifest {
    pub entries: Vec<FileEntry>,
    pub root_hash: u64,
}

impl Manifest {
    pub fn new(entries: Vec<FileEntry>) -> Self {
        let root_hash = Self::compute_hash(&entries);
        Self {
            entries,
            root_hash,
        }
    }

    fn compute_hash(entries: &[FileEntry]) -> u64 {
        let mut hash = 0u64;
        for entry in entries {
            hash = hash.wrapping_mul(31)
                .wrapping_add(entry.size)
                .wrapping_add(entry.modified)
                .wrapping_add(entry.path.len() as u64);
        }
        hash
    }

    pub fn generate<P: AsRef<Path>>(root_path: P) -> io::Result<Self> {
        let root = root_path.as_ref();
        
        if !root.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Path does not exist: {}", root.display())
            ));
        }

        let walker = WalkBuilder::new(root)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .require_git(false)
            .follow_links(false)
            .max_depth(None)
            .threads(std::thread::available_parallelism()
                .map(|n| n.get().min(8))
                .unwrap_or(4))
            .add_custom_ignore_filename(".rdeltaignore")
            .filter_entry(|entry| {
                let file_name = entry.file_name().to_string_lossy();
                !matches!(file_name.as_ref(), "target" | "node_modules" | ".git")
            })
            .build();

        let entries: Result<Vec<_>, _> = walker
            .filter_map(|entry| {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => return Some(Err(io::Error::new(io::ErrorKind::Other, e))),
                };

                let path = entry.path();
                if path == root {
                    return None;
                }

                let relative_path = match pathdiff::diff_paths(path, root) {
                    Some(p) => p.to_string_lossy().replace('\\', "/"),
                    None => return Some(Err(io::Error::new(
                        io::ErrorKind::Other,
                        format!("Failed to compute relative path for: {}", path.display())
                    ))),
                };

                let metadata = match entry.metadata() {
                    Ok(m) => m,
                    Err(e) => return Some(Err(io::Error::new(io::ErrorKind::Other, e))),
                };

                let file_entry = match FileEntry::from_metadata(relative_path, &metadata) {
                    Ok(e) => e,
                    Err(e) => return Some(Err(e)),
                };

                // Compute checksum for files (not directories)
                let file_entry_with_checksum = if !file_entry.is_dir {
                    match file_entry.with_checksum(path) {
                        Ok(e) => e,
                        Err(e) => return Some(Err(e)),
                    }
                } else {
                    file_entry
                };

                Some(Ok(file_entry_with_checksum))
            })
            .collect();

        let mut entries = entries?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));

        Ok(Self::new(entries))
    }

    pub fn find_entry(&self, path: &str) -> Option<&FileEntry> {
        self.entries.iter().find(|e| e.path == path)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn total_size(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }

    pub fn serialize(&self) -> Result<Vec<u8>, bincode::error::EncodeError> {
        bincode::encode_to_vec(self, bincode::config::standard())
    }

    pub fn deserialize(data: &[u8]) -> Result<Self, bincode::error::DecodeError> {
        let (manifest, _) = bincode::decode_from_slice(data, bincode::config::standard())?;
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_manifest_generation() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::create_dir(root.join("subdir")).unwrap();
        File::create(root.join("file1.txt"))
            .unwrap()
            .write_all(b"test content")
            .unwrap();
        File::create(root.join("subdir").join("file2.txt"))
            .unwrap()
            .write_all(b"more content")
            .unwrap();

        let manifest = Manifest::generate(root).unwrap();

        assert_eq!(manifest.len(), 3);
        assert!(manifest.find_entry("file1.txt").is_some());
        assert!(manifest.find_entry("subdir").is_some());
        assert!(manifest.find_entry("subdir/file2.txt").is_some());
    }

    #[test]
    fn test_manifest_ignores_target() {
        let temp = TempDir::new().unwrap();
        let root = temp.path();

        fs::create_dir(root.join("target")).unwrap();
        File::create(root.join("target").join("ignored.txt"))
            .unwrap()
            .write_all(b"should be ignored")
            .unwrap();
        File::create(root.join("normal.txt"))
            .unwrap()
            .write_all(b"visible")
            .unwrap();

        let manifest = Manifest::generate(root).unwrap();

        assert_eq!(manifest.len(), 1);
        assert!(manifest.find_entry("normal.txt").is_some());
        assert!(manifest.find_entry("target/ignored.txt").is_none());
    }

    #[test]
    fn test_manifest_serialization() {
        let entries = vec![
            FileEntry::new("file1.txt".to_string(), 100, 1234567890, false),
            FileEntry::new("file2.txt".to_string(), 200, 1234567891, false),
        ];
        let manifest = Manifest::new(entries);

        let serialized = manifest.serialize().unwrap();
        let deserialized = Manifest::deserialize(&serialized).unwrap();

        assert_eq!(manifest.len(), deserialized.len());
        assert_eq!(manifest.root_hash, deserialized.root_hash);
    }

    #[test]
    fn test_file_entry_from_metadata() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");
        File::create(&file_path)
            .unwrap()
            .write_all(b"test")
            .unwrap();

        let metadata = fs::metadata(&file_path).unwrap();
        let entry = FileEntry::from_metadata("test.txt".to_string(), &metadata).unwrap();

        assert_eq!(entry.path, "test.txt");
        assert_eq!(entry.size, 4);
        assert!(!entry.is_dir);
        assert!(entry.modified > 0);
    }
}
