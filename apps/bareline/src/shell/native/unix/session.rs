// SPDX-License-Identifier: MPL-2.0
//! Logoff and shutdown reach Bareline as SIGTERM or SIGHUP on these systems
//! (ADR-C); until that handler exists the signal state is kept but never fed.
//! The protocol is the Windows adapter's, so the shell's session code and its
//! tests behave the same.
use super::{
    error::{Error, Result},
    window::RawWindow,
};
use std::{cell::Cell, rc::Rc};

#[allow(
    dead_code,
    reason = "nothing feeds session-end notifications here until the signal handler exists"
)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEndMessage {
    /// The session may end. Answered at once and never vetoed.
    Query,
    /// The session ends; `ending` is false when it was cancelled.
    End { ending: bool },
}
/// Side effects of a session-end notification.
#[allow(
    dead_code,
    reason = "nothing feeds session-end notifications here until the signal handler exists"
)]
pub trait SessionEndHost {
    /// Route one flush request to the application.
    fn deliver(&mut self);
    /// Ask the system for more time. Returns whether it took effect.
    fn block(&mut self) -> bool;
    fn unblock(&mut self);
}
/// UI-thread state shared by the session-end source and the application; the
/// same protocol as the Windows adapter's.
#[allow(
    dead_code,
    reason = "nothing feeds session-end notifications here until the signal handler exists"
)]
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
    #[allow(
        dead_code,
        reason = "nothing feeds session-end notifications here until the signal handler exists"
    )]
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
    #[allow(
        dead_code,
        reason = "nothing feeds session-end notifications here until the signal handler exists"
    )]
    fn request(&self, host: &mut dyn SessionEndHost) {
        self.flushed.set(false);
        self.requests.set(self.requests.get().saturating_add(1));
        host.deliver();
    }
}
pub struct SessionEndMonitor;
impl SessionEndMonitor {
    /// # Safety
    /// None for this stand-in, which keeps no handle; see `Platform::new`.
    pub unsafe fn attach(_raw: RawWindow, _signal: Rc<SessionEndSignal>) -> Result<Self> {
        Err(Error::other(
            "This system does not report logoff or shutdown to Bareline yet",
        ))
    }
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
