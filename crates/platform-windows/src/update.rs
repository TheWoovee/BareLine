// SPDX-License-Identifier: MPL-2.0
//! Explicit, off-thread update primitives. No startup/network activation occurs here.
use bareline_distribution::update::PublisherPin;
use sha2::{Digest, Sha256};
use std::ffi::c_void;
use std::fs::File;
use std::io::Write;
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::Security::Cryptography::{
    CERT_CONTEXT, CERT_FIND_EXT_ONLY_ENHKEY_USAGE_FLAG, CERT_NAME_ISSUER_FLAG, CERT_NAME_SIMPLE_DISPLAY_TYPE,
    CTL_USAGE, CertGetEnhancedKeyUsage, CertGetNameStringW, szOID_PKIX_KP_CODE_SIGNING,
};
use windows::Win32::Security::WinTrust::*;
use windows::core::{PCWSTR, w};

#[derive(Debug)]
pub enum UpdateError {
    InvalidEndpoint,
    Network,
    HttpStatus(u32),
    Cancelled,
    Limit,
    Io,
    Signature,
    Publisher,
}
/// Plain-language reason shown to the user (UI-03); `Debug` stays for diagnostics.
impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEndpoint => f.write_str("the update address in this build is not valid"),
            Self::Network => f.write_str("the update server could not be reached; check the connection and try again"),
            Self::HttpStatus(status) => write!(
                f,
                "the update server answered with HTTP status {status}; try again later"
            ),
            Self::Cancelled => f.write_str("the update check was cancelled"),
            Self::Limit => f.write_str("the update is larger than allowed"),
            Self::Io => f.write_str("the downloaded update could not be saved"),
            Self::Signature => f.write_str("the update's signature is not valid; nothing was installed"),
            Self::Publisher => f.write_str("the update is not signed by the expected publisher; nothing was installed"),
        }
    }
}
struct HttpHandle(*mut c_void);
impl HttpHandle {
    fn new(handle: *mut c_void) -> Result<Self, UpdateError> {
        if handle.is_null() {
            Err(UpdateError::Network)
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for HttpHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

/// HTTPS only, no redirects or ambient credentials. Host/path come from owner policy.
/// The system/automatic (WPAD/PAC) proxy applies, like other Windows HTTPS clients
/// (SEC-18); the signed manifest, not the transport, authenticates the bytes.
/// Fixed 5s operation timeouts plus an overall deadline; cancellation checked per chunk.
/// Caller must supply a private staging sink and remove incomplete downloads on failure.
pub fn download_https(
    host: &str,
    path: &str,
    maximum_bytes: u64,
    cancel: &AtomicBool,
    output: &mut (impl Write + ?Sized),
) -> Result<u64, UpdateError> {
    if host.is_empty()
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        || !path.starts_with('/')
        || path.starts_with("//")
        || path.chars().any(|c| c.is_control())
        || maximum_bytes == 0
    {
        return Err(UpdateError::InvalidEndpoint);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(UpdateError::Cancelled);
    }
    let host: Vec<u16> = host.encode_utf16().chain(Some(0)).collect();
    let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let deadline = Instant::now() + Duration::from_secs(300);
    unsafe {
        let session = HttpHandle::new(WinHttpOpen(
            w!("Bareline-Updater/1"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ))?;
        WinHttpSetTimeouts(session.0, 5000, 5000, 5000, 5000).map_err(|_| UpdateError::Network)?;
        let connection = HttpHandle::new(WinHttpConnect(session.0, PCWSTR(host.as_ptr()), 443, 0))?;
        let request = HttpHandle::new(WinHttpOpenRequest(
            connection.0,
            w!("GET"),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        let disabled = WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_COOKIES | WINHTTP_DISABLE_AUTHENTICATION;
        WinHttpSetOption(
            Some(request.0),
            WINHTTP_OPTION_DISABLE_FEATURE,
            Some(&disabled.to_ne_bytes()),
        )
        .map_err(|_| UpdateError::Network)?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(|_| UpdateError::Network)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|_| UpdateError::Network)?;
        let mut status = 0_u32;
        let mut size = 4_u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast()),
            &mut size,
            std::ptr::null_mut(),
        )
        .map_err(|_| UpdateError::Network)?;
        if status != 200 {
            return Err(UpdateError::HttpStatus(status));
        }
        let mut total = 0_u64;
        let mut chunk = [0_u8; 64 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(UpdateError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(UpdateError::Limit);
            }
            let mut count = 0;
            WinHttpReadData(request.0, chunk.as_mut_ptr().cast(), chunk.len() as u32, &mut count)
                .map_err(|_| UpdateError::Network)?;
            if count == 0 {
                return Ok(total);
            }
            total = total.checked_add(u64::from(count)).ok_or(UpdateError::Limit)?;
            if total > maximum_bytes {
                return Err(UpdateError::Limit);
            }
            output
                .write_all(&chunk[..count as usize])
                .map_err(|_| UpdateError::Io)?;
        }
    }
}

/// Revocation evidence required by [`verify_authenticode`] (SEC-07).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Revocation {
    /// Explicit user-initiated flows (update check and apply, runtime install): retrieve
    /// current revocation evidence online for the whole chain except the root.
    Online,
    /// Launch, acknowledgement and recovery: no network and no revocation requirement.
    /// The signed SHA-256 of the exact held file carries the decision there.
    Offline,
}

/// Authenticode verifies the same held handle used for hashing and requires the signer
/// to match the owner [`PublisherPin`] (subject, issuer, code-signing EKU), never a leaf
/// certificate hash (SEC-08). Callers always pair this with a signed hash of the file.
pub fn verify_authenticode(file: &File, pin: &PublisherPin, revocation: Revocation) -> Result<(), UpdateError> {
    let (checks, flags) = match revocation {
        Revocation::Online => (WTD_REVOKE_WHOLECHAIN, WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT),
        Revocation::Offline => (
            WTD_REVOKE_NONE,
            WTD_REVOCATION_CHECK_NONE | WTD_CACHE_ONLY_URL_RETRIEVAL,
        ),
    };
    unsafe {
        let mut info = WINTRUST_FILE_INFO {
            cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
            hFile: HANDLE(file.as_raw_handle()),
            ..Default::default()
        };
        let mut data = WINTRUST_DATA {
            cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: checks,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 { pFile: &mut info },
            dwStateAction: WTD_STATEACTION_VERIFY,
            dwProvFlags: flags,
            ..Default::default()
        };
        let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
        let status = WinVerifyTrust(
            Default::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
        let result = if status != 0 {
            Err(UpdateError::Signature)
        } else {
            let provider = WTHelperProvDataFromStateData(data.hWVTStateData);
            if provider.is_null() {
                Err(UpdateError::Signature)
            } else {
                let signer = WTHelperGetProvSignerFromChain(provider, 0, false, 0);
                if signer.is_null() {
                    Err(UpdateError::Signature)
                } else {
                    let certificate = WTHelperGetProvCertFromChain(signer, 0);
                    if certificate.is_null() || (*certificate).pCert.is_null() {
                        Err(UpdateError::Signature)
                    } else {
                        let cert = &*(*certificate).pCert;
                        let subject = certificate_name(cert, 0);
                        let issuer = certificate_name(cert, CERT_NAME_ISSUER_FLAG);
                        if subject
                            .zip(issuer)
                            .is_some_and(|(subject, issuer)| pin.accepts(&subject, &issuer))
                            && code_signing_usage(cert)
                        {
                            Ok(())
                        } else {
                            Err(UpdateError::Publisher)
                        }
                    }
                }
            }
        };
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        let _ = WinVerifyTrust(
            Default::default(),
            &mut action,
            (&mut data as *mut WINTRUST_DATA).cast(),
        );
        result
    }
}

/// The subject's (or, with `CERT_NAME_ISSUER_FLAG`, the issuer's) simple display name,
/// as PowerShell's `X509Certificate2.GetNameInfo(SimpleName, ..)` reports it.
fn certificate_name(certificate: &CERT_CONTEXT, flags: u32) -> Option<String> {
    let mut buffer = [0_u16; 256];
    // SAFETY: the context outlives this call (held by the caller's WinVerifyTrust state).
    let length = unsafe {
        CertGetNameStringW(
            certificate,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            flags,
            None,
            Some(&mut buffer[..]),
        )
    } as usize;
    // One unit is only the terminator (no name); a full buffer may be truncated.
    if length <= 1 || length >= buffer.len() {
        return None;
    }
    String::from_utf16(&buffer[..length - 1]).ok()
}

/// The leaf must explicitly carry the code-signing extended key usage.
fn code_signing_usage(certificate: &CERT_CONTEXT) -> bool {
    let flags = CERT_FIND_EXT_ONLY_ENHKEY_USAGE_FLAG.0;
    let mut size = 0_u32;
    // SAFETY: a size query, then a u64-aligned buffer of at least the reported size.
    if unsafe { CertGetEnhancedKeyUsage(certificate, flags, None, &mut size) }.is_err() || size == 0 || size > 65536 {
        return false;
    }
    let mut buffer = vec![0_u64; (size as usize).div_ceil(8)];
    let usage = buffer.as_mut_ptr().cast::<CTL_USAGE>();
    if unsafe { CertGetEnhancedKeyUsage(certificate, flags, Some(usage), &mut size) }.is_err() {
        return false;
    }
    // SAFETY: the call filled the header and its identifier array inside `buffer`.
    let usage = unsafe { &*usage };
    if usage.cUsageIdentifier == 0 || usage.rgpszUsageIdentifier.is_null() {
        return false;
    }
    let identifiers =
        unsafe { std::slice::from_raw_parts(usage.rgpszUsageIdentifier, usage.cUsageIdentifier as usize) };
    identifiers
        .iter()
        .any(|oid| !oid.is_null() && unsafe { oid.as_bytes() == szOID_PKIX_KP_CODE_SIGNING.as_bytes() })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_endpoint_and_precancel_never_connect() {
        let mut sink = Vec::new();
        assert!(matches!(
            download_https("evil/host", "/x", 4, &AtomicBool::new(false), &mut sink),
            Err(UpdateError::InvalidEndpoint)
        ));
        assert!(matches!(
            download_https("example.com", "/x", 4, &AtomicBool::new(true), &mut sink),
            Err(UpdateError::Cancelled)
        ));
        assert!(sink.is_empty());
    }
    #[test]
    fn unsigned_held_file_rejected() {
        let file = File::open(std::env::current_exe().unwrap()).unwrap();
        let pin = PublisherPin::parse("Unsigned Test", "Unsigned Test CA").unwrap();
        for revocation in [Revocation::Online, Revocation::Offline] {
            assert!(verify_authenticode(&file, &pin, revocation).is_err());
        }
    }
}

/// Open an exact regular local file for verification and handle-based rename.
/// Denies concurrent writes and deletion/renaming by other opens.
pub fn open_update_file(path: &std::path::Path) -> std::io::Result<File> {
    open_update_file_mode(path, true)
}
/// Read-only verification also works while the target image is running.
pub fn open_update_read_file(path: &std::path::Path) -> std::io::Result<File> {
    open_update_file_mode(path, false)
}
fn open_update_file_mode(path: &std::path::Path, rename: bool) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::*;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .access_mode(windows::Win32::Foundation::GENERIC_READ.0 | if rename { DELETE.0 } else { 0 })
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(path)?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info) }.map_err(std::io::Error::other)?;
    if info.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT.0 | FILE_ATTRIBUTE_DIRECTORY.0) != 0
        || info.nNumberOfLinks != 1
    {
        return Err(std::io::Error::other("update file must be a regular unlinked file"));
    }
    Ok(file)
}

