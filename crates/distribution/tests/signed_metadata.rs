// SPDX-License-Identifier: MPL-2.0
use bareline_distribution::update::{TrustPolicy, VerifyError, verify_manifest};
#[test]
fn signed_metadata_policy_vectors_and_package_tampering() {
    let policy = TrustPolicy {
        release_public_key: include_str!("fixtures/public-key.txt"),
        channel: "stable",
        artifact_type: "bareline-x64",
        platform: "windows-x64",
        publisher: "test-publisher",
        protocol: 1,
        highest_metadata_version: 3,
        maximum_package_bytes: 4096,
    };
    macro_rules! fixture {
        ($name:literal) => {
            (
                include_bytes!(concat!("fixtures/", $name, ".json")).as_slice(),
                include_str!(concat!("fixtures/", $name, ".minisig")),
            )
        };
    }
    let (bytes, signature) = fixture!("valid");
    let valid = verify_manifest(bytes, signature, &policy, 100).unwrap();
    valid.verify_package(&mut &b"test"[..]).unwrap();
    assert_eq!(valid.verify_package(&mut &b"tent"[..]), Err(VerifyError::Hash));
    let mut changed = bytes.to_vec();
    changed[0] = b'[';
    assert_eq!(
        verify_manifest(&changed, signature, &policy, 100).unwrap_err(),
        VerifyError::Signature
    );
    for ((bytes, signature), expected) in [
        (fixture!("expired"), VerifyError::Expired),
        (fixture!("rollback"), VerifyError::Rollback),
        (fixture!("channel"), VerifyError::Policy),
        (fixture!("publisher"), VerifyError::Policy),
        (fixture!("platform"), VerifyError::Policy),
        (fixture!("protocol"), VerifyError::Policy),
        (fixture!("artifact"), VerifyError::Policy),
        (fixture!("length"), VerifyError::Policy),
    ] {
        assert_eq!(verify_manifest(bytes, signature, &policy, 100).unwrap_err(), expected);
    }
    let (bytes, signature) = fixture!("hash");
    assert_eq!(
        verify_manifest(bytes, signature, &policy, 100)
            .unwrap()
            .verify_package(&mut &b"test"[..]),
        Err(VerifyError::Hash)
    );
}
