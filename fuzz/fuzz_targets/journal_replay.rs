// SPDX-License-Identifier: MPL-2.0
//! Recovery journal inspection, replay and reconstruction over a real sealed journal
//! whose tail, framed records or manifest are replaced by fuzz input. The first byte
//! selects the mode. A validated prefix must reconstruct exactly the matching state.
#![no_main]
use bareline_file_io::{
    cancellation::Cancellation,
    recovery::{self, RecoveryEdit, RecoveryMetadata, RecoveryStatus, RecoveryWriter},
};
use bareline_platform::{FileIdentity, LocalFileSystem};
use libfuzzer_sys::fuzz_target;
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

struct Fs;
impl LocalFileSystem for Fs {
    fn identity(&self, _: &File) -> io::Result<FileIdentity> {
        Err(io::Error::other("identity is unused by the journal fuzzer"))
    }
    fn validate_target(&self, _: &Path) -> io::Result<()> {
        Ok(())
    }
    fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> {
        fs::rename(staged, target)
    }
}

/// One sealed journal per process; each run restores its files before mutating them.
struct Template {
    root: PathBuf,
    directory: PathBuf,
    journal: Vec<u8>,
    first_record_end: usize,
    manifest: Vec<u8>,
    manifest_previous: Option<Vec<u8>>,
    /// Text after zero, one and two records.
    states: [Vec<u8>; 3],
}
fn template() -> &'static Template {
    static TEMPLATE: OnceLock<Template> = OnceLock::new();
    TEMPLATE.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("bareline-fuzz-journal-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let directory = root.join("journal");
        let baseline = b"hello recovery\n".to_vec();
        let mut writer = RecoveryWriter::create(
            &directory,
            RecoveryMetadata {
                original_path: None,
                source_generation: "fuzz".into(),
                codec_catalog_version: "utf8-v1".into(),
                original_len: baseline.len() as u64,
            },
            &Fs,
        )
        .unwrap();
        writer
            .seal_baseline(&mut baseline.as_slice(), || Ok(true), &Cancellation::default(), &Fs)
            .unwrap();
        let first = [RecoveryEdit {
            offset: 0,
            removed: b"hello".to_vec(),
            inserted: b"HELLO, WORLD".to_vec(),
        }];
        writer.append(1, &first).unwrap();
        let first_record_end = fs::metadata(directory.join("journal.bin")).unwrap().len() as usize;
        let second = [
            RecoveryEdit {
                offset: 5,
                removed: b",".to_vec(),
                inserted: Vec::new(),
            },
            RecoveryEdit {
                offset: 21,
                removed: b"\n".to_vec(),
                inserted: b"!\r\n".to_vec(),
            },
        ];
        writer.append(2, &second).unwrap();
        drop(writer);
        let read_optional = |name: &str| fs::read(directory.join(name)).ok();
        Template {
            journal: fs::read(directory.join("journal.bin")).unwrap(),
            manifest: fs::read(directory.join("manifest.json")).unwrap(),
            manifest_previous: read_optional("manifest.previous.json"),
            first_record_end,
            states: [
                baseline,
                b"HELLO, WORLD recovery\n".to_vec(),
                b"HELLO WORLD recovery!\r\n".to_vec(),
            ],
            root,
            directory,
        }
    })
}
fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82f6_3b78 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fuzz_target!(|data: &[u8]| {
    let Some((&mode, input)) = data.split_first() else {
        return;
    };
    let template = template();
    let directory = &template.directory;
    let cancel = Cancellation::default();
    let mut journal = template.journal.clone();
    let mut manifest = template.manifest.clone();
    match mode % 3 {
        // Valid first record followed by an arbitrary tail.
        0 => {
            journal.truncate(template.first_record_end);
            journal.extend_from_slice(input);
        }
        // Records framed with valid lengths and CRCs reach JSON and edit validation.
        1 => {
            journal.clear();
            for record in input.split(|byte| *byte == 0xff) {
                journal.extend_from_slice(&(record.len() as u32).to_le_bytes());
                journal.extend_from_slice(&crc32c(record).to_le_bytes());
                journal.extend_from_slice(record);
            }
        }
        _ => manifest = input.to_vec(),
    }
    fs::write(directory.join("journal.bin"), &journal).unwrap();
    fs::write(directory.join("manifest.json"), &manifest).unwrap();
    match &template.manifest_previous {
        Some(previous) => fs::write(directory.join("manifest.previous.json"), previous).unwrap(),
        None => {
            let _ = fs::remove_file(directory.join("manifest.previous.json"));
        }
    }
    let Ok(inspection) = recovery::inspect(directory, &cancel) else {
        assert!(mode % 3 != 0, "valid manifest and first record must inspect");
        return;
    };
    // Only segment-1.bin and segment-2.bin exist, and revisions must increase.
    assert!(inspection.validated_records <= 2);
    if mode % 3 == 0 {
        assert!(inspection.validated_records >= 1, "valid first record was lost");
        assert!(matches!(
            inspection.status,
            RecoveryStatus::Complete | RecoveryStatus::CorruptTail
        ));
    }
    let mut replayed: Vec<Vec<RecoveryEdit>> = Vec::new();
    let replay = recovery::replay_transactions(directory, &cancel, |_, edits| {
        replayed.push(edits);
        Ok(())
    });
    if let Ok(replay) = &replay {
        assert_eq!(replay.validated_records, inspection.validated_records);
        assert_eq!(replayed.len(), replay.validated_records);
    }
    let destination = template.root.join("recovered.bin");
    let _ = fs::remove_file(&destination);
    match recovery::recover_to(directory, &destination, &cancel) {
        Ok(recovered) => {
            assert_eq!(recovered.validated_records, inspection.validated_records);
            let bytes = fs::read(&destination).unwrap();
            if replay.is_ok() {
                // Reconstruction must equal the baseline with exactly the replayed edits.
                let mut expected = template.states[0].clone();
                for edits in &replayed {
                    expected = apply(&expected, edits);
                }
                assert_eq!(bytes, expected, "reconstruction differs from the replayed edits");
            }
            if mode % 3 == 0 && recovered.validated_records == 1 {
                assert_eq!(bytes, template.states[1], "first record reconstructed wrongly");
            }
        }
        Err(_) => assert!(!destination.exists(), "failed reconstruction left a destination"),
    }
    let _ = fs::remove_file(&destination);
});
/// Apply one validated transaction (sorted, nonoverlapping pre-transaction offsets).
fn apply(text: &[u8], edits: &[RecoveryEdit]) -> Vec<u8> {
    let mut result = Vec::with_capacity(text.len());
    let mut cursor = 0;
    for edit in edits {
        let start = usize::try_from(edit.offset).expect("validated offset");
        let end = start + edit.removed.len();
        assert!(cursor <= start && end <= text.len(), "replayed edit outside its source");
        assert_eq!(&text[start..end], edit.removed.as_slice(), "replayed inverse mismatch");
        result.extend_from_slice(&text[cursor..start]);
        result.extend_from_slice(&edit.inserted);
        cursor = end;
    }
    result.extend_from_slice(&text[cursor..]);
    result
}
