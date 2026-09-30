// SPDX-License-Identifier: MPL-2.0
//! Seeded property tests (QA-05, QA-16): recovery journal replay and reconstruction
//! against a byte oracle, including truncation at every kind of journal offset.
use bareline_file_io::{
    cancellation::Cancellation,
    recovery::{self, DurableReceipt, RecoveryEdit, RecoveryMetadata, RecoveryStatus, RecoveryWriter},
};
use bareline_platform::{FileIdentity, LocalFileSystem};
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// SplitMix64. Report the seed of a failure to reproduce it exactly.
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
    fn bytes(&mut self, max: usize) -> Vec<u8> {
        (0..self.below(max + 1)).map(|_| self.next_u64() as u8).collect()
    }
}

struct Fs;
impl LocalFileSystem for Fs {
    fn identity(&self, _: &File) -> io::Result<FileIdentity> {
        Err(io::Error::other("identity is unused by the recovery property fixture"))
    }
    fn validate_target(&self, _: &Path) -> io::Result<()> {
        Ok(())
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
            "bareline-prop-recovery-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Sorted, nonoverlapping edits in the pre-transaction byte domain; never empty.
fn random_edits(rng: &mut Rng, text: &[u8]) -> Vec<RecoveryEdit> {
    let mut edits = Vec::new();
    let mut cursor = 0;
    for _ in 0..1 + rng.below(3) {
        if cursor > text.len() {
            break;
        }
        let offset = cursor + rng.below(text.len() - cursor + 1);
        let end = offset + rng.below((text.len() - offset).min(12) + 1);
        let mut inserted = rng.bytes(12);
        if inserted.is_empty() && end == offset {
            inserted.push(b'+');
        }
        edits.push(RecoveryEdit {
            offset: offset as u64,
            removed: text[offset..end].to_vec(),
            inserted,
        });
        // Leave a gap so the next edit never shares this edit's offset.
        cursor = end + 1;
    }
    edits
}
fn apply(text: &[u8], edits: &[RecoveryEdit]) -> Vec<u8> {
    let mut result = Vec::with_capacity(text.len());
    let mut cursor = 0;
    for edit in edits {
        let start = edit.offset as usize;
        result.extend_from_slice(&text[cursor..start]);
        result.extend_from_slice(&edit.inserted);
        cursor = start + edit.removed.len();
    }
    result.extend_from_slice(&text[cursor..]);
    result
}
fn reconstruct(directory: &Path, scratch: &Path, name: &str) -> (Vec<u8>, recovery::RecoveryInspection) {
    let destination = scratch.join(name);
    let inspection = recovery::recover_to(directory, &destination, &Cancellation::default()).unwrap();
    (fs::read(destination).unwrap(), inspection)
}

#[test]
fn journal_replay_and_reconstruction_match_byte_oracle_under_truncation() {
    for seed in [0x0ecc_0001u64, 0x0ecc_0002] {
        let mut rng = Rng(seed);
        let scratch = Scratch::new();
        let directory = scratch.0.join("journal");
        let baseline = rng.bytes(48);
        let mut writer = RecoveryWriter::create(
            &directory,
            RecoveryMetadata {
                original_path: None,
                source_generation: format!("prop-{seed:x}"),
                codec_catalog_version: "utf8-v1".into(),
                original_len: baseline.len() as u64,
            },
            &Fs,
        )
        .unwrap();
        writer
            .seal_baseline(&mut baseline.as_slice(), || Ok(true), &Cancellation::default(), &Fs)
            .unwrap();
        // states[k] is the text after k records; ends[k] is the journal length after k records.
        let mut states = vec![baseline];
        let mut ends = vec![0u64];
        let mut appended: Vec<(DurableReceipt, Vec<RecoveryEdit>)> = Vec::new();
        let mut revision = 0u64;
        for _ in 0..8 {
            let current = states.last().unwrap();
            let edits = random_edits(&mut rng, current);
            let next = apply(current, &edits);
            revision += 1 + rng.below(3) as u64;
            let receipt = writer.append(revision, &edits).unwrap();
            assert_eq!(receipt.revision, revision);
            states.push(next);
            ends.push(fs::metadata(directory.join("journal.bin")).unwrap().len());
            appended.push((receipt, edits));
        }
        drop(writer);
        let context = format!("seed {seed:#x}");
        let inspection = recovery::inspect(&directory, &Cancellation::default()).unwrap();
        assert_eq!(inspection.status, RecoveryStatus::Complete, "{context}");
        assert_eq!(inspection.validated_records, appended.len(), "{context}");
        assert_eq!(inspection.last_durable, appended.last().map(|(receipt, _)| *receipt));
        let mut replayed = Vec::new();
        recovery::replay_transactions(&directory, &Cancellation::default(), |receipt, edits| {
            replayed.push((receipt, edits));
            Ok(())
        })
        .unwrap();
        assert_eq!(replayed.len(), appended.len(), "{context}: replay count");
        for ((receipt, edits), (expected_receipt, expected)) in replayed.iter().zip(&appended) {
            assert_eq!(receipt, expected_receipt, "{context}");
            assert_eq!(edits.len(), expected.len(), "{context}");
            for (edit, expected) in edits.iter().zip(expected) {
                assert_eq!(
                    (edit.offset, &edit.removed, &edit.inserted),
                    (expected.offset, &expected.removed, &expected.inserted),
                    "{context}: replayed edit"
                );
            }
        }
        let (text, inspection) = reconstruct(&directory, &scratch.0, "full.bin");
        assert_eq!(&text, states.last().unwrap(), "{context}: reconstruction");
        assert_eq!(inspection.status, RecoveryStatus::Complete, "{context}");
        // Truncation keeps exactly the fully written prefix; a partial record is a corrupt tail.
        let journal = fs::read(directory.join("journal.bin")).unwrap();
        for cut_index in 0..4 {
            let cut = match cut_index {
                0 => ends[rng.below(ends.len())] as usize,
                _ => rng.below(journal.len() + 1),
            };
            fs::write(directory.join("journal.bin"), &journal[..cut]).unwrap();
            let kept = ends.iter().filter(|end| **end as usize <= cut).count() - 1;
            let at_boundary = ends.contains(&(cut as u64));
            let (text, inspection) = reconstruct(&directory, &scratch.0, &format!("cut-{cut_index}.bin"));
            let context = format!("{context} cut {cut}");
            assert_eq!(inspection.validated_records, kept, "{context}");
            assert_eq!(
                inspection.status,
                if at_boundary {
                    RecoveryStatus::Complete
                } else {
                    RecoveryStatus::CorruptTail
                },
                "{context}"
            );
            assert_eq!(text, states[kept], "{context}: truncated reconstruction");
        }
        fs::write(directory.join("journal.bin"), journal).unwrap();
    }
}
