use std::collections::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use bincode::{Encode, Decode};
use crate::manifest::{Manifest, FileEntry};

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, PartialEq, Eq)]
pub enum SyncAction {
    SendFull,
    SendDelta,
    Skip,
    Delete,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode)]
pub struct SyncItem {
    pub path: String,
    pub action: SyncAction,
}

impl SyncItem {
    pub fn new(path: String, action: SyncAction) -> Self {
        Self { path, action }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode)]
pub struct SyncPlan {
    pub items: Vec<SyncItem>,
}

impl SyncPlan {
    pub fn new(items: Vec<SyncItem>) -> Self {
        Self { items }
    }

    pub fn empty() -> Self {
        Self { items: Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn count_by_action(&self) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for item in &self.items {
            let action_name = match item.action {
                SyncAction::SendFull => "SendFull",
                SyncAction::SendDelta => "SendDelta",
                SyncAction::Skip => "Skip",
                SyncAction::Delete => "Delete",
            };
            *counts.entry(action_name.to_string()).or_insert(0) += 1;
        }
        counts
    }

    pub fn serialize(&self) -> Result<Vec<u8>, bincode::error::EncodeError> {
        bincode::encode_to_vec(self, bincode::config::standard())
    }

    pub fn deserialize(data: &[u8]) -> Result<Self, bincode::error::DecodeError> {
        let (plan, _) = bincode::decode_from_slice(data, bincode::config::standard())?;
        Ok(plan)
    }
}

pub fn diff(client_manifest: &Manifest, server_manifest: &Manifest) -> SyncPlan {
    let server_map: HashMap<&str, &FileEntry> = server_manifest
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e))
        .collect();

    let client_paths: HashSet<&str> = client_manifest
        .entries
        .iter()
        .map(|e| e.path.as_str())
        .collect();

    let mut items = Vec::new();

    for client_entry in &client_manifest.entries {
        if client_entry.is_dir {
            continue;
        }

        let action = match server_map.get(client_entry.path.as_str()) {
            None => SyncAction::SendFull,
            Some(server_entry) => {
                if client_entry.size == server_entry.size 
                    && client_entry.modified == server_entry.modified {
                    SyncAction::Skip
                } else {
                    SyncAction::SendDelta
                }
            }
        };

        items.push(SyncItem::new(client_entry.path.clone(), action));
    }

    for server_entry in &server_manifest.entries {
        if server_entry.is_dir {
            continue;
        }

        if !client_paths.contains(server_entry.path.as_str()) {
            items.push(SyncItem::new(server_entry.path.clone(), SyncAction::Delete));
        }
    }

    SyncPlan::new(items)
}

pub fn diff_quick_check(client_manifest: &Manifest, server_manifest: &Manifest) -> bool {
    if client_manifest.root_hash == server_manifest.root_hash {
        return true;
    }

    if client_manifest.entries.len() != server_manifest.entries.len() {
        return false;
    }

    for (client_entry, server_entry) in client_manifest.entries.iter()
        .zip(server_manifest.entries.iter()) {
        if client_entry.path != server_entry.path
            || client_entry.size != server_entry.size
            || client_entry.modified != server_entry.modified {
            return false;
        }
    }

    true
}

pub fn filter_sync_plan(plan: &SyncPlan, include_actions: &[SyncAction]) -> SyncPlan {
    let items: Vec<_> = plan.items.iter()
        .filter(|item| include_actions.contains(&item.action))
        .cloned()
        .collect();
    
    SyncPlan::new(items)
}

