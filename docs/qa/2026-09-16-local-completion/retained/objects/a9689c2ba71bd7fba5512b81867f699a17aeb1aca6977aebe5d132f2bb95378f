// SPDX-License-Identifier: MPL-2.0
//! Read-only input probe for owned small lab fixtures; output copies are create-new.
use bareline_file_io::{cancellation::Cancellation, recovery};
use sha2::{Digest, Sha256};
use std::{fs, io, path::Path};

fn ordinary(path: &Path) -> io::Result<()> {
    for part in path.ancestors() {
        let meta = fs::symlink_metadata(part)?;
        if meta.file_type().is_symlink() {
            return Err(io::Error::other("link refused"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err(io::Error::other("reparse refused"));
            }
        }
    }
    Ok(())
}
fn copy_checkpoint(source: &Path, destination: &Path, total: &mut u64, entries: &mut usize) -> io::Result<()> {
    fs::create_dir(destination)?;
    for item in fs::read_dir(source)? {
        let item = item?;
        ordinary(&item.path())?;
        *entries += 1;
        if *entries > 4096 {
            return Err(io::Error::other("copy entry bound"));
        }
        if item.metadata()?.is_dir() {
            copy_checkpoint(&item.path(), &destination.join(item.file_name()), total, entries)?;
        } else {
            use std::io::Read;
            let mut input = fs::File::open(item.path())?.take((64 * 1024 * 1024u64).saturating_sub(*total) + 1);
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(destination.join(item.file_name()))?;
            let size = io::copy(&mut input, &mut output)?;
            *total = total.saturating_add(size);
            if *total >= 64 * 1024 * 1024 {
                return Err(io::Error::other("copy byte bound"));
            }
        }
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 2 {
        return Err("usage: recovery_inspect <owned-recovery-root> <new-output-directory>".into());
    }
    let root = Path::new(&args[0]);
    let output = Path::new(&args[1]);
    println!("{}", serde_json::to_string(&inspect_owned(root, output)?)?);
    Ok(())
}
fn inspect_owned(root: &Path, output: &Path) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    ordinary(root)?;
    ordinary(output.parent().ok_or("no output parent")?)?;
    if output
        .parent()
        .ok_or("no output parent")?
        .canonicalize()?
        .starts_with(root.canonicalize()?)
    {
        return Err("output must be outside the recovery input".into());
    }
    if output.exists() {
        return Err("output must be new".into());
    }
    let mut directories = Vec::new();
    let mut total = 0u64;
    let mut entries = 0usize;
    // Reject links anywhere in the small fixture before calling the real reader.
    let mut pending = vec![root.to_owned()];
    while let Some(path) = pending.pop() {
        for item in fs::read_dir(&path)? {
            let item = item?;
            ordinary(&item.path())?;
            entries += 1;
            if entries > 4096 {
                return Err("probe entry bound".into());
            }
            let meta = item.metadata()?;
            if meta.is_dir() {
                pending.push(item.path());
            } else {
                total = total.saturating_add(meta.len());
            }
            if total > 64 * 1024 * 1024 {
                return Err("probe input bound".into());
            }
            if path == root && meta.is_dir() && item.file_name().to_string_lossy().starts_with("paged-") {
                directories.push(item.path());
            }
        }
    }
    if directories.len() > 8 {
        return Err("probe checkpoint bound".into());
    }
    fs::create_dir(output)?;
    let mut rows = Vec::new();
    let mut copied = 0;
    let mut copied_entries = 0;
    for (index, directory) in directories.iter().enumerate() {
        let snapshot = output.join(format!("checkpoint-{index}"));
        copy_checkpoint(directory, &snapshot, &mut copied, &mut copied_entries)?;
        let inspection = recovery::inspect(&snapshot, &Cancellation::default())?;
        let mut row = serde_json::json!({"directory":directory,"status":format!("{:?}",inspection.status),"metadata":inspection.metadata,
            "last_durable":inspection.last_durable,"checkpoint_durable":inspection.checkpoint_durable,
            "document_metadata":inspection.document_metadata,"validated_records":inspection.validated_records});
        if inspection.status == recovery::RecoveryStatus::Complete {
            if inspection.validated_records > 128 {
                return Err("probe transaction bound".into());
            }
            let mut upper = fs::metadata(snapshot.join("baseline.bin"))?.len();
            if upper > 8 * 1024 * 1024 {
                return Err("probe recovered-size bound".into());
            }
            recovery::replay_transactions(&snapshot, &Cancellation::default(), |_, edits| {
                for edit in edits {
                    upper = upper.saturating_add(edit.inserted.len() as u64);
                }
                if upper > 8 * 1024 * 1024 {
                    return Err(io::Error::other("probe recovered-size bound"));
                }
                Ok(())
            })?;
            let copy = output.join(format!("{index}.bin"));
            recovery::recover_to(&snapshot, &copy, &Cancellation::default())?;
            let bytes = fs::read(&copy)?;
            row["recovered"] =
                serde_json::json!({"path":copy,"sha256":format!("{:x}",Sha256::digest(&bytes)),"bytes":bytes.len()});
        }
        rows.push(row);
    }
    Ok(serde_json::json!({"schema_version":1,"rows":rows}))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Files;
    impl bareline_platform::LocalFileSystem for Files {
        fn identity(&self, _: &fs::File) -> io::Result<bareline_platform::FileIdentity> {
            Err(io::Error::other("unused"))
        }
        fn validate_target(&self, _: &Path) -> io::Result<()> {
            Ok(())
        }
        fn commit(&self, staged: &Path, target: &Path, _: bool) -> io::Result<()> {
            fs::rename(staged, target)
        }
    }
    #[test]
    fn owned_probe_recovers_exact_durable_edits_without_mutating_source() {
        let root = std::env::temp_dir().join(format!(
            "bareline-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let recovery_root = root.join("recovery");
        fs::create_dir(&recovery_root).unwrap();
        let directory = recovery_root.join("paged-fixture");
        let mut writer = recovery::RecoveryWriter::create(
            &directory,
            recovery::RecoveryMetadata {
                original_path: None,
                source_generation: "fixture".into(),
                codec_catalog_version: "utf8".into(),
                original_len: 5,
            },
            &Files,
        )
        .unwrap();
        writer
            .seal_baseline(&mut &b"hello"[..], || Ok(true), &Cancellation::default(), &Files)
            .unwrap();
        let durable = writer
            .append(
                1,
                &[recovery::RecoveryEdit {
                    offset: 5,
                    removed: vec![],
                    inserted: b"!".to_vec(),
                }],
            )
            .unwrap();
        let before = fs::read(directory.join("journal.bin")).unwrap();
        let observed = inspect_owned(&recovery_root, &root.join("observations")).unwrap();
        assert_eq!(observed["rows"][0]["last_durable"]["revision"], durable.revision);
        assert_eq!(
            observed["rows"][0]["recovered"]["sha256"],
            format!("{:x}", Sha256::digest(b"hello!"))
        );
        assert_eq!(fs::read(directory.join("journal.bin")).unwrap(), before);
        assert!(!directory.join("replay").exists());
        assert!(inspect_owned(&recovery_root, &recovery_root.join("nested-output")).is_err());
        fs::write(directory.join("baseline.bin"), b"other").unwrap();
        let broken = inspect_owned(&recovery_root, &root.join("corrupt-observations")).unwrap();
        assert_ne!(broken["rows"][0]["status"], "Complete");
        assert!(broken["rows"][0].get("recovered").is_none());
        drop(writer);
        fs::remove_dir_all(root).unwrap();
    }
}
