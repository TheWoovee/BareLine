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
    let state = create_private_stage(&parent).unwrap();
    let embedded = PublisherPin::parse("Unused Embedded Publisher", "Unused CA").unwrap();
    let resolve = || {
        resolve_release_authority(
            &root,
            &state,
            "unused-embedded-key",
            &embedded,
            3,
            Some(OfflineRootPolicy {
                public_key: ROOT_KEY,
                minimum_version: 1,
            }),
            AuthorityFreshness::Required,
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
    for action in [HelperAction::Acknowledge, HelperAction::Apply, HelperAction::Recover] {
        assert_eq!(
            launch_update_helper(&root, &rotated, action).unwrap_err().to_string(),
            "update helper differs from the signed release authority"
        );
    }
    // Without a signed helper hash nothing is launched, whatever its Authenticode (SEC-08).
    let unpinned = ResolvedReleaseAuthority {
        release_public_key: rotated.release_public_key.clone(),
        signer: rotated.signer.clone(),
        update_helper_sha256: None,
        minimum_metadata_version: rotated.minimum_metadata_version,
        minimum_runtime_metadata_version: 0,
        minimum_catalog_metadata_version: 0,
        catalog_public_key: None,
    };
    assert_eq!(
        launch_update_helper(&root, &unpinned, HelperAction::Acknowledge)
            .unwrap_err()
            .to_string(),
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
        launch_update_helper(&root, &hashed, HelperAction::Acknowledge)
            .unwrap_err()
            .to_string()
            .starts_with("helper publisher:")
    );
    assert_eq!(rotated.minimum_metadata_version, 5);
    assert_eq!(
        rotated.catalog_public_key.unwrap(),
        String::from_utf8(fixture("next-catalog.txt")).unwrap()
    );
    // Ledgers and the lock are per-user state; the installation is only read (SEC-04).
    assert!(!root.join("bareline.root-versions").exists() && !root.join("bareline.update-lock").exists());
    let ledger = std::fs::read(state.join("root-versions")).unwrap();
    assert_eq!(
        std::str::from_utf8(&ledger).unwrap().lines().collect::<Vec<_>>(),
        vec!["1", "2"]
    );
    resolve().unwrap();
    assert_eq!(std::fs::read(state.join("root-versions")).unwrap(), ledger);
    std::fs::remove_file(root.join("bareline.root-transitions.json")).unwrap();
    publish_fixture(&root, "initial");
    assert!(resolve().is_err());
    assert_eq!(std::fs::read(state.join("root-versions")).unwrap(), ledger);
    assert!(root.canonicalize().unwrap().starts_with(parent.canonicalize().unwrap()));
    remove_all(root);
    remove_all(state);
}

fn remove_all(directory: std::path::PathBuf) {
    for entry in std::fs::read_dir(&directory).unwrap() {
        let path = entry.unwrap().path();
        assert!(path.is_file());
        std::fs::remove_file(path).unwrap();
    }
    std::fs::remove_dir(directory).unwrap();
}

fn root_policy() -> OfflineRootPolicy<'static> {
    OfflineRootPolicy {
        public_key: ROOT_KEY,
        minimum_version: 1,
    }
}

#[test]
fn legacy_install_root_ledgers_are_honored_read_only() {
    // SEC-04: existing ledgers in the installation still bind the root floor after the
    // move to per-user state, and nothing new is written to the installation.
    let parent = std::env::temp_dir();
    let (root, state) = (
        create_private_stage(&parent).unwrap(),
        create_private_stage(&parent).unwrap(),
    );
    let embedded = PublisherPin::parse("Unused Embedded Publisher", "Unused CA").unwrap();
    publish_fixture(&root, "initial");
    std::fs::write(root.join("bareline.root-versions"), b"1\n2\n").unwrap();
    let before: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    for freshness in [AuthorityFreshness::Required, AuthorityFreshness::Installed] {
        let error = resolve_release_authority(
            &root,
            &state,
            "unused",
            &embedded,
            3,
            Some(root_policy()),
            freshness,
            100,
        )
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("Rollback"), "{error}");
    }
    std::fs::write(root.join("bareline.root-versions"), b"1\n").unwrap();
    resolve_release_authority(
        &root,
        &state,
        "unused",
        &embedded,
        3,
        Some(root_policy()),
        AuthorityFreshness::Required,
        100,
    )
    .unwrap();
    let after: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(before, after);
    assert!(state.join("update-lock").is_file() && state.join("root-keys").is_file());
    remove_all(root);
    remove_all(state);
}

