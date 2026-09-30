// SPDX-License-Identifier: MPL-2.0
use std::{
    borrow::Cow,
    collections::VecDeque,
    ffi::{OsStr, OsString},
    io::{self, Read},
    path::{Path, PathBuf},
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
    /// Appends `line` to `command` verbatim, for a program that parses its own command
    /// line (shell mode's `cmd.exe`, whose rules std's argv quoting would corrupt).
    /// Launchers without raw command lines refuse, so shell mode never falls back to argv.
    fn set_raw_command_line(&self, _command: &mut Command, _line: &OsStr) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "This launcher cannot pass a raw shell command line",
        ))
    }
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
    validate_request(&request)?;
    let shell_line = match &request.mode {
        LaunchMode::Shell { arguments, .. } => Some(command_shell_line(arguments)?),
        LaunchMode::Direct { .. } => None,
    };
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
            match &shell_line {
                // cmd.exe parses its own command line; std's argv quoting would corrupt it (SEC-10).
                Some(line) => {
                    if let Err(error) = launcher.set_raw_command_line(&mut command, line) {
                        update(ProcessState::Failed(error.to_string()));
                        return;
                    }
                }
                None => {
                    command.args(args);
                }
            }
            command.stdin(Stdio::null());
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

/// Launch policy, checked before the consent prompt and again at launch.
pub fn validate_request(request: &ProcessRequest) -> Result<(), String> {
    let (program, arguments) = match &request.mode {
        LaunchMode::Direct { program, arguments } | LaunchMode::Shell { program, arguments } => (program, arguments),
    };
    if arguments.len() > 4096 || arguments.iter().map(|argument| argument.len()).sum::<usize>() > 1024 * 1024 {
        return Err("External argument budget exceeded".into());
    }
    // Interpreter checks come first: they depend only on the name, so they read the
    // same on every host (a `C:\` path is not absolute off Windows).
    match &request.mode {
        LaunchMode::Direct { .. } => {
            // std quotes argv for CommandLineToArgvW; cmd.exe re-parses that text as commands.
            if Interpreter::of(program) == Some(Interpreter::CommandShell) {
                return Err("cmd.exe and batch files run only in shell mode".into());
            }
        }
        LaunchMode::Shell { .. } => {
            if !cfg!(windows) || !system_command_shell().is_some_and(|shell| same_windows_path(program, &shell)) {
                return Err("Shell mode runs only %SystemRoot%\\System32\\cmd.exe".into());
            }
            command_shell_line(arguments)?;
        }
    }
    if !program.is_absolute() {
        return Err("External program must be an absolute configured path".into());
    }
    Ok(())
}
/// `%SystemRoot%\System32\cmd.exe`, the only program shell mode runs.
pub fn system_command_shell() -> Option<PathBuf> {
    command_shell_under(&std::env::var_os("SystemRoot")?)
}
fn command_shell_under(system_root: &OsStr) -> Option<PathBuf> {
    let root = system_root.to_str()?.trim_end_matches(['\\', '/']);
    (!root.is_empty()).then(|| PathBuf::from(format!("{root}\\System32\\cmd.exe")))
}
fn same_windows_path(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| path.to_str().map(|text| text.replace('/', "\\").to_ascii_lowercase());
    normalize(left).is_some_and(|left| Some(left) == normalize(right))
}
/// The raw `cmd.exe` command line for a shell launch: leading switches, then
/// `/s /c "<command>"`. With `/s` cmd strips exactly the outer quotes added here;
/// without it cmd may strip the quotes of `"C:\a b\x.exe" "y"` and split the path.
pub fn command_shell_line(arguments: &[OsString]) -> Result<OsString, String> {
    let mut joined = String::new();
    for argument in arguments {
        if !joined.is_empty() {
            joined.push(' ');
        }
        joined.push_str(argument.to_str().ok_or("Shell commands must be text")?);
    }
    if joined.contains(['\0', '\r', '\n']) {
        return Err("Shell commands cannot contain line breaks".into());
    }
    let mut switches = String::new();
    let mut rest = joined.trim_start();
    let switch = loop {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let token = &rest[..end];
        if token.eq_ignore_ascii_case("/c") || token.eq_ignore_ascii_case("/k") {
            rest = rest[end..].trim_start();
            break token;
        }
        const SWITCHES: [&str; 13] = [
            "/a", "/u", "/q", "/d", "/x", "/y", "/s", "/e:on", "/e:off", "/f:on", "/f:off", "/v:on", "/v:off",
        ];
        let lower = token.to_ascii_lowercase();
        let known = SWITCHES.contains(&lower.as_str())
            || lower
                .strip_prefix("/t:")
                .is_some_and(|color| (1..=2).contains(&color.len()) && color.chars().all(|c| c.is_ascii_hexdigit()));
        if !known {
            return Err("Shell mode arguments must be cmd.exe switches followed by /c or /k and the command".into());
        }
        switches.push_str(token);
        switches.push(' ');
        rest = rest[end..].trim_start();
    };
    if rest.is_empty() {
        return Err("The shell command after /c or /k is empty".into());
    }
    let mut state = QuoteState::default();
    state.scan(rest, PlaceholderSafety::CommandShell);
    if state.open() {
        return Err("The shell command has an unbalanced quote or a trailing ^".into());
    }
    Ok(format!("{switches}/s {switch} \"{rest}\"").into())
}
/// Consent prompt text. Long values end in a visible truncation marker instead of
/// being cut silently, and control or bidi-override characters are shown escaped so
/// they cannot disguise what runs. Shell mode shows the exact cmd.exe command line.
pub fn consent_text(program: &Path, arguments: &[OsString], shell: bool) -> String {
    const MAX_ARGUMENTS: usize = 32;
    const MAX_CHARS: usize = 1024;
    let mut text = format!(
        "Run this {} command?\n\n{}\n\n",
        if shell { "shell" } else { "direct executable" },
        visible(&program.to_string_lossy(), MAX_CHARS)
    );
    if let Some(line) = shell.then(|| command_shell_line(arguments).ok()).flatten() {
        text.push_str("Command line passed to cmd.exe:\n");
        text.push_str(&visible(&line.to_string_lossy(), 4 * MAX_CHARS));
        return text;
    }
    text.push_str("Arguments (one per line):");
    for argument in arguments.iter().take(MAX_ARGUMENTS) {
        text.push('\n');
        if argument.is_empty() {
            text.push_str("(empty argument)");
        } else {
            text.push_str(&visible(&argument.to_string_lossy(), MAX_CHARS));
        }
    }
    if arguments.len() > MAX_ARGUMENTS {
        text.push_str(&format!(
            "\n… [truncated: {} more arguments not shown]",
            arguments.len() - MAX_ARGUMENTS
        ));
    }
    text
}
fn visible(value: &str, limit: usize) -> String {
    let mut output = String::new();
    for c in value.chars().take(limit) {
        if c.is_control() || matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}') {
            output.push_str(&format!("\\u{{{:04X}}}", c as u32));
        } else {
            output.push(c);
        }
    }
    let total = value.chars().count();
    if total > limit {
        output.push_str(&format!(" … [truncated: {} more characters not shown]", total - limit));
    }
    output
}