/// Rename the exact verified handle; no filename reopening or overwrite fallback.
/// Parent directories must be trusted local installation paths.
pub fn rename_update_handle(file: &File, destination: &std::path::Path) -> std::io::Result<()> {
    rename_update_handle_inner(file, destination, false)
}
fn rename_update_handle_inner(file: &File, destination: &std::path::Path, replace: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows::Win32::Storage::FileSystem::*;
    if !destination.is_absolute() {
        return Err(std::io::Error::other("absolute destination required"));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| std::io::Error::other("destination parent required"))?;
    let directory = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
        .open(parent)?;
    if directory.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(std::io::Error::other("reparse destination parent refused"));
    }
    let name: Vec<u16> = destination
        .file_name()
        .ok_or_else(|| std::io::Error::other("destination filename required"))?
        .encode_wide()
        .collect();
    if name.is_empty() || name.contains(&0) || name.len() > 32767 {
        return Err(std::io::Error::other("invalid destination"));
    }
    crate::rename::rename(file, &directory, &name, replace)
}

/// Preserve exact old bytes in an unused flushed backup, then atomically rename
/// the verified staged handle over the target. No missing-target crash window.
/// Current editor must have exited; sharing violations refuse replacement.
/// A failed final rename retains both original target and backup for review.
pub fn replace_with_rollback(
    staged: &File,
    mut current: File,
    target: &std::path::Path,
    backup: &std::path::Path,
) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom};
    use std::os::windows::fs::OpenOptionsExt;
    let mut saved = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(backup)?;
    current.seek(SeekFrom::Start(0))?;
    std::io::copy(&mut current, &mut saved)?;
    saved.sync_all()?;
    drop(saved);
    // Release our no-delete-share old handle only after durable backup. The staged
    // executable remains the exact verified held handle across the atomic rename.
    drop(current);
    rename_update_handle_inner(staged, target, true)
}
#[cfg(test)]
mod apply_tests {
    use super::*;
    #[test]
    fn exact_handle_replacement_and_rollback_on_apply_failure() {
        let root = std::env::temp_dir().join(format!(
            "bareline-update-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let target = root.join("app.exe");
        let stage = root.join("stage.exe");
        let backup = root.join("backup.exe");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&stage, b"new").unwrap();
        {
            let current = open_update_file(&target).unwrap();
            // Read-only handle lacks DELETE access: atomic replacement fails, old stays.
            let cannot_rename = File::open(&stage).unwrap();
            assert!(replace_with_rollback(&cannot_rename, current, &target, &backup).is_err());
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old");
        std::fs::remove_file(&backup).unwrap();
        {
            let current = open_update_file(&target).unwrap();
            let staged = open_update_file(&stage).unwrap();
            replace_with_rollback(&staged, current, &target, &backup).unwrap();
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old");
        assert!(open_update_file(&stage).is_err());
        // Explicit rollback uses the same atomic primitive and retains failed bytes.
        let failed = root.join("failed.exe");
        {
            let current = open_update_file(&target).unwrap();
            let previous = open_update_file(&backup).unwrap();
            replace_with_rollback(&previous, current, &target, &failed).unwrap();
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(std::fs::read(&failed).unwrap(), b"new");
        assert!(!backup.exists());
        // Existing evidence is never overwritten, including during a retry.
        {
            let current = open_update_file(&target).unwrap();
            let previous = open_update_file(&failed).unwrap();
            assert!(replace_with_rollback(&previous, current, &target, &failed).is_err());
        }
        assert_eq!(std::fs::read(&target).unwrap(), b"old");
        assert_eq!(std::fs::read(&failed).unwrap(), b"new");
        assert!(open_update_file(&root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

/// Fresh, ACL-protected, same-user stage. The path is never taken from metadata.
pub fn create_private_stage(parent: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::{
        Foundation::*,
        Security::{Authorization::*, Cryptography::*, *},
        Storage::FileSystem::*,
        System::Threading::*,
    };
    use windows::core::PWSTR;
    if !matches!(parent.components().next(), Some(std::path::Component::Prefix(prefix)) if matches!(prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)))
    {
        return Err(std::io::Error::other("absolute local stage parent required"));
    }
    for ancestor in parent.ancestors() {
        use std::os::windows::fs::MetadataExt;
        let metadata = std::fs::symlink_metadata(ancestor)?;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
            return Err(std::io::Error::other("reparse stage root refused"));
        }
    }
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(std::io::Error::other)?;
        let result = (|| {
            let mut needed = 0;
            let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
            if needed == 0 || needed > 65536 {
                return Err(std::io::Error::other("token size"));
            }
            let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
            GetTokenInformation(token, TokenUser, Some(buffer.as_mut_ptr().cast()), needed, &mut needed)
                .map_err(std::io::Error::other)?;
            let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
            let mut sid = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut sid).map_err(std::io::Error::other)?;
            let text = sid.to_string().map_err(std::io::Error::other);
            let _ = LocalFree(Some(HLOCAL(sid.0.cast())));
            let text = text?;
            let descriptor_text: Vec<u16> = format!("D:P(A;OICI;FA;;;{text})")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(descriptor_text.as_ptr()),
                1,
                &mut descriptor,
                None,
            )
            .map_err(std::io::Error::other)?;
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            let mut random = [0_u8; 16];
            let result = BCryptGenRandom(None, &mut random, BCRYPT_USE_SYSTEM_PREFERRED_RNG)
                .ok()
                .map_err(std::io::Error::other)
                .and_then(|()| {
                    let suffix: String = random.iter().map(|b| format!("{b:02x}")).collect();
                    let path = parent.join(format!("bareline-update-{suffix}"));
                    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
                    CreateDirectoryW(PCWSTR(wide.as_ptr()), Some(&attributes)).map_err(std::io::Error::other)?;
                    Ok(path)
                });
            let _ = LocalFree(Some(HLOCAL(descriptor.0)));
            result
        })();
        let _ = CloseHandle(token);
        result
    }
}

pub struct PreparedUpdate {
    pub manifest: bareline_distribution::update::VerifiedManifest,
    pub file: File,
    pub directory: std::path::PathBuf,
    pub metadata_bytes: Vec<u8>,
    pub signature_text: String,
    /// Pending trust-state files downloaded into `directory` because the signed
    /// manifest delivers them (SEC-02); verified with [`verify_delivered_trust`].
    pub delivered_trust: Vec<&'static str>,
}
mod lifecycle;
pub use lifecycle::*;
mod runtime;
pub use runtime::*;
pub struct ResolvedReleaseAuthority {
    pub release_public_key: String,
    /// Authenticode signer pin. Manifest `publisher` fields are compared with the
    /// compiled `trust.publisher` identity instead, never with this pin (SEC-01).
    pub signer: PublisherPin,
    /// Signed SHA-256 of the installed update helper; `None` without a root policy.
    pub update_helper_sha256: Option<String>,
    /// Core executable floor only; the runtime and catalogs have their own (SEC-03).
    pub minimum_metadata_version: u64,
    pub minimum_runtime_metadata_version: u64,
    pub minimum_catalog_metadata_version: u64,
    pub catalog_public_key: Option<String>,
}
#[derive(Clone, Copy, Debug)]
pub struct OfflineRootPolicy<'a> {
    pub public_key: &'a str,
    pub minimum_version: u64,
}
#[cfg(test)]
mod authority_tests;
/// Optional offline root policy. A deployment opting in must supply a signed
/// authority; missing or revoked authority never falls back. The installation is read
/// only: ledgers and the lock live in the per-user `state` directory (SEC-04), and the
/// legacy install-root ledgers are still honored. `freshness` separates accepting new
/// metadata from using already-verified installed state (SEC-02).
#[allow(clippy::too_many_arguments)]
pub fn resolve_release_authority(
    root: &std::path::Path,
    state: &std::path::Path,
    embedded_key: &str,
    embedded_signer: &PublisherPin,
    embedded_floor: u64,
    offline_policy: Option<OfflineRootPolicy<'_>>,
    freshness: AuthorityFreshness,
    now: u64,
) -> std::io::Result<ResolvedReleaseAuthority> {
    let mut key = embedded_key.to_owned();
    let mut signer = embedded_signer.clone();
    let mut floor = embedded_floor;
    let mut runtime_floor = 0;
    let mut catalog_floor = 0;
    let mut catalog_public_key = None;
    let mut update_helper_sha256 = None;
    if let Some(policy) = offline_policy {
        if policy.public_key.is_empty() || policy.minimum_version == 0 {
            return Err(std::io::Error::other("invalid offline root policy"));
        }
        validate_install_root(root)?;
        let _lock = lock_update_installation(state)?;
        let chain_path = root.join(lifecycle::ROOT_TRANSITIONS);
        let chain = if chain_path.try_exists()? {
            Some(lifecycle::read_update_file(&chain_path, 262144)?)
        } else {
            None
        };
        let evaluated = lifecycle::evaluate_authority(
            root,
            state,
            chain.as_deref(),
            &lifecycle::read_update_file(&root.join(lifecycle::RELEASE_AUTHORITY), 16384)?,
            &lifecycle::read_update_file(&root.join(lifecycle::RELEASE_AUTHORITY_SIGNATURE), 8192)?,
            policy,
            freshness,
            now,
        )?;
        lifecycle::record_evaluated_authority(state, &evaluated)?;
        let authority = evaluated.authority;
        signer = authority.publisher_pin();
        key = authority.release_public_key;
        floor = floor.max(authority.minimum_metadata_version);
        runtime_floor = authority.minimum_runtime_metadata_version;
        catalog_floor = authority.minimum_catalog_metadata_version;
        catalog_public_key = Some(authority.catalog_public_key);
        update_helper_sha256 = Some(authority.update_helper_sha256);
    }
    signer
        .validate()
        .map_err(|_| std::io::Error::other("invalid Authenticode publisher pin"))?;
    Ok(ResolvedReleaseAuthority {
        release_public_key: key,
        signer,
        update_helper_sha256,
        minimum_metadata_version: floor,
        minimum_runtime_metadata_version: runtime_floor,
        minimum_catalog_metadata_version: catalog_floor,
        catalog_public_key,
    })
}

/// Transfer authenticated bytes to fixed helper inputs, never trusting metadata paths.
/// Each file is create-new and flushed; the manifest is committed last. The helper
/// repeats signature, digest and publisher checks, so interrupted transfer cannot apply.
pub fn transfer_update(
    mut prepared: PreparedUpdate,
    root: &std::path::Path,
    state: &std::path::Path,
) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    use std::os::windows::fs::OpenOptionsExt;
    validate_install_root(root)?;
    let _lock = lock_update_installation(state)?;
    let write_new = |name: &str, bytes: &[u8]| -> std::io::Result<()> {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(root.join(name))?;
        out.write_all(bytes)?;
        out.sync_all()
    };
    // Delivered trust state first; like the package it is inert until the helper
    // reverifies it against the signed manifest committed last (SEC-02).
    for name in &prepared.delivered_trust {
        let mut source = open_update_read_file(&prepared.directory.join(name))?;
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .share_mode(0)
            .open(root.join(name))?;
        std::io::copy(&mut source, &mut out)?;
        out.sync_all()?;
    }
    let mut package = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .share_mode(0)
        .open(root.join("bareline.pending.exe"))?;
    prepared.file.seek(SeekFrom::Start(0))?;
    std::io::copy(&mut prepared.file, &mut package)?;
    package.sync_all()?;
    drop(package);
    write_new("bareline.update.minisig", prepared.signature_text.as_bytes())?;
    write_new("bareline.update.json", &prepared.metadata_bytes)?;
    discard_prepared_update(prepared);
    Ok(())
}

/// Remove a verified but unused staging directory. Deletes only the known files in the
/// private directory created by our worker.
pub fn discard_prepared_update(prepared: PreparedUpdate) {
    drop(prepared.file);
    let _ = std::fs::remove_file(prepared.directory.join("package.exe"));
    for name in prepared.delivered_trust {
        let _ = std::fs::remove_file(prepared.directory.join(name));
    }
    let _ = std::fs::remove_dir(prepared.directory);
}

pub fn validate_install_root(root: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::fs::MetadataExt;
    if !matches!(root.components().next(), Some(std::path::Component::Prefix(prefix)) if matches!(prefix.kind(), std::path::Prefix::Disk(_)))
        || !root.is_absolute()
    {
        return Err(std::io::Error::other("absolute local installation root required"));
    }
    for ancestor in root.ancestors() {
        if std::fs::symlink_metadata(ancestor)?.file_attributes() & 0x400 != 0 {
            return Err(std::io::Error::other("reparse installation root refused"));
        }
    }
    Ok(())
}

/// Serializes transfer, apply, acknowledgement and ledger updates without deleting a
/// lock pathname. The lock lives in the per-user update `state` directory from
/// [`update_state_root`], never in the (possibly read-only) installation (SEC-04).
pub fn lock_update_installation(state: &std::path::Path) -> std::io::Result<File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    validate_install_root(state)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .custom_flags(0x00200000)
        .open(state.join("update-lock"))?;
    if file.metadata()?.file_attributes() & 0x400 != 0 {
        return Err(std::io::Error::other("reparse update lock refused"));
    }
    Ok(file)
}