#[test]
fn per_artifact_floors_are_independent() {
    // SEC-03: the authority's core floor never becomes the runtime or catalog floor.
    let parent = std::env::temp_dir();
    let (root, state) = (
        create_private_stage(&parent).unwrap(),
        create_private_stage(&parent).unwrap(),
    );
    let embedded = PublisherPin::parse("Unused Embedded Publisher", "Unused CA").unwrap();
    publish_fixture(&root, "refreshed");
    let resolved = resolve_release_authority(
        &root,
        &state,
        "unused",
        &embedded,
        3,
        Some(root_policy()),
        AuthorityFreshness::Required,
        100,
    )
    .unwrap();
    assert_eq!(resolved.minimum_metadata_version, 4);
    assert_eq!(resolved.minimum_runtime_metadata_version, 0);
    assert_eq!(resolved.minimum_catalog_metadata_version, 0);
    // A newer core release raises only the core ledger.
    record_core_metadata_version(&state, 9).unwrap();
    assert_eq!(
        core_metadata_floor(&root, &state, resolved.minimum_metadata_version).unwrap(),
        9
    );
    assert_eq!(resolved.minimum_runtime_metadata_version, 0);
    // An installed runtime accepted under an older release keeps restoring.
    let runtime = bareline_distribution::update::TrustPolicy {
        release_public_key: include_str!("../../../distribution/tests/fixtures/public-key.txt"),
        channel: "stable",
        artifact_type: "bareline-x64",
        platform: "windows-x64",
        publisher: "test-publisher",
        protocol: 1,
        highest_metadata_version: 3.max(resolved.minimum_runtime_metadata_version),
        maximum_package_bytes: 4096,
    };
    assert!(
        bareline_distribution::update::verify_manifest(
            include_bytes!("../../../distribution/tests/fixtures/valid.json"),
            include_str!("../../../distribution/tests/fixtures/valid.minisig"),
            &runtime,
            100
        )
        .is_ok()
    );
    remove_all(root);
    remove_all(state);
}

#[test]
fn expired_authority_still_serves_installed_state_only() {
    // SEC-02: after the authority expires, new metadata is refused, but installed
    // extensions, acknowledgement and recovery keep resolving the same pins.
    let parent = std::env::temp_dir();
    let (root, state) = (
        create_private_stage(&parent).unwrap(),
        create_private_stage(&parent).unwrap(),
    );
    let embedded = PublisherPin::parse("Unused Embedded Publisher", "Unused CA").unwrap();
    publish_fixture(&root, "expired");
    let resolve = |freshness| {
        resolve_release_authority(
            &root,
            &state,
            "unused",
            &embedded,
            3,
            Some(root_policy()),
            freshness,
            100,
        )
    };
    assert!(
        resolve(AuthorityFreshness::Required)
            .err()
            .unwrap()
            .to_string()
            .contains("Expired")
    );
    let installed = resolve(AuthorityFreshness::Installed).unwrap();
    assert!(
        installed
            .signer
            .accepts("Initial Fixture Publisher", "Fixture Code Signing CA")
    );
    remove_all(root);
    remove_all(state);
}

const REFRESHED_HELPER: &[u8] = b"bareline fixture helper, refreshed";

