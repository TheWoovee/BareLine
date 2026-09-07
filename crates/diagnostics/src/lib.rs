// SPDX-License-Identifier: MPL-2.0
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    time::Instant,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartupAction {
    ParseCli,
    /// PR-018 bounded local instance negotiation and protocol-only listener.
    InstanceHandoff,
    ReadSettings,
    ReadKeymap,
    ReadSessionHeader,
    CreateWindow,
    CreateRenderer,
    ReadDocument,
    SpawnWorker,
    Network,
}
impl StartupAction {
    fn allowed(self) -> bool {
        matches!(
            self,
            Self::ParseCli
                | Self::InstanceHandoff
                | Self::ReadSettings
                | Self::ReadKeymap
                | Self::ReadSessionHeader
                | Self::CreateWindow
                | Self::CreateRenderer
        )
    }
}
pub struct StartupLedger {
    started: Instant,
    pub entries: Vec<(StartupAction, u128)>,
    first_frame: Option<u128>,
}
impl Default for StartupLedger {
    fn default() -> Self {
        Self {
            started: Instant::now(),
            entries: Vec::new(),
            first_frame: None,
        }
    }
}
impl StartupLedger {
    /// All configuration reads pass here so their category and size are bounded.
    pub fn read_config(
        &mut self,
        path: &Path,
        action: StartupAction,
        limit: usize,
    ) -> io::Result<Option<Vec<u8>>> {
        if !matches!(
            action,
            StartupAction::ReadSettings
                | StartupAction::ReadKeymap
                | StartupAction::ReadSessionHeader
        ) {
            self.record(action);
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "not a startup configuration read",
            ));
        }
        self.record(action);
        let file = match File::open(path) {
            Ok(f) => f,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "configuration exceeds read budget",
            ));
        }
        Ok(Some(bytes))
    }
    pub fn record(&mut self, action: StartupAction) {
        if self.first_frame.is_none() {
            self.entries
                .push((action, self.started.elapsed().as_micros()));
        }
    }
    pub fn validate(&self) -> Result<(), StartupAction> {
        self.entries
            .iter()
            .find(|(a, _)| !a.allowed())
            .map_or(Ok(()), |(a, _)| Err(*a))
    }
    pub fn presented(&mut self) -> u128 {
        debug_assert!(
            self.validate().is_ok(),
            "startup budget violated: {:?}",
            self.entries
        );
        *self
            .first_frame
            .get_or_insert_with(|| self.started.elapsed().as_micros())
    }
}

/// No arbitrary strings are accepted: file paths and document contents cannot enter records.
pub enum Event {
    FirstFrame { micros: u128, software: bool },
    Idle { private_bytes: u64, frames: u64 },
    RendererRecreated,
}
pub struct LocalLog {
    file: File,
}
impl LocalLog {
    /// Initialize only after first frame. Keep at most two bounded log files.
    pub fn open(directory: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        let path = directory.join("bareline.log");
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 512 * 1024) {
            let backup = directory.join("bareline.previous.log");
            if backup.exists() {
                std::fs::remove_file(&backup)?;
            }
            std::fs::rename(&path, backup)?;
        }
        Ok(Self {
            file: OpenOptions::new().create(true).append(true).open(path)?,
        })
    }
    pub fn event(&mut self, event: Event) -> io::Result<()> {
        match event {
            Event::FirstFrame { micros, software } => writeln!(
                self.file,
                "{{\"event\":\"first_frame\",\"version\":\"{}\",\"microseconds\":{micros},\"software\":{software}}}",
                env!("CARGO_PKG_VERSION")
            ),
            Event::Idle {
                private_bytes,
                frames,
            } => writeln!(
                self.file,
                "{{\"event\":\"idle\",\"private_bytes\":{private_bytes},\"frames\":{frames}}}"
            ),
            Event::RendererRecreated => writeln!(self.file, "{{\"event\":\"renderer_recreated\"}}"),
        }
    }
    pub fn ledger(&mut self, ledger: &StartupLedger) -> io::Result<()> {
        for (action, micros) in &ledger.entries {
            writeln!(
                self.file,
                "{{\"event\":\"startup_action\",\"action\":\"{action:?}\",\"microseconds\":{micros}}}"
            )?;
        }
        Ok(())
    }
}
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        // Deliberately omit the panic payload: it can contain document text and paths.
        if let Some(location) = info.location() {
            eprintln!(
                "event=panic version={} file={} line={}",
                env!("CARGO_PKG_VERSION"),
                location.file(),
                location.line()
            );
        } else {
            eprintln!("event=panic version={}", env!("CARGO_PKG_VERSION"));
        }
    }));
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preframe_reads_rejected_but_postframe_reads_allowed() {
        let mut ledger = StartupLedger::default();
        ledger.record(StartupAction::CreateWindow);
        ledger.record(StartupAction::ReadDocument);
        assert_eq!(ledger.validate(), Err(StartupAction::ReadDocument));
        let mut ledger = StartupLedger::default();
        ledger.presented();
        ledger.record(StartupAction::ReadDocument);
        assert_eq!(ledger.validate(), Ok(()));
    }
    #[test]
    fn instance_handoff_does_not_allow_generic_workers_or_network() {
        let mut ledger = StartupLedger::default();
        ledger.record(StartupAction::InstanceHandoff);
        assert_eq!(ledger.validate(), Ok(()));
        ledger.record(StartupAction::SpawnWorker);
        assert_eq!(ledger.validate(), Err(StartupAction::SpawnWorker));
        let mut ledger = StartupLedger::default();
        ledger.record(StartupAction::InstanceHandoff);
        ledger.record(StartupAction::Network);
        assert_eq!(ledger.validate(), Err(StartupAction::Network));
    }
}
