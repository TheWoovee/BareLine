// SPDX-License-Identifier: MPL-2.0
use std::{
    collections::VecDeque,
    ffi::{OsStr, OsString},
    io::{self, Read},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub enum LaunchMode {
    Direct { program: PathBuf, arguments: Vec<OsString> },
    Shell { program: PathBuf, arguments: Vec<OsString> },
}
#[derive(Clone, Debug)]
pub struct ProcessRequest {
    pub mode: LaunchMode,
    pub directory: Option<PathBuf>,
    pub capture: bool,
}
/// Construct only after an explicit user grant for the configured command; workspace files cannot grant it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessPermission {
    Denied,
    UserGrantedDirect,
    UserGrantedShell,
}
pub trait ProcessTreeGuard: Send {
    fn terminate(&mut self) -> io::Result<()>;
}
/// Platform launcher must contain descendants before child code executes; no default uncontained fallback.
pub trait ProcessLauncher: Send + Sync + 'static {
    fn spawn(&self, command: &mut Command) -> io::Result<(Child, Box<dyn ProcessTreeGuard>)>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputStream {
    Stdout,
    Stderr,
}
#[derive(Clone, Debug)]
pub struct OutputChunk {
    pub stream: OutputStream,
    pub bytes: Vec<u8>,
}
#[derive(Debug)]
pub struct OutputBuffer {
    chunks: VecDeque<OutputChunk>,
    capacity: usize,
    bytes: usize,
    pub discarded_bytes: u64,
}
impl OutputBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            chunks: VecDeque::new(),
            capacity: capacity.clamp(1024, 16 * 1024 * 1024),
            bytes: 0,
            discarded_bytes: 0,
        }
    }
    pub fn push(&mut self, stream: OutputStream, bytes: &[u8]) {
        // Charge 64 bytes for each chunk's Vec/queue slot and allocator bookkeeping.
        const ENTRY_BUDGET: usize = 64;
        let keep = bytes.len().min(self.capacity - ENTRY_BUDGET);
        self.discarded_bytes = self.discarded_bytes.saturating_add((bytes.len() - keep) as u64);
        // Bound allocation/entry overhead as well as payload, including alternating one-byte reads.
        while self.bytes + keep + (self.chunks.len() + 1) * ENTRY_BUDGET > self.capacity || self.chunks.len() >= 2048 {
            let removed = self.chunks.pop_front().unwrap();
            self.bytes -= removed.bytes.len();
            self.discarded_bytes = self.discarded_bytes.saturating_add(removed.bytes.len() as u64);
        }
        if keep > 0 {
            self.chunks.push_back(OutputChunk {
                stream,
                bytes: bytes[bytes.len() - keep..].to_vec(),
            });
            self.bytes += keep;
        }
    }
    pub fn chunks(&self) -> &VecDeque<OutputChunk> {
        &self.chunks
    }
    pub fn byte_len(&self) -> usize {
        self.bytes
    }
    pub fn clear(&mut self) {
        self.chunks.clear();
        self.bytes = 0;
        self.discarded_bytes = 0;
    }
    pub fn text(&self) -> String {
        let mut bytes = Vec::with_capacity(self.bytes);
        for chunk in &self.chunks {
            bytes.extend_from_slice(&chunk.bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProcessState {
    Starting,
    Running,
    Exited { code: Option<i32>, elapsed: Duration },
    Cancelled { elapsed: Duration },
    Failed(String),
}
pub struct ProcessHandle {
    cancel: mpsc::SyncSender<()>,
    state: Arc<Mutex<ProcessState>>,
    output: Arc<Mutex<OutputBuffer>>,
}
impl ProcessHandle {
    pub fn cancel(&self) {
        let _ = self.cancel.try_send(());
    }
    pub fn state(&self) -> ProcessState {
        self.state.lock().unwrap_or_else(|error| error.into_inner()).clone()
    }
    pub fn output(&self) -> std::sync::MutexGuard<'_, OutputBuffer> {
        self.output.lock().unwrap_or_else(|error| error.into_inner())
    }
}
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}
pub fn launch(
    request: ProcessRequest,
    permission: ProcessPermission,
    launcher: Arc<dyn ProcessLauncher>,
    output_capacity: usize,
) -> Result<ProcessHandle, String> {
    let authorized = matches!(
        (&request.mode, permission),
        (
            LaunchMode::Direct { .. },
            ProcessPermission::UserGrantedDirect | ProcessPermission::UserGrantedShell
        ) | (LaunchMode::Shell { .. }, ProcessPermission::UserGrantedShell)
    );
    if !authorized {
        return Err(
            "External command requires an explicit user grant; workspace settings cannot authorize processes".into(),
        );
    }
    let (program, args) = match &request.mode {
        LaunchMode::Direct { program, arguments } | LaunchMode::Shell { program, arguments } => (program, arguments),
    };
    if !program.is_absolute() {
        return Err("External program must be an absolute configured path".into());
    }
    if matches!(request.mode, LaunchMode::Direct { .. })
        && program
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("bat") || extension.eq_ignore_ascii_case("cmd"))
    {
        return Err("Batch files require explicit shell mode".into());
    }
    if args.len() > 4096 || args.iter().map(|arg| arg.len()).sum::<usize>() > 1024 * 1024 {
        return Err("External argument budget exceeded".into());
    }
    let (cancel_tx, cancel_rx) = mpsc::sync_channel(1);
    let state = Arc::new(Mutex::new(ProcessState::Starting));
    let worker_state = state.clone();
    let output = Arc::new(Mutex::new(OutputBuffer::new(output_capacity)));
    let worker_output = output.clone();
    thread::Builder::new()
        .name("bareline-process".into())
        .spawn(move || {
            let start = Instant::now();
            let update = |value| {
                *worker_state.lock().unwrap_or_else(|error| error.into_inner()) = value;
            };
            let (program, args) = match request.mode {
                LaunchMode::Direct { program, arguments } | LaunchMode::Shell { program, arguments } => {
                    (program, arguments)
                }
            };
            let mut command = Command::new(program);
            command.args(args).stdin(Stdio::null());
            // cmd.exe and CreateProcess otherwise probe the working directory first.
            command.env("NoDefaultCurrentDirectoryInExePath", "1");
            if let Some(directory) = request.directory {
                command.current_dir(directory);
            }
            if request.capture {
                command.stdout(Stdio::piped()).stderr(Stdio::piped());
            } else {
                command.stdout(Stdio::null()).stderr(Stdio::null());
            }
            if cancel_rx.try_recv().is_ok() {
                update(ProcessState::Cancelled {
                    elapsed: start.elapsed(),
                });
                return;
            }
            let (mut child, mut tree) = match launcher.spawn(&mut command) {
                Ok(value) => value,
                Err(error) => {
                    update(ProcessState::Failed(error.to_string()));
                    return;
                }
            };
            update(ProcessState::Running);
            let mut readers = Vec::new();
            fn reader(
                pipe: impl Read + Send + 'static,
                stream: OutputStream,
                output: Arc<Mutex<OutputBuffer>>,
            ) -> io::Result<thread::JoinHandle<io::Result<()>>> {
                thread::Builder::new().name("bareline-output".into()).spawn(move || {
                    let mut pipe = pipe;
                    let mut bytes = [0; 8192];
                    loop {
                        match pipe.read(&mut bytes) {
                            Ok(0) => return Ok(()),
                            Ok(count) => output
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .push(stream, &bytes[..count]),
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                                continue;
                            }
                            Err(error) => return Err(error),
                        }
                    }
                })
            }
            let mut setup_error = None;
            if let Some(pipe) = child.stdout.take() {
                match reader(pipe, OutputStream::Stdout, worker_output.clone()) {
                    Ok(reader) => readers.push(reader),
                    Err(error) => setup_error = Some(error.to_string()),
                }
            }
            if let Some(pipe) = child.stderr.take() {
                match reader(pipe, OutputStream::Stderr, worker_output.clone()) {
                    Ok(reader) => readers.push(reader),
                    Err(error) => setup_error = Some(error.to_string()),
                }
            }
            let result = if let Some(error) = setup_error {
                ProcessState::Failed(error)
            } else {
                loop {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            break ProcessState::Exited {
                                code: status.code(),
                                elapsed: start.elapsed(),
                            };
                        }
                        Err(error) => break ProcessState::Failed(error.to_string()),
                        Ok(None) => {}
                    }
                    match cancel_rx.recv_timeout(Duration::from_millis(20)) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                            break ProcessState::Cancelled {
                                elapsed: start.elapsed(),
                            };
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            };
            // Also terminate descendants after root exit so inherited pipe handles cannot keep readers alive.
            let cleanup = tree.terminate();
            if cleanup.is_err() {
                let _ = child.kill();
            }
            let waited = child.wait();
            drop(tree);
            let mut read_error = None;
            for reader in readers {
                match reader.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => read_error = Some(error.to_string()),
                    Err(_) => read_error = Some("Output reader stopped unexpectedly".into()),
                }
            }
            if let Err(error) = cleanup {
                update(ProcessState::Failed(format!("Process tree cleanup failed: {error}")));
            } else if let Err(error) = waited {
                update(ProcessState::Failed(format!("Process wait failed: {error}")));
            } else if let Some(error) = read_error {
                update(ProcessState::Failed(format!("Output capture failed: {error}")));
            } else {
                update(result);
            }
        })
        .map_err(|error| error.to_string())?;
    Ok(ProcessHandle {
        cancel: cancel_tx,
        state,
        output,
    })
}

