// SPDX-License-Identifier: MPL-2.0
use bareline_distribution::{
    trust::{verify_authority, verify_installed_authority, verify_installed_root_chain, verify_root_chain},
    update::{VerifyError, core_update_policy, verify_manifest},
};
use sha2::{Digest, Sha256};

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
    let pin = authority.publisher_pin();
    assert!(pin.accepts("Initial Fixture Publisher", "Fixture Code Signing CA"));
    assert!(!pin.accepts("Initial Fixture Publisher", "Next Fixture Code Signing CA"));
    assert_eq!(authority.update_helper_sha256, "07".repeat(32));
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
    reject!("revoked-publisher", VerifyError::Policy);
    // Leaf certificate hash pins are no longer an authority format (SEC-08).
    reject!("leaf-hash-pin", VerifyError::Metadata);
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
    // A signed authority rotates the Authenticode pin, including its issuer list.
    assert!(
        authority
            .publisher_pin()
            .accepts("Rotated Fixture Publisher", "Next Fixture Code Signing CA")
    );
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

#[test]
fn authority_expiry_is_separate_from_installed_use() {
    // SEC-02: accepting new metadata needs an unexpired authority; already-verified
    // installed state keeps working, with signature, floors and revocation still applied.
    let expired = include_bytes!("fixtures/authority/expired.json");
    let signature = include_str!("fixtures/authority/expired.minisig");
    assert_eq!(
        verify_authority(expired, signature, ROOT, 1, 100).unwrap_err(),
        VerifyError::Expired
    );
    let installed = verify_installed_authority(expired, signature, ROOT, 1).unwrap();
    assert_eq!(installed.expires_unix, 99);
    assert_eq!(
        verify_installed_authority(expired, signature, ROOT, 2).unwrap_err(),
        VerifyError::Rollback
    );
    assert_eq!(
        verify_installed_authority(
            include_bytes!("fixtures/authority/revoked.json"),
            include_str!("fixtures/authority/revoked.minisig"),
            ROOT,
            1
        )
        .unwrap_err(),
        VerifyError::Policy
    );
    let mut tampered = expired.to_vec();
    tampered[1] ^= 1;
    assert_eq!(
        verify_installed_authority(&tampered, signature, ROOT, 1).unwrap_err(),
        VerifyError::Signature
    );
    let chain = include_bytes!("fixtures/authority/transitions.json");
    assert_eq!(
        verify_root_chain(chain, ROOT, 4102444800).unwrap_err(),
        VerifyError::Expired
    );
    assert_eq!(verify_installed_root_chain(chain, ROOT).unwrap().1, 2);
    assert!(verify_installed_root_chain(include_bytes!("fixtures/authority/bad-transitions.json"), ROOT).is_err());
    // Runtime and catalog floors are separate from the core floor and default to none.
    let initial = verify_authority(
        include_bytes!("fixtures/authority/initial.json"),
        include_str!("fixtures/authority/initial.minisig"),
        ROOT,
        1,
        100,
    )
    .unwrap();
    assert_eq!(initial.minimum_metadata_version, 3);
    assert_eq!(initial.minimum_runtime_metadata_version, 0);
    assert_eq!(initial.minimum_catalog_metadata_version, 0);
}

#[test]
fn signed_core_metadata_delivers_the_next_authority_by_digest() {
    // SEC-02: the current authority's release key signs a core manifest that binds the
    // exact next authority; the offline root still has to verify that authority.
    let current = verify_authority(
        include_bytes!("fixtures/authority/initial.json"),
        include_str!("fixtures/authority/initial.minisig"),
        ROOT,
        1,
        100,
    )
    .unwrap();
    let manifest = verify_manifest(
        include_bytes!("fixtures/authority/delivery.json"),
        include_str!("fixtures/authority/delivery.minisig"),
        &core_update_policy(&current.release_public_key, "Bareline", "stable", 3),
        100,
    )
    .unwrap();
    let refreshed = include_bytes!("fixtures/authority/refreshed.json");
    assert_eq!(
        manifest.metadata().authority_sha256.as_deref(),
        Some(format!("{:x}", Sha256::digest(refreshed)).as_str())
    );
    assert_eq!(manifest.metadata().root_transitions_sha256, None);
    let next = verify_authority(
        refreshed,
        include_str!("fixtures/authority/refreshed.minisig"),
        ROOT,
        current.root_version,
        100,
    )
    .unwrap();
    assert_eq!(next.root_version, 2);
    assert_eq!(
        next.update_helper_sha256,
        format!("{:x}", Sha256::digest(b"bareline fixture helper, refreshed"))
    );
    // The delivered authority can never lower the accepted root version.
    assert_eq!(
        verify_authority(
            include_bytes!("fixtures/authority/initial.json"),
            include_str!("fixtures/authority/initial.minisig"),
            ROOT,
            next.root_version,
            100
        )
        .unwrap_err(),
        VerifyError::Rollback
    );
}
