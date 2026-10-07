// SPDX-License-Identifier: MPL-2.0
//! Session end as Linux and macOS report it. Logout, shutdown, `systemctl
//! stop`, a closed terminal and Ctrl+C reach the process as SIGTERM, SIGHUP or
//! SIGINT, whose default actions end it at once, before unsaved text is
//! journaled or the session is written.
//!
//! [`SessionEndSignals::install`] catches them. The handler only does what is
//! async-signal-safe: it writes the signal number to a self-pipe. A watcher
//! thread reads it, restores the default actions (so a second signal ends the
//! process at once, as it did before), and reports the first signal to the
//! application, which saves what it must and exits. If the process still runs
//! when the deadline passes, the watcher ends it with that signal's default
//! action, so a hung save never keeps a logout waiting.
//!
//! A signal the process inherited as ignored (`nohup`, a background job of a
//! non-interactive shell) stays ignored, as POSIX shells expect.
use std::{
    io::{self, Read},
    mem::MaybeUninit,
    os::{fd::IntoRawFd, unix::net::UnixStream},
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
    time::Duration,
};

/// The signals that end a session: SIGTERM (logout, shutdown, service stop),
/// SIGHUP (the controlling terminal closed) and SIGINT (Ctrl+C in it).
pub const SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGHUP, libc::SIGINT];

/// The self-pipe's write end, which the handler reads; -1 until installed.
static WRITE_END: AtomicI32 = AtomicI32::new(-1);
/// Signal dispositions are process-wide, so they are installed once.
static INSTALLED: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(signal: libc::c_int) {
    let fd = WRITE_END.load(Ordering::Relaxed);
    if fd < 0 {
        return;
    }
    // Every caught signal number is below 32.
    let byte = signal as u8;
    let errno = errno();
    // SAFETY: `errno` is this thread's errno location (or null where it is
    // not known); write(2) is async-signal-safe, `fd` is the non-blocking
    // write end that is never closed, and `byte` lives for the call.
    unsafe {
        let saved = if errno.is_null() { 0 } else { *errno };
        let _ = libc::write(fd, (&raw const byte).cast(), 1);
        if !errno.is_null() {
            *errno = saved;
        }
    }
}

/// This thread's errno, which the handler must leave as it found it.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn errno() -> *mut libc::c_int {
    // SAFETY: returns the calling thread's errno location; async-signal-safe.
    unsafe { libc::__errno_location() }
}
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))]
fn errno() -> *mut libc::c_int {
    // SAFETY: returns the calling thread's errno location; async-signal-safe.
    unsafe { libc::__error() }
}
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd"
)))]
fn errno() -> *mut libc::c_int {
    std::ptr::null_mut()
}

fn disposition(signal: libc::c_int) -> io::Result<libc::sighandler_t> {
    let mut current = MaybeUninit::<libc::sigaction>::zeroed();
    // SAFETY: a null new action only reads the current one into `current`,
    // which is a valid, writable `sigaction`.
    if unsafe { libc::sigaction(signal, std::ptr::null(), current.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: sigaction(2) filled it in; an all-zero `sigaction` is valid too.
    Ok(unsafe { current.assume_init() }.sa_sigaction)
}

fn set_disposition(signal: libc::c_int, handler: libc::sighandler_t, flags: libc::c_int) -> io::Result<()> {
    // SAFETY: an all-zero `sigaction` is a valid value; its mask is emptied below.
    let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
    action.sa_sigaction = handler;
    action.sa_flags = flags;
    // SAFETY: `action.sa_mask` is a valid `sigset_t` to initialize, and the
    // action is fully initialized before sigaction(2) reads it.
    if unsafe { libc::sigemptyset(&mut action.sa_mask) } != 0
        || unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The session-end signals this process catches.
#[derive(Debug)]
pub struct SessionEndSignals {
    caught: Vec<libc::c_int>,
}
impl SessionEndSignals {
    /// Catches the session-end signals for the rest of the process. `on_end`
    /// runs once, on the watcher thread, with the first signal; the
    /// application then flushes and exits. `deadline` after that signal a
    /// process that has not exited is ended by the signal's default action.
    /// Fails when called a second time in a process.
    pub fn install(on_end: impl FnOnce(libc::c_int) + Send + 'static, deadline: Duration) -> io::Result<Self> {
        if INSTALLED.swap(true, Ordering::AcqRel) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "session-end signals are already caught in this process",
            ));
        }
        let (mut reader, writer) = UnixStream::pair()?;
        writer.set_nonblocking(true)?;
        let mut caught = Vec::new();
        for signal in SIGNALS {
            if disposition(signal)? != libc::SIG_IGN {
                caught.push(signal);
            }
        }
        let watched = caught.clone();
        std::thread::Builder::new()
            .name("session-end-signals".into())
            .spawn(move || {
                let mut byte = [0u8; 1];
                let signal = loop {
                    match reader.read(&mut byte) {
                        Ok(1) => break libc::c_int::from(byte[0]),
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        // The write end is never closed; nothing else ends the read.
                        _ => return,
                    }
                };
                // A second signal now ends the process at once.
                for signal in &watched {
                    let _ = set_disposition(*signal, libc::SIG_DFL, 0);
                }
                on_end(signal);
                std::thread::sleep(deadline);
                eprintln!("event=session_end_deadline signal={signal}");
                // SAFETY: plain system calls on this process; the default action
                // (restored above) ends it.
                unsafe {
                    libc::kill(libc::getpid(), signal);
                }
            })?;
        // The handler writes to this end for the rest of the process.
        WRITE_END.store(writer.into_raw_fd(), Ordering::Release);
        let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        for signal in &caught {
            // SA_RESETHAND: the same signal again takes the default action even
            // before the watcher restored it.
            set_disposition(*signal, handler, libc::SA_RESTART | libc::SA_RESETHAND)?;
        }
        Ok(Self { caught })
    }
    /// The signals caught; the others were inherited as ignored.
    pub fn caught(&self) -> &[libc::c_int] {
        &self.caught
    }
}
