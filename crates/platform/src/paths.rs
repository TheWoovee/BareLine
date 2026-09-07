// SPDX-License-Identifier: MPL-2.0
//! FC-09 path identity format, version 1. Display text is never an identity.
#![allow(clippy::chunks_exact_to_as_chunks, clippy::manual_is_multiple_of)] // Rust 1.85 contract.
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathEncoding {
    WindowsUtf16Le,
    UnixBytes,
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SerializedPath {
    pub version: u32,
    pub encoding: PathEncoding,
    /// RFC 4648 padded base64; UTF-16 units use little-endian byte order.
    pub data: String,
    pub display: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathDecodeError {
    UnsupportedVersion,
    ForeignPlatform,
    InvalidBase64,
    InvalidCodeUnits,
    TooLong,
}
impl std::fmt::Display for PathDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid persisted path: {self:?}")
    }
}
impl std::error::Error for PathDecodeError {}
const MAX_ENCODED_PATH: usize = 1024 * 1024;
impl SerializedPath {
    pub fn from_native(path: &Path) -> Self {
        #[cfg(windows)]
        let (encoding, bytes) = {
            use std::os::windows::ffi::OsStrExt;
            (
                PathEncoding::WindowsUtf16Le,
                path.as_os_str()
                    .encode_wide()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>(),
            )
        };
        #[cfg(unix)]
        let (encoding, bytes) = {
            use std::os::unix::ffi::OsStrExt;
            (
                PathEncoding::UnixBytes,
                path.as_os_str().as_bytes().to_vec(),
            )
        };
        Self {
            version: 1,
            encoding,
            data: encode(&bytes),
            display: path.to_string_lossy().into_owned(),
        }
    }
    /// Decode identity bytes on any OS for migration/validation, without interpreting labels.
    pub fn identity_bytes(&self) -> Result<Vec<u8>, PathDecodeError> {
        if self.version != 1 {
            return Err(PathDecodeError::UnsupportedVersion);
        }
        if self.data.len() > MAX_ENCODED_PATH {
            return Err(PathDecodeError::TooLong);
        }
        let bytes = decode(&self.data)?;
        if self.encoding == PathEncoding::WindowsUtf16Le && bytes.len() % 2 != 0 {
            return Err(PathDecodeError::InvalidCodeUnits);
        }
        Ok(bytes)
    }
    /// Validate persisted identities without interpreting them on the current OS.
    pub fn validate(&self) -> Result<(), PathDecodeError> {
        let bytes = self.identity_bytes()?;
        let nul = match self.encoding {
            PathEncoding::UnixBytes => bytes.contains(&0),
            PathEncoding::WindowsUtf16Le => bytes.chunks_exact(2).any(|p| p == [0, 0]),
        };
        if nul {
            return Err(PathDecodeError::InvalidCodeUnits);
        }
        Ok(())
    }
    /// Foreign OS paths stay encoded; do not silently reinterpret their identity.
    pub fn to_native(&self) -> Result<PathBuf, PathDecodeError> {
        self.validate()?;
        let bytes = self.identity_bytes()?;
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStringExt;
            if self.encoding != PathEncoding::WindowsUtf16Le {
                return Err(PathDecodeError::ForeignPlatform);
            }
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect();
            Ok(std::ffi::OsString::from_wide(&units).into())
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            if self.encoding != PathEncoding::UnixBytes {
                return Err(PathDecodeError::ForeignPlatform);
            }
            Ok(std::ffi::OsString::from_vec(bytes).into())
        }
    }
}
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
fn encode(bytes: &[u8]) -> String {
    let mut result = String::new();
    for chunk in bytes.chunks(3) {
        let n = ((chunk[0] as u32) << 16)
            | ((chunk.get(1).copied().unwrap_or(0) as u32) << 8)
            | chunk.get(2).copied().unwrap_or(0) as u32;
        result.push(ALPHABET[((n >> 18) & 63) as usize] as char);
        result.push(ALPHABET[((n >> 12) & 63) as usize] as char);
        result.push(if chunk.len() > 1 {
            ALPHABET[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        result.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    result
}
fn decode(text: &str) -> Result<Vec<u8>, PathDecodeError> {
    if text.len() % 4 != 0 {
        return Err(PathDecodeError::InvalidBase64);
    }
    let mut bytes = Vec::new();
    for (i, chunk) in text.as_bytes().chunks_exact(4).enumerate() {
        let last = (i + 1) * 4 == text.len();
        let padding = if chunk[2] == b'=' {
            2
        } else if chunk[3] == b'=' {
            1
        } else {
            0
        };
        if padding != 0 && (!last || chunk[3] != b'=') {
            return Err(PathDecodeError::InvalidBase64);
        }
        let mut n = 0u32;
        for (j, ch) in chunk.iter().enumerate() {
            let v = if j >= 4 - padding {
                0
            } else {
                ALPHABET
                    .iter()
                    .position(|a| a == ch)
                    .ok_or(PathDecodeError::InvalidBase64)? as u32
            };
            n = (n << 6) | v;
        }
        if (padding == 2 && n & 0xffff != 0) || (padding == 1 && n & 0xff != 0) {
            return Err(PathDecodeError::InvalidBase64);
        }
        bytes.push((n >> 16) as u8);
        if padding < 2 {
            bytes.push((n >> 8) as u8);
        }
        if padding == 0 {
            bytes.push(n as u8);
        }
    }
    Ok(bytes)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base64_vectors_and_malformed_input() {
        for (raw, encoded) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v")] {
            assert_eq!(encode(raw.as_bytes()), encoded);
            assert_eq!(decode(encoded).unwrap(), raw.as_bytes());
        }
        for bad in ["Zg=", "Zg==AAAA", "Zh==", "Zm9=", "====", "Zg=!"] {
            assert!(decode(bad).is_err(), "{bad}");
        }
        let bytes: Vec<u8> = (0..=255).collect();
        assert_eq!(decode(&encode(&bytes)).unwrap(), bytes);
    }
    #[test]
    fn version_and_platform_are_not_guessed() {
        let mut path = SerializedPath::from_native(Path::new("example"));
        path.version = 2;
        assert_eq!(path.to_native(), Err(PathDecodeError::UnsupportedVersion));
        path.version = 1;
        path.encoding = if cfg!(windows) {
            PathEncoding::UnixBytes
        } else {
            PathEncoding::WindowsUtf16Le
        };
        path.data = "YWE=".into();
        assert_eq!(path.to_native(), Err(PathDecodeError::ForeignPlatform));
        path.encoding = PathEncoding::WindowsUtf16Le;
        path.data = "YQ==".into();
        assert_eq!(
            path.identity_bytes(),
            Err(PathDecodeError::InvalidCodeUnits)
        );
    }
    #[test]
    fn validation_rejects_nul_and_oversized_state() {
        for (encoding, data) in [
            (PathEncoding::UnixBytes, "AA=="),
            (PathEncoding::WindowsUtf16Le, "AAA="),
        ] {
            let mut path = SerializedPath {
                version: 1,
                encoding,
                data: data.into(),
                display: String::new(),
            };
            assert_eq!(path.validate(), Err(PathDecodeError::InvalidCodeUnits));
            path.data = "A".repeat(MAX_ENCODED_PATH + 4);
            assert_eq!(path.validate(), Err(PathDecodeError::TooLong));
        }
    }
    #[test]
    fn both_identity_encodings_preserve_opaque_values() {
        for (encoding, bytes) in [
            (PathEncoding::WindowsUtf16Le, vec![0, 0xd8, 0x61, 0]),
            (PathEncoding::UnixBytes, vec![0xff, b'a']),
        ] {
            let p = SerializedPath {
                version: 1,
                encoding,
                data: encode(&bytes),
                display: "irrelevant".into(),
            };
            assert_eq!(p.identity_bytes().unwrap(), bytes);
        }
    }
    #[test]
    fn native_non_unicode_identity_round_trips_without_display() {
        #[cfg(windows)]
        let path = {
            use std::os::windows::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_wide(&[0x61, 0xd800, 0x62]))
        };
        #[cfg(unix)]
        let path = {
            use std::os::unix::ffi::OsStringExt;
            PathBuf::from(std::ffi::OsString::from_vec(vec![b'a', 0xff, b'b']))
        };
        let mut stored = SerializedPath::from_native(&path);
        stored.display = "wrong identity".into();
        assert_eq!(stored.to_native().unwrap(), path);
    }
}
