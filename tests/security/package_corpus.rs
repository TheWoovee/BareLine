// SPDX-License-Identifier: MIT OR Apache-2.0
// Included only in package::signed_tests to reuse its TEST-ONLY signing fixture.
// Every variant is rehashed and signed, so rejection exercises the actual archive
// and manifest parser after authentication rather than stopping at a bad signature.
fn corpus_archive(manifest: &str, entry: &str) -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    archive
        .start_file("manifest.toml", zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(manifest.as_bytes()).unwrap();
    archive
        .start_file(entry, zip::write::SimpleFileOptions::default())
        .unwrap();
    archive.write_all(b"nonexecuted corpus component").unwrap();
    archive.finish().unwrap().into_inner()
}

fn corpus_install(bytes: &[u8], mut catalog: Catalog) -> Result<(), PackageError> {
    catalog.entries[0].length = bytes.len() as u64;
    catalog.entries[0].sha256 = format!("{:x}", Sha256::digest(bytes));
    let metadata = serde_json::to_vec(&catalog).unwrap();
    let (key, signature) = sign(&metadata);
    let root = std::env::temp_dir().join(format!(
        "bareline-auth-parser-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    struct OwnedDirectory(PathBuf);
    impl Drop for OwnedDirectory {
        fn drop(&mut self) {
            if self
                .0
                .starts_with(std::env::temp_dir().canonicalize().unwrap())
            {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
    }
    let _owned = OwnedDirectory(root.canonicalize().unwrap());
    fs::write(
        root.join(format!("{}.blex", catalog.entries[0].sha256)),
        bytes,
    )
    .unwrap();
    let source = OfflinePackageSource::open(root.clone(), &metadata, &signature, &policy(&key))?;
    let package = source.fetch(&PackageRequest {
        id: "fixture.tools".into(),
        version: "1".into(),
    })?;
    let installed = package.install(&root, &std::sync::atomic::AtomicBool::new(false))?;
    assert_eq!(installed.manifest.id, "fixture.tools");
    assert!(
        installed
            .directory()
            .canonicalize()
            .unwrap()
            .starts_with(&_owned.0)
    );
    Ok(())
}

#[test]
fn authenticated_package_mutations_reach_manifest_and_archive_guards() {
    let (seed, catalog) = fixture();
    corpus_install(&seed, catalog).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(&seed)).unwrap();
    let mut manifest = String::new();
    archive
        .by_name("manifest.toml")
        .unwrap()
        .read_to_string(&mut manifest)
        .unwrap();
    assert!(manifest.len() < 4096 && seed.len() < 8192);
    for index in 0..16 {
        let cut = index * seed.len() / 16;
        assert!(
            corpus_install(&seed[..cut], fixture().1).is_err(),
            "truncated archive {index}"
        );
    }
    for index in 0..16 {
        let cut = index * manifest.len() / 16;
        let bytes = corpus_archive(&manifest[..cut], "entry.wasm");
        assert!(
            corpus_install(&bytes, fixture().1).is_err(),
            "truncated manifest {index}"
        );
    }
    for text in [
        manifest.replace("schema_version = 1", "schema_version = 99"),
        manifest.replace("fixture.tools", "other.tools"),
        manifest.replace("entry.wasm", "../entry.wasm"),
        manifest.replace("commands = []", "commands = [\"same\", \"same\"]"),
        manifest.replace("capabilities = []", "capabilities = [\"UnknownGrant\"]"),
        format!("{manifest}\nunknown_security_field = true\n"),
    ] {
        assert_ne!(text, manifest, "mutation must change serialized fixture");
        assert!(corpus_install(&corpus_archive(&text, "entry.wasm"), fixture().1).is_err());
    }
    for entry in ["../entry.wasm", "C:entry.wasm", "NUL.wasm"] {
        assert!(corpus_install(&corpus_archive(&manifest, entry), fixture().1).is_err());
    }
}
