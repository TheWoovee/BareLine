// SPDX-License-Identifier: MPL-2.0
//! The extension host's macOS isolation as data: the sandbox profile, the
//! resource limits taken from the execution budget, the scrubbed environment
//! and the launch chain. `MacIsolation` (macOS only) spawns it.
//!
//! Launch chain. `posix_spawn` starts `/bin/sh` with only `PATH` in its
//! environment, every descriptor but standard input, output and error closed
//! (`POSIX_SPAWN_CLOEXEC_DEFAULT`, the three opened on `/dev/null`) and its own
//! process group. The shell runs one fixed script that takes its data only as
//! positional parameters: it lowers the resource limits with `ulimit` and
//! replaces itself with `/usr/bin/sandbox-exec -p <profile> <runtime> <args>`.
//! `sandbox-exec` applies the profile with `sandbox_init(3)` and then execs the
//! runtime, so the host's first instruction already runs confined and keeps
//! the spawned pid. If any step fails, the host is not started.
//!
//! Why this shape: `sandbox_init` confines the process that calls it, so the
//! editor cannot call it for a child, and `posix_spawn` has no hook between
//! spawn and exec where the child could call it or `setrlimit`. `sandbox-exec`
//! is Apple's front end for `sandbox_init`. Both are marked deprecated but
//! remain functional through current macOS releases and are what Bazel, Nix
//! and the Swift package manager use; the readiness probe
//! ([`probe_profile`]) runs the same chain on `/usr/bin/true` first, so a
//! system where they stop working reports isolation as Unsupported instead of
//! running the host unconfined.
//!
//! The profile denies everything by default, imports Apple's `system.sb`
//! baseline (a few system services and devices), and allows only: reading
//! the operating system's own files ([`SYSTEM_READABLE`]: the dyld shared
//! cache, system libraries and frameworks, the random devices), looking up the
//! folders on the way to the runtime and the granted files, executing and
//! reading the runtime, reading the granted files (the verified component),
//! and connecting to the transport socket when one is named. There is no
//! network, no file writing, no reading of the person's files and no `fork`.
//! Memory is not capped: macOS does not
//! enforce `RLIMIT_AS` or `RLIMIT_DATA`; the budget's wall-clock watchdog and
//! the CPU limit bound a runaway host instead.
use bareline_extensions_protocol::ExecutionBudget;
use bareline_platform_posix::extension_transport::HostSpawn;
use std::{
    ffi::OsString,
    io,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub const SHELL: &str = "/bin/sh";
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
pub const PROBE_PROGRAM: &str = "/usr/bin/true";
/// How the host is confined, as the Extensions page and the launch report it
/// (`isolation=sandbox_init`): `sandbox-exec` applies the profile with it.
pub const MECHANISM: &str = "sandbox_init";
/// The script's exit status when a resource limit could not be set.
pub const LIMITS_FAILED: i32 = 125;
/// `$0` of the launch script, as seen in process listings until the exec.
const SCRIPT_NAME: &str = "bareline-isolation";
/// Operating-system locations any process may read: no user data lives
/// there, and dyld needs them to start the runtime (the shared cache is under
/// /System/Cryptexes since macOS 13, /private/var/db/dyld before).
pub const SYSTEM_READABLE: [&str; 5] = [
    "/System",
    "/usr/lib",
    "/usr/share",
    "/Library/Apple",
    "/private/var/db/dyld",
];
const DEVICES: [&str; 3] = ["/dev/null", "/dev/random", "/dev/urandom"];

#[derive(Clone, Debug)]
pub struct IsolationRequest {
    /// The extension host executable, already verified by hash.
    pub runtime: PathBuf,
    pub arguments: Vec<OsString>,
    /// Files the host may read besides the runtime: the verified component.
    pub readable: Vec<PathBuf>,
    /// The Unix-domain socket the host connects to, when the transport uses a path.
    pub socket: Option<PathBuf>,
    pub budget: ExecutionBudget,
}

/// The request for a verified host launch of the shared Unix transport. The
/// sandbox matches real paths (`/var/folders` is `/private/var/folders`), so
/// the runtime and the transport socket are named with their links resolved;
/// the component is granted under both its spelling, which the host opens,
/// and its real path. The resolved runtime must still be the file that was
/// hashed, since macOS cannot launch from the verified descriptor.
pub fn host_request(spawn: &HostSpawn<'_>) -> io::Result<IsolationRequest> {
    let runtime = std::fs::canonicalize(spawn.executable)?;
    let (named, verified) = (std::fs::metadata(&runtime)?, spawn.executable_file.metadata()?);
    if (named.dev(), named.ino()) != (verified.dev(), verified.ino()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the extension host changed after it was verified",
        ));
    }
    let component = std::fs::canonicalize(spawn.component)?;
    let mut readable = vec![spawn.component.to_path_buf()];
    if component != spawn.component {
        readable.push(component);
    }
    let socket = match spawn.socket {
        Some(socket) => {
            let (Some(folder), Some(name)) = (socket.parent(), socket.file_name()) else {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the transport socket has no folder",
                ));
            };
            Some(std::fs::canonicalize(folder)?.join(name))
        }
        None => None,
    };
    Ok(IsolationRequest {
        runtime,
        arguments: spawn.arguments.clone(),
        readable,
        socket,
        budget: spawn.budget,
    })
}

