// SPDX-License-Identifier: MPL-2.0
//! Explicit QA metadata export; normal launches never write an inventory.
use super::*;
use std::{collections::BTreeSet, ffi::OsString, io::Write, sync::mpsc};
pub(super) struct Request {
    path: PathBuf,
    commit: String,
}
#[derive(Default)]
pub(super) struct InventoryRuntime {
    request: Option<Request>,
    deadline: Option<Instant>,
    pending: Option<mpsc::Receiver<Result<(), String>>>,
}
pub(super) fn parse(
    args: Vec<OsString>,
) -> Result<(Vec<OsString>, Option<Request>), Box<dyn std::error::Error>> {
    let mut remaining = Vec::new();
    let mut path = None;
    let mut commit = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            remaining.push(arg);
            remaining.extend(args);
            break;
        }
        if arg == "--export-command-inventory" || arg == "--inventory-commit" {
            if !cfg!(feature = "qa-inventory") {
                return Err("Command inventory requires the qa-inventory build feature".into());
            }
            let value = args.next().ok_or("Missing command inventory argument")?;
            if arg == "--export-command-inventory" {
                if path.replace(PathBuf::from(value)).is_some() {
                    return Err("Duplicate inventory path".into());
                }
            } else if commit
                .replace(
                    value
                        .into_string()
                        .map_err(|_| "Inventory commit must be UTF-8")?,
                )
                .is_some()
            {
                return Err("Duplicate inventory commit".into());
            }
        } else {
            remaining.push(arg);
        }
    }
    let request =
        match (path, commit) {
            (None, None) => None,
            (Some(path), Some(commit))
                if path.is_absolute()
                    && commit.len() == 40
                    && commit
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) =>
            {
                Some(Request { path, commit })
            }
            _ => return Err(
                "Inventory requires an absolute new output path and a full lowercase commit SHA"
                    .into(),
            ),
        };
    if request.is_some() {
        remaining.splice(
            0..0,
            [
                OsString::from("--no-session"),
                OsString::from("--new-instance"),
            ],
        );
    }
    Ok((remaining, request))
}
impl InventoryRuntime {
    pub(super) fn deadline(&self) -> Option<Instant> {
        self.deadline
    }
    pub(super) fn configure(&mut self, request: Option<Request>) {
        self.request = request;
        self.deadline = self
            .request
            .as_ref()
            .map(|_| Instant::now() + Duration::from_secs(60));
    }
}
impl Shell {
    pub(super) fn inventory_pump(&mut self, el: &ActiveEventLoop) {
        if self.inventory.deadline.is_none() {
            return;
        }
        if self
            .inventory
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            eprintln!("Command inventory discovery timed out");
            self.failed = true;
            self.inventory.deadline = None;
            el.exit();
            return;
        }
        if let Some(pending) = &self.inventory.pending {
            match pending.try_recv() {
                Ok(Ok(())) => {
                    self.inventory.deadline = None;
                    el.exit();
                }
                Ok(Err(error)) => {
                    eprintln!("command inventory: {error}");
                    self.failed = true;
                    self.inventory.deadline = None;
                    el.exit();
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.failed = true;
                    self.inventory.deadline = None;
                    el.exit();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
            return;
        }
        if !self.first_frame {
            return;
        }
        match self.extensions.command_inventory_ready() {
            Ok(false) => return,
            Err(error) => {
                eprintln!("Command inventory incomplete: {error}");
                self.failed = true;
                self.inventory.deadline = None;
                el.exit();
                return;
            }
            Ok(true) => {}
        }
        self.sync_contributions();
        let mut ids = BTreeSet::new();
        let valid = self
            .app
            .commands
            .entries()
            .map(|command| command.id.0.to_string())
            .chain(
                self.app
                    .commands
                    .contributions
                    .entries()
                    .map(|command| command.identity.id.clone()),
            )
            .all(|id| {
                !id.is_empty()
                    && id.len() <= 160
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                    && ids.insert(id)
            });
        if !valid || ids.len() > 10_000 {
            eprintln!("Command inventory has duplicate or unsupported IDs");
            self.failed = true;
            self.inventory.deadline = None;
            el.exit();
            return;
        }
        let Some(request) = self.inventory.request.take() else {
            return;
        };
        let json = format!(
            "{{\"schema_version\":1,\"commit\":\"{}\",\"commands\":[{}]}}\n",
            request.commit,
            ids.iter()
                .map(|id| format!("\"{id}\""))
                .collect::<Vec<_>>()
                .join(",")
        );
        let (tx, rx) = mpsc::sync_channel(1);
        self.inventory.pending = Some(rx);
        let notify = self.notify.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                bareline_platform::LocalFileSystem::validate_target(
                    &bareline_platform_windows::WindowsFileSystem,
                    &request.path,
                )
                .map_err(|e| e.to_string())?;
                let parent = request
                    .path
                    .parent()
                    .ok_or("Inventory parent unavailable")?;
                let stage = parent.join(format!(
                    ".bareline-command-inventory-{}.tmp",
                    std::process::id()
                ));
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&stage)
                    .map_err(|e| e.to_string())?;
                let result = (|| {
                    file.write_all(json.as_bytes())?;
                    file.sync_all()?;
                    drop(file);
                    bareline_platform::LocalFileSystem::rename_entry(
                        &bareline_platform_windows::WindowsFileSystem,
                        &stage,
                        &request.path,
                    )
                })();
                if result.is_err() {
                    let _ = std::fs::remove_file(&stage);
                }
                result.map_err(|e: std::io::Error| e.to_string())
            })();
            let _ = tx.send(result);
            notify();
        });
    }
}

#[cfg(all(test, feature = "qa-inventory"))]
mod tests {
    use super::*;
    #[test]
    fn inventory_arguments_require_provenance_and_preserve_option_terminator() {
        let hash = "a".repeat(40);
        let args = vec![
            "--export-command-inventory".into(),
            "C:\\qa\\commands.json".into(),
            "--inventory-commit".into(),
            hash.into(),
            "--".into(),
            "-literal.txt".into(),
        ];
        let (rest, request) = parse(args).unwrap();
        assert!(request.is_some());
        assert_eq!(
            rest,
            vec![
                OsString::from("--no-session"),
                OsString::from("--new-instance"),
                OsString::from("--"),
                OsString::from("-literal.txt")
            ]
        );
        assert!(parse(vec!["--inventory-commit".into(), "bad".into()]).is_err());
        assert!(
            parse(vec![
                "--export-command-inventory".into(),
                "relative.json".into(),
                "--inventory-commit".into(),
                "b".repeat(40).into()
            ])
            .is_err()
        );
    }
}
