// SPDX-License-Identifier: MPL-2.0
use bareline_distribution::{
    trust::{verify_authority, verify_root_chain},
    update::VerifyError,
};

const ROOT: &str = include_str!("fixtures/authority/root.txt");
#[test]
fn signed_root_policy_rejects_revocation_expiry_and_key_id_aliases() {
    let authority = verify_authority(
        include_bytes!("fixtures/authority/initial.json"),
        include_str!("fixtures/authority/initial.minisig"),
        ROOT,
        1,
        100,
    )
    .unwrap();
    assert_eq!(
        authority.catalog_public_key,
        include_str!("fixtures/authority/catalog.txt")
    );
    macro_rules! reject {
        ($name:literal, $error:expr) => {
            assert_eq!(
                verify_authority(
                    include_bytes!(concat!("fixtures/authority/", $name, ".json")),
                    include_str!(concat!("fixtures/authority/", $name, ".minisig")),
                    ROOT,
                    1,
                    100
                )
                .unwrap_err(),
                $error
            );
        };
    }
    reject!("revoked", VerifyError::Policy);
    reject!("expired", VerifyError::Expired);
    reject!("zero-version", VerifyError::Rollback);
    reject!("aliased-root", VerifyError::Policy);
    reject!("aliased-revoked", VerifyError::Policy);
}

#[test]
fn root_rotation_requires_both_signatures_and_advances_authority() {
    let (key, version, lineage) =
        verify_root_chain(include_bytes!("fixtures/authority/transitions.json"), ROOT, 100).unwrap();
    assert_eq!(key, include_str!("fixtures/authority/next-root.txt"));
    assert_eq!(version, 2);
    assert_eq!(lineage.len(), 2);
    let authority = verify_authority(
        include_bytes!("fixtures/authority/rotated.json"),
        include_str!("fixtures/authority/rotated.minisig"),
        &key,
        version,
        100,
    )
    .unwrap();
    assert_eq!(authority.minimum_metadata_version, 5);
    assert_eq!(
        authority.catalog_public_key,
        include_str!("fixtures/authority/next-catalog.txt")
    );
    assert!(verify_root_chain(include_bytes!("fixtures/authority/bad-transitions.json"), ROOT, 100).is_err());
    assert!(
        verify_authority(
            include_bytes!("fixtures/authority/initial.json"),
            include_str!("fixtures/authority/initial.minisig"),
            &key,
            version,
            100
        )
        .is_err()
    );
}
