// SPDX-License-Identifier: MPL-2.0
//! Logoff, shutdown, a closed terminal and Ctrl+C reach Bareline as SIGTERM,
//! SIGHUP or SIGINT on these systems (ADR-C). The monitor catches them
//! (`bareline_platform_posix::session_end`) and feeds the Windows adapter's
//! protocol, so the shell's session code and its tests behave the same: the
//! first signal is a session that really ends (`End { ending: true }`), which
//! the shell flushes and then exits on, because nothing else ends the process
//! here. A second signal, or a flush that outlives `SIGNAL_DEADLINE`, ends the
//! process with the signal's default action.
use super::{
    error::{Error, Result},
    window::{RawWindow, event_notify},
};
use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    },
    time::Duration,
};

/// How long after the signal the process may take to flush and exit. The
/// shell's own flush budget (`SESSION_END_BUDGET`, 3 s) fits inside it with
/// room for the exit; systemd waits 90 s and logout managers about 5 s.
const SIGNAL_DEADLINE: Duration = Duration::from_secs(5);

#[allow(
    dead_code,
    reason = "a signal is never a cancellable query; `Query` completes the shared protocol"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEndMessage {
    /// The session may end. Answered at once and never vetoed.
    Query,
    /// The session ends; `ending` is false when it was cancelled.
    End { ending: bool },
}
/// Side effects of a session-end notification.
pub trait SessionEndHost {
    /// Route one flush request to the application.
    fn deliver(&mut self);
    /// Ask the system for more time. Returns whether it took effect.
    fn block(&mut self) -> bool;
    fn unblock(&mut self);
}
/// UI-thread state shared by the session-end source and the application; the
/// same protocol as the Windows adapter's.
#[derive(Debug, Default)]
pub struct SessionEndSignal {
    requests: Cell<u32>,
    flushed: Cell<bool>,
    dirty: Cell<bool>,
    blocked: Cell<bool>,
}
impl SessionEndSignal {
    pub fn set_dirty(&self, dirty: bool) {
        self.dirty.set(dirty);
    }
    /// Claim one routed flush request. False means an ordinary close request.
    pub fn take_request(&self) -> bool {
        let requests = self.requests.get();
        if requests == 0 {
            return false;
        }
        self.requests.set(requests - 1);
        true
    }
    pub fn finish(&self, complete: bool) {
        self.flushed.set(complete);
    }
    /// Answer one notification and return its result.
    pub fn respond(&self, host: &mut dyn SessionEndHost, message: SessionEndMessage) -> isize {
        match message {
            SessionEndMessage::Query => {
                if self.dirty.get() && !self.blocked.get() {
                    self.blocked.set(host.block());
                }
                self.request(host);
                1
            }
            SessionEndMessage::End { ending } => {
                if ending && !self.flushed.get() {
                    self.request(host);
                }
                if self.blocked.replace(false) {
                    host.unblock();
                }
                0
            }
        }
    }
    fn request(&self, host: &mut dyn SessionEndHost) {
        self.flushed.set(false);
        self.requests.set(self.requests.get().saturating_add(1));
        host.deliver();
    }
}
/// The signals have no window message to route through, so the flush request
/// waits in `SessionEndSignal` until the shell polls (`session_end_signalled`).
struct Queued;
impl SessionEndHost for Queued {
    fn deliver(&mut self) {}
    /// A signal grants no extra time; `SIGNAL_DEADLINE` bounds the flush.
    fn block(&mut self) -> bool {
        false
    }
    fn unblock(&mut self) {}
}
pub struct SessionEndMonitor {
    signal: Rc<SessionEndSignal>,
    /// The signal that ended the session and is not yet routed; 0 for none.
    pending: Arc<AtomicI32>,
}
impl SessionEndMonitor {
    /// # Safety
    /// None here: no handle is kept. The signature matches the Windows adapter;
    /// see `Platform::new`.
    pub unsafe fn attach(_raw: RawWindow, signal: Rc<SessionEndSignal>) -> Result<Self> {
        let pending = Arc::new(AtomicI32::new(0));
        let raised = pending.clone();
        let notify = event_notify();
        let signals = bareline_platform_posix::session_end::SessionEndSignals::install(
            move |number| {
                raised.store(number, Ordering::Release);
                notify();
            },
            SIGNAL_DEADLINE,
        )
        .map_err(|error| Error::other(format!("Session-end signals cannot be caught: {error}")))?;
        eprintln!("event=session_end_signals caught={:?}", signals.caught());
        Ok(Self::with_pending(signal, pending))
    }
    fn with_pending(signal: Rc<SessionEndSignal>, pending: Arc<AtomicI32>) -> Self {
        Self { signal, pending }
    }
    /// Routes a signal that arrived since the last poll as a session that
    /// really ends; true when the shell must now flush and exit.
    fn poll(&self) -> bool {
        let number = self.pending.swap(0, Ordering::AcqRel);
        if number == 0 {
            return false;
        }
        eprintln!("event=session_end_signal signal={number}");
        self.signal
            .respond(&mut Queued, SessionEndMessage::End { ending: true });
        true
    }
}
/// Whether a session-end signal arrived: its flush request is then queued in
/// the monitor's `SessionEndSignal`, and the shell flushes and exits.
pub fn session_end_signalled(monitor: &SessionEndMonitor) -> bool {
    monitor.poll()
}
/// A monitor whose signal is raised by the test, not by the system.
#[cfg(test)]
pub fn session_end_monitor_for_tests(signal: Rc<SessionEndSignal>) -> (SessionEndMonitor, Arc<AtomicI32>) {
    let pending = Arc::new(AtomicI32::new(0));
    (SessionEndMonitor::with_pending(signal, pending.clone()), pending)
}
/// Restart registration exists for Windows update restarts; these systems
/// update through their package managers, so there is nothing to register.
pub fn register_application_restart() -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_end_routes_one_flush_per_notification() {
        #[derive(Default)]
        struct Host {
            delivered: u32,
            blocked: bool,
        }
        impl SessionEndHost for Host {
            fn deliver(&mut self) {
                self.delivered += 1;
            }
            fn block(&mut self) -> bool {
                self.blocked = true;
                true
            }
            fn unblock(&mut self) {
                self.blocked = false;
            }
        }
        let signal = SessionEndSignal::default();
        let mut host = Host::default();
        signal.set_dirty(true);
        assert_eq!(signal.respond(&mut host, SessionEndMessage::Query), 1);
        assert!(host.blocked);
        assert!(signal.take_request());
        assert!(!signal.take_request());
        // An unfinished flush is requested again when the session really ends.
        signal.finish(false);
        assert_eq!(signal.respond(&mut host, SessionEndMessage::End { ending: true }), 0);
        assert_eq!(host.delivered, 2);
        assert!(!host.blocked);
    }
}