#[derive(Default)]
pub struct PlaceholderContext {
    pub file: Option<PathBuf>,
    pub workspace: Option<PathBuf>,
    pub selection: String,
    /// The selection, or the word at the caret when nothing is selected.
    pub word: String,
    /// Folder holding the Bareline executable.
    pub app_dir: Option<PathBuf>,
    pub line: u64,
    pub column: u64,
}
/// Notepad++ Run variables and the Bareline placeholders they map onto, so both
/// spellings share one expansion and escaping path.
pub const NOTEPAD_PLUS_PLUS_VARIABLES: [(&str, &str); 9] = [
    ("$(FULL_CURRENT_PATH)", "${file}"),
    ("$(CURRENT_DIRECTORY)", "${dir}"),
    ("$(FILE_NAME)", "${file_name}"),
    ("$(NAME_PART)", "${name_part}"),
    ("$(EXT_PART)", "${ext_part}"),
    ("$(CURRENT_WORD)", "${word}"),
    ("$(CURRENT_LINE)", "${line}"),
    ("$(CURRENT_COLUMN)", "${column}"),
    ("$(NPP_DIRECTORY)", "${app_dir}"),
];
/// Rewrites the Notepad++ variables above; any other `$(...)` text (a PowerShell
/// subexpression, for example) stays literal.
pub fn normalize_notepad_variables(template: &str) -> String {
    NOTEPAD_PLUS_PLUS_VARIABLES
        .iter()
        .fold(template.to_string(), |text, (variable, placeholder)| {
            text.replace(variable, placeholder)
        })
}
/// The word (letters, digits, `_`) touching byte `index` of `text`, as Notepad++'s
/// `$(CURRENT_WORD)` takes it when nothing is selected.
pub fn word_at(text: &str, index: usize) -> &str {
    let index = index.min(text.len());
    if !text.is_char_boundary(index) {
        return "";
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let start = text[..index]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_word(*c))
        .last()
        .map_or(index, |(start, _)| start);
    let end = text[index..]
        .char_indices()
        .find(|(_, c)| !is_word(*c))
        .map_or(text.len(), |(end, _)| index + end);
    &text[start..end]
}
/// Programs that re-parse their command line as code, recognized by the executable's
/// file name (a renamed copy is not recognized). The list is not exhaustive: runtimes
/// such as `python -c` or `node -e` run only the code argument the template writes, so
/// a value passed as a separate argument stays data there, but a template that puts a
/// value inside that code argument is not protected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpreter {
    /// `cmd.exe` and the batch files it runs.
    CommandShell,
    /// Windows PowerShell and PowerShell 7.
    PowerShell,
    /// Hosts whose arguments can name script or entry points Bareline cannot quote
    /// for (`wscript`, `cscript`, `mshta`, `rundll32`), launchers that hand their
    /// command line to another shell (`wsl`, `bash`, `conhost`, `forfiles`), and 8.3
    /// aliases that could hide any interpreter.
    ScriptHost,
}
impl Interpreter {
    pub fn of(program: &Path) -> Option<Self> {
        Self::named(&executable_name(program), true)
    }
    /// `name` as [`executable_name`] returns it. An 8.3 alias counts only when `program`
    /// says the name is used as a program, so text such as `HEAD~1` stays plain.
    fn named(name: &str, program: bool) -> Option<Self> {
        let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
        if stem == "cmd" || matches!(extension, "bat" | "cmd") {
            Some(Self::CommandShell)
        } else if matches!(stem, "powershell" | "powershell_ise") || stem.starts_with("pwsh") {
            Some(Self::PowerShell)
        } else if matches!(
            stem,
            "wscript" | "cscript" | "mshta" | "rundll32" | "wsl" | "bash" | "conhost" | "forfiles"
        ) || (program && stem.contains('~'))
        {
            Some(Self::ScriptHost)
        } else {
            None
        }
    }
}
/// The lower-case file name Windows runs for `program`.
fn executable_name(program: &Path) -> String {
    // Split on both separators so Windows paths classify the same on every host.
    let path = program.to_string_lossy().to_ascii_lowercase();
    let name = path.rsplit(['\\', '/']).next().unwrap_or_default();
    // `C:cmd.exe` names cmd.exe relative to drive C.
    let name = match name.as_bytes() {
        [_, b':', ..] => &name[2..],
        _ => name,
    };
    // Windows ignores trailing dots and spaces and a `::$DATA` stream suffix.
    name.split(':')
        .next()
        .unwrap_or_default()
        .trim_end_matches(['.', ' '])
        .to_string()
}
/// A batch file (not cmd.exe itself): cmd.exe hands it values as quoted arguments.
fn is_batch_file(program: &Path) -> bool {
    executable_name(program)
        .rsplit_once('.')
        .is_some_and(|(stem, extension)| stem != "cmd" && matches!(extension, "bat" | "cmd"))
}
/// Whether `name`, one word of command text, may name a program that re-parses its
/// arguments as code. `program` marks a word in a place that names a program; a word
/// with a path separator or a `.exe`/`.com` extension counts as one anywhere.
/// `batch_quoted`: a batch file receives cmd.exe-quoted values safely (shell mode).
fn names_code_runner(name: &str, program: bool, batch_quoted: bool) -> bool {
    if name.is_empty() {
        return false;
    }
    let path = Path::new(name);
    let executable = executable_name(path);
    let program = program
        || name.contains(['\\', '/'])
        || executable
            .rsplit_once('.')
            .is_some_and(|(_, extension)| matches!(extension, "exe" | "com"));
    match Interpreter::named(&executable, program) {
        Some(Interpreter::CommandShell) => !(batch_quoted && is_batch_file(path)),
        Some(_) => true,
        None => false,
    }
}
/// Whether a word of a `cmd.exe` command line may start a program that re-parses its
/// arguments as code (another cmd.exe, PowerShell, a script host). `%NAME%` references
/// are expanded as cmd.exe would; a reference Bareline cannot evaluate, a `!` (delayed
/// expansion) or expanded metacharacters count as an interpreter.
fn shell_word_runs_code(word: &str, program: bool) -> bool {
    if word.is_empty() {
        return false;
    }
    let Some(expanded) = expand_environment(word) else {
        return true;
    };
    if expanded.contains(['"', '&', '|', '<', '>', '^', '!']) {
        return true;
    }
    std::iter::once(expanded.as_str())
        .chain(expanded.split(|c: char| c.is_whitespace() || is_command_shell_break(c)))
        .any(|name| names_code_runner(name, program, true))
}
/// Whether a word of PowerShell command text may hand a value to code that PowerShell
/// quoting does not protect: a native interpreter or batch file (PowerShell passes a
/// value without spaces to it unquoted), the `--%` stop-parsing token, `Invoke-Expression`
/// or a script block built from text, `$env:ComSpec`, or a program named by a variable
/// after `&`, `.` or `Start-Process` (`invoked`). A `.ps1` script runs inside PowerShell
/// and receives values as literal parameters.
fn powershell_word_runs_code(word: &str, program: bool, invoked: bool) -> bool {
    let lower = word.to_ascii_lowercase();
    let name = executable_name(Path::new(word));
    if lower == "--%"
        || matches!(name.as_str(), "iex" | "invoke-expression")
        || ["scriptblock", "invokescript", "comspec"]
            .iter()
            .any(|code| lower.contains(*code))
        || (invoked && word.contains('$'))
    {
        return true;
    }
    std::iter::once(word)
        .chain(word.split(char::is_whitespace))
        .filter(|name| !executable_name(Path::new(name)).ends_with(".ps1"))
        .any(|name| names_code_runner(name, program, false))
}
/// Characters that end an unquoted word on a cmd.exe command line (`cmd/c` runs cmd).
fn is_command_shell_break(c: char) -> bool {
    matches!(c, ',' | ';' | '=' | '/' | '(' | ')' | '@' | '<' | '>' | '&' | '|')
}
/// `%NAME%` references replaced from this process's environment, as `cmd.exe /c` does
/// (an unset name stays literal). `None` when the text has a reference Bareline cannot
/// evaluate: a lone `%`, `%%`, or substring and substitution syntax.
fn expand_environment(text: &str) -> Option<String> {
    let mut output = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('%') {
        output.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after.find('%')?;
        let name = &after[..end];
        if name.is_empty() || name.contains([':', '=', '\0']) {
            return None;
        }
        match std::env::var(name) {
            Ok(value) => output.push_str(&value),
            Err(_) => output.push_str(&rest[start..start + end + 2]),
        }
        rest = &after[end + 1..];
    }
    output.push_str(rest);
    Some(output)
}
/// How an expanded placeholder value must be treated by the command interpreter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceholderSafety {
    /// Direct launch: values become argv entries, so no interpreter sees them.
    Argument,
    /// Shell launch: the expanded template becomes one `cmd.exe /c` string.
    CommandShell,
    /// PowerShell command text: values become single-quoted literals.
    PowerShell,
    /// An interpreter Bareline cannot quote for: placeholders are refused.
    Refused,
}
fn is_single_quote(c: char) -> bool {
    matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}')
}
fn is_double_quote(c: char) -> bool {
    matches!(c, '"' | '\u{201C}' | '\u{201D}' | '\u{201E}')
}
const SHELL_RUNS_CODE: &str = "This command starts, or may start, a program that runs its arguments as code (cmd.exe, a batch file, PowerShell or a script host), so Bareline cannot pass placeholder values to it safely. Run that program directly instead.";
/// Quote context of the command text assembled so far, so each value is quoted for
/// the place the template puts it in. Only template text is scanned, never values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct QuoteState {
    /// `'"'` or `'\''` while inside a quoted run.
    quote: Option<char>,
    /// The previous character was `^` (cmd) or `` ` `` (PowerShell).
    escape: bool,
    /// The previous character was an unescaped PowerShell `$`, which would join a
    /// following value into a variable name or a `$(...)` subexpression.
    dollar: bool,
    /// PowerShell `$(...)` subexpressions opened inside double quotes, innermost last,
    /// each with the count of its own `(` still open. Their text is code again.
    subexpressions: Vec<u32>,
    /// PowerShell text this scanner does not model (a `#` comment or a here-string);
    /// no placeholder may follow it.
    opaque: bool,
    /// The text of the word being read, without its quote characters.
    word: String,
    /// A word so far may start a program that re-parses its arguments.
    runs_code: bool,
    /// A placeholder value has been inserted.
    has_value: bool,
    /// The current command's program word has been read; later words are arguments.
    after_command: bool,
    /// `start`, `call` or `Start-Process` was read, so any later word of the current
    /// command may name the program it runs.
    launcher: bool,
    /// PowerShell: the word being read follows the `&` or `.` call operator.
    call: bool,
    /// cmd.exe: the word being read follows `/`, so it is a switch.
    switch: bool,
}
impl QuoteState {
    fn scan(&mut self, text: &str, safety: PlaceholderSafety) {
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            let dollar = std::mem::take(&mut self.dollar);
            if self.opaque {
                return;
            }
            if self.escape {
                self.escape = false;
                self.word.push(c);
                continue;
            }
            let next = chars.peek().copied();
            match (safety, self.quote) {
                (PlaceholderSafety::CommandShell, _) => self.scan_command_shell(c),
                (PlaceholderSafety::PowerShell, Some('\'')) => {
                    if !is_single_quote(c) {
                        self.word.push(c);
                    } else if next.is_some_and(is_single_quote) {
                        chars.next();
                        self.word.push(c);
                    } else {
                        self.quote = None;
                    }
                }
                (PlaceholderSafety::PowerShell, Some(_)) => {
                    if c == '`' {
                        self.escape = true;
                    } else if c == '(' && dollar {
                        // `"...$(...)..."` runs the parenthesized text as code.
                        self.subexpressions.push(0);
                        self.quote = None;
                        self.command_break(safety, false);
                    } else if is_double_quote(c) {
                        if next.is_some_and(is_double_quote) {
                            chars.next();
                            self.word.push(c);
                        } else {
                            self.quote = None;
                        }
                    } else {
                        self.dollar = c == '$';
                        self.word.push(c);
                    }
                }
                (PlaceholderSafety::PowerShell, None) => {
                    if c == '`' {
                        self.escape = true;
                    } else if c == '#' || (c == '@' && next.is_some_and(|n| is_single_quote(n) || is_double_quote(n))) {
                        self.opaque = true;
                    } else if is_single_quote(c) {
                        self.quote = Some('\'');
                    } else if is_double_quote(c) {
                        self.quote = Some('"');
                    } else if c == ')' && self.subexpressions.last() == Some(&0) {
                        // The subexpression ends; its enclosing double-quoted run resumes.
                        self.subexpressions.pop();
                        self.end_word(safety);
                        self.quote = Some('"');
                    } else {
                        if let Some(open) = self.subexpressions.last_mut() {
                            match c {
                                '(' => *open += 1,
                                ')' => *open -= 1,
                                _ => {}
                            }
                        }
                        self.dollar = c == '$';
                        self.scan_powershell_word(c);
                    }
                }
                (PlaceholderSafety::Argument | PlaceholderSafety::Refused, _) => {}
            }
        }
    }
    fn scan_command_shell(&mut self, c: char) {
        match (self.quote, c) {
            (_, '"') => self.quote = if self.quote.is_some() { None } else { Some('"') },
            (Some(_), c) => self.word.push(c),
            (None, '^') => self.escape = true,
            (None, '&' | '|' | '(') => self.command_break(PlaceholderSafety::CommandShell, false),
            (None, c) if c.is_whitespace() || is_command_shell_break(c) => {
                self.end_word(PlaceholderSafety::CommandShell);
                // `/c` is a switch; it does not name the command.
                self.switch = c == '/';
            }
            (None, c) => self.word.push(c),
        }
    }
    /// Unquoted PowerShell text outside strings.
    fn scan_powershell_word(&mut self, c: char) {
        match c {
            ';' | '|' | '&' | '(' | '{' | '=' => self.command_break(PlaceholderSafety::PowerShell, c == '&'),
            c if c.is_whitespace() || matches!(c, ')' | '}' | ',') => self.end_word(PlaceholderSafety::PowerShell),
            c => self.word.push(c),
        }
    }
    /// Ends the word; the next word names a new command (after `&` when `call`).
    fn command_break(&mut self, safety: PlaceholderSafety, call: bool) {
        self.end_word(safety);
        self.after_command = false;
        self.launcher = false;
        self.call = call;
    }
    /// Whether the word read so far may start a program that re-parses its arguments.
    fn word_runs_code(&self, safety: PlaceholderSafety) -> bool {
        let program = !self.after_command || self.launcher;
        match safety {
            PlaceholderSafety::CommandShell => shell_word_runs_code(&self.word, program),
            PlaceholderSafety::PowerShell => powershell_word_runs_code(&self.word, program, self.call || self.launcher),
            PlaceholderSafety::Argument | PlaceholderSafety::Refused => false,
        }
    }
    fn end_word(&mut self, safety: PlaceholderSafety) {
        let switch = std::mem::take(&mut self.switch);
        if self.word.is_empty() {
            return;
        }
        self.runs_code |= self.word_runs_code(safety);
        let word = std::mem::take(&mut self.word).to_ascii_lowercase();
        self.call = false;
        let powershell = safety == PlaceholderSafety::PowerShell;
        // Switches and parameters leave the command's program word ahead.
        if switch || (powershell && word.starts_with('-') && word != "--%") {
            return;
        }
        if powershell && word == "." && !self.after_command {
            // Dot-sourcing: the next word names what runs.
            self.call = true;
            return;
        }
        if !self.after_command {
            self.launcher = matches!(word.as_str(), "start" | "call" | "saps" | "start-process");
        }
        self.after_command = true;
    }
    /// Quote or escape state left open.
    fn open(&self) -> bool {
        self.quote.is_some() || self.escape || !self.subexpressions.is_empty()
    }
    fn finish(mut self, safety: PlaceholderSafety) -> Result<(), String> {
        self.end_word(safety);
        // A value piped or passed into an interpreter named later in the command.
        if self.runs_code && self.has_value {
            return Err(SHELL_RUNS_CODE.into());
        }
        // Text after a comment or here-string is not scanned, so it could pass a value on.
        if self.opaque && self.has_value {
            return Err("A placeholder cannot be used with a PowerShell comment (#) or here-string".into());
        }
        if matches!(safety, PlaceholderSafety::CommandShell | PlaceholderSafety::PowerShell)
            && self.open()
            && !self.opaque
        {
            return Err("The command template has an unbalanced quote or a trailing escape character".into());
        }
        Ok(())
    }
}
/// `cmd.exe` expands `%VAR%` (and `!VAR!` under delayed expansion) even inside quotes,
/// and a `"` inside a value would re-open the metacharacter context, so values carrying
/// them are refused. Everything else is literal inside double quotes, so values are
/// quoted unless they are plain path-like text. `before_quote`: a closing quote follows
/// the value, so its trailing backslashes must be doubled.
fn command_shell_value(text: &str, quoted: bool, before_quote: bool) -> Result<String, String> {
    if text.chars().any(|c| matches!(c, '"' | '%' | '!') || c.is_control()) {
        return Err("This name or selection contains characters cmd.exe cannot take literally (\" % ! or control characters). Run the program directly instead.".into());
    }
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '\\' | '.' | '-' | '_' | ':' | '/' | '+'));
    if plain && !quoted {
        return Ok(text.to_string());
    }
    // Doubled trailing backslashes keep the closing quote from reading as `\"` when the
    // program splits its command line (CommandLineToArgvW rules).
    let trailing = if before_quote {
        text.len() - text.trim_end_matches('\\').len()
    } else {
        0
    };
    let body = format!("{text}{}", "\\".repeat(trailing));
    Ok(if quoted { body } else { format!("\"{body}\"") })
}
/// PowerShell literal escaping: a single-quoted string (every single-quote character
/// doubled) outside quotes, doubled quotes inside a single-quoted run, and backtick
/// escapes inside a double-quoted (expandable) run.
fn powershell_value(text: &str, quote: Option<char>) -> Result<String, String> {
    if text.chars().any(|c| c == '"' || c.is_control()) {
        return Err("This name or selection contains characters PowerShell cannot take literally (\" or control characters). Run the program directly instead.".into());
    }
    let mut output = String::with_capacity(text.len() + 2);
    if quote.is_none() {
        output.push('\'');
    }
    for c in text.chars() {
        match quote {
            Some('"') if matches!(c, '$' | '`') || is_double_quote(c) => output.push('`'),
            Some('"') => {}
            _ if is_single_quote(c) => output.push(c),
            _ => {}
        }
        output.push(c);
    }
    if quote.is_none() {
        output.push('\'');
    }
    Ok(output)
}
/// `next`: the template character right after the placeholder, if any.
fn quote_value<'a>(
    value: Cow<'a, OsStr>,
    safety: PlaceholderSafety,
    state: &mut QuoteState,
    next: Option<char>,
) -> Result<Cow<'a, OsStr>, String> {
    let text = match safety {
        PlaceholderSafety::Argument => return Ok(value),
        PlaceholderSafety::Refused => {
            return Err("This program runs its arguments as script, so Bareline cannot pass placeholder values to it safely. Pass the value through a script file instead.".into());
        }
        PlaceholderSafety::CommandShell | PlaceholderSafety::PowerShell => value
            .to_str()
            .ok_or("Shell commands require text placeholder values; use direct mode")?,
    };
    if state.escape {
        return Err("A placeholder cannot follow an escape character (^ or `)".into());
    }
    if state.opaque {
        return Err("A placeholder cannot follow a PowerShell comment (#) or here-string".into());
    }
    if state.dollar {
        // `$` + `(calc)` would form a subexpression, `$` + `name` a variable.
        return Err("A placeholder cannot directly follow a PowerShell $".into());
    }
    // Quoting for one interpreter does not protect a value from another one it starts.
    if state.runs_code || state.word_runs_code(safety) {
        return Err(SHELL_RUNS_CODE.into());
    }
    state.has_value = true;
    let quoted = if safety == PlaceholderSafety::CommandShell {
        let quoted = state.quote.is_some();
        // Trailing backslashes matter only before the quote that closes the value. They
        // stay doubled for batch files too, which commonly forward `%1` to a program.
        command_shell_value(text, quoted, !quoted || next == Some('"'))?
    } else {
        powershell_value(text, state.quote)?
    };
    Ok(Cow::Owned(quoted.into()))
}
fn placeholder_value<'a>(name: &str, context: &'a PlaceholderContext) -> Result<Cow<'a, OsStr>, String> {
    let file = move || {
        context
            .file
            .as_deref()
            .ok_or_else(|| format!("Save the document before using ${{{name}}}"))
    };
    let file_name = move || {
        file()?
            .file_name()
            .ok_or_else(|| format!("The document has no file name for ${{{name}}}"))
    };
    Ok(match name {
        "file" => Cow::Borrowed(file()?.as_os_str()),
        "dir" => Cow::Borrowed(
            file()?
                .parent()
                .ok_or("Save the document before using ${dir}")?
                .as_os_str(),
        ),
        "file_name" => Cow::Borrowed(file_name()?),
        // Notepad++ splits at the last dot and keeps it on the extension (".txt").
        "name_part" | "ext_part" => {
            let text = file_name()?.to_str().ok_or("The file name is not valid Unicode")?;
            let dot = text.rfind('.').unwrap_or(text.len());
            Cow::Borrowed(OsStr::new(if name == "name_part" {
                &text[..dot]
            } else {
                &text[dot..]
            }))
        }
        "workspace" => Cow::Borrowed(context.workspace.as_ref().ok_or("No workspace is open")?.as_os_str()),
        "selection" => Cow::Borrowed(OsStr::new(context.selection.as_str())),
        "word" => Cow::Borrowed(OsStr::new(context.word.as_str())),
        "line" => Cow::Owned(context.line.to_string().into()),
        "column" => Cow::Owned(context.column.to_string().into()),
        "app_dir" => Cow::Borrowed(
            context
                .app_dir
                .as_ref()
                .ok_or("The Bareline folder is unavailable")?
                .as_os_str(),
        ),
        unknown => return Err(format!("Unknown external command placeholder: {unknown}")),
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
    let mut state = QuoteState::default();
    let output = expand(template, context, safety, &mut state)?;
    state.finish(safety)?;
    Ok(output)
}
fn expand(
    template: &str,
    context: &PlaceholderContext,
    safety: PlaceholderSafety,
    state: &mut QuoteState,
) -> Result<OsString, String> {
    fn append(output: &mut OsString, value: impl AsRef<OsStr>) -> Result<(), String> {
        let value = value.as_ref();
        if value.len() > 1024 * 1024 - output.len() {
            return Err("Expanded argument exceeds 1 MiB".into());
        }
        output.push(value);
        Ok(())
    }
    let mut output = OsString::new();
    let mut rest = template;
    while let Some(start) = rest.find("${") {
        append(&mut output, &rest[..start])?;
        state.scan(&rest[..start], safety);
        rest = &rest[start + 2..];
        let end = rest.find('}').ok_or("Unclosed external command placeholder")?;
        let value = placeholder_value(&rest[..end], context)?;
        rest = &rest[end + 1..];
        append(&mut output, quote_value(value, safety, state, rest.chars().next())?)?;
    }
    append(&mut output, rest)?;
    state.scan(rest, safety);
    Ok(output)
}
/// Index of a PowerShell `-File` parameter that precedes any command text. Arguments
/// after it reach the script as literal values; anything uncertain returns `None` so
/// every value is quoted as PowerShell code.
fn powershell_file_parameter(arguments: &[String]) -> Option<usize> {
    const VALUE_PARAMETERS: [&str; 10] = [
        "executionpolicy",
        "windowstyle",
        "psconsolefile",
        "outputformat",
        "inputformat",
        "configurationname",
        "workingdirectory",
        "settingsfile",
        "custompipename",
        "version",
    ];
    let mut index = 0;
    while index < arguments.len() {
        let name = arguments[index].strip_prefix(['-', '/'])?.to_ascii_lowercase();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        let abbreviates = |full: &str, shortest: usize| name.len() >= shortest && full.starts_with(name.as_str());
        if abbreviates("command", 1)
            || abbreviates("commandwithargs", 1)
            || abbreviates("encodedcommand", 1)
            || matches!(name.as_str(), "ec" | "cwa")
        {
            return None;
        }
        if abbreviates("file", 1) {
            return Some(index);
        }
        if VALUE_PARAMETERS.iter().any(|full| abbreviates(full, 2))
            || matches!(name.as_str(), "ep" | "w" | "wd" | "o" | "of" | "if" | "v")
        {
            // A value that looks like a parameter means the line is not what it seems.
            if arguments.get(index + 1)?.starts_with(['-', '/']) {
                return None;
            }
            index += 1;
        }
        index += 1;
    }
    None
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
    fn shell_mode_quotes_metacharacters_and_refuses_expansion_characters() {
        let context = PlaceholderContext {
            file: Some(PathBuf::from(r"C:\notes\a&calc.exe (x86)^;$`'.txt")),
            workspace: Some(PathBuf::from(r"C:\notes & tools\")),
            selection: "& calc | x < y > z".into(),
            ..Default::default()
        };
        let shell = |template: &str| expand_argument_for(template, &context, PlaceholderSafety::CommandShell);
        // Inside double quotes cmd.exe takes & | < > ^ ( ) ; $ ` ' literally.
        assert_eq!(
            shell("${file}").unwrap(),
            OsString::from("\"C:\\notes\\a&calc.exe (x86)^;$`'.txt\"")
        );
        assert_eq!(shell("${selection}").unwrap(), OsString::from("\"& calc | x < y > z\""));
        // A trailing backslash is doubled so the closing quote survives CommandLineToArgvW.
        assert_eq!(
            shell("--root=${workspace}").unwrap(),
            OsString::from("--root=\"C:\\notes & tools\\\\\"")
        );
        // Already inside the template's quotes: no second pair that would unquote the value.
        assert_eq!(
            shell("\"${selection}\"").unwrap(),
            OsString::from("\"& calc | x < y > z\"")
        );
        // Inside the template's quotes only a backslash run right before the closing quote
        // is doubled; text the template continues with keeps the path intact.
        assert_eq!(
            shell("\"${workspace}\"").unwrap(),
            OsString::from("\"C:\\notes & tools\\\\\"")
        );
        assert_eq!(
            shell("\"${workspace}out.txt\"").unwrap(),
            OsString::from("\"C:\\notes & tools\\out.txt\"")
        );
        // %VAR% and !VAR! expand even inside quotes, and a quote would end the quoted run.
        for selection in ["%PATH%", "a!b!", "a\"&calc", "line\nbreak", "tab\there"] {
            let context = PlaceholderContext {
                selection: selection.into(),
                ..Default::default()
            };
            assert!(
                expand_argument_for("${selection}", &context, PlaceholderSafety::CommandShell).is_err(),
                "{selection:?} must be refused in shell mode"
            );
        }
        // A caret would escape the opening quote; an unbalanced template has no safe context.
        assert!(shell("^${selection}").is_err());
        assert!(shell("\"${file}").is_err());
        // The same values stay literal for direct launches, which never reach a shell.
        assert_eq!(
            expand_argument("${selection}", &context).unwrap(),
            OsString::from("& calc | x < y > z")
        );
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
    fn powershell_values_are_single_quoted_literals() {
        let context = PlaceholderContext {
            file: Some(PathBuf::from(r"C:\a b\it's;calc $(x) `y`.ps1")),
            selection: "a'b; $(calc) `whoami` & | < > ^ % ! \u{2019}x".into(),
            ..Default::default()
        };
        let definition = |arguments: &[&str]| ExternalDefinition {
            name: "ps".into(),
            program: r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".into(),
            arguments: arguments.iter().map(|argument| argument.to_string()).collect(),
            shell: false,
            capture: true,
        };
        let arguments = |arguments: &[&str]| match definition(arguments).request(&context).map(|request| request.mode) {
            Ok(LaunchMode::Direct { arguments, .. }) => Ok(arguments),
            Ok(LaunchMode::Shell { .. }) => panic!("PowerShell must launch directly"),
            Err(error) => Err(error),
        };
        let quoted_selection = "'a''b; $(calc) `whoami` & | < > ^ % ! \u{2019}\u{2019}x'";
        assert_eq!(
            arguments(&["-NoProfile", "-Command", "Write-Output", "${selection}"]).unwrap()[3],
            OsString::from(quoted_selection)
        );
        assert_eq!(
            arguments(&["-c", "Get-Item", "${file}"]).unwrap()[2],
            OsString::from("'C:\\a b\\it''s;calc $(x) `y`.ps1'")
        );
        // Inside the template's single quotes only the quotes are doubled.
        assert_eq!(
            arguments(&["-c", "Write-Output 'x${selection}'"]).unwrap()[1],
            OsString::from("Write-Output 'xa''b; $(calc) `whoami` & | < > ^ % ! \u{2019}\u{2019}x'")
        );
        // Inside double quotes $ and ` are backtick-escaped.
        assert_eq!(
            arguments(&["-c", "\"${selection}\""]).unwrap()[1],
            OsString::from("\"a'b; `$(calc) ``whoami`` & | < > ^ % ! \u{2019}x\"")
        );
        // A quote opened in an earlier argument still applies after the space join.
        assert_eq!(
            arguments(&["-c", "echo \"", "${selection}", "\""]).unwrap()[2],
            OsString::from("a'b; `$(calc) ``whoami`` & | < > ^ % ! \u{2019}x")
        );
        // -File passes later arguments to the script as values, not code.
        assert_eq!(
            arguments(&["-ExecutionPolicy", "Bypass", "-File", "${file}", "${selection}"]).unwrap()[3..],
            [
                context.file.clone().unwrap().into_os_string(),
                OsString::from(context.selection.as_str())
            ]
        );
        // Anything that makes the -File reading uncertain falls back to quoting.
        assert_eq!(
            arguments(&["-v", "-c", "-f", "${selection}"]).unwrap()[3],
            OsString::from(quoted_selection)
        );
        assert_eq!(
            arguments(&["pwsh-script.ps1", "-File", "${selection}"]).unwrap()[2],
            quoted_selection
        );
        assert!(arguments(&["-c", "`${selection}"]).is_err());
        assert!(arguments(&["-c", "'${selection}"]).is_err());
        let quoted = PlaceholderContext {
            selection: "say \"hi\"".into(),
            ..Default::default()
        };
        assert!(definition(&["-c", "${selection}"]).request(&quoted).is_err());
    }
    #[test]
    fn powershell_subexpressions_in_double_quotes_are_code() {
        let context = PlaceholderContext {
            file: Some(PathBuf::from(r"C:\x\a'; calc; '.txt")),
            selection: "(calc)".into(),
            ..Default::default()
        };
        let command = |arguments: &[&str]| -> Result<OsString, String> {
            let definition = ExternalDefinition {
                name: "ps".into(),
                program: r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".into(),
                arguments: arguments.iter().map(|argument| argument.to_string()).collect(),
                shell: false,
                capture: true,
            };
            match definition.request(&context)?.mode {
                LaunchMode::Direct { arguments, .. } => Ok(arguments[1].clone()),
                LaunchMode::Shell { .. } => panic!("PowerShell must launch directly"),
            }
        };
        // `"$(...)"` is code: a value in its single quotes gets the quotes doubled.
        assert_eq!(
            command(&["-c", "Write-Output \"$(Get-Item '${file}')\""]).unwrap(),
            OsString::from("Write-Output \"$(Get-Item 'C:\\x\\a''; calc; ''.txt')\"")
        );
        // Unquoted inside the subexpression, it becomes a single-quoted literal.
        assert_eq!(
            command(&["-c", "\"$(Get-Item ${file})\""]).unwrap(),
            OsString::from("\"$(Get-Item 'C:\\x\\a''; calc; ''.txt')\"")
        );
        // After the matching `)` the double-quoted run resumes, with nested parentheses.
        let dollar = PlaceholderContext {
            selection: "$(calc)".into(),
            ..Default::default()
        };
        let definition = |argument: &str| ExternalDefinition {
            name: "ps".into(),
            program: r"C:\Program Files\PowerShell\7\pwsh.exe".into(),
            arguments: vec!["-c".into(), argument.into()],
            shell: false,
            capture: true,
        };
        assert!(matches!(
            definition("\"$((1 + 2)) ${selection}\"").request(&dollar).map(|request| request.mode),
            Ok(LaunchMode::Direct { ref arguments, .. }) if arguments[1] == "\"$((1 + 2)) `$(calc)\""
        ));
        // A value right after `$` would become a subexpression or variable name.
        assert!(command(&["-c", "\"$${selection}\""]).is_err());
        assert!(command(&["-c", "Write-Output $${selection}"]).is_err());
        // An unclosed subexpression has no safe context.
        assert!(command(&["-c", "\"$(Get-Item ${file}\""]).is_err());
        // Comments and here-strings are not modelled, so no value may follow them.
        assert!(command(&["-c", "Get-Date # it's", "${selection}"]).is_err());
        assert!(command(&["-c", "@\"\n${selection}\n\"@"]).is_err());
        assert!(command(&["-c", "Get-Date # note"]).is_ok());
    }
    #[test]
    fn shell_mode_refuses_values_for_interpreters_it_starts() {
        let context = PlaceholderContext {
            file: Some(PathBuf::from(r"C:\notes\a.txt")),
            selection: "a;calc".into(),
            word: "$(calc)".into(),
            ..Default::default()
        };
        let shell = |command: &str| {
            ExternalDefinition {
                name: "x".into(),
                program: r"C:\Windows\System32\cmd.exe".into(),
                arguments: vec!["/c".into(), command.into()],
                shell: true,
                capture: true,
            }
            .request(&context)
        };
        for refused in [
            "powershell -c Write-Output ${selection}",
            "cmd /c powershell -c Write-Output ${word}",
            "\"C:\\Program Files\\PowerShell\\7\\pwsh.exe\" -c ${selection}",
            "p^owershell -c ${selection}",
            "@(pwsh.exe -c ${selection})",
            "cmd/c echo ${selection}",
            "call cmd /c echo ${selection}",
            "start \"\" /b mshta ${file}",
            "wsl ls ${file}",
            "\"${file}\\..\\powershell.exe\" -c ${selection}",
            // The value reaches an interpreter named later in the pipeline.
            "echo ${selection} | powershell -c -",
            // Delayed or unparsable expansion could name any program.
            "!x! ${selection}",
            "%x ${selection}",
            // 8.3 aliases can hide an interpreter's name.
            "POWERS~1 -c ${selection}",
            "start \"\" /b POWERS~1 -c ${selection}",
            "C:\\Windows\\System32\\WINDOW~1\\v1.0\\POWERS~1 -c ${selection}",
        ] {
            assert_eq!(shell(refused).unwrap_err(), SHELL_RUNS_CODE, "{refused}");
        }
        #[cfg(windows)]
        assert_eq!(shell("%ComSpec% /c echo ${selection}").unwrap_err(), SHELL_RUNS_CODE);
        // Ordinary programs and batch files get the value as one quoted argument.
        for allowed in [
            "where ${selection}",
            "\"C:\\tools\\build.bat\" \"${file}\" ${selection}",
            "type \"${file}\" & echo done",
            "echo %bareline_unset_variable% ${selection}",
            // `~` marks an 8.3 alias only where a program is named.
            "git log HEAD~1 ${selection}",
            "start \"\" /b tool.exe ${selection}",
        ] {
            assert!(shell(allowed).is_ok(), "{allowed}");
        }
        // Without placeholders the command is entirely the user's own text.
        assert!(shell("powershell -c Get-Date").is_ok());
    }
    #[test]
    fn interpreters_are_detected_by_executable_name() {
        for (program, expected) in [
            (r"C:\Windows\System32\cmd.exe", Some(Interpreter::CommandShell)),
            (r"C:\Windows\System32\CMD.EXE. ", Some(Interpreter::CommandShell)),
            (r"C:\Windows\System32\cmd.exe::$DATA", Some(Interpreter::CommandShell)),
            (r"C:\tools\build.BAT", Some(Interpreter::CommandShell)),
            (r"C:\tools\build.cmd", Some(Interpreter::CommandShell)),
            (r"C:\Program Files\PowerShell\7\pwsh.exe", Some(Interpreter::PowerShell)),
            (
                r"C:\Windows\System32\WindowsPowerShell\v1.0\PowerShell.exe",
                Some(Interpreter::PowerShell),
            ),
            (
                r"C:\Windows\System32\WindowsPowerShell\v1.0\POWERS~1.EXE",
                Some(Interpreter::ScriptHost),
            ),
            (r"C:\Windows\System32\wscript.exe", Some(Interpreter::ScriptHost)),
            (r"C:\Windows\System32\cscript.exe", Some(Interpreter::ScriptHost)),
            (r"C:\Windows\System32\mshta.exe", Some(Interpreter::ScriptHost)),
            (r"C:\Windows\System32\rundll32.exe", Some(Interpreter::ScriptHost)),
            (r"C:\Windows\System32\wsl.exe", Some(Interpreter::ScriptHost)),
            (r"C:\Windows\System32\bash.exe", Some(Interpreter::ScriptHost)),
            (r"C:\Windows\System32\conhost.exe", Some(Interpreter::ScriptHost)),
            (r"C:\Windows\System32\forfiles.exe", Some(Interpreter::ScriptHost)),
            (r"C:\Windows\System32\where.exe", None),
            (r"C:\tools\cmdlet-runner.exe", None),
        ] {
            assert_eq!(Interpreter::of(Path::new(program)), expected, "{program}");
        }
        let context = PlaceholderContext {
            file: Some(PathBuf::from(r"C:\notes\a.txt")),
            ..Default::default()
        };
        let definition = |program: &str, argument: &str| ExternalDefinition {
            name: "x".into(),
            program: program.into(),
            arguments: vec!["/c".into(), argument.into()],
            shell: false,
            capture: true,
        };
        // cmd.exe as a direct program would re-parse argv as commands.
        assert!(
            definition(r"C:\Windows\System32\cmd.exe", "dir")
                .request(&context)
                .is_err()
        );
        // Script hosts accept literal arguments but no document text.
        assert!(
            definition(r"C:\Windows\System32\mshta.exe", "${file}")
                .request(&context)
                .is_err()
        );
        assert!(
            definition(r"C:\Windows\System32\rundll32.exe", "x")
                .request(&context)
                .is_ok()
        );
        let direct = ProcessRequest {
            mode: LaunchMode::Direct {
                program: PathBuf::from(r"C:\Windows\System32\cmd.exe"),
                arguments: vec!["/c".into(), "dir".into()],
            },
            directory: None,
            capture: true,
        };
        // Name checks run before the absolute-path check, so this holds on every host.
        assert_eq!(
            validate_request(&direct).unwrap_err(),
            "cmd.exe and batch files run only in shell mode"
        );
        let shell = |program: PathBuf| ProcessRequest {
            mode: LaunchMode::Shell {
                program,
                arguments: vec!["/c".into(), "dir".into()],
            },
            directory: None,
            capture: true,
        };
        const PINNED: &str = "Shell mode runs only %SystemRoot%\\System32\\cmd.exe";
        assert_eq!(
            validate_request(&shell(PathBuf::from(r"C:\Program Files\PowerShell\7\pwsh.exe"))).unwrap_err(),
            PINNED
        );
        // Shell mode exists only on Windows (elsewhere every shell request is refused), so
        // the accepting side of the pin is checked there; `same_windows_path` is tested on
        // every host in `shell_mode_is_pinned_to_system_cmd_with_a_raw_command_line`.
        #[cfg(windows)]
        {
            assert!(validate_request(&shell(system_command_shell().unwrap())).is_ok());
            assert_eq!(
                validate_request(&shell(PathBuf::from(r"C:\tools\cmd.exe"))).unwrap_err(),
                PINNED
            );
        }
    }
    #[test]
    fn powershell_refuses_values_for_interpreters_it_starts() {
        let context = PlaceholderContext {
            file: Some(PathBuf::from(r"C:\notes\a&calc&b.txt")),
            selection: "a;calc".into(),
            ..Default::default()
        };
        let request = |arguments: &[&str]| {
            ExternalDefinition {
                name: "ps".into(),
                program: r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe".into(),
                arguments: arguments.iter().map(|argument| argument.to_string()).collect(),
                shell: false,
                capture: true,
            }
            .request(&context)
        };
        // PowerShell hands `C:\notes\a&calc&b.txt` to a native program without cmd.exe
        // quoting, so a batch file or cmd.exe would run `calc` (SEC-10, one hop away).
        for refused in [
            &["-NoProfile", "-Command", "&", r"'C:\tools\build.bat'", "${file}"][..],
            &["-c", "cmd /c type ${file}"],
            &["-c", r#"& "C:\Windows\System32\cmd.exe" /c type ${file}"#],
            // The value reaches the interpreter later in the pipeline or through a variable.
            &["-c", "Write-Output ${selection} | cmd /c more"],
            &["-c", r"$v = ${file}; & 'C:\tools\build.cmd' $v"],
            &["-c", "Start-Process powershell -ArgumentList ${selection}"],
            &["-c", "pwsh -c ${selection}"],
            &["-c", "wsl ls ${file}"],
            &["-c", r"& 'C:\Windows\System32\WINDOW~1\v1.0\POWERS~1' -c ${selection}"],
            // Stop-parsing passes the text on verbatim, before or after the value.
            &["-c", "tool.exe --% ${file}"],
            &["-c", "tool.exe ${file} --% x"],
            // Text evaluated as PowerShell code again.
            &["-c", "iex ${selection}"],
            &["-c", "Invoke-Expression ${selection}"],
            &["-c", "& ([scriptblock]::Create(${selection}))"],
            // A program chosen at run time.
            &["-c", "& $env:ComSpec /c type ${file}"],
            &["-c", "$p = 'x'; & $p ${file}"],
        ] {
            assert_eq!(request(refused).unwrap_err(), SHELL_RUNS_CODE, "{refused:?}");
        }
        // Text after a comment is not scanned, so it could pass the value on.
        assert!(request(&["-c", "$v = ${file} # x"]).is_err());
        for allowed in [
            &["-c", "Get-Content -LiteralPath ${file}"][..],
            &["-c", r"& 'C:\tools\tool.exe' ${file}"],
            &["-c", "git log HEAD~1 ${selection}"],
            &["-c", "$v = ${file}; Write-Output $v"],
            &["-File", r"C:\tools\build.ps1", "${file}"],
        ] {
            assert!(request(allowed).is_ok(), "{allowed:?}");
        }
        // Without placeholders the command is entirely the user's own text.
        assert!(request(&["-c", "cmd /c ver"]).is_ok());
    }
    #[test]
    fn shell_mode_is_pinned_to_system_cmd_with_a_raw_command_line() {
        let shell = command_shell_under(OsStr::new(r"C:\WINDOWS\")).unwrap();
        assert_eq!(shell, PathBuf::from(r"C:\WINDOWS\System32\cmd.exe"));
        assert!(same_windows_path(Path::new("c:/windows/system32/CMD.exe"), &shell));
        assert!(!same_windows_path(Path::new(r"C:\tools\cmd.exe"), &shell));
        assert!(command_shell_under(OsStr::new("")).is_none());
        let line = |arguments: &[&str]| {
            command_shell_line(&arguments.iter().map(OsString::from).collect::<Vec<_>>())
                .map(|line| line.into_string().unwrap())
        };
        assert_eq!(
            line(&["/c", "\"C:\\a b\\tool.exe\"", "\"C:\\x & y\\z.txt\""]).unwrap(),
            "/s /c \"\"C:\\a b\\tool.exe\" \"C:\\x & y\\z.txt\"\""
        );
        assert_eq!(line(&["/d /C type x.txt"]).unwrap(), "/d /s /C \"type x.txt\"");
        assert_eq!(line(&["/v:off", "/k", "ver"]).unwrap(), "/v:off /s /k \"ver\"");
        for refused in [
            &["tool"][..],
            &["/c"],
            &["/c", "echo \"x"],
            &["/c", "echo x^"],
            &["/c", "echo a\nb"],
            &["/c\"x\"", "y"],
            &["\"/c\"", "y"],
        ] {
            assert!(line(refused).is_err(), "{refused:?}");
        }
    }
    #[test]
    fn consent_text_marks_truncation_and_escapes_invisible_characters() {
        let long = OsString::from("x".repeat(5000));
        let mut arguments = vec![long, OsString::new(), OsString::from("a\u{202E}b\nc")];
        arguments.extend((0..40).map(|index| OsString::from(index.to_string())));
        let text = consent_text(Path::new(r"C:\tools\tool.exe"), &arguments, false);
        assert!(text.contains(r"C:\tools\tool.exe"));
        assert!(text.contains(&format!(
            "{} … [truncated: 3976 more characters not shown]",
            "x".repeat(1024)
        )));
        assert!(text.contains("(empty argument)"));
        assert!(text.contains(r"a\u{202E}b\u{000A}c"));
        assert!(text.contains("… [truncated: 11 more arguments not shown]"));
        let text = consent_text(
            Path::new(r"C:\Windows\System32\cmd.exe"),
            &[OsString::from("/c"), OsString::from("dir \"C:\\a b\"")],
            true,
        );
        assert!(text.contains("Command line passed to cmd.exe:\n/s /c \"dir \"C:\\a b\"\""));
    }
    #[test]
    fn notepad_plus_plus_variables_share_the_placeholder_rules() {
        let normalized = normalize_notepad_variables(
            "$(FULL_CURRENT_PATH)|$(CURRENT_DIRECTORY)|$(FILE_NAME)|$(NAME_PART)|$(EXT_PART)|$(CURRENT_WORD)|$(CURRENT_LINE)|$(CURRENT_COLUMN)|$(NPP_DIRECTORY)|$(Get-Date)",
        );
        assert_eq!(
            normalized,
            "${file}|${dir}|${file_name}|${name_part}|${ext_part}|${word}|${line}|${column}|${app_dir}|$(Get-Date)"
        );
        // Native separators, so path splitting behaves the same on every test host.
        let file = Path::new("src").join("main.test.rs");
        let context = PlaceholderContext {
            file: Some(file.clone()),
            word: "needle".into(),
            app_dir: Some(PathBuf::from("Bareline app")),
            line: 12,
            column: 4,
            ..Default::default()
        };
        assert_eq!(
            expand_argument(&normalized, &context).unwrap(),
            OsString::from(format!(
                "{}|src|main.test.rs|main.test|.rs|needle|12|4|Bareline app|$(Get-Date)",
                file.display()
            ))
        );
        let dotfile = PlaceholderContext {
            file: Some(Path::new("src").join(".gitignore")),
            ..Default::default()
        };
        assert_eq!(
            expand_argument("[${name_part}][${ext_part}]", &dotfile).unwrap(),
            OsString::from("[][.gitignore]")
        );
        assert!(expand_argument("${file_name}", &PlaceholderContext::default()).is_err());
        assert!(expand_argument("${app_dir}", &PlaceholderContext::default()).is_err());
        // Same escaping as the Bareline spelling.
        let word = PlaceholderContext {
            word: "%PATH%".into(),
            ..Default::default()
        };
        assert!(
            expand_argument_for(
                &normalize_notepad_variables("$(CURRENT_WORD)"),
                &word,
                PlaceholderSafety::CommandShell
            )
            .is_err()
        );
        assert_eq!(word_at("let needle_2 = 1;", 6), "needle_2");
        assert_eq!(word_at("let needle_2 = 1;", 12), "needle_2");
        assert_eq!(word_at("let needle_2 = 1;", 13), "");
        assert_eq!(word_at("héllo wörld", 7), "wörld");
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
    #[cfg(windows)]
    #[test]
    fn shell_launches_pass_the_prepared_line_through_the_launcher_or_fail() {
        /// Records the raw line it is handed, then refuses to start anything.
        #[derive(Default)]
        struct RecordingLauncher(Mutex<Vec<OsString>>);
        impl ProcessLauncher for RecordingLauncher {
            fn spawn(&self, _: &mut Command) -> io::Result<(Child, Box<dyn ProcessTreeGuard>)> {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "fixture rejects launch",
                ))
            }
            fn set_raw_command_line(&self, _: &mut Command, line: &OsStr) -> io::Result<()> {
                self.0.lock().unwrap().push(line.to_os_string());
                Ok(())
            }
        }
        let arguments = vec![OsString::from("/c"), OsString::from("dir \"C:\\a b\"")];
        let request = ProcessRequest {
            mode: LaunchMode::Shell {
                program: system_command_shell().unwrap(),
                arguments: arguments.clone(),
            },
            directory: None,
            capture: true,
        };
        let finished = |handle: &ProcessHandle| {
            let deadline = Instant::now() + Duration::from_secs(2);
            while handle.state() == ProcessState::Starting && Instant::now() < deadline {
                thread::yield_now();
            }
            handle.state()
        };
        let recording = Arc::new(RecordingLauncher::default());
        let launcher: Arc<dyn ProcessLauncher> = recording.clone();
        let handle = launch(request.clone(), ProcessPermission::UserGrantedShell, launcher, 1024).unwrap();
        assert!(matches!(finished(&handle), ProcessState::Failed(reason) if reason.contains("fixture rejects")));
        assert_eq!(
            recording.0.lock().unwrap()[..],
            [command_shell_line(&arguments).unwrap()]
        );
        // A launcher without raw command lines fails the launch instead of quoting argv.
        let handle = launch(
            request,
            ProcessPermission::UserGrantedShell,
            Arc::new(DeniedLauncher),
            1024,
        )
        .unwrap();
        assert!(matches!(finished(&handle), ProcessState::Failed(reason) if reason.contains("raw shell command line")));
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
        let program = PathBuf::from(&self.program);
        // Interpreters are detected by executable name: each gets its own quoting, or
        // none of the document's text at all (SEC-10).
        let safety = if self.shell {
            PlaceholderSafety::CommandShell
        } else {
            match Interpreter::of(&program) {
                Some(Interpreter::CommandShell) => return Err("cmd.exe and batch files run only in shell mode".into()),
                Some(Interpreter::PowerShell) => PlaceholderSafety::PowerShell,
                Some(Interpreter::ScriptHost) => PlaceholderSafety::Refused,
                None => PlaceholderSafety::Argument,
            }
        };
        // Arguments after `-File <script>` reach the script as values, not PowerShell code.
        let literal_after = if safety == PlaceholderSafety::PowerShell {
            powershell_file_parameter(&self.arguments)
        } else {
            None
        };
        // The interpreter sees the arguments joined by spaces, so quote context carries over.
        let mut state = QuoteState::default();
        for (index, template) in self.arguments.iter().enumerate() {
            let safety = if literal_after.is_some_and(|file| index > file) {
                PlaceholderSafety::Argument
            } else {
                safety
            };
            let argument = expand(template, context, safety, &mut state)?;
            state.scan(" ", safety);
            budget = budget.checked_add(argument.len()).ok_or("Argument budget exceeded")?;
            if budget > 1024 * 1024 {
                return Err("Expanded arguments exceed 1 MiB".into());
            }
            arguments.push(argument);
        }
        state.finish(safety)?;
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
