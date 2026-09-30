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
                Some(line) => append_raw_command_line(&mut command, line),
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

/// Appends the prepared `cmd.exe` command line verbatim. Shell mode is refused before
/// launch on other platforms, so the fallback is never reached there.
fn append_raw_command_line(command: &mut Command, line: &OsStr) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.raw_arg(line);
    }
    #[cfg(not(windows))]
    command.arg(line);
}
/// Launch policy, checked before the consent prompt and again at launch.
pub fn validate_request(request: &ProcessRequest) -> Result<(), String> {
    let (program, arguments) = match &request.mode {
        LaunchMode::Direct { program, arguments } | LaunchMode::Shell { program, arguments } => (program, arguments),
    };
    if !program.is_absolute() {
        return Err("External program must be an absolute configured path".into());
    }
    if arguments.len() > 4096 || arguments.iter().map(|argument| argument.len()).sum::<usize>() > 1024 * 1024 {
        return Err("External argument budget exceeded".into());
    }
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
    if QuoteState::default().scan(rest, PlaceholderSafety::CommandShell) != QuoteState::default() {
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
/// file name (a renamed copy is not recognized).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpreter {
    /// `cmd.exe` and the batch files it runs.
    CommandShell,
    /// Windows PowerShell and PowerShell 7.
    PowerShell,
    /// Hosts whose arguments can name script or entry points Bareline cannot quote
    /// for (`wscript`, `cscript`, `mshta`, `rundll32`), and 8.3 aliases that could
    /// hide any interpreter.
    ScriptHost,
}
impl Interpreter {
    pub fn of(program: &Path) -> Option<Self> {
        // Split on both separators so Windows paths classify the same on every host.
        let path = program.to_string_lossy().to_ascii_lowercase();
        let name = path.rsplit(['\\', '/']).next().unwrap_or_default();
        // `C:cmd.exe` names cmd.exe relative to drive C.
        let name = match name.as_bytes() {
            [_, b':', ..] => &name[2..],
            _ => name,
        };
        // Windows ignores trailing dots and spaces and a `::$DATA` stream suffix.
        let name = name.split(':').next().unwrap_or_default().trim_end_matches(['.', ' ']);
        let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
        if stem == "cmd" || matches!(extension, "bat" | "cmd") {
            Some(Self::CommandShell)
        } else if matches!(stem, "powershell" | "powershell_ise") || stem.starts_with("pwsh") {
            Some(Self::PowerShell)
        } else if matches!(stem, "wscript" | "cscript" | "mshta" | "rundll32") || stem.contains('~') {
            Some(Self::ScriptHost)
        } else {
            None
        }
    }
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
/// Quote context of the command text assembled so far, so each value is quoted for
/// the place the template puts it in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct QuoteState {
    /// `'"'` or `'\''` while inside a quoted run.
    quote: Option<char>,
    /// The previous character was `^` (cmd) or `` ` `` (PowerShell).
    escape: bool,
}
impl QuoteState {
    fn scan(mut self, text: &str, safety: PlaceholderSafety) -> Self {
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            if self.escape {
                self.escape = false;
                continue;
            }
            match (safety, self.quote) {
                (PlaceholderSafety::CommandShell, None) if c == '^' => self.escape = true,
                (PlaceholderSafety::CommandShell, None) if c == '"' => self.quote = Some('"'),
                (PlaceholderSafety::CommandShell, Some(_)) if c == '"' => self.quote = None,
                (PlaceholderSafety::PowerShell, None | Some('"')) if c == '`' => self.escape = true,
                (PlaceholderSafety::PowerShell, None) if is_single_quote(c) => self.quote = Some('\''),
                (PlaceholderSafety::PowerShell, None) if is_double_quote(c) => self.quote = Some('"'),
                (PlaceholderSafety::PowerShell, Some('\'')) if is_single_quote(c) => {
                    if chars.peek().copied().is_some_and(is_single_quote) {
                        chars.next();
                    } else {
                        self.quote = None;
                    }
                }
                (PlaceholderSafety::PowerShell, Some('"')) if is_double_quote(c) => {
                    if chars.peek().copied().is_some_and(is_double_quote) {
                        chars.next();
                    } else {
                        self.quote = None;
                    }
                }
                _ => {}
            }
        }
        self
    }
    fn finish(self, safety: PlaceholderSafety) -> Result<(), String> {
        if matches!(safety, PlaceholderSafety::CommandShell | PlaceholderSafety::PowerShell) && self != Self::default()
        {
            return Err("The command template has an unbalanced quote or a trailing escape character".into());
        }
        Ok(())
    }
}
/// `cmd.exe` expands `%VAR%` (and `!VAR!` under delayed expansion) even inside quotes,
/// and a `"` inside a value would re-open the metacharacter context, so values carrying
/// them are refused. Everything else is literal inside double quotes, so values are
/// quoted unless they are plain path-like text.
fn command_shell_value(text: &str, quoted: bool) -> Result<String, String> {
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
    let trailing = text.len() - text.trim_end_matches('\\').len();
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
fn quote_value(value: Cow<'_, OsStr>, safety: PlaceholderSafety, state: QuoteState) -> Result<Cow<'_, OsStr>, String> {
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
    let quoted = if safety == PlaceholderSafety::CommandShell {
        command_shell_value(text, state.quote.is_some())?
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
        *state = state.scan(&rest[..start], safety);
        rest = &rest[start + 2..];
        let end = rest.find('}').ok_or("Unclosed external command placeholder")?;
        let value = placeholder_value(&rest[..end], context)?;
        append(&mut output, quote_value(value, safety, *state)?)?;
        rest = &rest[end + 1..];
    }
    append(&mut output, rest)?;
    *state = state.scan(rest, safety);
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
        assert!(validate_request(&direct).is_err());
        let shell = ProcessRequest {
            mode: LaunchMode::Shell {
                program: PathBuf::from(r"C:\Program Files\PowerShell\7\pwsh.exe"),
                arguments: vec!["/c".into(), "dir".into()],
            },
            directory: None,
            capture: true,
        };
        assert!(validate_request(&shell).is_err());
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
            state = state.scan(" ", safety);
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