/// Preserve a completed/recovered attempt under unique names, journal last.
/// No deletion and no caller-supplied filenames; interruptions remain retryable.
pub fn retain_update_evidence(root: &std::path::Path) -> std::io::Result<()> {
    validate_install_root(root)?;
    let generation = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?
        .as_nanos();
    for name in lifecycle::RETAINED_UPDATE_FILES {
        let path = root.join(name);
        if !path.try_exists()? {
            continue;
        }
        let held = open_update_file(&path)?;
        rename_update_handle(&held, &root.join(format!("{name}.retained-{generation}")))?;
    }
    Ok(())
}

/// Explicit cancellation/recovery of unapplied staging. Never discards an apply receipt.
pub fn discard_pending_update(root: &std::path::Path, state: &std::path::Path) -> std::io::Result<()> {
    validate_install_root(root)?;
    let _lock = lock_update_installation(state)?;
    if root.join("bareline.update-journal").try_exists()? {
        return Err(std::io::Error::other(
            "applied update requires acknowledgement or rollback",
        ));
    }
    retain_update_evidence(root)
}

/// What [`launch_update_helper`] asks the helper to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelperAction {
    /// Apply the staged update after this editor exits.
    Apply,
    /// Acknowledge that this freshly updated editor reached a healthy frame.
    Acknowledge,
    /// Restore the build the last update replaced after this editor exits: the
    /// "Roll back last update" command (SEC-09).
    Recover,
    /// Automatic recovery after repeated failed launches of a freshly updated build: as
    /// [`HelperAction::Recover`] from the apply journal only, then the helper starts the
    /// editor again, restored or reporting the failure (SEC-09).
    AutoRecover,
}