/// The current (initial) authority, a signed core manifest delivering `refreshed`, and
/// the pending files the check stages next to the pending update.
fn delivery_fixture() -> (
    std::path::PathBuf,
    std::path::PathBuf,
    bareline_distribution::update::VerifiedManifest,
    File,
) {
    let parent = std::env::temp_dir();
    let (root, state) = (
        create_private_stage(&parent).unwrap(),
        create_private_stage(&parent).unwrap(),
    );
    let embedded = PublisherPin::parse("Unused Embedded Publisher", "Unused CA").unwrap();
    publish_fixture(&root, "initial");
    std::fs::write(root.join("bareline-update-helper.exe"), b"current helper").unwrap();
    let current = resolve_release_authority(
        &root,
        &state,
        "unused",
        &embedded,
        3,
        Some(root_policy()),
        AuthorityFreshness::Required,
        100,
    )
    .unwrap();
    let manifest = bareline_distribution::update::verify_manifest(
        &fixture("delivery.json"),
        std::str::from_utf8(&fixture("delivery.minisig")).unwrap(),
        &bareline_distribution::update::core_update_policy(&current.release_public_key, "Bareline", "stable", 3),
        100,
    )
    .unwrap();
    std::fs::write(root.join("bareline.pending-authority.json"), fixture("refreshed.json")).unwrap();
    std::fs::write(
        root.join("bareline.pending-authority.minisig"),
        fixture("refreshed.minisig"),
    )
    .unwrap();
    std::fs::write(root.join("bareline.pending-update-helper.exe"), REFRESHED_HELPER).unwrap();
    std::fs::write(root.join("bareline.pending.exe"), b"core").unwrap();
    let core = open_update_file(&root.join("bareline.pending.exe")).unwrap();
    (root, state, manifest, core)
}

fn fixture_publisher(_: &File, pin: &PublisherPin) -> Result<(), UpdateError> {
    if pin.accepts("Initial Fixture Publisher", "Fixture Code Signing CA") {
        Ok(())
    } else {
        Err(UpdateError::Publisher)
    }
}

#[test]
fn signed_update_metadata_refreshes_authority_and_helper_atomically() {
    // SEC-02: the core manifest signed by the current release key delivers the next
    // root-signed authority and the helper it pins; they are installed together.
    let (root, state, manifest, core) = delivery_fixture();
    let verify = || {
        lifecycle::verify_delivered_trust_with(
            &root,
            &root,
            &state,
            manifest.metadata(),
            root_policy(),
            &core,
            100,
            &fixture_publisher,
        )
    };
    // Each delivered file is bound: a changed authority, helper or publisher is refused.
    let original = fixture("refreshed.json");
    std::fs::write(
        root.join("bareline.pending-authority.json"),
        [&original[..], b" "].concat(),
    )
    .unwrap();
    assert!(
        verify()
            .err()
            .unwrap()
            .to_string()
            .contains("differs from the signed update")
    );
    std::fs::write(root.join("bareline.pending-authority.json"), &original).unwrap();
    std::fs::write(root.join("bareline.pending-update-helper.exe"), b"other helper").unwrap();
    assert!(
        verify()
            .err()
            .unwrap()
            .to_string()
            .contains("differs from the delivered authority")
    );
    std::fs::write(root.join("bareline.pending-update-helper.exe"), REFRESHED_HELPER).unwrap();
    assert!(
        lifecycle::verify_delivered_trust_with(
            &root,
            &root,
            &state,
            manifest.metadata(),
            root_policy(),
            &core,
            100,
            &|_: &File, _: &PublisherPin| Err(UpdateError::Publisher),
        )
        .is_err()
    );
    // An expired current time refuses delivery; delivery is an explicit, fresh flow.
    assert!(
        lifecycle::verify_delivered_trust_with(
            &root,
            &root,
            &state,
            manifest.metadata(),
            root_policy(),
            &core,
            4102444800,
            &fixture_publisher,
        )
        .is_err()
    );
    let delivered = verify().unwrap().expect("a newer authority is delivered");
    install_delivered_trust(&root, delivered).unwrap();
    assert_eq!(
        fixture("refreshed.json"),
        std::fs::read(root.join("bareline.release-authority.json")).unwrap()
    );
    assert_eq!(
        std::fs::read(root.join("bareline-update-helper.exe")).unwrap(),
        REFRESHED_HELPER
    );
    assert!(!root.join("bareline.pending-authority.json").exists());
    assert!(!root.join("bareline.pending-update-helper.exe").exists());
    assert!(!root.join("bareline.trust-journal").exists());
    let embedded = PublisherPin::parse("Unused Embedded Publisher", "Unused CA").unwrap();
    let refreshed = resolve_release_authority(
        &root,
        &state,
        "unused",
        &embedded,
        3,
        Some(root_policy()),
        AuthorityFreshness::Required,
        100,
    )
    .unwrap();
    assert_eq!(refreshed.minimum_metadata_version, 4);
    assert_eq!(
        refreshed.update_helper_sha256,
        Some(format!("{:x}", Sha256::digest(REFRESHED_HELPER)))
    );
    // The refreshed root version is now the floor: the older authority is refused.
    std::fs::write(root.join("bareline.pending-authority.json"), fixture("initial.json")).unwrap();
    std::fs::write(
        root.join("bareline.pending-authority.minisig"),
        fixture("initial.minisig"),
    )
    .unwrap();
    std::fs::write(root.join("bareline.pending-update-helper.exe"), REFRESHED_HELPER).unwrap();
    assert!(verify().is_err());
    // Delivering what is already installed changes nothing.
    std::fs::write(root.join("bareline.pending-authority.json"), fixture("refreshed.json")).unwrap();
    std::fs::write(
        root.join("bareline.pending-authority.minisig"),
        fixture("refreshed.minisig"),
    )
    .unwrap();
    assert!(verify().unwrap().is_none());
    drop(core);
    remove_all(root);
    remove_all(state);
}

