// SPDX-License-Identifier: MPL-2.0
#![cfg(target_os = "linux")]
//! The real system clipboard. These tests replace the clipboard of the display
//! they run on, so they are ignored by default; run them on a disposable display
//! with the `xclip` and `wl-clipboard` tools installed:
//!
//! ```text
//! env -u WAYLAND_DISPLAY xvfb-run -a cargo test -p bareline-platform-linux --test clipboard -- --ignored --test-threads=1
//! cargo test -p bareline-platform-linux --test clipboard -- --ignored --test-threads=1   # a Wayland session (WSLg)
//! ```
use bareline_platform::clipboard::{
    DEFAULT_CLIPBOARD_MAX_BYTES, MULTISELECTION_CLIPBOARD_FORMAT, RECTANGLE_CLIPBOARD_FORMAT, encode_clipboard_metadata,
};
use bareline_platform_linux::clipboard::{ClipboardBackend, LinuxClipboard, metadata_mime_type};
use std::{
    io::Write,
    process::{Command, Stdio},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

const MAX: usize = DEFAULT_CLIPBOARD_MAX_BYTES;

/// Tests that own the system clipboard must not contend with one another.
fn serial() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
fn sample(bytes: usize) -> String {
    let line = "Tab\tseparated, CJK \u{754c}, emoji \u{1f980}, ZWJ \u{1f469}\u{200d}\u{1f4bb}, RTL \u{05e9}\u{05dc}\u{05d5}\u{05dd}\n";
    let mut text = String::with_capacity(bytes + line.len());
    while text.len() < bytes {
        text.push_str(line);
    }
    text
}
/// Reads what another clipboard client pastes (bounded by `timeout`).
fn paste(program: &str, args: &[&str]) -> Vec<u8> {
    let output = Command::new("timeout")
        .arg("20")
        .arg(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .unwrap_or_else(|error| panic!("{program} is needed for this test: {error}"));
    assert!(output.status.success(), "{program} {args:?} failed: {}", output.status);
    output.stdout
}
/// Copies with another client. Those keep serving the selection from a
/// background process, which must not hold on to this test's pipes.
fn copy(program: &str, args: &[&str], input: Option<&[u8]>) {
    let mut child = Command::new(program)
        .args(args)
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap_or_else(|error| panic!("{program} is needed for this test: {error}"));
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    assert!(child.wait().unwrap().success(), "{program} {args:?} failed");
    std::thread::sleep(Duration::from_millis(200));
}
fn xclip_out(target: &str) -> Vec<u8> {
    paste("xclip", &["-o", "-selection", "clipboard", "-t", target])
}
fn xclip_in(text: &[u8]) {
    copy("xclip", &["-i", "-selection", "clipboard"], Some(text));
}

#[test]
#[ignore = "replaces the X11 clipboard; run under xvfb-run with --ignored --test-threads=1"]
fn text_round_trips_within_the_process_and_through_other_clients() {
    let _serial = serial();
    let clipboard = LinuxClipboard::x11().unwrap();
    assert_eq!(clipboard.backend(), ClipboardBackend::X11);
    let text = sample(4096);
    clipboard.write(&text, MAX).unwrap();
    assert_eq!(clipboard.read(MAX).unwrap().as_deref(), Some(text.as_str()));
    // Another connection reads it through the server; this process's owner
    // thread serves the request while the reading thread waits.
    let other = LinuxClipboard::x11().unwrap();
    assert_eq!(other.read(MAX).unwrap().as_deref(), Some(text.as_str()));
    // So does another process.
    assert_eq!(xclip_out("UTF8_STRING"), text.as_bytes());
    assert_eq!(xclip_out("text/plain;charset=utf-8"), text.as_bytes());
    // Text another application owns is pasted, with or without a trailing NUL.
    xclip_in(b"from xclip\0ignored");
    assert_eq!(clipboard.read(MAX).unwrap().as_deref(), Some("from xclip"));
    // Refusals happen before the user's clipboard is touched.
    assert!(clipboard.write("a\0b", MAX).is_err());
    assert!(clipboard.write("long", 3).is_err());
    assert_eq!(clipboard.read(MAX).unwrap().as_deref(), Some("from xclip"));
    // A limit smaller than the text is an error, not a truncated paste.
    assert!(clipboard.read(4).unwrap_err().to_string().contains("clipboard limit"));
}

#[test]
#[ignore = "replaces the X11 clipboard; run under xvfb-run with --ignored --test-threads=1"]
fn metadata_travels_beside_the_text_and_only_with_it() {
    let _serial = serial();
    let clipboard = LinuxClipboard::x11().unwrap();
    let other = LinuxClipboard::x11().unwrap();
    clipboard
        .write_with_metadata("ab\ncd", MAX, RECTANGLE_CLIPBOARD_FORMAT, b"rows")
        .unwrap();
    for reader in [&clipboard, &other] {
        let pasted = reader
            .read_with_metadata(MAX, RECTANGLE_CLIPBOARD_FORMAT, 64)
            .unwrap()
            .unwrap();
        assert_eq!(pasted.text, "ab\ncd");
        assert_eq!(pasted.metadata.as_deref(), Some(b"rows".as_slice()));
        assert!(!pasted.rectangular);
        assert_eq!(
            reader.metadata(RECTANGLE_CLIPBOARD_FORMAT, 64).unwrap().as_deref(),
            Some(b"rows".as_slice())
        );
        // Another format's metadata is not this one's.
        assert_eq!(reader.metadata(MULTISELECTION_CLIPBOARD_FORMAT, 64).unwrap(), None);
        // A payload larger than the caller accepts is dropped, the text kept.
        let small = reader
            .read_with_metadata(MAX, RECTANGLE_CLIPBOARD_FORMAT, 2)
            .unwrap()
            .unwrap();
        assert_eq!((small.text.as_str(), small.metadata), ("ab\ncd", None));
    }
    // Other applications see the private type beside the text.
    let mime = metadata_mime_type(RECTANGLE_CLIPBOARD_FORMAT).unwrap();
    let targets = String::from_utf8(xclip_out("TARGETS")).unwrap();
    assert!(targets.lines().any(|target| target == mime), "{targets}");
    assert!(targets.lines().any(|target| target == "UTF8_STRING"), "{targets}");
    assert_eq!(xclip_out(mime), encode_clipboard_metadata(b"rows").unwrap());
    // A plain copy carries no metadata.
    clipboard.write("plain", MAX).unwrap();
    let plain = other
        .read_with_metadata(MAX, RECTANGLE_CLIPBOARD_FORMAT, 64)
        .unwrap()
        .unwrap();
    assert_eq!((plain.text.as_str(), plain.metadata), ("plain", None));
    // Neither does foreign text.
    xclip_in(b"foreign");
    let foreign = clipboard
        .read_with_metadata(MAX, RECTANGLE_CLIPBOARD_FORMAT, 64)
        .unwrap()
        .unwrap();
    assert_eq!((foreign.text.as_str(), foreign.metadata), ("foreign", None));
}

#[test]
#[ignore = "replaces the X11 clipboard; run under xvfb-run with --ignored --test-threads=1"]
fn large_text_crosses_incrementally_in_both_directions() {
    let _serial = serial();
    // 6 MB exceeds the 4 MiB history entry limit and any single X request.
    let text = sample(6_000_000);
    let clipboard = LinuxClipboard::x11().unwrap();
    let other = LinuxClipboard::x11().unwrap();
    clipboard.write(&text, MAX).unwrap();
    let pasted = other.read(MAX).unwrap().unwrap();
    assert!(pasted == text, "6 MB text changed in transit through INCR");
    assert!(
        xclip_out("UTF8_STRING") == text.as_bytes(),
        "xclip read a different text"
    );
    assert!(other.read(text.len() - 1).is_err());
    xclip_in(text.as_bytes());
    let pasted = clipboard.read(MAX).unwrap().unwrap();
    assert!(pasted == text, "6 MB text from xclip changed in transit");
}

#[test]
#[ignore = "replaces the session clipboard; run in a Wayland session (WSLg) with --ignored"]
fn the_session_clipboard_reaches_wayland_clients() {
    let _serial = serial();
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        panic!("this test needs a Wayland session");
    }
    let clipboard = LinuxClipboard::new().unwrap();
    eprintln!("session clipboard backend: {:?}", clipboard.backend());
    let text = format!("bareline {}", std::process::id());
    clipboard.write(&text, MAX).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    // A native Wayland client pastes what Bareline copied...
    let pasted = paste("wl-paste", &["--no-newline"]);
    assert_eq!(String::from_utf8(pasted).unwrap(), text);
    // ...and Bareline pastes what it copied.
    copy("wl-copy", &["from wayland"], None);
    assert_eq!(clipboard.read(MAX).unwrap().as_deref(), Some("from wayland"));
    // Metadata survives the round trip within the session too.
    clipboard
        .write_with_metadata("ab\ncd", MAX, RECTANGLE_CLIPBOARD_FORMAT, b"rows")
        .unwrap();
    let pasted = clipboard
        .read_with_metadata(MAX, RECTANGLE_CLIPBOARD_FORMAT, 64)
        .unwrap()
        .unwrap();
    assert_eq!(pasted.metadata.as_deref(), Some(b"rows".as_slice()));
}
