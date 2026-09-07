// SPDX-License-Identifier: MIT OR Apache-2.0
//! Version-one extension contracts. Authentication is performed by the native pipe
//! adapter before these framed messages enter a broker.
pub mod broker;
pub mod package;
pub use package::*;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_PENDING: usize = 32;
pub const MEMORY_LIMIT: usize = 128 * 1024 * 1024;
pub const INTERACTIVE_TIMEOUT_MS: u64 = 5000;
pub const BACKGROUND_TIMEOUT_MS: u64 = 120_000;
/// Chosen by the editor after an explicit user action for a signed manifest's
/// declared background command. This is launch policy, never a guest RPC grant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecutionBudget {
    Interactive,
    Background,
}
impl ExecutionBudget {
    pub fn timeout_ms(self) -> u64 {
        match self {
            Self::Interactive => INTERACTIVE_TIMEOUT_MS,
            Self::Background => BACKGROUND_TIMEOUT_MS,
        }
    }
    pub fn fuel(self) -> u64 {
        match self {
            Self::Interactive => 50_000_000,
            Self::Background => 200_000_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Capability {
    DocumentRead,
    DocumentEdit,
    WorkspaceRead,
    WorkspaceWrite,
    Network,
    ProcessSpawn,
    UiPanel,
    Settings,
}
impl Capability {
    pub fn name(self) -> &'static str {
        match self {
            Self::DocumentRead => "document.read",
            Self::DocumentEdit => "document.edit",
            Self::WorkspaceRead => "workspace.read",
            Self::WorkspaceWrite => "workspace.write",
            Self::Network => "network",
            Self::ProcessSpawn => "process.spawn",
            Self::UiPanel => "ui.panel",
            Self::Settings => "settings",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scope {
    Document(u64),
    Workspace(String),
    Extension,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityContext {
    pub capability: Capability,
    pub scope: Scope,
    pub grant_generation: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange {
    pub start: u64,
    pub end: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawRange {
    pub start: u64,
    pub end: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEdit {
    pub range: TextRange,
    pub replacement: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    ReadTextRange {
        document: u64,
        revision: u64,
        range: TextRange,
    },
    ReadOriginalBytes {
        document: u64,
        generation: u64,
        range: RawRange,
    },
    ApplyEdits {
        document: u64,
        revision: u64,
        #[serde(deserialize_with = "broker::deserialize_edits")]
        edits: Vec<TextEdit>,
    },
    BeginEdits {
        document: u64,
        revision: u64,
    },
    AppendChunk {
        transaction: u64,
        chunk: Vec<u8>,
    },
    CommitEdits {
        transaction: u64,
        revision: u64,
    },
    Cancel {
        request: u64,
    },
    InvokeCommand {
        command: String,
    },
    Panel {
        panel: String,
        text: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub protocol: u16,
    pub request_id: u64,
    pub extension_id: String,
    pub context: CapabilityContext,
    pub request: Request,
}
#[derive(Debug, PartialEq, Eq)]
pub enum ProtocolError {
    Io,
    Oversized,
    Malformed,
    Version,
    InvalidIdentity,
    ChunkLimit,
}
/// The length prefix is checked before allocating any attacker-sized buffer.
pub fn read_frame(reader: &mut impl Read) -> Result<Envelope, ProtocolError> {
    let mut prefix = [0; 4];
    reader
        .read_exact(&mut prefix)
        .map_err(|_| ProtocolError::Io)?;
    let len = u32::from_le_bytes(prefix) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(ProtocolError::Oversized);
    }
    let mut bytes = vec![0; len];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| ProtocolError::Io)?;
    let (message, rest): (Envelope, _) =
        postcard::take_from_bytes(&bytes).map_err(|_| ProtocolError::Malformed)?;
    if !rest.is_empty() {
        return Err(ProtocolError::Malformed);
    }
    validate(&message)?;
    Ok(message)
}
pub fn write_frame(writer: &mut impl Write, message: &Envelope) -> Result<(), ProtocolError> {
    validate(message)?;
    let bytes = postcard::to_stdvec(message).map_err(|_| ProtocolError::Malformed)?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::Oversized);
    }
    writer
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .map_err(|_| ProtocolError::Io)?;
    writer.write_all(&bytes).map_err(|_| ProtocolError::Io)
}
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
        && id != "."
        && id != ".."
}
fn validate(message: &Envelope) -> Result<(), ProtocolError> {
    if message.protocol != PROTOCOL_VERSION {
        return Err(ProtocolError::Version);
    }
    if !valid_id(&message.extension_id) {
        return Err(ProtocolError::InvalidIdentity);
    }
    if matches!(&message.request, Request::AppendChunk { chunk, .. } if chunk.len() > MAX_CHUNK_BYTES)
    {
        return Err(ProtocolError::ChunkLimit);
    }
    Ok(())
}
pub fn negotiate(minimum: u16, maximum: u16) -> Result<u16, ProtocolError> {
    if minimum <= PROTOCOL_VERSION && maximum >= PROTOCOL_VERSION {
        Ok(PROTOCOL_VERSION)
    } else {
        Err(ProtocolError::Version)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_prefix_never_reads_payload() {
        let data = ((MAX_FRAME_BYTES + 1) as u32).to_le_bytes();
        assert_eq!(read_frame(&mut &data[..]), Err(ProtocolError::Oversized));
    }
    #[test]
    fn truncated_and_unknown_version_rejected() {
        assert_eq!(read_frame(&mut &[1, 0, 0, 0][..]), Err(ProtocolError::Io));
        assert_eq!(negotiate(2, 3), Err(ProtocolError::Version));
        assert_eq!(negotiate(1, 2), Ok(1));
    }
    #[test]
    fn version_one_fixture_roundtrip() {
        let message = Envelope {
            protocol: 1,
            request_id: 7,
            extension_id: "fixture".into(),
            context: CapabilityContext {
                capability: Capability::DocumentRead,
                scope: Scope::Document(3),
                grant_generation: 1,
            },
            request: Request::ReadTextRange {
                document: 3,
                revision: 4,
                range: TextRange { start: 0, end: 2 },
            },
        };
        // Fixed postcard wire bytes preserve enum ordinals and field order.
        let fixture = [
            19, 0, 0, 0, 1, 7, 7, 102, 105, 120, 116, 117, 114, 101, 0, 0, 3, 1, 0, 3, 4, 0, 2,
        ];
        let mut out = Vec::new();
        write_frame(&mut out, &message).unwrap();
        assert_eq!(out, fixture);
        assert_eq!(read_frame(&mut &fixture[..]).unwrap(), message);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invocation {
    pub extension_id: String,
    pub command: String,
    pub arguments: String,
    pub document: u64,
    pub revision: u64,
    pub source_generation: u64,
    pub text_length: u64,
    pub raw_length: u64,
    pub grant_generation: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokerResponse {
    pub request_id: u64,
    pub result: Result<BrokerValue, String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BrokerValue {
    Text(String),
    Bytes(Vec<u8>),
    Transaction(u64),
    Applied { revision: u64 },
    Acknowledged,
}
pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolError> {
    let bytes = postcard::to_stdvec(value).map_err(|_| ProtocolError::Malformed)?;
    if bytes.len() > MAX_CHUNK_BYTES {
        return Err(ProtocolError::ChunkLimit);
    }
    Ok(bytes)
}
pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, ProtocolError> {
    if bytes.len() > MAX_CHUNK_BYTES {
        return Err(ProtocolError::ChunkLimit);
    }
    postcard::from_bytes(bytes).map_err(|_| ProtocolError::Malformed)
}