#[test]
fn failed_trust_install_rolls_back_to_the_previous_authority_and_helper() {
    let (root, state, manifest, core) = delivery_fixture();
    let delivered = lifecycle::verify_delivered_trust_with(
        &root,
        &root,
        &state,
        manifest.metadata(),
        root_policy(),
        &core,
        100,
        &fixture_publisher,
    )
    .unwrap()
    .unwrap();
    // The helper cannot be retained under this generation, so the swap fails midway.
    std::fs::create_dir(root.join("bareline-update-helper.exe.retained-7")).unwrap();
    assert!(lifecycle::install_trust_generation(&root, delivered, 7).is_err());
    std::fs::remove_dir(root.join("bareline-update-helper.exe.retained-7")).unwrap();
    assert_eq!(
        std::fs::read(root.join("bareline.release-authority.json")).unwrap(),
        fixture("initial.json")
    );
    assert_eq!(
        std::fs::read(root.join("bareline-update-helper.exe")).unwrap(),
        b"current helper"
    );
    assert_eq!(
        std::fs::read(root.join("bareline.pending-authority.json")).unwrap(),
        fixture("refreshed.json")
    );
    assert_eq!(
        std::fs::read(root.join("bareline.pending-update-helper.exe")).unwrap(),
        REFRESHED_HELPER
    );
    assert!(!root.join("bareline.trust-journal").exists());
    // A crash mid-swap is rolled back by reconciliation before the authority is read.
    std::fs::rename(
        root.join("bareline.release-authority.json"),
        root.join("bareline.release-authority.json.retained-8"),
    )
    .unwrap();
    std::fs::rename(
        root.join("bareline.pending-authority.json"),
        root.join("bareline.release-authority.json"),
    )
    .unwrap();
    std::fs::write(
        root.join("bareline.trust-journal"),
        "generation=8\nreplaced bareline.release-authority.json\nreplaced bareline.release-authority.minisig\nreplaced bareline-update-helper.exe\n",
    )
    .unwrap();
    assert_eq!(
        record_update_launch(&root, &state, 3).unwrap(),
        LaunchDecision::NotPending
    );
    assert_eq!(
        std::fs::read(root.join("bareline.release-authority.json")).unwrap(),
        fixture("initial.json")
    );
    assert_eq!(
        std::fs::read(root.join("bareline.pending-authority.json")).unwrap(),
        fixture("refreshed.json")
    );
    assert!(!root.join("bareline.trust-journal").exists());
    assert!(root.join("bareline.trust-journal.retained-8").exists());
    drop(core);
    remove_all(root);
    remove_all(state);
}

#[test]
fn helper_apply_fetches_revocation_online_and_acknowledgement_does_not() {
    // SEC-07: only the explicit apply flow needs network revocation evidence.
    assert_eq!(helper_revocation(HelperAction::Apply), Revocation::Online);
    assert_eq!(helper_revocation(HelperAction::Acknowledge), Revocation::Offline);
    assert_eq!(helper_revocation(HelperAction::Recover), Revocation::Offline);
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
