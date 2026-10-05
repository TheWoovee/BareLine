// SPDX-License-Identifier: MPL-2.0
//! Real Windows store/restart checks; no native window or user profile needed.
use bareline_app::language::{LanguageController, catalog::Store};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "bareline-language-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn store(&self) -> Store {
        Store::new(
            self.0.join("languages"),
            Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn wait(controller: &mut LanguageController) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !controller.catalog_ready() {
        assert!(Instant::now() < deadline, "{}", controller.status);
        controller.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn xml(name: &str) -> Vec<u8> {
    format!(r#"<NotepadPlus><UserLang name="{name}" ext="qaudl"><KeywordLists><Keywords name="Keywords1">sentinel</Keywords></KeywordLists></UserLang></NotepadPlus>"#).into_bytes()
}

#[test]
fn imported_language_is_durable_before_publication_and_reloaded_on_restart() {
    let scratch = Scratch::new();
    let mut controller = LanguageController::default();
    controller.configure_catalog(scratch.store(), Arc::new(|| {}));
    wait(&mut controller);
    controller.import_udl_bytes(xml("QA Test"), Arc::new(|| {}));
    wait(&mut controller);
    assert!(
        controller.status.starts_with("Imported QA Test"),
        "{}",
        controller.status
    );
    assert_eq!(controller.definition_by_id("qa-test").unwrap().keywords, ["sentinel"]);
    let bytes = std::fs::read(scratch.0.join("languages/qa-test.json")).unwrap();
    drop(controller);
    let mut restarted = LanguageController::default();
    restarted.configure_catalog(scratch.store(), Arc::new(|| {}));
    assert!(!restarted.catalog_ready());
    wait(&mut restarted);
    assert_eq!(
        restarted
            .definition_for_path(std::path::Path::new("sample.QAUDL"))
            .unwrap()
            .id,
        "qa-test"
    );
    restarted.import_udl_bytes(b"{invalid".to_vec(), Arc::new(|| {}));
    wait(&mut restarted);
    assert_eq!(std::fs::read(scratch.0.join("languages/qa-test.json")).unwrap(), bytes);
    assert!(restarted.definition_by_id("qa-test").is_some());
    restarted.import_udl_bytes(xml("Ambiguous"), Arc::new(|| {}));
    wait(&mut restarted);
    assert!(
        restarted
            .definition_for_path(std::path::Path::new("sample.qaudl"))
            .is_none()
    );
    assert!(restarted.definition_by_id("ambiguous").is_some());
}

#[test]
fn cancelled_or_corrupt_store_preserves_installed_bytes_and_does_not_publish_success() {
    let scratch = Scratch::new();
    let (mut definition, _) =
        bareline_syntax::udl::import_notepad_xml(std::str::from_utf8(&xml("QA Test")).unwrap()).unwrap();
    scratch
        .store()
        .save(&definition, &bareline_syntax::Cancellation::default())
        .unwrap();
    let path = scratch.0.join("languages/qa-test.json");
    let before = std::fs::read(&path).unwrap();
    definition.keywords.push("changed".into());
    let cancel = bareline_syntax::Cancellation::default();
    cancel.cancel();
    assert!(scratch.store().save(&definition, &cancel).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    std::fs::write(path, b"{corrupt").unwrap();
    let mut controller = LanguageController::default();
    controller.configure_catalog(scratch.store(), Arc::new(|| {}));
    wait(&mut controller);
    assert!(controller.open);
    assert!(controller.definition_by_id("qa-test").is_none());
    controller.import_udl_bytes(xml("New"), Arc::new(|| {}));
    wait(&mut controller);
    assert!(controller.definition_by_id("new").is_none());
    assert!(!scratch.0.join("languages/new.json").exists());
}
