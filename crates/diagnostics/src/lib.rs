// SPDX-License-Identifier: MPL-2.0
use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    sync::{
        Mutex,
        atomic::{AtomicU8, Ordering},
    },
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
/// A closed action tag prevents paths/content entering tracing fields.
#[cfg(feature = "perf-spans")]
pub fn startup_span(action: StartupAction) -> tracing::span::EnteredSpan {
    tracing::info_span!("startup.phase", action = ?action).entered()
}
#[cfg(not(feature = "perf-spans"))]
pub struct DisabledStartupSpan;
#[cfg(not(feature = "perf-spans"))]
#[inline(always)]
pub fn startup_span(_action: StartupAction) -> DisabledStartupSpan {
    DisabledStartupSpan
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
    pub fn read_config(&mut self, path: &Path, action: StartupAction, limit: usize) -> io::Result<Option<Vec<u8>>> {
        let _phase = startup_span(action);
        if !matches!(
            action,
            StartupAction::ReadSettings | StartupAction::ReadKeymap | StartupAction::ReadSessionHeader
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
            self.entries.push((action, self.started.elapsed().as_micros()));
        }
    }
    pub fn validate(&self) -> Result<(), StartupAction> {
        self.entries
            .iter()
            .find(|(a, _)| !a.allowed())
            .map_or(Ok(()), |(a, _)| Err(*a))
    }
    pub fn presented(&mut self) -> u128 {
        debug_assert!(self.validate().is_ok(), "startup budget violated: {:?}", self.entries);
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
    BackendInitFailed { code: i32, software: bool },
    OperationBlocked { operation: BlockedOperation },
}
#[derive(Clone, Copy, Debug)]
pub enum BlockedOperation {
    UntrustedPath,
    PackageVerification,
    StartupBudget,
}
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum RendererState {
    Uninitialized,
    Hardware,
    Software,
    Recreating,
    Failed,
}
static RENDERER_STATE: AtomicU8 = AtomicU8::new(0);
static PANIC_LOG: Mutex<Option<File>> = Mutex::new(None);
pub fn set_renderer_state(state: RendererState) {
    RENDERER_STATE.store(state as u8, Ordering::Relaxed);
}
fn renderer_state() -> &'static str {
    match RENDERER_STATE.load(Ordering::Relaxed) {
        1 => "hardware",
        2 => "software",
        3 => "recreating",
        4 => "failed",
        _ => "uninitialized",
    }
}
fn build_hash() -> &'static str {
    option_env!("BARELINE_BUILD_HASH")
        .filter(|s| !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or("unknown")
}
fn write_panic(mut out: impl Write, location: Option<(&str, u32, u32)>) -> io::Result<()> {
    // Location is compiler metadata, never a payload or an opened document path.
    let (file, line, column) = location.unwrap_or(("unknown", 0, 0));
    let file: String = file
        .chars()
        .take(160)
        .map(|c| {
            if c.is_ascii_alphanumeric() || "/._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    writeln!(
        out,
        "{{\"event\":\"panic\",\"version\":\"{}\",\"build_hash\":\"{}\",\"renderer\":\"{}\",\"file\":\"{file}\",\"line\":{line},\"column\":{column}}}",
        env!("CARGO_PKG_VERSION"),
        build_hash(),
        renderer_state()
    )
}
pub struct LocalLog {
    file: File,
    directory: std::path::PathBuf,
}
impl LocalLog {
    /// Initialize only after first frame. Keep at most two bounded log files.
    pub fn open(directory: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(directory)?;
        // One fixed-size crash slot: no accumulation, upload, or panic payload.
        let panic = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(directory.join("bareline.crash.log"))?;
        if let Ok(mut sink) = PANIC_LOG.lock() {
            *sink = Some(panic);
        }
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
            directory: directory.to_owned(),
        })
    }
    fn bound(&mut self) -> io::Result<()> {
        if self.file.metadata()?.len() >= 512 * 1024 {
            self.file.flush()?;
            // Copy the bounded old generation, then truncate the already-owned
            // handle; this also works on Windows without renaming an open file.
            std::fs::copy(
                self.directory.join("bareline.log"),
                self.directory.join("bareline.previous.log"),
            )?;
            self.file.set_len(0)?;
        }
        Ok(())
    }
    pub fn event(&mut self, event: Event) -> io::Result<()> {
        self.bound()?;
        match event {
            Event::FirstFrame { micros, software } => writeln!(
                self.file,
                "{{\"event\":\"first_frame\",\"version\":\"{}\",\"build_hash\":\"{}\",\"microseconds\":{micros},\"software\":{software}}}",
                env!("CARGO_PKG_VERSION"),
                build_hash()
            ),
            Event::Idle { private_bytes, frames } => writeln!(
                self.file,
                "{{\"event\":\"idle\",\"private_bytes\":{private_bytes},\"frames\":{frames}}}"
            ),
            Event::RendererRecreated => writeln!(self.file, "{{\"event\":\"renderer_recreated\"}}"),
            Event::BackendInitFailed { code, software } => writeln!(
                self.file,
                "{{\"event\":\"backend_init_failed\",\"code\":{code},\"software\":{software}}}"
            ),
            Event::OperationBlocked { operation } => writeln!(
                self.file,
                "{{\"event\":\"operation_blocked\",\"operation\":\"{operation:?}\"}}"
            ),
        }
    }
    pub fn ledger(&mut self, ledger: &StartupLedger) -> io::Result<()> {
        for (action, micros) in &ledger.entries {
            self.bound()?;
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
        let location = info.location().map(|l| (l.file(), l.line(), l.column()));
        let _ = write_panic(io::stderr().lock(), location);
        // Nonblocking: a panic while configuring the sink must not deadlock.
        if let Ok(mut sink) = PANIC_LOG.try_lock()
            && let Some(mut file) = sink.take()
        {
            let _ = write_panic(&mut file, location);
            let _ = file.sync_all();
        }
    }));
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panic_payload_privacy_subprocess() {
        const SENTINEL: &str = "PRIVATE_DOCUMENT_SENTINEL_do_not_log";
        if let Some(directory) = std::env::var_os("BARELINE_PRIVACY_CHILD") {
            let mut log = LocalLog::open(Path::new(&directory)).unwrap();
            log.event(Event::OperationBlocked {
                operation: BlockedOperation::UntrustedPath,
            })
            .unwrap();
            set_renderer_state(RendererState::Software);
            install_panic_hook();
            panic!("{SENTINEL}");
        }
        let directory = std::env::temp_dir().join(format!(
            "bareline-privacy-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::panic_payload_privacy_subprocess", "--nocapture"])
            .env("BARELINE_PRIVACY_CHILD", &directory)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let crash = std::fs::read_to_string(directory.join("bareline.crash.log")).unwrap();
        let blocked = std::fs::read_to_string(directory.join("bareline.log")).unwrap();
        for record in [
            &crash,
            &blocked,
            &String::from_utf8_lossy(&output.stderr).into_owned(),
            &String::from_utf8_lossy(&output.stdout).into_owned(),
        ] {
            assert!(!record.contains(SENTINEL));
        }
        assert!(crash.len() < 512 && crash.contains("\"build_hash\"") && crash.contains("\"renderer\":\"software\""));
        assert!(blocked.contains("operation_blocked"));
        std::fs::remove_dir_all(directory).unwrap();
    }
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