/// Limits applied with `ulimit` before the sandbox is entered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceLimits {
    /// `RLIMIT_CPU`: CPU seconds before SIGXCPU.
    pub cpu_seconds: u64,
    /// `RLIMIT_NOFILE`.
    pub open_files: u64,
    /// `RLIMIT_FSIZE`, in 512-byte blocks as `ulimit -f` counts them. The host
    /// writes no files; the sandbox refuses it as well.
    pub file_blocks: u64,
    /// `RLIMIT_CORE`: no core dumps of guest memory.
    pub core_blocks: u64,
}
impl ResourceLimits {
    /// The budget's wall-clock limit, in whole seconds rounded up, plus one:
    /// the watchdog ends the host at the deadline, and the CPU limit still
    /// ends one that outlives it.
    pub fn for_budget(budget: ExecutionBudget) -> Self {
        Self {
            cpu_seconds: budget.timeout_ms().div_ceil(1000) + 1,
            open_files: 64,
            file_blocks: 0,
            core_blocks: 0,
        }
    }
}

/// An SBPL string literal for an absolute path, or an error for a path the
/// profile cannot name exactly: relative, not UTF-8, or with control characters.
fn literal(path: &Path) -> io::Result<String> {
    let invalid = |reason: &str| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("The sandbox cannot grant {}: {reason}", path.display()),
        )
    };
    let text = path.to_str().ok_or_else(|| invalid("the path is not UTF-8"))?;
    if !path.is_absolute() {
        return Err(invalid("the path is not absolute"));
    }
    if text.chars().any(char::is_control) {
        return Err(invalid("the path contains control characters"));
    }
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        if matches!(character, '"' | '\\') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('"');
    Ok(quoted)
}

