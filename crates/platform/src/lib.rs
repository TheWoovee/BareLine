// SPDX-License-Identifier: MPL-2.0
use std::path::{Path, PathBuf};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageKind {
    Local,
    Removable,
    Network,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathOrigin {
    User,
    Session,
    Extension,
}
#[derive(Clone, Debug)]
pub struct PathTrust {
    pub canonical: PathBuf,
    pub storage: StorageKind,
    pub origin: PathOrigin,
    pub traverses_reparse_point: bool,
}
#[derive(Clone, Copy, Debug)]
pub enum PathOperation {
    Read,
    Write,
    Execute,
}
/// Retained handles bind classification to the actual opened object and its ancestors.
pub struct TrustedRead {
    pub trust: PathTrust,
    pub file: std::fs::File,
    pub ancestors: Vec<std::fs::File>,
}
pub trait PathTrustProvider {
    fn open_read(&self, _: &Path, _: PathOrigin) -> std::io::Result<TrustedRead> {
        Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "retained read capability unavailable"))
    }
    fn canonicalize(&self, path: &Path, origin: PathOrigin) -> std::io::Result<PathTrust>;
    fn permits(&self, _: &PathTrust, _: PathOperation) -> bool {
        false
    }
}
/// Classification is intentionally unknown until PR-015 supplies handle-based evaluation.
pub struct RestrictedPaths;
impl PathTrustProvider for RestrictedPaths {
    fn canonicalize(&self, _: &Path, _: PathOrigin) -> std::io::Result<PathTrust> {
        Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "path trust provider unavailable"))
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Support {
    Supported,
    Unsupported,
    Unknown,
}
#[derive(Clone, Copy, Debug)]
pub struct CapabilityReport {
    pub atomic_replace: Support,
    pub acl: Support,
    pub ads: Support,
    pub hard_links: Support,
}
pub trait FilesystemCapability {
    fn report(&self, path: &Path) -> std::io::Result<CapabilityReport>;
}
pub trait PlatformServices {
    fn about(&self);
    fn open_file(&self) -> Result<Option<PathBuf>, String>;
    fn save_file(&self) -> Result<Option<PathBuf>, String>;
    fn pick_folder(&self) -> Result<Option<PathBuf>, String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    pub volume: u64,
    pub file: u64,
    pub length: u64,
    pub modified: u64,
}
/// Background file operations; implementations must refuse unsupported replacement semantics.
pub trait LocalFileSystem: Send + Sync {
    /// Hold a verified local directory chain against reparse retargeting during recovery.
    fn guard_directory(&self, _: &Path) -> std::io::Result<std::sync::Arc<dyn Send + Sync>> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "directory guards unavailable"))
    }

    fn available_space(&self, _: &Path) -> std::io::Result<u64> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "free-space query unavailable"))
    }

    /// Keep bytes and directory identity immutable against write/delete until handle drop.
    /// Platforms without this capability must refuse sealed-store export.
    fn open_sealed_read(&self, _: &Path) -> std::io::Result<std::fs::File> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "sealed read handles unavailable"))
    }

    /// Explicit explorer action; implementations must retain safe parent handles.
    fn create_entry(&self, _: &Path, _: bool) -> std::io::Result<()> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "entry creation unavailable"))
    }
    /// Never replace an existing destination, including in a race.
    fn rename_entry(&self, _: &Path, _: &Path) -> std::io::Result<()> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "entry rename unavailable"))
    }
    /// Delete one entry only; a nonempty directory must fail.
    fn delete_entry(&self, _: &Path) -> std::io::Result<()> {
        Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "entry deletion unavailable"))
    }
    /// Read eligibility may allow read-only and hard-linked sources while replacement does not.
    fn validate_source(&self, path: &Path) -> std::io::Result<()> {
        self.validate_target(path)
    }
    fn identity(&self, file: &std::fs::File) -> std::io::Result<FileIdentity>;
    fn validate_target(&self, path: &Path) -> std::io::Result<()>;
    fn commit(&self, staged: &Path, target: &Path, existed: bool) -> std::io::Result<()>;
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_network_origin_never_grants_access() {
        assert!(RestrictedPaths.canonicalize(Path::new(r"\\never-contact.invalid\share\file"), PathOrigin::Session).is_err());
        let path = PathTrust {
            canonical: PathBuf::from(r"\\server\share\file"),
            storage: StorageKind::Network,
            origin: PathOrigin::Session,
            traverses_reparse_point: false,
        };
        for op in [
            PathOperation::Read,
            PathOperation::Write,
            PathOperation::Execute,
        ] {
            assert!(!RestrictedPaths.permits(&path, op));
        }
    }
}

mod paths;
pub use paths::{PathDecodeError, PathEncoding, SerializedPath};
mod readiness;
pub use readiness::*;

mod watch;
pub use watch::{WatchEvent, WatchKind};

pub mod accessibility;





pub mod printing;