/// Spawn the exact adjacent helper whose bytes match the signed release authority and
/// whose Authenticode signer matches the pin (never Authenticode alone, SEC-08), without
/// a console window. Applying is an explicit flow with online revocation; the
/// acknowledgement and recovery rely on the signed hash (SEC-07).
pub fn launch_update_helper(
    root: &std::path::Path,
    authority: &ResolvedReleaseAuthority,
    action: HelperAction,
) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    validate_install_root(root)?;
    let expected = authority
        .update_helper_sha256
        .as_deref()
        .ok_or_else(|| std::io::Error::other("the signed release authority does not pin the update helper"))?;
    let path = root.join("bareline-update-helper.exe");
    let mut held = open_update_read_file(&path)?;
    if update_file_sha256(&mut held)? != expected {
        return Err(std::io::Error::other(
            "update helper differs from the signed release authority",
        ));
    }
    verify_authenticode(&held, &authority.signer, helper_revocation(action))
        .map_err(|e| std::io::Error::other(format!("helper publisher: {e:?}")))?;
    let mut command = std::process::Command::new(&path);
    command.creation_flags(0x08000000).current_dir(root);
    {
        use windows::Win32::Security::Cryptography::*;
        use windows::Win32::System::Threading::*;
        let mut random = [0u8; 16];
        unsafe { BCryptGenRandom(None, &mut random, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
            .ok()
            .map_err(std::io::Error::other)?;
        let nonce: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let name = format!("Local\\Bareline.UpdateReady.{}.{nonce}", std::process::id());
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let event = unsafe { CreateEventW(None, true, false, PCWSTR(wide.as_ptr())) }.map_err(std::io::Error::other)?;
        let result = (|| {
            let (verb, pid) = match action {
                HelperAction::Apply => ("--apply", "--wait-pid"),
                HelperAction::Acknowledge => ("--acknowledge", "--healthy-pid"),
                HelperAction::Recover => ("--recover", "--wait-pid"),
                HelperAction::AutoRecover => ("--auto-recover", "--wait-pid"),
            };
            command.args([verb, pid, &std::process::id().to_string(), "--ready-event", &name]);
            command.spawn()?;
            // Do not let this process disappear before the helper holds its identity.
            if unsafe { WaitForSingleObject(event, 10_000) } != windows::Win32::Foundation::WAIT_OBJECT_0 {
                return Err(std::io::Error::other("update helper did not acknowledge parent"));
            }
            Ok(())
        })();
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(event) };
        result?;
    }
    Ok(())
}

