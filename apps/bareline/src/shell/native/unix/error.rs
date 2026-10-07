// SPDX-License-Identifier: MPL-2.0
//! The native error the shell's diagnostics read (`code().0`, `message()`), and
//! the plain-language refusals for features these systems do not have yet.
use bareline_platform::{Capability, Unsupported};
use std::io;

/// `E_NOTIMPL`, the code the shell's diagnostics record for a native operation
/// this system does not provide (the value the Windows adapter would report).
pub(super) const NOT_IMPLEMENTED: i32 = -2_147_467_263;
/// `E_FAIL`: the window surface could not be created or presented.
pub(super) const FAILED: i32 = -2_147_467_259;
/// `E_INVALIDARG`: a surface size, frame or layout the renderer refused.
pub(super) const INVALID_ARGUMENT: i32 = -2_147_024_809;

/// A native operation that failed, with the code the shell's diagnostics record.
#[derive(Clone, Debug)]
pub struct Error {
    code: i32,
    message: String,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErrorCode(pub i32);
pub type Result<T> = std::result::Result<T, Error>;
impl Error {
    pub(super) fn other(message: impl Into<String>) -> Self {
        Self::with_code(NOT_IMPLEMENTED, message)
    }
    pub(super) fn with_code(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub fn code(&self) -> ErrorCode {
        ErrorCode(self.code)
    }
    pub fn message(&self) -> String {
        self.message.clone()
    }
}
impl From<Unsupported> for Error {
    fn from(unsupported: Unsupported) -> Self {
        Self::other(unsupported.to_string())
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}

pub(super) fn unsupported(capability: Capability) -> Unsupported {
    Unsupported { capability }
}
pub(super) fn unsupported_io(capability: Capability) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, unsupported(capability).to_string())
}