pub fn get_files_to_sync(plan: &SyncPlan) -> Vec<String> {
    plan.items.iter()
        .filter(|item| matches!(item.action, SyncAction::SendFull | SyncAction::SendDelta))
        .map(|item| item.path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_entry(path: &str, size: u64, modified: u64, is_dir: bool) -> FileEntry {
        FileEntry::new(path.to_string(), size, modified, is_dir)
    }

    #[test]
    fn test_diff_new_file() {
        let client = Manifest::new(vec![
            create_test_entry("file1.txt", 100, 1000, false),
            create_test_entry("file2.txt", 200, 2000, false),
        ]);

        let server = Manifest::new(vec![
            create_test_entry("file1.txt", 100, 1000, false),
        ]);

        let plan = diff(&client, &server);

        assert_eq!(plan.len(), 2);
        
        let file1_action = plan.items.iter().find(|i| i.path == "file1.txt").unwrap();
        assert_eq!(file1_action.action, SyncAction::Skip);

        let file2_action = plan.items.iter().find(|i| i.path == "file2.txt").unwrap();
        assert_eq!(file2_action.action, SyncAction::SendFull);
    }

    #[test]
    fn test_diff_modified_file() {
        let client = Manifest::new(vec![
            create_test_entry("file1.txt", 150, 2000, false),
        ]);

        let server = Manifest::new(vec![
            create_test_entry("file1.txt", 100, 1000, false),
        ]);

        let plan = diff(&client, &server);

        assert_eq!(plan.len(), 1);
        assert_eq!(plan.items[0].action, SyncAction::SendDelta);
    }

    #[test]
    fn test_diff_deleted_file() {
        let client = Manifest::new(vec![
            create_test_entry("file1.txt", 100, 1000, false),
        ]);

        let server = Manifest::new(vec![
            create_test_entry("file1.txt", 100, 1000, false),
            create_test_entry("file2.txt", 200, 2000, false),
        ]);

        let plan = diff(&client, &server);

        let delete_action = plan.items.iter().find(|i| i.path == "file2.txt").unwrap();
        assert_eq!(delete_action.action, SyncAction::Delete);
    }

    #[test]
    fn test_diff_ignores_directories() {
        let client = Manifest::new(vec![
            create_test_entry("dir1", 0, 1000, true),
            create_test_entry("dir1/file1.txt", 100, 1000, false),
        ]);

        let server = Manifest::new(vec![]);

        let plan = diff(&client, &server);

        assert_eq!(plan.len(), 1);
        assert_eq!(plan.items[0].path, "dir1/file1.txt");
    }

    #[test]
    fn test_diff_quick_check_identical() {
        let entries = vec![
            create_test_entry("file1.txt", 100, 1000, false),
            create_test_entry("file2.txt", 200, 2000, false),
        ];

        let manifest1 = Manifest::new(entries.clone());
        let manifest2 = Manifest::new(entries);

        assert!(diff_quick_check(&manifest1, &manifest2));
    }

    #[test]
    fn test_diff_quick_check_different() {
        let manifest1 = Manifest::new(vec![
            create_test_entry("file1.txt", 100, 1000, false),
        ]);

        let manifest2 = Manifest::new(vec![
            create_test_entry("file1.txt", 150, 1000, false),
        ]);

        assert!(!diff_quick_check(&manifest1, &manifest2));
    }

    #[test]
    fn test_filter_sync_plan() {
        let plan = SyncPlan::new(vec![
            SyncItem::new("file1.txt".to_string(), SyncAction::SendFull),
            SyncItem::new("file2.txt".to_string(), SyncAction::Skip),
            SyncItem::new("file3.txt".to_string(), SyncAction::SendDelta),
        ]);

        let filtered = filter_sync_plan(&plan, &[SyncAction::SendFull, SyncAction::SendDelta]);

        assert_eq!(filtered.len(), 2);
        assert!(filtered.items.iter().all(|i| 
            matches!(i.action, SyncAction::SendFull | SyncAction::SendDelta)
        ));
    }

    #[test]
    fn test_get_files_to_sync() {
        let plan = SyncPlan::new(vec![
            SyncItem::new("file1.txt".to_string(), SyncAction::SendFull),
            SyncItem::new("file2.txt".to_string(), SyncAction::Skip),
            SyncItem::new("file3.txt".to_string(), SyncAction::SendDelta),
            SyncItem::new("file4.txt".to_string(), SyncAction::Delete),
        ]);

        let files = get_files_to_sync(&plan);

        assert_eq!(files.len(), 2);
        assert!(files.contains(&"file1.txt".to_string()));
        assert!(files.contains(&"file3.txt".to_string()));
    }

    #[test]
    fn test_sync_plan_serialization() {
        let plan = SyncPlan::new(vec![
            SyncItem::new("file1.txt".to_string(), SyncAction::SendFull),
            SyncItem::new("file2.txt".to_string(), SyncAction::Skip),
        ]);

        let serialized = plan.serialize().unwrap();
        let deserialized = SyncPlan::deserialize(&serialized).unwrap();

        assert_eq!(plan.len(), deserialized.len());
        assert_eq!(plan.items[0].path, deserialized.items[0].path);
        assert_eq!(plan.items[0].action, deserialized.items[0].action);
    }

    #[test]
    fn test_count_by_action() {
        let plan = SyncPlan::new(vec![
            SyncItem::new("file1.txt".to_string(), SyncAction::SendFull),
            SyncItem::new("file2.txt".to_string(), SyncAction::SendFull),
            SyncItem::new("file3.txt".to_string(), SyncAction::Skip),
            SyncItem::new("file4.txt".to_string(), SyncAction::SendDelta),
        ]);

        let counts = plan.count_by_action();

        assert_eq!(counts.get("SendFull"), Some(&2));
        assert_eq!(counts.get("Skip"), Some(&1));
        assert_eq!(counts.get("SendDelta"), Some(&1));
    }
}