/// Applying is explicit and fetches revocation online; acknowledging a healthy launch
/// and restoring the previous build (possibly offline, after failed launches) do not.
fn helper_revocation(action: HelperAction) -> Revocation {
    match action {
        HelperAction::Apply => Revocation::Online,
        HelperAction::Acknowledge | HelperAction::Recover | HelperAction::AutoRecover => Revocation::Offline,
    }
}

pub struct HealthyUpdateProcess(HANDLE, Option<[File; 2]>);
impl Drop for HealthyUpdateProcess {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
}
/// Bind a health acknowledgement to the live image that produced the ready frame.
/// A rollback/retained executable in the same directory cannot acknowledge the target.
pub fn hold_healthy_update_process(pid: u32, target: &std::path::Path) -> std::io::Result<HealthyUpdateProcess> {
    use windows::Win32::System::Threading::*;
    use windows::core::PWSTR;
    unsafe {
        use windows::Win32::System::Diagnostics::ToolHelp::*;
        let snapshot = HealthyUpdateProcess(
            CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(std::io::Error::other)?,
            None,
        );
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        Process32FirstW(snapshot.0, &mut entry).map_err(std::io::Error::other)?;
        loop {
            if entry.th32ProcessID == std::process::id() {
                break;
            }
            Process32NextW(snapshot.0, &mut entry).map_err(std::io::Error::other)?;
        }
        if entry.th32ParentProcessID != pid {
            return Err(std::io::Error::other("healthy PID is not helper's parent"));
        }
        let mut process = HealthyUpdateProcess(
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, pid)
                .map_err(std::io::Error::other)?,
            None,
        );
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        QueryFullProcessImageNameW(process.0, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut length)
            .map_err(std::io::Error::other)?;
        use std::os::windows::ffi::OsStringExt;
        let actual = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]));
        #[cfg(test)]
        if std::env::var_os("BARELINE_TEST_HEALTH_PROBE").is_some() {
            eprintln!("health fixture paths: observed={actual:?}, expected={target:?}");
        }
        // File identity respects case-sensitive directories and does not reinterpret path text.
        // Both sealed handles survive the acknowledgement; neither file can be replaced/written.
        let observed = open_update_read_file(&actual)?;
        let expected = open_update_read_file(target)?;
        if !same_file(&observed, &expected)?
            || WaitForSingleObject(process.0, 0) != windows::Win32::Foundation::WAIT_TIMEOUT
        {
            return Err(std::io::Error::other(
                "healthy process is not the running update target",
            ));
        }
        process.1 = Some([observed, expected]);
        Ok(process)
    }
}

/// Volume serial number plus file index, never path text (SEC-18).
fn same_file(left: &File, right: &File) -> std::io::Result<bool> {
    use bareline_platform::LocalFileSystem;
    let left = crate::files::WindowsFileSystem.identity(left)?;
    let right = crate::files::WindowsFileSystem.identity(right)?;
    Ok(left.volume == right.volume && left.file == right.file)
}

pub fn signal_update_parent_ready(pid: u32, ready_event: &str) -> std::io::Result<()> {
    use windows::Win32::System::Threading::*;
    if !ready_event.starts_with(&format!("Local\\Bareline.UpdateReady.{pid}."))
        || ready_event.len() > 128
        || ready_event.contains('\0')
    {
        return Err(std::io::Error::other("invalid ready event"));
    }
    let name: Vec<u16> = ready_event.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let event = HealthyUpdateProcess(
            OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(name.as_ptr())).map_err(std::io::Error::other)?,
            None,
        );
        SetEvent(event.0).map_err(std::io::Error::other)
    }
}

