// SPDX-License-Identifier: MPL-2.0
use std::{io::Read, process::{Command, Stdio}, sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc}, time::{Duration, Instant}};

pub struct Capture {
    pub stdout: String,
    pub stderr: String,
    pub status: &'static str,
    pub exit_code: Option<i32>,
}

fn reader(mut stream: impl Read + Send + 'static, overflow: Arc<AtomicBool>) -> mpsc::Receiver<Vec<u8>> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut block = [0u8; 8192];
        while let Ok(count) = stream.read(&mut block) {
            if count == 0 { break; }
            let retained = count.min((256 * 1024usize).saturating_sub(output.len()));
            output.extend_from_slice(&block[..retained]);
            if retained != count { overflow.store(true, Ordering::Release); }
        }
        let _ = sender.send(output);
    });
    receiver
}

pub fn run(command: &mut Command, timeout: Duration) -> std::io::Result<Capture> {
    let mut child = command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let overflow = Arc::new(AtomicBool::new(false));
    let stdout = reader(child.stdout.take().expect("piped stdout"), overflow.clone());
    let stderr = reader(child.stderr.take().expect("piped stderr"), overflow.clone());
    let started = Instant::now();
    let mut status = "ok";
    let exit = loop {
        match child.try_wait() {
            Ok(Some(exit)) => break exit,
            Ok(None) => {},
            Err(error) => { let _ = child.kill(); let _ = child.wait(); return Err(error); }
        }
        if overflow.load(Ordering::Acquire) || started.elapsed() >= timeout {
            status = if overflow.load(Ordering::Acquire) { "output_limit" } else { "timeout" };
            // This runner starts only the isolated editor diagnostic process. It
            // never kills processes selected by name or existing user instances.
            let _ = child.kill();
            break child.wait()?;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if status == "ok" && !exit.success() { status = "failed"; }
    let stdout = stdout.recv_timeout(Duration::from_secs(1));
    let stderr = stderr.recv_timeout(Duration::from_secs(1));
    if stdout.is_err() || stderr.is_err() { status = "inherited_pipe_open"; }
    else if overflow.load(Ordering::Acquire) { status = "output_limit"; }
    Ok(Capture { stdout: String::from_utf8_lossy(&stdout.unwrap_or_default()).into_owned(),
        stderr: String::from_utf8_lossy(&stderr.unwrap_or_default()).into_owned(), status, exit_code: exit.code() })
}
