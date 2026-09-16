// SPDX-License-Identifier: MPL-2.0
//! Read-only verifier for the exact signed bootstrap files packaged for Windows.
use std::{collections::BTreeMap, io::Read, path::Path};

fn bounded(path: &Path, limit: u64) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err("metadata size limit".into());
    }
    Ok(bytes)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let mut values = BTreeMap::new();
    while let Some(key) = args.next() {
        let key = key.into_string().map_err(|_| "invalid argument")?;
        if !["--config", "--directory", "--now", "--delivery-directory"].contains(&key.as_str()) {
            return Err("expected --config, --directory and --now".into());
        }
        if values
            .insert(key, args.next().ok_or("missing argument value")?)
            .is_some()
        {
            return Err("duplicate argument".into());
        }
    }
    let get = |key: &str| values.get(key).ok_or("missing required argument");
    let config: serde_json::Value = serde_json::from_slice(&bounded(Path::new(get("--config")?), 65536)?)?;
    let trust = &config["trust"];
    let key = trust["offline_root_public_key"]
        .as_str()
        .ok_or("missing offline root key")?;
    let floor = trust["minimum_root_version"]
        .as_u64()
        .filter(|floor| *floor > 0)
        .ok_or("invalid root floor")?;
    let now: u64 = get("--now")?.to_str().ok_or("invalid time")?.parse()?;
    let directory = Path::new(get("--directory")?);
    let transitions = directory.join("bareline.root-transitions.json");
    let (active, version, lineage) = if transitions.try_exists()? {
        bareline_distribution::trust::verify_root_chain(&bounded(&transitions, 262144)?, key, now)
            .map_err(|error| format!("root transition: {error:?}"))?
    } else {
        (key.to_owned(), 0, vec![key.to_owned()])
    };
    let bytes = bounded(&directory.join("bareline.release-authority.json"), 16384)?;
    let signature = bounded(&directory.join("bareline.release-authority.minisig"), 8192)?;
    let authority = bareline_distribution::trust::verify_authority(
        &bytes,
        std::str::from_utf8(&signature)?,
        &active,
        floor.max(version),
        now,
    )
    .map_err(|error| format!("release authority: {error:?}"))?;
    if let Some(delivery) = values.get("--delivery-directory") {
        let delivery = Path::new(delivery);
        let channel = config["distribution"]["channel"].as_str().ok_or("missing channel")?;
        let publisher = trust["publisher"].as_str().ok_or("missing publisher")?;
        let floor = config["updates"]["minimum_metadata_version"]
            .as_u64()
            .ok_or("missing metadata floor")?
            .max(authority.minimum_metadata_version);
        for (metadata, signature, executable, artifact) in [
            (
                "bareline.update.json",
                "bareline.update.minisig",
                "bareline.exe",
                "core_artifact_type",
            ),
            (
                "runtime.json",
                "runtime.minisig",
                "bareline-extension-host.exe",
                "runtime_artifact_type",
            ),
        ] {
            let bytes = bounded(&delivery.join(metadata), 65536)?;
            let signature = bounded(&delivery.join(signature), 8192)?;
            let manifest = bareline_distribution::update::verify_manifest(
                &bytes,
                std::str::from_utf8(&signature)?,
                &bareline_distribution::update::TrustPolicy {
                    release_public_key: &authority.release_public_key,
                    channel,
                    artifact_type: config["distribution"][artifact]
                        .as_str()
                        .ok_or("missing artifact type")?,
                    platform: "windows-x64",
                    publisher,
                    protocol: 1,
                    highest_metadata_version: floor,
                    maximum_package_bytes: 1024 * 1024 * 1024,
                },
                now,
            )
            .map_err(|error| format!("{metadata}: {error:?}"))?;
            if Some(manifest.metadata().version.as_str()) != config["distribution"]["version"].as_str() {
                return Err("delivery version differs from config".into());
            }
            manifest
                .verify_package(&mut std::fs::File::open(delivery.join(executable))?)
                .map_err(|error| format!("{executable}: {error:?}"))?;
        }
        use bareline_extensions_protocol::{
            CatalogPolicy, OfflinePackageSource, PackageRequest, VerifiedPackageSource,
        };
        let catalog = bounded(&delivery.join("catalog.json"), 1024 * 1024)?;
        let signature = bounded(&delivery.join("catalog.json.minisig"), 8192)?;
        let source = OfflinePackageSource::open(
            delivery.to_owned(),
            &catalog,
            std::str::from_utf8(&signature)?,
            &CatalogPolicy {
                public_key: &authority.catalog_public_key,
                publisher,
                channel,
                platform: "windows-x64",
                artifact_type: "extension",
                highest_metadata_version: floor,
                now_unix: now,
            },
        )
        .map_err(|error| format!("catalog: {error:?}"))?;
        let expected = [
            "org.bareline.json-tools",
            "org.bareline.xml-tools",
            "org.bareline.hex-view",
        ];
        if source.entries().len() != expected.len()
            || expected
                .iter()
                .any(|id| !source.entries().iter().any(|entry| entry.id == *id))
        {
            return Err("first-party catalog set mismatch".into());
        }
        for entry in source.entries() {
            if Some(entry.version.as_str()) != config["distribution"]["version"].as_str() {
                return Err("component version differs from config".into());
            }
            source
                .fetch(&PackageRequest {
                    id: entry.id.clone(),
                    version: entry.version.clone(),
                })
                .map_err(|error| format!("component: {error:?}"))?;
        }
    }
    // This verifier establishes cryptographic policy, not production ownership.
    // Shipping callers separately require a validated configured release JSON.
    println!(
        "{}",
        serde_json::json!({"schema_version": 1, "kind": "verified_bootstrap_authority",
        "verified_at_unix": now, "root_lineage": lineage, "authority": authority})
    );
    Ok(())
}