/// Hold the parent process identity before waiting; PID reuse cannot redirect the wait.
pub fn wait_for_update_parent(pid: u32, target: &std::path::Path, ready_event: &str) -> std::io::Result<()> {
    use windows::Win32::System::Threading::*;
    use windows::core::PWSTR;
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, false, pid)
            .map_err(std::io::Error::other)?;
        let result = (|| {
            let mut buffer = vec![0u16; 32768];
            let mut length = buffer.len() as u32;
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut length)
                .map_err(std::io::Error::other)?;
            use std::os::windows::ffi::OsStringExt;
            let actual = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..length as usize]));
            // Compare file identity; path spelling (case, 8.3, prefixes) is not identity (SEC-18).
            // Handles close before waiting so the exited image can be replaced.
            if !same_file(&open_update_read_file(&actual)?, &open_update_read_file(target)?)? {
                return Err(std::io::Error::other("update parent image differs"));
            }
            if !ready_event.starts_with(&format!("Local\\Bareline.UpdateReady.{pid}."))
                || ready_event.len() > 128
                || ready_event.contains('\0')
            {
                return Err(std::io::Error::other("invalid ready event"));
            }
            let name: Vec<u16> = ready_event.encode_utf16().chain(Some(0)).collect();
            let event = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(name.as_ptr())).map_err(std::io::Error::other)?;
            let signaled = SetEvent(event).map_err(std::io::Error::other);
            let _ = windows::Win32::Foundation::CloseHandle(event);
            signaled?;
            if WaitForSingleObject(process, 120_000) != windows::Win32::Foundation::WAIT_OBJECT_0 {
                return Err(std::io::Error::other("editor did not exit within update timeout"));
            }
            Ok(())
        })();
        let _ = windows::Win32::Foundation::CloseHandle(process);
        result
    }
}
/// Explicit worker API: authenticate metadata in bounded memory before creating
/// application staging files, then download/hash/WinVerifyTrust the same held file.
/// Caller owns cleanup of successful staging and must reverify in the helper.
#[allow(clippy::too_many_arguments)]
pub fn fetch_verified_update(
    host: &str,
    manifest_path: &str,
    signature_path: &str,
    artifact_path: &str,
    policy: &bareline_distribution::update::TrustPolicy<'_>,
    now_unix: u64,
    signer: &PublisherPin,
    stage_parent: &std::path::Path,
    cancel: &AtomicBool,
) -> Result<PreparedUpdate, UpdateError> {
    fetch_verified_update_with(
        manifest_path,
        signature_path,
        artifact_path,
        policy,
        now_unix,
        signer,
        stage_parent,
        cancel,
        |path, maximum, output| download_https(host, path, maximum, cancel, output),
        |file: &File, pin: &PublisherPin| verify_authenticode(file, pin, Revocation::Online),
    )
}