#[derive(Default)]
pub struct PlaceholderContext {
    pub file: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub selection: String,
    pub line: u64,
    pub column: u64,
}
/// How an expanded placeholder value must be treated by the command interpreter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceholderSafety {
    /// Direct launch: values become argv entries, so no interpreter sees them.
    Argument,
    /// Shell launch: the expanded template becomes one `cmd.exe /c` string.
    CommandShell,
}
/// Characters `cmd.exe` acts on. A value carrying any of them cannot be made safe by
/// quoting alone (a quote inside the value re-opens the metacharacter context), so
/// shell mode refuses it and points the user at direct mode.
const SHELL_METACHARACTERS: &[char] = &['&', '|', '<', '>', '^', '%', '"', '(', ')', '!', '\n', '\r'];
/// Env-var passing (`%BARELINE_FILE%`) was considered and rejected: `cmd.exe` expands
/// `%VAR%` while it parses the `/c` string, so a `&` inside the value would still
/// separate commands. Refusing metacharacters is the interpreter-independent guarantee.
fn shell_safe(value: &OsStr) -> Result<OsString, String> {
    let text = value
        .to_str()
        .ok_or("Shell commands require text placeholder values; use direct mode")?;
    if text
        .chars()
        .any(|c| SHELL_METACHARACTERS.contains(&c) || c.is_control())
    {
        return Err("This name or selection contains characters a command shell would run as commands (& | < > ^ % \" ( ) !). Run the command in direct mode instead.".into());
    }
    Ok(if text.is_empty() || text.contains(char::is_whitespace) {
        OsString::from(format!("\"{text}\""))
    } else {
        value.to_owned()
    })
}
/// Preserves native path units and argv boundaries. Unsaved file/dir or missing workspace refuses expansion.
pub fn expand_argument(template: &str, context: &PlaceholderContext) -> Result<OsString, String> {
    expand_argument_for(template, context, PlaceholderSafety::Argument)
}
/// Preserves native path units and argv boundaries. Unsaved file/dir or missing workspace refuses expansion.
pub fn expand_argument_for(
    template: &str,
    context: &PlaceholderContext,
    safety: PlaceholderSafety,
) -> Result<OsString, String> {
    fn append(output: &mut OsString, value: impl AsRef<OsStr>) -> Result<(), String> {
        let value = value.as_ref();
        if value.len() > 1024 * 1024 - output.len() {
            return Err("Expanded argument exceeds 1 MiB".into());
        }
        output.push(value);
        Ok(())
    }
    let place = |output: &mut OsString, value: &OsStr| -> Result<(), String> {
        match safety {
            PlaceholderSafety::Argument => append(output, value),
            PlaceholderSafety::CommandShell => append(output, shell_safe(value)?),
        }
    };
    let mut output = OsString::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        append(&mut output, &rest[..start])?;
        rest = &rest[start + 2..];
        let end = rest.find('}').ok_or("Unclosed external command placeholder")?;
        match &rest[..end] {
            "file" => place(
                &mut output,
                context
                    .file
                    .as_ref()
                    .ok_or("Save the document before using ${file}")?
                    .as_os_str(),
            )?,
            "dir" => place(
                &mut output,
                context
                    .file
                    .as_ref()
                    .and_then(|path| path.parent())
                    .ok_or("Save the document before using ${dir}")?
                    .as_os_str(),
            )?,
            "workspace" => place(
                &mut output,
                context.workspace.as_ref().ok_or("No workspace is open")?.as_os_str(),
            )?,
            "selection" => place(&mut output, OsStr::new(context.selection.as_str()))?,
            "line" => place(&mut output, OsStr::new(&context.line.to_string()))?,
            "column" => place(&mut output, OsStr::new(&context.column.to_string()))?,
            unknown => return Err(format!("Unknown external command placeholder: {unknown}")),
        }
        rest = &rest[end + 1..];
    }
    append(&mut output, rest)?;
    Ok(output)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputLink {
    pub path: PathBuf,
    pub line: u64,
    pub column: u64,
}
/// Parse a literal complete path:line:column only; never interpret output as a shell command or URL.
pub fn parse_output_link(value: &str) -> Option<OutputLink> {
    if value.len() > 32 * 1024 || value.chars().any(char::is_control) {
        return None;
    }
    let mut parts = value.rsplitn(3, ':');
    let column = parts.next()?.parse::<u64>().ok()?;
    let line = parts.next()?.parse::<u64>().ok()?;
    let path = parts.next()?;
    if path.is_empty() || line == 0 || column == 0 || path.contains("://") {
        return None;
    }
    Some(OutputLink {
        path: PathBuf::from(OsStr::new(path)),
        line,
        column,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_placeholders_keep_quotes_metacharacters_and_native_paths_literal() {
        let context = PlaceholderContext {
            file: Some(PathBuf::from(r"C:\some folder\file & name.txt")),
            selection: "a\"; $(unsafe) & | >".into(),
            line: 12,
            column: 3,
            ..Default::default()
        };
        assert_eq!(
            expand_argument("--selection=${selection}", &context).unwrap(),
            OsString::from("--selection=a\"; $(unsafe) & | >")
        );
        assert_eq!(
            expand_argument("${file}", &context).unwrap(),
            context.file.unwrap().into_os_string()
        );
        assert!(expand_argument("${file}", &PlaceholderContext::default()).is_err());
        assert!(expand_argument("${bad}", &PlaceholderContext::default()).is_err());
        assert_eq!(parse_output_link(r"C:\a b.rs:12:3").unwrap().line, 12);
        assert!(parse_output_link("https://bad:1:2").is_none());
    }
    #[test]
    fn shell_mode_refuses_command_metacharacters_in_expanded_placeholders() {
        let context = PlaceholderContext {
            file: Some(PathBuf::from(r"C:\notes\a&calc.exe.txt")),
            workspace: Some(PathBuf::from(r"C:\notes & tools")),
            selection: "& calc".into(),
            ..Default::default()
        };
        for template in ["${file}", "${selection}", "--root=${workspace}"] {
            assert!(
                expand_argument_for(template, &context, PlaceholderSafety::CommandShell).is_err(),
                "{template} must be refused in shell mode"
            );
        }
        // The same values stay literal for direct launches, which never reach a shell.
        assert!(expand_argument("${file}", &context).is_ok());
        let safe = PlaceholderContext {
            file: Some(PathBuf::from(r"C:\some folder\notes.txt")),
            selection: "plain".into(),
            line: 7,
            ..Default::default()
        };
        assert_eq!(
            expand_argument_for("${file}", &safe, PlaceholderSafety::CommandShell).unwrap(),
            OsString::from("\"C:\\some folder\\notes.txt\"")
        );
        assert_eq!(
            expand_argument_for("${selection}:${line}", &safe, PlaceholderSafety::CommandShell).unwrap(),
            OsString::from("plain:7")
        );
    }
    #[test]
    fn shell_mode_runs_in_the_workspace_root_not_the_document_folder() {
        let definition = ExternalDefinition {
            name: "x".into(),
            program: r"C:\Windows\System32\cmd.exe".into(),
            arguments: vec!["/c".into(), "tool".into()],
            shell: true,
            capture: true,
        };
        let context = PlaceholderContext {
            file: Some(PathBuf::from(r"C:\untrusted\notes.txt")),
            workspace: Some(PathBuf::from(r"C:\work")),
            ..Default::default()
        };
        assert_eq!(
            definition.request(&context).unwrap().directory,
            Some(PathBuf::from(r"C:\work"))
        );
    }
    #[test]
    fn gigabyte_output_budget_stays_bounded_without_materializing_fixture() {
        let mut buffer = OutputBuffer::new(65536);
        let bytes = [b'x'; 8192];
        for _ in 0..131073 {
            buffer.push(OutputStream::Stdout, &bytes);
        }
        assert!(buffer.byte_len() <= 65536);
        assert!(buffer.discarded_bytes > 1024 * 1024 * 1024 - 65536);
        buffer.clear();
        assert_eq!(buffer.byte_len(), 0);
    }
    #[test]
    fn tiny_reads_and_repeated_placeholders_respect_allocation_budgets() {
        let mut buffer = OutputBuffer::new(65536);
        for index in 0..100_000 {
            buffer.push(
                if index % 2 == 0 {
                    OutputStream::Stdout
                } else {
                    OutputStream::Stderr
                },
                b"x",
            );
        }
        assert!(buffer.byte_len() + buffer.chunks().len() * 64 <= 65536);
        assert!(buffer.chunks().len() <= 2048);
        let context = PlaceholderContext {
            selection: "x".repeat(1024 * 1024),
            ..Default::default()
        };
        assert!(expand_argument(&"${selection}".repeat(1024), &context).is_err());
    }
    struct DeniedLauncher;
    impl ProcessLauncher for DeniedLauncher {
        fn spawn(&self, _: &mut Command) -> io::Result<(Child, Box<dyn ProcessTreeGuard>)> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "fixture rejects launch",
            ))
        }
    }
    #[test]
    fn grants_and_launch_errors_are_visible_without_starting_a_process() {
        let request = ProcessRequest {
            mode: LaunchMode::Direct {
                program: std::env::current_exe().unwrap(),
                arguments: vec![],
            },
            directory: None,
            capture: true,
        };
        assert!(
            launch(
                request.clone(),
                ProcessPermission::Denied,
                Arc::new(DeniedLauncher),
                1024
            )
            .is_err()
        );
        let handle = launch(
            request,
            ProcessPermission::UserGrantedDirect,
            Arc::new(DeniedLauncher),
            1024,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while handle.state() == ProcessState::Starting && Instant::now() < deadline {
            thread::yield_now();
        }
        assert!(matches!(handle.state(),ProcessState::Failed(reason) if reason.contains("fixture rejects")));
    }
}