fn profile_with(request: &IsolationRequest, extra_program: Option<&Path>) -> io::Result<String> {
    let runtime = literal(&request.runtime)?;
    let mut programs = vec![runtime.clone()];
    if let Some(extra) = extra_program {
        programs.push(literal(extra)?);
    }
    let mut readable = programs.clone();
    for path in &request.readable {
        readable.push(literal(path)?);
    }
    // Path resolution (realpath, `current_exe`) stats each folder on the way.
    let mut folders: Vec<String> = Vec::new();
    for path in std::iter::once(&request.runtime).chain(&request.readable) {
        for folder in path.ancestors().skip(1) {
            let folder = literal(folder)?;
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
    }
    let quoted = |paths: &[&str]| -> Vec<String> { paths.iter().map(|path| format!("\"{path}\"")).collect() };
    let rule = |operation: &str, filter: &str, values: &[String]| {
        let filters: Vec<String> = values.iter().map(|value| format!("({filter} {value})")).collect();
        format!("(allow {operation} {})\n", filters.join(" "))
    };
    let mut profile = String::from(
        "(version 1)\n\
         (deny default)\n\
         (import \"system.sb\")\n\
         (allow sysctl-read)\n",
    );
    profile.push_str(&rule("file-read*", "subpath", &quoted(&SYSTEM_READABLE)));
    profile.push_str(&rule("file-read*", "literal", &quoted(&DEVICES)));
    profile.push_str(&rule("file-read-metadata", "literal", &folders));
    profile.push_str(&rule("process-exec", "literal", &programs));
    profile.push_str(&rule("file-read*", "literal", &readable));
    if let Some(socket) = &request.socket {
        profile.push_str(&format!(
            "(allow network-outbound (remote unix-socket (path-literal {})))\n",
            literal(socket)?
        ));
    }
    Ok(profile)
}

/// The sandbox profile for the host.
pub fn sandbox_profile(request: &IsolationRequest) -> io::Result<String> {
    profile_with(request, None)
}
/// The profile the readiness probe applies: the host's own, plus permission
/// to run [`PROBE_PROGRAM`] in its place.
pub fn probe_profile(request: &IsolationRequest) -> io::Result<String> {
    profile_with(request, Some(Path::new(PROBE_PROGRAM)))
}

/// The fixed launch script. Its data arrives as `$1` (the profile) and the
/// remaining positional parameters (the program and its arguments), never as
/// script text, so nothing in a path or argument is interpreted by the shell.
pub fn launch_script(limits: ResourceLimits) -> String {
    format!(
        "ulimit -c {} && ulimit -t {} && ulimit -n {} && ulimit -f {} || exit {LIMITS_FAILED}\n\
         profile=$1\n\
         shift\n\
         exec {SANDBOX_EXEC} -p \"$profile\" \"$@\"\n",
        limits.core_blocks, limits.cpu_seconds, limits.open_files, limits.file_blocks
    )
}

/// `argv` for `posix_spawn` of [`SHELL`]: the script, its name, the profile,
/// then the program and its arguments.
pub fn launch_arguments(
    limits: ResourceLimits,
    profile: &str,
    program: &Path,
    arguments: &[OsString],
) -> Vec<OsString> {
    let mut argv: Vec<OsString> = vec![
        SHELL.into(),
        "-c".into(),
        launch_script(limits).into(),
        SCRIPT_NAME.into(),
        profile.into(),
        program.into(),
    ];
    argv.extend(arguments.iter().cloned());
    argv
}

/// The host's whole environment: no user variables (HOME, TMPDIR, tokens),
/// only a system `PATH`, as the Windows host keeps only system variables.
pub fn scrubbed_environment() -> Vec<(OsString, OsString)> {
    vec![("PATH".into(), "/usr/bin:/bin".into())]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> IsolationRequest {
        IsolationRequest {
            runtime: PathBuf::from("/Applications/Bareline.app/Contents/MacOS/bareline-extension-host"),
            arguments: vec!["--component".into(), "/tmp/ext/a \"b\".wasm".into()],
            readable: vec![PathBuf::from("/tmp/ext/a \"b\".wasm")],
            socket: Some(PathBuf::from("/tmp/bareline-1000/host.sock")),
            budget: ExecutionBudget::Interactive,
        }
    }

    #[test]
    fn the_profile_denies_by_default_and_grants_only_named_paths() {
        let profile = sandbox_profile(&request()).unwrap();
        let lines: Vec<&str> = profile.lines().collect();
        assert_eq!(lines[..3], ["(version 1)", "(deny default)", "(import \"system.sb\")"]);
        assert!(profile.contains(
            "(allow process-exec (literal \"/Applications/Bareline.app/Contents/MacOS/bareline-extension-host\"))"
        ));
        // Quotes in a granted path are escaped, not interpreted.
        assert!(profile.contains("(literal \"/tmp/ext/a \\\"b\\\".wasm\")"));
        assert!(profile.contains("(remote unix-socket (path-literal \"/tmp/bareline-1000/host.sock\"))"));
        for absent in [
            "network*",
            "file-write",
            "process-fork",
            "regex",
            "/usr/bin/true",
            "(allow default",
        ] {
            assert!(!profile.contains(absent), "{absent}");
        }
        // Whole folders are readable only where the operating system lives.
        let subpaths: Vec<&str> = profile
            .split("(subpath \"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap())
            .collect();
        assert_eq!(subpaths, SYSTEM_READABLE);
        // The folders above the granted files may be looked up, not read.
        let lookups = profile
            .lines()
            .find(|line| line.starts_with("(allow file-read-metadata "))
            .unwrap();
        for folder in ["/", "/Applications", "/tmp", "/tmp/ext"] {
            assert!(lookups.contains(&format!("(literal \"{folder}\")")), "{folder}");
        }
        assert!(!lookups.contains("bareline-extension-host\")"));
        assert!(!profile.contains("(allow file-read* (literal \"/tmp/ext\")"));
        let probe = probe_profile(&request()).unwrap();
        assert!(probe.contains("(literal \"/usr/bin/true\")"));
        assert!(probe.starts_with("(version 1)\n(deny default)\n"));
    }

    #[test]
    fn paths_the_profile_cannot_name_exactly_are_refused() {
        for bad in ["relative/host", "/tmp/line\nbreak", "/tmp/nul\0byte"] {
            let mut request = request();
            request.readable = vec![PathBuf::from(bad)];
            let error = sandbox_profile(&request).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{bad:?}");
        }
        let mut request = request();
        request.socket = None;
        assert!(!sandbox_profile(&request).unwrap().contains("unix-socket"));
    }

    /// A host in the macOS temporary folder (`/var` is a link to `/private/var`)
    /// is granted by its real paths, which the sandbox matches.
    #[test]
    fn host_requests_name_real_paths() {
        let root = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "bareline-host-request-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("link")).unwrap();
        let (host, component) = (root.join("link/host"), root.join("link/c.wasm"));
        std::fs::write(&host, b"host").unwrap();
        std::fs::write(&component, b"component").unwrap();
        let file = std::fs::File::open(&host).unwrap();
        let socket = root.join("link/bareline-exthost-1-ab");
        let spawn = HostSpawn {
            executable: &host,
            executable_file: &file,
            component: &component,
            arguments: vec!["name".into(), "1".into()],
            budget: ExecutionBudget::Background,
            socket: Some(&socket),
        };
        let request = host_request(&spawn).unwrap();
        assert_eq!(request.runtime, root.join("real/host"));
        assert_eq!(request.readable, [component.clone(), root.join("real/c.wasm")]);
        assert_eq!(request.socket, Some(root.join("real/bareline-exthost-1-ab")));
        assert_eq!(request.arguments, spawn.arguments);
        assert_eq!(request.budget, ExecutionBudget::Background);
        let profile = sandbox_profile(&request).unwrap();
        assert!(profile.contains(&format!(
            "(allow process-exec (literal \"{}\"))",
            root.join("real/host").display()
        )));
        // A runtime replaced after its verification is not launched.
        let other = std::fs::File::open(&component).unwrap();
        let swapped = HostSpawn {
            executable_file: &other,
            ..spawn
        };
        assert_eq!(
            host_request(&swapped).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn limits_follow_the_budget() {
        let interactive = ResourceLimits::for_budget(ExecutionBudget::Interactive);
        assert_eq!(interactive.cpu_seconds, 6);
        let background = ResourceLimits::for_budget(ExecutionBudget::Background);
        assert_eq!(background.cpu_seconds, 121);
        assert!(background.cpu_seconds > interactive.cpu_seconds);
        assert_eq!((interactive.file_blocks, interactive.core_blocks), (0, 0));
    }

    #[test]
    fn the_launch_chain_passes_data_only_as_arguments() {
        let limits = ResourceLimits::for_budget(ExecutionBudget::Interactive);
        let request = request();
        let profile = sandbox_profile(&request).unwrap();
        let argv = launch_arguments(limits, &profile, &request.runtime, &request.arguments);
        assert_eq!(argv[0], SHELL);
        assert_eq!(argv[1], "-c");
        let script = argv[2].to_str().unwrap();
        assert_eq!(
            script,
            "ulimit -c 0 && ulimit -t 6 && ulimit -n 64 && ulimit -f 0 || exit 125\nprofile=$1\nshift\nexec /usr/bin/sandbox-exec -p \"$profile\" \"$@\"\n"
        );
        // Nothing from the request appears in the script itself.
        assert!(!script.contains("bareline-extension-host") && !script.contains("wasm"));
        assert_eq!(argv[4], profile.as_str());
        assert_eq!(argv[5], request.runtime.as_os_str());
        assert_eq!(argv[6..], request.arguments[..]);
        assert_eq!(scrubbed_environment(), [("PATH".into(), "/usr/bin:/bin".into())]);
    }
}
