// SPDX-License-Identifier: MPL-2.0
//! Explicit diagnostic build seam. No hook is compiled without qa-faults.
//! A marker acknowledges a REAL save boundary; timeout aborts the save.
use serde::Deserialize;
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arm {
    point: String,
    target: PathBuf,
    token: String,
}

fn ordinary(path: &Path) -> io::Result<()> {
    for part in path.ancestors() {
        let meta = fs::symlink_metadata(part)?;
        if meta.file_type().is_symlink() {
            return Err(io::Error::other("QA reparse path"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err(io::Error::other("QA reparse path"));
            }
        }
    }
    Ok(())
}

pub(crate) fn hit(point: &str, target: &Path) -> io::Result<()> {
    let Some(arm) = std::env::var_os("BARELINE_QA_SAVE_ARM") else {
        return Ok(());
    };
    pause(&PathBuf::from(arm), point, target, Duration::from_secs(30))
}

fn pause(arm: &Path, point: &str, target: &Path, timeout: Duration) -> io::Result<()> {
    if !arm.is_absolute() {
        return Err(io::Error::other("QA arm must be absolute"));
    }
    if !arm.try_exists()? {
        return Ok(());
    }
    ordinary(arm)?;
    let mut bytes = Vec::new();
    fs::File::open(arm)?.take(8193).read_to_end(&mut bytes)?;
    if bytes.len() > 8192 {
        return Err(io::Error::other("QA arm limit"));
    }
    let config: Arm = serde_json::from_slice(&bytes)?;
    if config.point != point || config.target != target {
        return Ok(());
    }
    let root = arm
        .parent()
        .ok_or_else(|| io::Error::other("QA root absent"))?
        .canonicalize()?;
    ordinary(target)?;
    if !target.canonicalize()?.starts_with(&root)
        || config.token.len() != 32
        || !config.token.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(io::Error::other("QA target/token outside owned boundary"));
    }
    let marker = arm.with_extension("reached.json");
    let mut file = fs::OpenOptions::new().create_new(true).write(true).open(marker)?;
    let observation = serde_json::json!({"schema_version":1,"point":point,"pid":std::process::id(),"target":target,"token":config.token});
    file.write_all(serde_json::to_string(&observation)?.as_bytes())?;
    file.sync_all()?;
    drop(file);
    let started = Instant::now();
    while arm.try_exists()? {
        if started.elapsed() >= timeout {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "QA save boundary hold expired"));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_boundary_marker_is_exact_single_use_and_bounded() {
        let root = std::env::temp_dir().join(format!(
            "bareline-qa-boundary-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let target = root.join("document.txt");
        fs::write(&target, b"original").unwrap();
        let arm = root.join("save.arm");
        fs::write(
            &arm,
            serde_json::to_vec(
                &serde_json::json!({"point":"StageFlushed","target":target,"token":"0123456789abcdef0123456789abcdef"}),
            )
            .unwrap(),
        )
        .unwrap();
        pause(&arm, "BeforeReplace", &target, Duration::ZERO).unwrap();
        assert!(!arm.with_extension("reached.json").exists());
        let error = pause(&arm, "StageFlushed", &target, Duration::ZERO).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        let row: serde_json::Value =
            serde_json::from_slice(&fs::read(arm.with_extension("reached.json")).unwrap()).unwrap();
        assert_eq!(row["pid"], std::process::id());
        assert_eq!(row["point"], "StageFlushed");
        assert_eq!(fs::read(&target).unwrap(), b"original");
        assert_eq!(
            pause(&arm, "StageFlushed", &target, Duration::ZERO).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        fs::remove_dir_all(root).unwrap();
    }
}