/// Human-edited user configuration. Import never grants execution permission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalDefinition {
    pub name: String,
    pub program: String,
    pub arguments: Vec<String>,
    pub shell: bool,
    pub capture: bool,
}
impl ExternalDefinition {
    pub fn import_toml(text: &str) -> Result<Self, String> {
        if text.len() > 1024 * 1024 {
            return Err("External command definition exceeds 1 MiB".into());
        }
        let doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| error.to_string())?;
        if doc.get("format_version").and_then(|value| value.as_integer()) != Some(1) {
            return Err("Unsupported external command format_version".into());
        }
        if doc
            .iter()
            .any(|(key, _)| !["format_version", "name", "program", "args", "mode", "capture"].contains(&key))
        {
            return Err("Unknown external command field; grants cannot be imported".into());
        }
        let name = doc
            .get("name")
            .and_then(|value| value.as_str())
            .ok_or("Missing external command name")?
            .to_string();
        let program = doc
            .get("program")
            .and_then(|value| value.as_str())
            .ok_or("Missing external command program")?
            .to_string();
        if name.trim().is_empty() || !PathBuf::from(&program).is_absolute() {
            return Err("Command name and absolute program path are required".into());
        }
        let arguments = doc
            .get("args")
            .and_then(|value| value.as_array())
            .ok_or("args must be an array")?
            .iter()
            .map(|value| value.as_str().map(str::to_string).ok_or("Arguments must be strings"))
            .collect::<Result<Vec<_>, _>>()?;
        if arguments.len() > 4096 {
            return Err("Too many external command arguments".into());
        }
        let shell = match doc.get("mode").and_then(|value| value.as_str()).unwrap_or("direct") {
            "direct" => false,
            "shell" => true,
            _ => return Err("mode must be direct or shell".into()),
        };
        let capture = match doc.get("capture") {
            None => true,
            Some(value) => value.as_bool().ok_or("capture must be a boolean")?,
        };
        Ok(Self {
            name,
            program,
            arguments,
            shell,
            capture,
        })
    }
    pub fn request(&self, context: &PlaceholderContext) -> Result<ProcessRequest, String> {
        let mut arguments = Vec::new();
        let mut budget = 0usize;
        let safety = if self.shell {
            PlaceholderSafety::CommandShell
        } else {
            PlaceholderSafety::Argument
        };
        for template in &self.arguments {
            let argument = expand_argument_for(template, context, safety)?;
            budget = budget.checked_add(argument.len()).ok_or("Argument budget exceeded")?;
            if budget > 1024 * 1024 {
                return Err("Expanded arguments exceed 1 MiB".into());
            }
            arguments.push(argument);
        }
        let program = PathBuf::from(&self.program);
        Ok(ProcessRequest {
            mode: if self.shell {
                LaunchMode::Shell { program, arguments }
            } else {
                LaunchMode::Direct { program, arguments }
            },
            // Shell templates resolve unqualified tool names against the working
            // directory, so they never run in the document's folder (SEC-04).
            directory: if self.shell {
                context.workspace.as_ref().map(PathBuf::from)
            } else {
                context.file.as_ref().and_then(|file| file.parent()).map(PathBuf::from)
            },
            capture: self.capture,
        })
    }
    pub fn export_toml(&self) -> String {
        format!(
            "format_version = 1\nname = {}\nprogram = {}\nargs = [{}]\nmode = '{}'\ncapture = {}\n",
            crate::quote(&self.name),
            crate::quote(&self.program),
            self.arguments
                .iter()
                .map(|argument| crate::quote(argument))
                .collect::<Vec<_>>()
                .join(", "),
            if self.shell { "shell" } else { "direct" },
            self.capture
        )
    }
}
