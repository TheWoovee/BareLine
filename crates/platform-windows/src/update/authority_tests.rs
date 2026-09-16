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
    let resolve = || {
        resolve_release_authority(
            &root,
            "unused-embedded-key",
            &"07".repeat(32),
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
    assert_eq!(initial.certificate, [7; 32]);
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
    assert_eq!(rotated.certificate, [9; 32]);
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
