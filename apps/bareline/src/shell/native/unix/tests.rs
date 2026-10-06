// SPDX-License-Identifier: MPL-2.0
//! Seam-wide checks; each concern's own tests live beside it.
use super::*;
use std::{
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

#[test]
fn missing_native_services_answer_in_plain_language() {
    let refusal = "This system does not support";
    assert!(resolve_program("sh").unwrap_err().starts_with(refusal));
    assert!(
        shell_integration::reveal(Path::new("/tmp"))
            .unwrap_err()
            .starts_with(refusal)
    );
    assert!(shell_integration::TrayIcon::new(0).is_err());
    assert_eq!(
        update::validate_install_root(Path::new("/opt/bareline"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::Unsupported
    );
    assert!(WatchService::start_notifying(vec![PathBuf::from("/tmp")], Arc::new(|| {})).is_err());
    assert!(spell_checker_factory()().is_err());
}

#[test]
fn every_window_runs_as_its_own_instance() {
    let outcome = instance::coordinate(
        Path::new("/tmp"),
        None,
        instance::OpenRequest::default(),
        false,
        Arc::new(|| {}),
    )
    .unwrap();
    assert!(matches!(outcome, instance::Outcome::Independent(_)));
}
