// SPDX-License-Identifier: MPL-2.0
use super::*;

const ROOT_KEY: &str = include_str!("../../../distribution/tests/fixtures/authority/root.txt");
fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../distribution/tests/fixtures/authority")
            .join(name),
    )
    .unwrap()
}
fn publish_fixture(root: &std::path::Path, name: &str) {
    std::fs::write(
        root.join("bareline.release-authority.json"),
        fixture(&format!("{name}.json")),
    )
    .unwrap();
    std::fs::write(
        root.join("bareline.release-authority.minisig"),
        fixture(&format!("{name}.minisig")),
    )
    .unwrap();
}

#[test]
fn installed_authority_rotation_persists_lineage_and_rejects_rollback() {
    let parent = std::env::temp_dir();
    let root = create_private_stage(&parent).unwrap();
    let embedded = PublisherPin::parse("Unused Embedded Publisher", "Unused CA").unwrap();
    let resolve = || {
        resolve_release_authority(
            &root,
            "unused-embedded-key",
            &embedded,
            3,
            Some(OfflineRootPolicy {
                public_key: ROOT_KEY,
                minimum_version: 1,
            }),
            100,
        )
    };
    publish_fixture(&root, "initial");
    let initial = resolve().unwrap();
    // The signed authority, not the compiled default, supplies the pin and helper hash.
    assert!(
        initial
            .signer
            .accepts("Initial Fixture Publisher", "Fixture Code Signing CA")
    );
    assert_eq!(initial.update_helper_sha256.as_deref(), Some("07".repeat(32).as_str()));
    assert_eq!(initial.minimum_metadata_version, 3);
    assert_eq!(
        initial.catalog_public_key.unwrap(),
        String::from_utf8(fixture("catalog.txt")).unwrap()
    );
    publish_fixture(&root, "revoked");
    assert!(resolve().is_err());
    publish_fixture(&root, "rotated");
    assert!(resolve().is_err()); // New root alone cannot authorize itself.
    std::fs::write(
        root.join("bareline.root-transitions.json"),
        fixture("bad-transitions.json"),
    )
    .unwrap();
    assert!(resolve().is_err());
    std::fs::write(root.join("bareline.root-transitions.json"), fixture("transitions.json")).unwrap();
    let rotated = resolve().unwrap();
    assert!(
        rotated
            .signer
            .accepts("Rotated Fixture Publisher", "Next Fixture Code Signing CA")
    );
    assert_eq!(rotated.update_helper_sha256.as_deref(), Some("09".repeat(32).as_str()));
    // The installed helper must match the signed hash before any Authenticode check.
    std::fs::write(root.join("bareline-update-helper.exe"), b"not the signed helper").unwrap();
    for acknowledge in [true, false] {
        assert_eq!(
            launch_update_helper(&root, &rotated, acknowledge)
                .unwrap_err()
                .to_string(),
            "update helper differs from the signed release authority"
        );
    }
    // Without a signed helper hash nothing is launched, whatever its Authenticode (SEC-08).
    let unpinned = ResolvedReleaseAuthority {
        release_public_key: rotated.release_public_key.clone(),
        signer: rotated.signer.clone(),
        update_helper_sha256: None,
        minimum_metadata_version: rotated.minimum_metadata_version,
        catalog_public_key: None,
    };
    assert_eq!(
        launch_update_helper(&root, &unpinned, true).unwrap_err().to_string(),
        "the signed release authority does not pin the update helper"
    );
    // Matching the signed hash is not enough either: Authenticode is still required.
    let helper = b"unsigned helper with the signed hash";
    std::fs::write(root.join("bareline-update-helper.exe"), helper).unwrap();
    let hashed = ResolvedReleaseAuthority {
        update_helper_sha256: Some(format!("{:x}", Sha256::digest(helper))),
        ..unpinned
    };
    assert!(
        launch_update_helper(&root, &hashed, true)
            .unwrap_err()
            .to_string()
            .starts_with("helper publisher:")
    );
    assert_eq!(rotated.minimum_metadata_version, 5);
    assert_eq!(
        rotated.catalog_public_key.unwrap(),
        String::from_utf8(fixture("next-catalog.txt")).unwrap()
    );
    let ledger = std::fs::read(root.join("bareline.root-versions")).unwrap();
    assert_eq!(
        std::str::from_utf8(&ledger).unwrap().lines().collect::<Vec<_>>(),
        vec!["1", "2"]
    );
    resolve().unwrap();
    assert_eq!(std::fs::read(root.join("bareline.root-versions")).unwrap(), ledger);
    std::fs::remove_file(root.join("bareline.root-transitions.json")).unwrap();
    publish_fixture(&root, "initial");
    assert!(resolve().is_err());
    assert_eq!(std::fs::read(root.join("bareline.root-versions")).unwrap(), ledger);
    assert!(root.canonicalize().unwrap().starts_with(parent.canonicalize().unwrap()));
    for entry in std::fs::read_dir(&root).unwrap() {
        let path = entry.unwrap().path();
        assert!(path.is_file());
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}

#[test]
fn helper_apply_fetches_revocation_online_and_acknowledgement_does_not() {
    // SEC-07: only the explicit apply flow needs network revocation evidence.
    assert_eq!(helper_revocation(false), Revocation::Online);
    assert_eq!(helper_revocation(true), Revocation::Offline);
}

#[test]
fn same_file_compares_file_ids_not_path_text() {
    // SEC-18: a second name for the same file matches; an identical copy does not.
    let root = create_private_stage(&std::env::temp_dir()).unwrap();
    let original = root.join("original.exe");
    let alias = root.join("alias.exe");
    let copy = root.join("copy.exe");
    std::fs::write(&original, b"image").unwrap();
    std::fs::hard_link(&original, &alias).unwrap();
    std::fs::write(&copy, b"image").unwrap();
    let open = |path: &std::path::Path| File::open(path).unwrap();
    assert!(same_file(&open(&original), &open(&original)).unwrap());
    assert!(same_file(&open(&original), &open(&alias)).unwrap());
    assert!(!same_file(&open(&original), &open(&copy)).unwrap());
    for path in [original, alias, copy] {
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir(root).unwrap();
}
