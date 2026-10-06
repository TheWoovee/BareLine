// SPDX-License-Identifier: MPL-2.0
//! End-to-end: the real `bareline-file-io` open and save pipeline drives the
//! POSIX filesystem services. Shared by the Linux and macOS adapters.
#![cfg(unix)]
use bareline_document::{Budget, Document, Edit, EditTransaction, TextOffset};
use bareline_file_io::{
    cancellation::Cancellation,
    lifecycle::{FileError, Opened, inspect_save_recovery, open_utf8, save_utf8},
};
use bareline_platform::{FilesystemCapability, SaveStrategy};
use bareline_platform_posix::PosixFileSystem;
use std::path::{Path, PathBuf};

struct Scratch(PathBuf);
impl Scratch {
    fn new(name: &str) -> Self {
        let directory = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("bareline-save-pipeline-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn open(path: &Path) -> Opened {
    open_utf8(path, &PosixFileSystem, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap()
}

fn text(opened: &Opened) -> String {
    let snapshot = opened.document.snapshot();
    snapshot
        .read(TextOffset(0)..TextOffset(snapshot.len()), 1 << 20)
        .unwrap()
}

fn insert_at_start(opened: &mut Opened, text: &str) {
    let snapshot = opened.document.snapshot();
    opened
        .document
        .apply(EditTransaction {
            base_revision: snapshot.revision,
            edits: vec![Edit {
                range: TextOffset(0)..TextOffset(0),
                insert: text.into(),
            }],
        })
        .unwrap();
}

#[test]
fn open_edit_save_reopen() {
    let scratch = Scratch::new("round-trip");
    let path = scratch.0.join("notes.txt");
    std::fs::write(&path, "first line\n").unwrap();
    std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600)).unwrap();

    let mut opened = open(&path);
    assert_eq!(text(&opened), "first line\n");
    insert_at_start(&mut opened, "edited ");
    let saved = save_utf8(
        opened.document.snapshot(),
        &path,
        Some(&opened.fingerprint),
        opened.bom,
        &PosixFileSystem,
    )
    .unwrap_or_else(|error| panic!("save failed: {error}"));
    assert!(saved.cleanup.is_none(), "a verified save cleans its transaction up");
    opened.document.mark_saved(&saved.captured).unwrap();
    assert!(!opened.document.dirty());

    // The stage and the transaction folder are gone; only the document remains,
    // still private to its owner.
    assert_eq!(scratch.names(), ["notes.txt"]);
    use std::os::unix::fs::MetadataExt;
    assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o600);

    let reopened = open(&path);
    assert_eq!(text(&reopened), "edited first line\n");
    assert_eq!(reopened.fingerprint, saved.fingerprint);

    // A second save against the new fingerprint succeeds too.
    let mut again = reopened;
    insert_at_start(&mut again, "twice ");
    save_utf8(
        again.document.snapshot(),
        &path,
        Some(&again.fingerprint),
        again.bom,
        &PosixFileSystem,
    )
    .unwrap_or_else(|error| panic!("second save failed: {error}"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "twice edited first line\n");
}

#[test]
fn external_change_is_a_conflict_that_keeps_both_versions() {
    let scratch = Scratch::new("conflict");
    let path = scratch.0.join("notes.txt");
    std::fs::write(&path, "original").unwrap();
    let opened = open(&path);
    std::fs::write(&path, "external").unwrap();
    let editor = Document::from_utf8("editor", Budget::new(1024), Budget::new(1024)).unwrap();
    let Err(FileError::Conflict { proposed, .. }) = save_utf8(
        editor.snapshot(),
        &path,
        Some(&opened.fingerprint),
        false,
        &PosixFileSystem,
    ) else {
        panic!("an external change must be reported as a conflict");
    };
    assert_eq!(std::fs::read(&path).unwrap(), b"external");
    assert_eq!(std::fs::read(&proposed).unwrap(), b"editor");
    // Restart discovery finds the retained editor version.
    let recovery = inspect_save_recovery(&scratch.0, &PosixFileSystem, &Cancellation::default()).unwrap();
    assert!(
        recovery
            .conflicts
            .iter()
            .any(|conflict| std::fs::read(&conflict.editor_version).ok().as_deref() == Some(b"editor".as_slice()))
    );
}

#[test]
fn save_as_creates_and_hard_link_saves_in_place() {
    let scratch = Scratch::new("create-and-link");
    let editor = Document::from_utf8("new file", Budget::new(1024), Budget::new(1024)).unwrap();
    let created = scratch.0.join("created.txt");
    save_utf8(editor.snapshot(), &created, None, false, &PosixFileSystem)
        .unwrap_or_else(|error| panic!("create failed: {error}"));
    assert_eq!(std::fs::read(&created).unwrap(), b"new file");
    assert!(matches!(
        save_utf8(editor.snapshot(), &created, None, false, &PosixFileSystem),
        Err(FileError::Conflict { .. })
    ));

    let alias = scratch.0.join("alias.txt");
    std::fs::hard_link(&created, &alias).unwrap();
    assert_eq!(PosixFileSystem.report(&created).unwrap().save, SaveStrategy::InPlace);
    let opened = open(&created);
    let replacement = Document::from_utf8("through both links", Budget::new(1024), Budget::new(1024)).unwrap();
    save_utf8(
        replacement.snapshot(),
        &created,
        Some(&opened.fingerprint),
        false,
        &PosixFileSystem,
    )
    .unwrap_or_else(|error| panic!("in-place save failed: {error}"));
    assert_eq!(std::fs::read(&alias).unwrap(), b"through both links");
}
