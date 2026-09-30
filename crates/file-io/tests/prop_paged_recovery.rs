// SPDX-License-Identifier: MPL-2.0
//! Seeded property tests (QA-16): a `PagedSession` opened from disk takes random
//! materialized edits and journals them through `PagedRecovery`. The session stamp
//! must follow every edit, and `paged_recovery::restore`/`restore_text` must rebuild
//! exactly the oracle text of the restored revision, also after the journal is cut
//! at random offsets.
use bareline_document::{
    Budget, Edit, EditTransaction, Revision, TextOffset,
    paged::{TextWindow, WindowPoll},
};
use bareline_file_io::{
    cancellation::Cancellation,
    codecs::disk::DiskOptions,
    lifecycle::{PagedOpenRequest, TranscodeOutcome, open_paged_encoded},
    paged_recovery,
    paged_service::{PagedDocumentGuard, PagedSession},
    recovery::RecoveryEdit,
    source::SourceOptions,
};
use bareline_platform::{FileIdentity, LocalFileSystem};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, UNIX_EPOCH},
};

/// SplitMix64. Report the seed and step of a failure to reproduce it exactly.
struct Rng(u64);
impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    /// ASCII only, so every byte offset is a character boundary and any detected
    /// codec decodes the file to the same text.
    fn text(&mut self, max: usize) -> String {
        const PIECES: &[&str] = &["a", "b", "Z", " ", "\n", "\r\n", "line", "0"];
        (0..self.below(max + 1))
            .map(|_| PIECES[self.below(PIECES.len())])
            .collect()
    }
}

struct Platform;
impl LocalFileSystem for Platform {
    fn validate_target(&self, _: &Path) -> io::Result<()> {
        Ok(())
    }
    fn available_space(&self, _: &Path) -> io::Result<u64> {
        Ok(1 << 40)
    }
    fn guard_directory(&self, _: &Path) -> io::Result<Arc<dyn Send + Sync>> {
        Ok(Arc::new(()))
    }
    fn open_sealed_read(&self, path: &Path) -> io::Result<File> {
        File::open(path)
    }
    fn identity(&self, file: &File) -> io::Result<FileIdentity> {
        let metadata = file.metadata()?;
        Ok(FileIdentity {
            volume: 1,
            file: metadata.len(),
            length: metadata.len(),
            modified: metadata
                .modified()?
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
        })
    }
    fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> {
        fs::rename(staged, target)
    }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bareline-prop-paged-recovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn open(scratch: &Path, source: &Path) -> PagedSession {
    let TranscodeOutcome::Complete(opened) = open_paged_encoded(
        PagedOpenRequest {
            path: source.to_path_buf(),
            bytes: Budget::new(4 * 1024 * 1024),
            history: Budget::new(1024 * 1024),
            cache: scratch.to_path_buf(),
            options: DiskOptions {
                temp_quota_bytes: 4 * 1024 * 1024,
                interpret: None,
            },
            source_options: SourceOptions {
                resident_max_bytes: 0,
                page_size_bytes: 4096,
                page_cache_bytes: 8192,
            },
        },
        Arc::new(Platform),
        Cancellation::default(),
        |_| {},
    ) else {
        panic!("paged fixture did not open")
    };
    PagedSession::new(opened)
}
/// Materialize the whole current text, serving every page the way the I/O worker does.
fn read_all(actor: &mut PagedDocumentGuard<'_>, budget: &Budget) -> TextWindow {
    let snapshot = actor.document().snapshot();
    let mut request = snapshot
        .begin_read(TextOffset(0)..TextOffset(snapshot.len()), snapshot.len(), budget)
        .unwrap();
    loop {
        match request.poll() {
            WindowPoll::Ready(window) => return window,
            WindowPoll::Pending(ticket) => {
                if !snapshot.resolve_owned(ticket).unwrap() {
                    actor.read_source_page(ticket).unwrap();
                }
            }
            _ => panic!("unexpected paged read outcome"),
        }
    }
}
/// Restored revision and complete text, or the reason restore refused the journal.
fn restore(directory: &Path) -> Result<(Revision, String), String> {
    let mut opened = paged_recovery::restore(
        directory,
        Arc::new(Platform),
        Budget::new(64 * 1024 * 1024),
        Budget::new(16 * 1024 * 1024),
        &Cancellation::default(),
    )?;
    assert_eq!(opened.recovery_origin.as_deref(), Some(directory));
    let revision = opened.transcoded.document.snapshot().revision;
    let text = paged_recovery::restore_text(
        &mut opened,
        u64::MAX,
        &Budget::new(1024 * 1024),
        &Cancellation::default(),
    )?
    .expect("restored text fits an unlimited adoption limit");
    Ok((revision, text))
}

#[test]
fn paged_session_edits_restore_to_the_oracle_text_of_the_restored_revision() {
    for seed in [0x9a6e_0001u64, 0x9a6e_0002] {
        let mut rng = Rng(seed);
        let scratch = Scratch::new();
        let source = scratch.0.join("source.txt");
        let mut oracle = rng.text(24);
        if oracle.is_empty() {
            oracle.push('a');
        }
        fs::write(&source, &oracle).unwrap();
        let session = open(&scratch.0, &source);
        session.configure_recovery(scratch.0.join("recovery"), Arc::new(Platform));
        let saved = session
            .state()
            .unwrap()
            .saved_state
            .expect("a document opened from disk starts saved");
        let baseline = session.lock_document().unwrap().document().snapshot();
        // The baseline worker reports completion through `notify`; a channel wakes the wait.
        let (sender, wakeups) = mpsc::channel::<()>();
        let sender = Mutex::new(sender);
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let Ok(sender) = sender.lock() {
                let _ = sender.send(());
            }
        });
        let read_budget = Budget::new(1024 * 1024);
        let mut states = BTreeMap::from([(baseline.revision.0, oracle.clone())]);
        for step in 0..8 {
            let context = format!("seed {seed:#x} step {step}");
            let mut actor = session.lock_document().unwrap();
            let before = actor.document().snapshot();
            let window = read_all(&mut actor, &read_budget);
            assert_eq!(window.text(), oracle, "{context}: paged read");
            let start = rng.below(oracle.len() + 1);
            let end = start + rng.below((oracle.len() - start).min(8) + 1);
            let mut insert = rng.text(3);
            if start == end && insert.is_empty() {
                insert.push('+');
            }
            actor
                .document_mut()
                .apply_materialized(
                    EditTransaction {
                        base_revision: before.revision,
                        edits: vec![Edit {
                            range: TextOffset(start)..TextOffset(end),
                            insert: insert.clone(),
                        }],
                    },
                    &[window],
                )
                .unwrap();
            let after = actor.document().snapshot();
            session
                .protect_recovery_edits(
                    &actor,
                    &baseline,
                    &after,
                    &[RecoveryEdit {
                        offset: start as u64,
                        removed: oracle.as_bytes()[start..end].to_vec(),
                        inserted: insert.as_bytes().to_vec(),
                    }],
                    false,
                    notify.clone(),
                )
                .unwrap();
            drop(actor);
            oracle.replace_range(start..end, &insert);
            states.insert(after.revision.0, oracle.clone());
            // The lifecycle stamp follows the actor; the saved state stays the opened one.
            let state = session.state().unwrap();
            assert_eq!(state.stamp.revision, after.revision, "{context}");
            assert_eq!(state.stamp.content, after.content_state, "{context}");
            assert_ne!(state.stamp.content, saved, "{context}");
            assert_eq!(state.saved_state, Some(saved), "{context}");
            assert_eq!(state.path, source, "{context}");
            assert!(!state.save_as_required && state.recovery_origin.is_none(), "{context}");
        }
        let final_revision = session.lock_document().unwrap().document().snapshot().revision;
        loop {
            let status = session.recovery_status();
            assert!(
                status.error.is_none(),
                "seed {seed:#x}: recovery failed: {:?}",
                status.error
            );
            if status.complete && status.durable.is_some() {
                assert_eq!(status.durable.map(|receipt| receipt.revision), Some(final_revision.0));
                break;
            }
            wakeups
                .recv_timeout(Duration::from_secs(60))
                .expect("recovery baseline copy never finished");
        }
        let directory = session.recovery_status().directory.expect("recovery journal directory");
        drop(session);

