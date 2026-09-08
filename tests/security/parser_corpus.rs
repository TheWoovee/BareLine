// SPDX-License-Identifier: MPL-2.0
//! Fixed-work parser corpus. No native processes, network access or fuzz campaign.
use bareline_extensions_protocol as rpc;
use bareline_file_io::{cancellation::Cancellation, recovery, session};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// At most 97 variants, each <=4097 bytes; deterministic prefixes, bit flips and
/// delimiter insertions exercise both incomplete and structurally altered inputs.
fn mutations(seed: &[u8]) -> Vec<Vec<u8>> {
    assert!(seed.len() <= 4096);
    let mut values = vec![Vec::new()];
    for n in 0..32 {
        let at = n * seed.len() / 32;
        values.push(seed[..at].to_vec());
        if at < seed.len() {
            let mut flipped = seed.to_vec();
            flipped[at] ^= 0x80;
            values.push(flipped);
            let mut inserted = seed.to_vec();
            inserted.insert(at, [0, b'"', b'<', b']'][n % 4]);
            values.push(inserted);
        }
    }
    values
}

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "bareline-parser-corpus-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        if self
            .0
            .starts_with(std::env::temp_dir().canonicalize().unwrap())
        {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

struct FixtureFs;
impl bareline_platform::LocalFileSystem for FixtureFs {
    fn identity(&self, _: &fs::File) -> io::Result<bareline_platform::FileIdentity> {
        Err(io::Error::other("identity is unused by the parser fixture"))
    }
    fn validate_target(&self, _: &Path) -> io::Result<()> {
        Ok(())
    }
    fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> {
        fs::rename(staged, target)
    }
}

#[test]
fn session_mutations_preserve_validation_and_pinned_roundtrip() {
    let original = session::SessionManifest {
        documents: vec![session::SessionDocument {
            id: 1,
            path: None,
            title: "unsaved sentinel".into(),
        }],
        tabs: vec![session::SessionTab {
            id: 7,
            document_id: 1,
            pinned: true,
            view: Default::default(),
        }],
        active_tab: Some(7),
        ..Default::default()
    };
    let seed = session::encode(&original).unwrap();
    let loaded = session::decode(&seed).unwrap();
    assert!(loaded.tabs[0].pinned);
    assert_eq!(loaded.tabs[0].document_id, 1);
    for bytes in mutations(&seed) {
        if let Ok(value) = session::decode(&bytes) {
            value.validate().unwrap();
            let stable = session::encode(&value).unwrap();
            assert_eq!(session::decode(&stable).unwrap(), value);
        }
    }
    let mut duplicate = original.clone();
    duplicate.tabs.push(duplicate.tabs[0].clone());
    assert!(session::decode(&serde_json::to_vec(&duplicate).unwrap()).is_err());
    let mut oversized = original;
    oversized.documents[0].title = "x".repeat(4097);
    assert!(session::decode(&serde_json::to_vec(&oversized).unwrap()).is_err());
    oversized.documents[0].title = "safe".into();
    oversized.documents[0].path = Some(bareline_platform::SerializedPath {
        version: 1,
        encoding: bareline_platform::PathEncoding::WindowsUtf16Le,
        data: "not-base64!".into(),
        display: "untrusted display".into(),
    });
    assert!(session::decode(&serde_json::to_vec(&oversized).unwrap()).is_err());
    assert!(session::decode(&vec![b' '; session::MAX_SESSION_BYTES + 1]).is_err());
}

#[test]
fn udl_mutations_reject_entities_and_preserve_registry_on_error() {
    let seed = r#"<NotepadPlus><UserLang name="Fixture" ext="fixture"><KeywordLists><Keywords name="Keywords1">hello world</Keywords></KeywordLists></UserLang></NotepadPlus>"#;
    let (definition, _) = bareline_syntax::udl::import_notepad_xml(seed).unwrap();
    let json = definition.to_json().unwrap();
    let mut registry = bareline_syntax::udl::Registry::default();
    registry.replace_json(&json).unwrap();
    for bytes in mutations(seed.as_bytes()) {
        if let Ok(text) = std::str::from_utf8(&bytes) {
            if let Ok((value, _)) = bareline_syntax::udl::import_notepad_xml(text) {
                value.validate().unwrap();
            }
        }
    }
    for bytes in mutations(json.as_bytes()) {
        if let Ok(text) = std::str::from_utf8(&bytes) {
            let before = registry.get(&definition.id).unwrap().to_json().unwrap();
            if registry.replace_json(text).is_err() {
                assert_eq!(
                    registry.get(&definition.id).unwrap().to_json().unwrap(),
                    before
                );
            }
        }
    }
    for hostile in [
        "<!DOCTYPE x SYSTEM 'file:///never-open'><NotepadPlus/>",
        "<!ENTITY x SYSTEM 'https://never-contact.invalid'><NotepadPlus/>",
        "<NotepadPlus><UserLang></NotepadPlus>",
    ] {
        assert!(bareline_syntax::udl::import_notepad_xml(hostile).is_err());
    }
}

#[test]
fn rpc_mutations_revalidate_and_oversized_prefix_never_reads_payload() {
    let message = rpc::Envelope {
        protocol: 1,
        request_id: 9,
        extension_id: "fixture.tools".into(),
        context: rpc::CapabilityContext {
            capability: rpc::Capability::DocumentRead,
            scope: rpc::Scope::Document(1),
            grant_generation: 1,
        },
        request: rpc::Request::Cancel { request: 8 },
    };
    let mut seed = Vec::new();
    rpc::write_frame(&mut seed, &message).unwrap();
    assert_eq!(rpc::read_frame(&mut &seed[..]).unwrap(), message);
    for bytes in mutations(&seed) {
        if let Ok(value) = rpc::read_frame(&mut &bytes[..]) {
            let mut encoded = Vec::new();
            rpc::write_frame(&mut encoded, &value).unwrap();
            assert_eq!(rpc::read_frame(&mut &encoded[..]).unwrap(), value);
        }
    }
    struct PrefixOnly {
        prefix: io::Cursor<[u8; 4]>,
    }
    impl io::Read for PrefixOnly {
        fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
            assert!(
                self.prefix.position() < 4,
                "oversized frame attempted payload read"
            );
            io::Read::read(&mut self.prefix, into)
        }
    }
    for length in [0, rpc::MAX_FRAME_BYTES as u32 + 1, u32::MAX] {
        let mut reader = PrefixOnly {
            prefix: io::Cursor::new(length.to_le_bytes()),
        };
        assert_eq!(
            rpc::read_frame(&mut reader),
            Err(rpc::ProtocolError::Oversized)
        );
    }
}

