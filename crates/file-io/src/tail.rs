// SPDX-License-Identifier: MPL-2.0
//! Incremental conservative tail continuity verification. Rehashes the complete old
//! prefix before accepting growth; O(file length) I/O, constant verification RAM.
use crate::{
    cancellation::Cancellation,
    lifecycle::{FileError, Fingerprint},
};
use bareline_platform::{FileIdentity, LocalFileSystem};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};
pub enum TailProgress {
    Pending { verified: u64, total: u64 },
    Verified(Fingerprint),
    SourceChanged,
}
pub struct TailVerifier {
    path: PathBuf,
    file: File,
    platform: Arc<dyn LocalFileSystem>,
    expected: Fingerprint,
    current: FileIdentity,
    hash: Sha256,
    offset: u64,
    prefix_verified: bool,
    changed: bool,
    cancellation: Cancellation,
}
impl TailVerifier {
    /// Establish an initial full fingerprint against the displayed source identity.
    /// The viewport remains usable while the caller drives these bounded steps.
    pub fn baseline(
        path: &Path,
        displayed: FileIdentity,
        platform: Arc<dyn LocalFileSystem>,
        cancellation: Cancellation,
    ) -> Result<Self, FileError> {
        let mut empty = displayed;
        empty.length = 0;
        let expected = Fingerprint {
            identity: empty,
            sha256: Sha256::digest([]).into(),
        };
        let mut verifier = Self::begin(path, expected, platform, cancellation)?;
        verifier.changed |= verifier.current != displayed;
        Ok(verifier)
    }
    /// Caller must have authorized the path and captured a complete old fingerprint.
    pub fn begin(
        path: &Path,
        expected: Fingerprint,
        platform: Arc<dyn LocalFileSystem>,
        cancellation: Cancellation,
    ) -> Result<Self, FileError> {
        cancellation.check()?;
        platform.validate_source(path)?;
        let file = File::open(path)?;
        let current = platform.identity(&file)?;
        let changed = current.volume != expected.identity.volume
            || current.file != expected.identity.file
            || current.length < expected.identity.length;
        Ok(Self {
            path: path.into(),
            file,
            platform,
            expected,
            current,
            hash: Sha256::new(),
            offset: 0,
            prefix_verified: false,
            changed,
            cancellation,
        })
    }
    /// One bounded 64 KiB step on an I/O worker. No bytes are accepted before verification.
    pub fn step(&mut self) -> Result<TailProgress, FileError> {
        self.cancellation.check()?;
        if self.changed {
            return Ok(TailProgress::SourceChanged);
        }
        if self.platform.identity(&self.file)? != self.current
            || self.platform.identity(&File::open(&self.path)?)? != self.current
        {
            self.changed = true;
            return Ok(TailProgress::SourceChanged);
        }
        if !self.prefix_verified && self.offset == self.expected.identity.length {
            if <[u8; 32]>::from(self.hash.clone().finalize()) != self.expected.sha256 {
                self.changed = true;
                return Ok(TailProgress::SourceChanged);
            }
            self.prefix_verified = true;
        }
        if self.offset == self.current.length {
            return Ok(TailProgress::Verified(Fingerprint {
                identity: self.current,
                sha256: self.hash.clone().finalize().into(),
            }));
        }
        let end = if self.prefix_verified {
            self.current.length
        } else {
            self.expected.identity.length
        };
        let count = (end - self.offset).min(64 * 1024) as usize;
        let mut bytes = [0u8; 64 * 1024];
        self.file.read_exact(&mut bytes[..count])?;
        self.hash.update(&bytes[..count]);
        self.offset += count as u64;
        Ok(TailProgress::Pending {
            verified: self.offset,
            total: self.current.length,
        })
    }
}
/// Follow-scroll is separate from ingestion. Unlock is an explicit fixed-generation transition.
#[derive(Default)]
pub struct FollowState {
    paused_scroll: bool,
    unlocked: bool,
}
impl FollowState {
    pub fn set_scrolled_away(&mut self, away: bool) {
        self.paused_scroll = away;
    }
    pub fn auto_scroll(&self) -> bool {
        !self.paused_scroll && !self.unlocked
    }
    pub fn following(&self) -> bool {
        !self.unlocked
    }
    pub fn unlock(&mut self, confirmed: bool) -> bool {
        if confirmed {
            self.unlocked = true;
        }
        self.unlocked
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };
    struct Platform;
    impl LocalFileSystem for Platform {
        fn validate_target(&self, _: &Path) -> std::io::Result<()> {
            Ok(())
        }
        fn identity(&self, f: &File) -> std::io::Result<FileIdentity> {
            let m = f.metadata()?;
            Ok(FileIdentity {
                volume: 1,
                file: 1,
                length: m.len(),
                modified: m
                    .modified()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64,
            })
        }
        fn commit(&self, _: &Path, _: &Path, _: bool) -> std::io::Result<()> {
            unreachable!()
        }
    }
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static ID: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "bareline-tail-{}-{}",
                std::process::id(),
                ID.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::write(&p, b"partial").unwrap();
            Self(p)
        }
        fn fingerprint(&self) -> Fingerprint {
            Fingerprint {
                identity: Platform.identity(&File::open(&self.0).unwrap()).unwrap(),
                sha256: Sha256::digest(b"partial").into(),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn finish(v: &mut TailVerifier) -> TailProgress {
        for _ in 0..20 {
            let p = v.step().unwrap();
            if !matches!(p, TailProgress::Pending { .. }) {
                return p;
            }
        }
        panic!("did not complete")
    }
    #[test]
    fn partial_page_append_verified_and_new_generation_created() {
        let f = Fixture::new();
        let mut baseline = TailVerifier::baseline(
            &f.0,
            f.fingerprint().identity,
            Arc::new(Platform),
            Cancellation::default(),
        )
        .unwrap();
        let TailProgress::Verified(old) = finish(&mut baseline) else {
            panic!("baseline not verified")
        };
        use bareline_document::source::SourceRead;
        let options = crate::source::SourceOptions {
            resident_max_bytes: 0,
            page_size_bytes: 8,
            page_cache_bytes: 16,
        };
        let mut before = crate::source::FileSource::open(
            &f.0,
            Arc::new(Platform),
            options,
            bareline_document::Budget::new(32),
            Cancellation::default(),
        )
        .unwrap();
        let old_source = before.source();
        let SourceRead::Pending(ticket) = old_source.read(0..7) else {
            panic!()
        };
        before.read_page(ticket).unwrap();
        let SourceRead::Ready(old_ready) = old_source.read(0..7) else {
            panic!()
        };
        let mut writer = std::fs::OpenOptions::new().append(true).open(&f.0).unwrap();
        writer.write_all(b" next").unwrap();
        drop(writer);
        let mut v =
            TailVerifier::begin(&f.0, old, Arc::new(Platform), Cancellation::default()).unwrap();
        let TailProgress::Verified(new) = finish(&mut v) else {
            panic!("append rejected")
        };
        assert_eq!(new.identity.length, 12);
        let mut p = crate::source::FileSource::open_verified_generation(
            &f.0,
            Arc::new(Platform),
            options,
            bareline_document::Budget::new(2 * 1024 * 1024),
            Cancellation::default(),
            &new,
        )
        .unwrap();
        assert_eq!(p.kind(), bareline_document::source::SourceKind::Paged);
        let source = p.source();
        let SourceRead::Pending(ticket) = source.read(0..8) else {
            panic!()
        };
        p.read_page(ticket).unwrap();
        let SourceRead::Ready(new_ready) = source.read(0..8) else {
            panic!()
        };
        assert_eq!(new_ready.bytes(), b"partial ");
        assert_eq!(old_ready.bytes(), b"partial");
        assert_ne!(old_ready.generation, new_ready.generation);
    }
    #[test]
    fn rewrite_and_truncate_never_accepted() {
        for replacement in [b"changed".as_slice(), b"x".as_slice()] {
            let f = Fixture::new();
            let old = f.fingerprint();
            std::fs::write(&f.0, replacement).unwrap();
            let mut v = TailVerifier::begin(&f.0, old, Arc::new(Platform), Cancellation::default())
                .unwrap();
            assert!(matches!(finish(&mut v), TailProgress::SourceChanged));
        }
    }
    #[test]
    fn cancellation_and_scroll_pause_are_independent() {
        let f = Fixture::new();
        let cancel = Cancellation::default();
        let mut v =
            TailVerifier::begin(&f.0, f.fingerprint(), Arc::new(Platform), cancel.clone()).unwrap();
        cancel.cancel();
        assert!(matches!(v.step(), Err(FileError::Cancelled)));
        let mut state = FollowState::default();
        state.set_scrolled_away(true);
        assert!(state.following());
        assert!(!state.auto_scroll());
        assert!(!state.unlock(false));
        assert!(state.unlock(true));
        assert!(!state.following());
    }
}