        let context = format!("seed {seed:#x}");
        assert_eq!(
            restore(&directory),
            Ok((final_revision, oracle.clone())),
            "{context}: restore"
        );
        let mut restored = paged_recovery::restore(
            &directory,
            Arc::new(Platform),
            Budget::new(64 * 1024 * 1024),
            Budget::new(16 * 1024 * 1024),
            &Cancellation::default(),
        )
        .unwrap();
        assert_eq!(
            paged_recovery::preview(&mut restored, &Budget::new(1024 * 1024), &Cancellation::default()),
            Ok(oracle.clone()),
            "{context}: preview"
        );
        // A recovered session has no saved state and must be saved under a new name.
        let recovered = PagedSession::new(Box::new(restored));
        let state = recovered.state().unwrap();
        assert_eq!(state.saved_state, None, "{context}");
        assert!(state.save_as_required, "{context}");
        assert_eq!(state.recovery_origin.as_deref(), Some(directory.as_path()), "{context}");
        drop(recovered);

        // A cut journal may refuse to restore, but never restores text that differs
        // from what the document held at the revision it reports.
        let journal_path = directory.join("journal.bin");
        let journal = fs::read(&journal_path).unwrap();
        for _ in 0..6 {
            let cut = rng.below(journal.len() + 1);
            fs::write(&journal_path, &journal[..cut]).unwrap();
            let context = format!("{context} cut {cut} of {}", journal.len());
            if let Ok((revision, text)) = restore(&directory) {
                let expected = states
                    .get(&revision.0)
                    .unwrap_or_else(|| panic!("{context}: restored uncommitted revision {}", revision.0));
                assert_eq!(&text, expected, "{context}: truncated restore");
            }
        }
        fs::write(&journal_path, &journal).unwrap();
        assert_eq!(
            restore(&directory),
            Ok((final_revision, oracle)),
            "{context}: restore after rewrite"
        );
    }
}