#[test]
fn journal_mutations_preserve_valid_prefix_without_promoting_bad_tail() {
    let scratch = Scratch::new();
    let directory = scratch.0.join("journal");
    let mut writer = recovery::RecoveryWriter::create(
        &directory,
        recovery::RecoveryMetadata {
            original_path: None,
            source_generation: "fixture".into(),
            codec_catalog_version: "utf8-v1".into(),
            original_len: 0,
        },
        &FixtureFs,
    )
    .unwrap();
    writer
        .append(
            1,
            &[recovery::RecoveryEdit {
                offset: 0,
                removed: vec![],
                inserted: b"first".to_vec(),
            }],
        )
        .unwrap();
    let prefix = fs::read(directory.join("journal.bin")).unwrap();
    writer
        .append(
            2,
            &[recovery::RecoveryEdit {
                offset: 5,
                removed: vec![],
                inserted: b"second".to_vec(),
            }],
        )
        .unwrap();
    drop(writer);
    let full = fs::read(directory.join("journal.bin")).unwrap();
    assert_eq!(
        recovery::inspect(&directory, &Cancellation::default())
            .unwrap()
            .validated_records,
        2
    );
    for tail in mutations(&full[prefix.len()..]) {
        let mut mutated = prefix.clone();
        mutated.extend_from_slice(&tail);
        fs::write(directory.join("journal.bin"), mutated).unwrap();
        let inspection = recovery::inspect(&directory, &Cancellation::default()).unwrap();
        assert_eq!(inspection.validated_records, 1);
        assert_eq!(inspection.last_durable.unwrap().revision, 1);
        if !tail.is_empty() {
            assert_eq!(inspection.status, recovery::RecoveryStatus::CorruptTail);
        }
    }
}