/// Shared verification pipeline. Tests inject a bounded, local byte transport;
/// production always supplies HTTPS and WinVerifyTrust above.
#[allow(clippy::too_many_arguments)]
fn fetch_verified_update_with(
    manifest_path: &str,
    signature_path: &str,
    artifact_path: &str,
    policy: &bareline_distribution::update::TrustPolicy<'_>,
    now_unix: u64,
    signer: &PublisherPin,
    stage_parent: &std::path::Path,
    cancel: &AtomicBool,
    mut download: impl FnMut(&str, u64, &mut dyn Write) -> Result<u64, UpdateError>,
    verify_publisher: impl Fn(&File, &PublisherPin) -> Result<(), UpdateError>,
) -> Result<PreparedUpdate, UpdateError> {
    use std::io::{Seek, SeekFrom};
    use std::os::windows::fs::OpenOptionsExt;
    let check_cancelled = || {
        if cancel.load(Ordering::Acquire) {
            Err(UpdateError::Cancelled)
        } else {
            Ok(())
        }
    };
    let mut metadata = Vec::new();
    let mut signature = Vec::new();
    check_cancelled()?;
    download(manifest_path, 65536, &mut metadata)?;
    check_cancelled()?;
    download(signature_path, 8192, &mut signature)?;
    check_cancelled()?;
    let signature = std::str::from_utf8(&signature).map_err(|_| UpdateError::Signature)?;
    let manifest = bareline_distribution::update::verify_manifest(&metadata, signature, policy, now_unix)
        .map_err(|_| UpdateError::Signature)?;
    check_cancelled()?;
    let directory = create_private_stage(stage_parent).map_err(|_| UpdateError::Io)?;
    let path = directory.join("package.exe");
    let result = (|| {
        check_cancelled()?;
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .share_mode(0)
            .open(&path)
            .map_err(|_| UpdateError::Io)?;
        download(artifact_path, manifest.metadata().length, &mut file)?;
        check_cancelled()?;
        file.sync_all().map_err(|_| UpdateError::Io)?;
        file.seek(SeekFrom::Start(0)).map_err(|_| UpdateError::Io)?;
        manifest.verify_package(&mut file).map_err(|_| UpdateError::Signature)?;
        check_cancelled()?;
        verify_publisher(&file, signer)?;
        check_cancelled()?;
        // A manifest delivering trust state names it by digest (SEC-02). The files are
        // served next to the manifest, never at metadata-chosen paths, and are verified
        // against the root policy by `verify_delivered_trust` before staging.
        let mut delivered_trust = Vec::new();
        if manifest.metadata().authority_sha256.is_some() {
            let transitions = manifest.metadata().root_transitions_sha256.is_some();
            for (name, published, limit) in lifecycle::DELIVERED_TRUST_FILES {
                if name == lifecycle::PENDING_ROOT_TRANSITIONS && !transitions {
                    continue;
                }
                let mut output = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .share_mode(0)
                    .open(directory.join(name))
                    .map_err(|_| UpdateError::Io)?;
                delivered_trust.push(name);
                download(
                    &lifecycle::delivered_trust_path(manifest_path, published),
                    limit,
                    &mut output,
                )?;
                output.sync_all().map_err(|_| UpdateError::Io)?;
                check_cancelled()?;
            }
        }
        Ok(PreparedUpdate {
            manifest,
            file,
            directory: directory.clone(),
            metadata_bytes: metadata.clone(),
            signature_text: signature.to_owned(),
            delivered_trust,
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
        for (name, _, _) in lifecycle::DELIVERED_TRUST_FILES {
            let _ = std::fs::remove_file(directory.join(name));
        }
        let _ = std::fs::remove_dir(directory);
    }
    result
}

#[cfg(test)]
mod stage_tests {
    use super::*;
    #[test]
    fn running_image_probe() {
        let Some(ready) = std::env::var_os("BARELINE_TEST_RUNNING_READY") else {
            return;
        };
        std::fs::write(ready, b"ready").unwrap();
        let mut byte = [0];
        std::io::Read::read_exact(&mut std::io::stdin(), &mut byte).unwrap();
    }
    #[test]
    fn atomic_update_refuses_a_running_target_image() {
        use std::io::Write;
        let root = create_private_stage(&std::env::temp_dir()).unwrap();
        let target = root.join("bareline.exe");
        let stage = root.join("stage.exe");
        let ready = root.join("ready");
        std::fs::copy(std::env::current_exe().unwrap(), &target).unwrap();
        std::fs::write(&stage, b"new-image").unwrap();
        let mut child = std::process::Command::new(&target)
            .args(["--exact", "update::stage_tests::running_image_probe"])
            .env("BARELINE_TEST_RUNNING_READY", &ready)
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        for _ in 0..200 {
            if ready.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(ready.exists());
        let result = open_update_file(&target).and_then(|current| {
            let staged = open_update_file(&stage)?;
            replace_with_rollback(&staged, current, &target, &root.join("backup.exe"))
        });
        child.stdin.take().unwrap().write_all(b"x").unwrap();
        assert!(child.wait().unwrap().success());
        assert!(result.is_err(), "running target must refuse atomic replacement");
        for entry in std::fs::read_dir(&root).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn healthy_parent_probe() {
        let Ok(mode) = std::env::var("BARELINE_TEST_HEALTH_PROBE") else {
            return;
        };
        let mut pid = std::env::var("BARELINE_TEST_HEALTH_PID")
            .unwrap()
            .parse::<u32>()
            .unwrap();
        let mut target = std::env::current_exe().unwrap();
        if mode == "case-variant" {
            use std::os::windows::ffi::{OsStrExt, OsStringExt};
            let units: Vec<u16> = target
                .as_os_str()
                .encode_wide()
                .map(|unit| {
                    if (b'a' as u16..=b'z' as u16).contains(&unit) {
                        unit - 32
                    } else if (b'A' as u16..=b'Z' as u16).contains(&unit) {
                        unit + 32
                    } else {
                        unit
                    }
                })
                .collect();
            target = std::ffi::OsString::from_wide(&units).into();
        }
        let wrong_root = if mode == "wrong-image" {
            let root = create_private_stage(&std::env::temp_dir()).unwrap();
            let copy = root.join("bareline.rollback.exe");
            std::fs::copy(&target, &copy).unwrap();
            target = copy;
            Some(root)
        } else {
            None
        };
        if mode == "forged-pid" {
            pid = std::process::id();
        }
        let result = hold_healthy_update_process(pid, &target);
        let accepted = result.is_ok();
        let error = result.err();
        if let Some(root) = wrong_root {
            std::fs::remove_file(&target).unwrap();
            std::fs::remove_dir(root).unwrap();
        }
        assert_eq!(
            accepted,
            matches!(mode.as_str(), "valid" | "case-variant"),
            "health fixture mode={mode}, expected={target:?}, error={error:?}"
        );
    }
    #[test]
    fn health_ack_requires_actual_parent_and_exact_running_image() {
        for mode in ["valid", "case-variant", "wrong-image", "forged-pid"] {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "update::stage_tests::healthy_parent_probe"])
                .env("BARELINE_TEST_HEALTH_PROBE", mode)
                .env("BARELINE_TEST_HEALTH_PID", std::process::id().to_string())
                .status()
                .unwrap();
            assert!(status.success(), "health probe {mode}");
        }
    }
    #[test]
    fn transfer_and_retention_preserve_exact_bytes_without_network() {
        use bareline_distribution::update::{TrustPolicy, verify_manifest};
        let root = create_private_stage(&std::env::temp_dir()).unwrap();
        let state = create_private_stage(&std::env::temp_dir()).unwrap();
        let lock = lock_update_installation(&state).unwrap();
        assert!(lock_update_installation(&state).is_err());
        drop(lock);
        let stage = create_private_stage(&std::env::temp_dir()).unwrap();
        let metadata = include_bytes!("../../distribution/tests/fixtures/valid.json");
        let signature = include_str!("../../distribution/tests/fixtures/valid.minisig");
        let policy = TrustPolicy {
            release_public_key: include_str!("../../distribution/tests/fixtures/public-key.txt"),
            channel: "stable",
            artifact_type: "bareline-x64",
            platform: "windows-x64",
            publisher: "test-publisher",
            protocol: 1,
            highest_metadata_version: 3,
            maximum_package_bytes: 4096,
        };
        std::fs::write(stage.join("package.exe"), b"test").unwrap();
        let prepared = PreparedUpdate {
            manifest: verify_manifest(metadata, signature, &policy, 100).unwrap(),
            file: open_update_file(&stage.join("package.exe")).unwrap(),
            directory: stage,
            metadata_bytes: metadata.to_vec(),
            signature_text: signature.into(),
            delivered_trust: Vec::new(),
        };
        transfer_update(prepared, &root, &state).unwrap();
        assert_eq!(std::fs::read(root.join("bareline.pending.exe")).unwrap(), b"test");
        assert_eq!(std::fs::read(root.join("bareline.update.json")).unwrap(), metadata);
        std::fs::write(root.join("bareline.update-journal"), b"receipt").unwrap();
        std::fs::write(root.join("bareline.exe"), b"running").unwrap();
        assert!(discard_pending_update(&root, &state).is_err());
        retain_update_evidence(&root).unwrap();
        assert_eq!(std::fs::read(root.join("bareline.exe")).unwrap(), b"running");
        assert!(!root.join("bareline.update-journal").exists());
        assert!(!root.join("bareline.pending.exe").exists());
        discard_pending_update(&root, &state).unwrap();
        // The lock lives in per-user state, never in the installation (SEC-04).
        assert!(state.join("update-lock").is_file());
        std::fs::remove_file(state.join("update-lock")).unwrap();
        std::fs::remove_dir(state).unwrap();
        let files = std::fs::read_dir(&root)
            .unwrap()
            .map(|f| f.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(files.len(), 5);
        assert!(files.iter().any(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("bareline.update-journal.retained-")
                && std::fs::read(p).unwrap() == b"receipt"
        }));
        for file in files {
            std::fs::remove_file(file).unwrap();
        }
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn local_fixture_transport_rejects_signature_policy_certificate_cancel_corruption_and_unavailable_runtime() {
        use bareline_distribution::update::TrustPolicy;
        use std::collections::BTreeMap;
        let metadata = include_bytes!("../../distribution/tests/fixtures/valid.json").to_vec();
        let signature = include_bytes!("../../distribution/tests/fixtures/valid.minisig").to_vec();
        let key: &'static str = include_str!("../../distribution/tests/fixtures/public-key.txt");
        let policy = |channel: &'static str, floor| TrustPolicy {
            release_public_key: key,
            channel,
            artifact_type: "bareline-x64",
            platform: "windows-x64",
            publisher: "test-publisher",
            protocol: 1,
            highest_metadata_version: floor,
            maximum_package_bytes: 4096,
        };
        let run = |metadata: Vec<u8>,
                   signature: Vec<u8>,
                   artifact: Vec<u8>,
                   trust: TrustPolicy<'_>,
                   cancel: bool,
                   certificate: u8,
                   runtime_available: bool| {
            let signer = PublisherPin::parse(&format!("Publisher {certificate}"), "Fixture CA").unwrap();
            let root = create_private_stage(&std::env::temp_dir()).unwrap();
            let paths = BTreeMap::from([
                ("/manifest", metadata),
                ("/signature", signature),
                ("/artifact", artifact),
            ]);
            let cancelled = AtomicBool::new(cancel);
            let result = fetch_verified_update_with(
                "/manifest",
                "/signature",
                "/artifact",
                &trust,
                100,
                &signer,
                &root,
                &cancelled,
                |path, maximum, output| {
                    if cancelled.load(Ordering::Acquire) {
                        return Err(UpdateError::Cancelled);
                    }
                    if path == "/artifact" && !runtime_available {
                        return Err(UpdateError::Network);
                    }
                    let bytes = paths.get(path).ok_or(UpdateError::InvalidEndpoint)?;
                    if bytes.len() as u64 > maximum {
                        return Err(UpdateError::Limit);
                    }
                    output.write_all(bytes).map_err(|_| UpdateError::Io)?;
                    Ok(bytes.len() as u64)
                },
                |_file: &File, expected: &PublisherPin| {
                    if expected.subject == "Publisher 7" {
                        Ok(())
                    } else {
                        Err(UpdateError::Publisher)
                    }
                },
            );
            let result = match result {
                Ok(prepared) => {
                    drop(prepared);
                    Ok(())
                }
                Err(error) => Err(error),
            };
            for entry in std::fs::read_dir(&root).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    for child in std::fs::read_dir(&path).unwrap() {
                        let _ = std::fs::remove_file(child.unwrap().path());
                    }
                    let _ = std::fs::remove_dir(path);
                }
            }
            let _ = std::fs::remove_dir(root);
            result
        };
        assert!(
            run(
                metadata.clone(),
                signature.clone(),
                b"test".to_vec(),
                policy("stable", 3),
                false,
                7,
                true
            )
            .is_ok()
        );
        let mut bad_signature = signature.clone();
        bad_signature[50] ^= 1;
        assert!(matches!(
            run(
                metadata.clone(),
                bad_signature,
                b"test".to_vec(),
                policy("stable", 3),
                false,
                7,
                true
            ),
            Err(UpdateError::Signature)
        ));
        assert!(matches!(
            run(
                metadata.clone(),
                signature.clone(),
                b"test".to_vec(),
                policy("beta", 3),
                false,
                7,
                true
            ),
            Err(UpdateError::Signature)
        ));
        assert!(matches!(
            run(
                metadata.clone(),
                signature.clone(),
                b"test".to_vec(),
                policy("stable", 4),
                false,
                7,
                true
            ),
            Err(UpdateError::Signature)
        ));
        assert!(matches!(
            run(
                metadata.clone(),
                signature.clone(),
                b"test".to_vec(),
                policy("stable", 3),
                false,
                8,
                true
            ),
            Err(UpdateError::Publisher)
        ));
        assert!(matches!(
            run(
                metadata.clone(),
                signature.clone(),
                b"test".to_vec(),
                policy("stable", 3),
                true,
                7,
                true
            ),
            Err(UpdateError::Cancelled)
        ));
        assert!(matches!(
            run(
                metadata.clone(),
                signature.clone(),
                b"tent".to_vec(),
                policy("stable", 3),
                false,
                7,
                true
            ),
            Err(UpdateError::Signature)
        ));
        assert!(matches!(
            run(
                metadata,
                signature,
                b"test".to_vec(),
                policy("stable", 3),
                false,
                7,
                false
            ),
            Err(UpdateError::Network)
        ));
    }
    #[test]
    fn private_stage_is_unique_and_missing_parent_refused() {
        let parent = std::env::temp_dir();
        let one = create_private_stage(&parent).unwrap();
        let two = create_private_stage(&parent).unwrap();
        assert_ne!(one, two);
        assert!(one.is_dir() && two.is_dir());
        assert!(create_private_stage(&one.join("missing")).is_err());
        assert!(create_private_stage(std::path::Path::new(r"\\example.invalid\share")).is_err());
        std::fs::remove_dir(one).unwrap();
        std::fs::remove_dir(two).unwrap();
    }

    #[test]
    fn explicit_offline_root_policy_never_falls_back_on_missing_authority() {
        let parent = std::env::temp_dir();
        let root = create_private_stage(&parent).unwrap();
        assert!(root.canonicalize().unwrap().starts_with(parent.canonicalize().unwrap()));
        let signer = PublisherPin::parse("Legacy Publisher", "Legacy CA").unwrap();
        // The explicit legacy path remains available to callers without root policy.
        let legacy = resolve_release_authority(
            &root,
            &root,
            "legacy",
            &signer,
            3,
            None,
            AuthorityFreshness::Required,
            100,
        )
        .unwrap();
        assert_eq!(legacy.release_public_key, "legacy");
        assert_eq!(legacy.signer, signer);
        // Without a signed authority there is no helper hash, so the helper cannot launch.
        assert_eq!(legacy.update_helper_sha256, None);
        assert!(launch_update_helper(&root, &legacy, HelperAction::Acknowledge).is_err());
        for policy in [
            OfflineRootPolicy {
                public_key: "",
                minimum_version: 1,
            },
            OfflineRootPolicy {
                public_key: "root",
                minimum_version: 0,
            },
            OfflineRootPolicy {
                public_key: "root",
                minimum_version: 1,
            },
        ] {
            for freshness in [AuthorityFreshness::Required, AuthorityFreshness::Installed] {
                assert!(
                    resolve_release_authority(&root, &root, "legacy", &signer, 3, Some(policy), freshness, 100)
                        .is_err()
                );
            }
        }
        for entry in std::fs::read_dir(&root).unwrap() {
            let path = entry.unwrap().path();
            assert!(path.is_file());
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(root).unwrap();
    }
}

/// Hash an exact held file with fixed memory, preserving its cursor for callers.
pub fn update_file_sha256(file: &mut File) -> std::io::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let position = file.stream_position()?;
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 65536];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    file.seek(SeekFrom::Start(position))?;
    Ok(format!("{:x}", hash.finalize()))
}
