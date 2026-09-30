// SPDX-License-Identifier: MPL-2.0
use lexopt::ValueExt;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct LaunchOptions {
    pub paths: Vec<PathBuf>,
    /// A lone `-` before `--`: read standard input into a new Untitled document.
    pub stdin: bool,
    /// One-based coordinates applied to opened files; absence preserves session position.
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub read_only: bool,
    pub monitor: bool,
    pub no_session: bool,
    pub no_extensions: bool,
    pub new_instance: bool,
    pub help: bool,
    pub version: bool,
}

pub const HELP: &str = "Bareline [OPTIONS] [--] [PATH ...]\n  --line N --column N  One-based location\n  --read-only --monitor --no-session --no-extensions --new-instance\n  --help --version\n  -  Read standard input into a new Untitled document\nNotepad++ spellings -n<line> -c<column> -ro -multiInst -nosession -noPlugin -notabbar are accepted.\nPaths are retained losslessly; use -- before paths beginning with '-'.";

/// Parses arguments excluding argv[0], without probing, canonicalizing or opening paths.
/// Device/UNC trust decisions belong to the open service, never the parser.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<LaunchOptions, lexopt::Error> {
    use lexopt::prelude::*;
    // Notepad++ spellings and `-` are rewritten before `--` only, so a path after
    // the separator is always taken literally.
    let mut stdin = false;
    let mut after_separator = false;
    let mut product = Vec::new();
    for arg in args {
        if after_separator {
            product.push(arg);
        } else if arg == "--" {
            after_separator = true;
            product.push(arg);
        } else if arg == "-" {
            stdin = true;
        } else if let Some(replacement) = notepad_plus_plus_alias(&arg) {
            product.extend(replacement);
        } else {
            product.push(arg);
        }
    }
    let mut parser = lexopt::Parser::from_args(product);
    let mut options = LaunchOptions {
        stdin,
        ..Default::default()
    };
    while let Some(arg) = parser.next()? {
        match arg {
            Long("line") => options.line = Some(coordinate(&mut parser)?),
            Long("column") => options.column = Some(coordinate(&mut parser)?),
            Long("read-only") => options.read_only = true,
            Long("monitor") => {
                options.monitor = true;
                options.read_only = true;
            }
            Long("no-session") => options.no_session = true,
            Long("no-extensions") => options.no_extensions = true,
            Long("new-instance") => options.new_instance = true,
            Long("help") | Short('h') => options.help = true,
            Long("version") | Short('V') => options.version = true,
            Value(path) => options.paths.push(PathBuf::from(path)),
            other => return Err(other.unexpected()),
        }
    }
    if options.column.is_some() && options.line.is_none() {
        return Err("--column requires --line".into());
    }
    Ok(options)
}
/// Notepad++ command-line spellings accepted for familiarity (APP-09). Switches
/// with no Bareline equivalent, such as `-notabbar`, are accepted and ignored.
fn notepad_plus_plus_alias(arg: &OsString) -> Option<Vec<OsString>> {
    let text = arg.to_str()?;
    let long = |option: &str| Some(vec![OsString::from(option)]);
    match text.to_ascii_lowercase().as_str() {
        "-ro" => return long("--read-only"),
        "-multiinst" => return long("--new-instance"),
        "-nosession" => return long("--no-session"),
        "-noplugin" => return long("--no-extensions"),
        "-notabbar" => return Some(Vec::new()),
        _ => {}
    }
    [("-n", "--line"), ("-c", "--column")]
        .into_iter()
        .find_map(|(prefix, option)| {
            let value = text.strip_prefix(prefix)?;
            (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| vec![OsString::from(option), OsString::from(value)])
        })
}
fn coordinate(parser: &mut lexopt::Parser) -> Result<u64, lexopt::Error> {
    let value = parser.value()?.parse::<u64>()?;
    if value == 0 {
        return Err("line and column must be greater than zero".into());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lossless_paths_and_options() {
        let result = parse(
            [
                "--line",
                "12",
                "--column=3",
                "--monitor",
                "--no-session",
                "--no-extensions",
                "C:\\a b\\𐐀.txt",
                "\\\\?\\C:\\device.txt",
                "--",
                "-file",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(result.line, Some(12));
        assert_eq!(result.column, Some(3));
        assert!(result.monitor && result.read_only && result.no_session && result.no_extensions);
        assert_eq!(
            result.paths,
            [
                PathBuf::from("C:\\a b\\𐐀.txt"),
                PathBuf::from("\\\\?\\C:\\device.txt"),
                PathBuf::from("-file")
            ]
        );
    }
    #[test]
    fn invalid_coordinates_and_options() {
        for args in [
            vec!["--line", "0"],
            vec!["--line", "-1"],
            vec!["--line"],
            vec!["--column", "2"],
            vec!["--unknown"],
        ] {
            assert!(parse(args.into_iter().map(OsString::from)).is_err());
        }
    }
    fn parsed(args: &[&str]) -> Result<LaunchOptions, lexopt::Error> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn notepad_plus_plus_aliases_match_the_long_options() {
        let alias = parsed(&[
            "-n42",
            "-c7",
            "-ro",
            "-multiInst",
            "-nosession",
            "-noPlugin",
            "-notabbar",
            "a.txt",
        ])
        .unwrap();
        let long = parsed(&[
            "--line",
            "42",
            "--column",
            "7",
            "--read-only",
            "--new-instance",
            "--no-session",
            "--no-extensions",
            "a.txt",
        ])
        .unwrap();
        assert_eq!(alias, long);
        assert_eq!(alias.line, Some(42));
        assert_eq!(alias.column, Some(7));
        assert!(alias.read_only && alias.new_instance && alias.no_session && alias.no_extensions);
        assert_eq!(alias.paths, [PathBuf::from("a.txt")]);
        // Case-insensitive switch names, as Notepad++ users type them.
        assert!(parsed(&["-MULTIINST"]).unwrap().new_instance);
        // A bare -n or a non-numeric suffix is not a line alias.
        for args in [vec!["-n"], vec!["-nx"], vec!["-n0"], vec!["-c3"]] {
            assert!(parsed(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn aliases_and_stdin_apply_only_before_the_separator() {
        let result = parsed(&["-", "b.txt", "--", "-", "-ro", "-n5"]).unwrap();
        assert!(result.stdin);
        assert!(!result.read_only);
        assert_eq!(result.line, None);
        assert_eq!(
            result.paths,
            [
                PathBuf::from("b.txt"),
                PathBuf::from("-"),
                PathBuf::from("-ro"),
                PathBuf::from("-n5")
            ]
        );
        let plain = parsed(&["a.txt"]).unwrap();
        assert!(!plain.stdin);
    }

    #[test]
    fn more_than_sixteen_paths_are_all_parsed() {
        let names: Vec<String> = (0..20).map(|index| format!("file-{index}.txt")).collect();
        let result = parse(names.iter().map(OsString::from)).unwrap();
        assert_eq!(result.paths.len(), 20);
    }

    #[cfg(windows)]
    #[test]
    fn unpaired_utf16_is_not_replaced() {
        let path = bareline_platform::SerializedPath {
            version: 1,
            encoding: bareline_platform::PathEncoding::WindowsUtf16Le,
            data: "QwA6AFwAANg=".into(),
            display: "unpaired UTF-16 fixture".into(),
        }
        .to_native()
        .unwrap();
        let parsed = parse([path.clone().into_os_string()]).unwrap();
        assert_eq!(parsed.paths[0].as_os_str(), path.as_os_str());
    }
}
