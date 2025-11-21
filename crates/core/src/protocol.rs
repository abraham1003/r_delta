use serde::{Deserialize, Serialize};
use bincode::{Encode, Decode};
use crate::signature::ChunkSignature;
use crate::manifest::FileEntry;
use crate::diff::SyncItem;

const PROTOCOL_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode)]
pub enum NetMessage {
    Handshake {
        filename: String,
        file_size: u64,
        protocol_version: u16,
    },
    HandshakeAck {
        has_old_file: bool,
    },
    RequestSignature,
    SignaturePacket {
        signatures: Vec<SerializableChunkSignature>,
    },
    SignatureEnd,
    StartPatch,
    PatchDone,
    VerifyRequest,
    VerifyResult {
        matches: bool,
        checksum: [u8; 32],
    },
    Error {
        message: String,
    },
    ManifestRequest,
    ManifestPacket {
        entries: Vec<FileEntry>,
    },
    ManifestEnd,
    SyncPlan {
        items: Vec<SyncItem>,
    },
    SendFullCompressed {
        filename: String,
        original_size: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode)]
pub struct SerializableChunkSignature {
    pub offset: u64,
    pub length: usize,
    pub weak_hash: u64,
    pub strong_hash: [u8; 32],
}

impl From<ChunkSignature> for SerializableChunkSignature {
    fn from(sig: ChunkSignature) -> Self {
        Self {
            offset: sig.offset,
            length: sig.length,
            weak_hash: sig.weak_hash,
            strong_hash: sig.strong_hash,
        }
    }
}

impl From<SerializableChunkSignature> for ChunkSignature {
    fn from(sig: SerializableChunkSignature) -> Self {
        Self {
            offset: sig.offset,
            length: sig.length,
            weak_hash: sig.weak_hash,
            strong_hash: sig.strong_hash,
        }
    }
}

impl NetMessage {
    pub fn handshake(filename: String, file_size: u64) -> Self {
        Self::Handshake {
            filename,
            file_size,
            protocol_version: PROTOCOL_VERSION,
        }
    }

    pub fn handshake_ack(has_old_file: bool) -> Self {
        Self::HandshakeAck { has_old_file }
    }

    pub fn signature_packet(signatures: Vec<ChunkSignature>) -> Self {
        Self::SignaturePacket {
            signatures: signatures.into_iter().map(|s| s.into()).collect(),
        }
    }

    pub fn verify_result(matches: bool, checksum: [u8; 32]) -> Self {
        Self::VerifyResult { matches, checksum }
    }

    pub fn error(message: String) -> Self {
        Self::Error { message }
    }

    pub fn manifest_packet(entries: Vec<FileEntry>) -> Self {
        Self::ManifestPacket { entries }
    }

    pub fn sync_plan(items: Vec<SyncItem>) -> Self {
        Self::SyncPlan { items }
    }

    pub fn serialize(&self) -> Result<Vec<u8>, bincode::error::EncodeError> {
        bincode::encode_to_vec(self, bincode::config::standard())
    }

    pub fn deserialize(data: &[u8]) -> Result<Self, bincode::error::DecodeError> {
        let (msg, _size) = bincode::decode_from_slice(data, bincode::config::standard())?;
        Ok(msg)
    }

    pub fn get_protocol_version() -> u16 {
        PROTOCOL_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_handshake_serialization() {
        let msg = NetMessage::handshake("test.bin".to_string(), 1024);
        let serialized = msg.serialize().expect("Failed to serialize");
        let deserialized = NetMessage::deserialize(&serialized).expect("Failed to deserialize");

        match deserialized {
            NetMessage::Handshake { filename, file_size, protocol_version } => {
                assert_eq!(filename, "test.bin");
                assert_eq!(file_size, 1024);
                assert_eq!(protocol_version, PROTOCOL_VERSION);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_handshake_ack_serialization() {
        let msg = NetMessage::handshake_ack(true);
        let serialized = msg.serialize().expect("Failed to serialize");
        let deserialized = NetMessage::deserialize(&serialized).expect("Failed to deserialize");

        match deserialized {
            NetMessage::HandshakeAck { has_old_file } => {
                assert!(has_old_file);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_error_message_serialization() {
        let msg = NetMessage::error("Test error".to_string());
        let serialized = msg.serialize().expect("Failed to serialize");
        let deserialized = NetMessage::deserialize(&serialized).expect("Failed to deserialize");

        match deserialized {
            NetMessage::Error { message } => {
                assert_eq!(message, "Test error");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_verify_result_serialization() {
        let checksum = [42u8; 32];
        let msg = NetMessage::verify_result(true, checksum);
        let serialized = msg.serialize().expect("Failed to serialize");
        let deserialized = NetMessage::deserialize(&serialized).expect("Failed to deserialize");

        match deserialized {
            NetMessage::VerifyResult { matches, checksum: chk } => {
                assert!(matches);
                assert_eq!(chk, checksum);
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_manifest_packet_serialization() {
        use crate::manifest::FileEntry;
        
        let entries = vec![
            FileEntry::new("file1.txt".to_string(), 100, 1000, false),
            FileEntry::new("file2.txt".to_string(), 200, 2000, false),
        ];
        let msg = NetMessage::manifest_packet(entries.clone());
        let serialized = msg.serialize().expect("Failed to serialize");
        let deserialized = NetMessage::deserialize(&serialized).expect("Failed to deserialize");

        match deserialized {
            NetMessage::ManifestPacket { entries: deserialized_entries } => {
                assert_eq!(deserialized_entries.len(), 2);
                assert_eq!(deserialized_entries[0].path, "file1.txt");
                assert_eq!(deserialized_entries[1].path, "file2.txt");
            }
            _ => panic!("Wrong message type"),
        }
    }

    #[test]
    fn test_sync_plan_serialization() {
        use crate::diff::{SyncItem, SyncAction};
        
        let items = vec![
            SyncItem::new("file1.txt".to_string(), SyncAction::SendFull),
            SyncItem::new("file2.txt".to_string(), SyncAction::Skip),
        ];
        let msg = NetMessage::sync_plan(items);
        let serialized = msg.serialize().expect("Failed to serialize");
        let deserialized = NetMessage::deserialize(&serialized).expect("Failed to deserialize");

        match deserialized {
            NetMessage::SyncPlan { items: deserialized_items } => {
                assert_eq!(deserialized_items.len(), 2);
                assert_eq!(deserialized_items[0].path, "file1.txt");
            }
            _ => panic!("Wrong message type"),
        }
    }
}

