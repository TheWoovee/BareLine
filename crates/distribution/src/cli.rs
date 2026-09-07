// SPDX-License-Identifier: MPL-2.0
use lexopt::ValueExt;
use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct LaunchOptions {
    pub paths: Vec<PathBuf>,
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

pub const HELP: &str = "Bareline [OPTIONS] [--] [PATH ...]\n  --line N --column N  One-based location\n  --read-only --monitor --no-session --no-extensions --new-instance\n  --help --version\nPaths are retained losslessly; use -- before paths beginning with '-'.";

/// Parses arguments excluding argv[0], without probing, canonicalizing or opening paths.
/// Device/UNC trust decisions belong to the open service, never the parser.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<LaunchOptions, lexopt::Error> {
    use lexopt::prelude::*;
    let mut parser = lexopt::Parser::from_args(args);
    let mut options = LaunchOptions::default();
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
    #[cfg(windows)]
    #[test]
    fn unpaired_utf16_is_not_replaced() {
        let path = bareline_platform::SerializedPath {
            version: 1,
            encoding: bareline_platform::PathEncoding::WindowsUtf16Le,
            data: "QwA6AFwAANg=".into(),
            display: "unpaired UTF-16 fixture".into(),
        }.to_native().unwrap();
        let parsed = parse([path.clone().into_os_string()]).unwrap();
        assert_eq!(parsed.paths[0].as_os_str(), path.as_os_str());
    }
}
