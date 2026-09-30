// SPDX-License-Identifier: MPL-2.0
//! Logoff, shutdown and update-restart safety. winit 0.30 forwards neither
//! WM_QUERYENDSESSION nor WM_ENDSESSION, so a subclass on the top-level window
//! answers them. The flush itself needs the application's state, so the
//! subclass routes it through winit's own handler as a synthetic WM_CLOSE: winit
//! delivers that as `CloseRequested` synchronously while its handler is idle, or
//! buffers it while the handler is busy (for example inside a modal dialog). The
//! application tells a routed flush from an ordinary close with
//! [`SessionEndSignal::take_request`].
use std::{cell::Cell, rc::Rc};
use windows::{
    Win32::{
        Foundation::*,
        System::{
            Recovery::{RESTART_NO_CRASH, RESTART_NO_HANG, RegisterApplicationRestart},
            Shutdown::{ShutdownBlockReasonCreate, ShutdownBlockReasonDestroy},
        },
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::w,
};

const SUBCLASS_ID: usize = 0xBAAE1002;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionEndMessage {
    /// WM_QUERYENDSESSION: the session may end. Answered at once and never vetoed.
    Query,
    /// WM_ENDSESSION: `ending` is false when another application cancelled.
    End { ending: bool },
}
impl SessionEndMessage {
    pub fn from_raw(message: u32, wparam: usize) -> Option<Self> {
        match message {
            WM_QUERYENDSESSION => Some(Self::Query),
            WM_ENDSESSION => Some(Self::End { ending: wparam != 0 }),
            _ => None,
        }
    }
}

/// Side effects of a session-end notification; the window subclass supplies the
/// Win32 ones and tests inject their own.
pub trait SessionEndHost {
    /// Route one flush request to the application. It runs synchronously while
    /// the application's handler is idle, and later otherwise.
    fn deliver(&mut self);
    /// Tell Windows why the application still needs time. Returns whether it took effect.
    fn block(&mut self) -> bool;
    fn unblock(&mut self);
}

/// UI-thread state shared by the window subclass and the application.
#[derive(Debug, Default)]
pub struct SessionEndSignal {
    requests: Cell<u32>,
    flushed: Cell<bool>,
    dirty: Cell<bool>,
    blocked: Cell<bool>,
}
impl SessionEndSignal {
    /// Keep current so a shutdown delay is explained even while a modal dialog
    /// holds the application's handler.
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
    /// Record the outcome of a claimed flush. An incomplete flush is retried
    /// when WM_ENDSESSION confirms that the session really ends.
    pub fn finish(&self, complete: bool) {
        self.flushed.set(complete);
    }
    /// Answer one notification and return its message result.
    pub fn respond(&self, host: &mut dyn SessionEndHost, message: SessionEndMessage) -> isize {
        match message {
            SessionEndMessage::Query => {
                if self.dirty.get() && !self.blocked.get() {
                    self.blocked.set(host.block());
                }
                self.request(host);
                // TRUE: logoff, shutdown and update restarts are never vetoed.
                1
            }
            SessionEndMessage::End { ending } => {
                // After this returns the process may be terminated at any time.
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

struct WindowHost {
    hwnd: HWND,
}
impl SessionEndHost for WindowHost {
    fn deliver(&mut self) {
        // Only valid inside the subclass callback, which is the sole caller.
        // SAFETY: the next procedure in this live window's chain receives a plain WM_CLOSE.
        unsafe {
            let _ = DefSubclassProc(self.hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    fn block(&mut self) -> bool {
        // SAFETY: called on the window's own thread with a static reason string.
        unsafe { ShutdownBlockReasonCreate(self.hwnd, w!("Saving recovery data...")).is_ok() }
    }
    fn unblock(&mut self) {
        // SAFETY: called on the window's own thread; destroying an absent reason is harmless.
        unsafe {
            let _ = ShutdownBlockReasonDestroy(self.hwnd);
        }
    }
}

/// Owns the session-end subclass of one top-level window.
pub struct SessionEndMonitor {
    hwnd: HWND,
    signal: Box<Rc<SessionEndSignal>>,
}
impl SessionEndMonitor {
    /// # Safety
    /// `raw` must be a live top-level window created on the current thread.
    pub unsafe fn attach(raw: isize, signal: Rc<SessionEndSignal>) -> windows::core::Result<Self> {
        let hwnd = HWND(raw as *mut _);
        let signal = Box::new(signal);
        // SAFETY: the boxed state outlives the subclass; Drop and WM_NCDESTROY remove it.
        unsafe {
            SetWindowSubclass(
                hwnd,
                Some(callback),
                SUBCLASS_ID,
                (&*signal as *const Rc<SessionEndSignal>) as usize,
            )
            .ok()?;
        }
        Ok(Self { hwnd, signal })
    }
}
impl Drop for SessionEndMonitor {
    fn drop(&mut self) {
        // SAFETY: removes only this subclass; a destroyed window already dropped it.
        unsafe {
            let _ = RemoveWindowSubclass(self.hwnd, Some(callback), SUBCLASS_ID);
            if self.signal.blocked.replace(false) {
                let _ = ShutdownBlockReasonDestroy(self.hwnd);
            }
        }
    }
}

unsafe extern "system" fn callback(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    if message == WM_NCDESTROY {
        // SAFETY: a subclass must be removed before its window is gone.
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(callback), SUBCLASS_ID);
        }
    } else if let Some(message) = SessionEndMessage::from_raw(message, wparam.0) {
        // The monitor owns this boxed state and removes the subclass before
        // releasing it. The clone keeps the signal alive across the flush.
        // SAFETY: `data` is the pointer registered by `attach`.
        let signal = unsafe { &*(data as *const Rc<SessionEndSignal>) }.clone();
        return LRESULT(signal.respond(&mut WindowHost { hwnd }, message));
    }
    // SAFETY: forwards the unchanged message to the next procedure in the chain.
    unsafe { DefSubclassProc(hwnd, message, wparam, lparam) }
}

/// Ask Windows to relaunch the editor after an update or reboot restart, but
/// not after a crash or hang. The session and recovery restore the documents.
pub fn register_application_restart() -> windows::core::Result<()> {
    // SAFETY: a static empty command line; the call only records process metadata.
    unsafe { RegisterApplicationRestart(w!(""), RESTART_NO_CRASH | RESTART_NO_HANG) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stands in for the window: an idle handler claims each request at once.
    struct Host<'a> {
        signal: &'a SessionEndSignal,
        idle: bool,
        complete: bool,
        delivered: u32,
        flushed: u32,
        blocked: u32,
        unblocked: u32,
    }
    impl<'a> Host<'a> {
        fn new(signal: &'a SessionEndSignal) -> Self {
            Self {
                signal,
                idle: true,
                complete: true,
                delivered: 0,
                flushed: 0,
                blocked: 0,
                unblocked: 0,
            }
        }
    }
    impl SessionEndHost for Host<'_> {
        fn deliver(&mut self) {
            self.delivered += 1;
            if self.idle && self.signal.take_request() {
                self.flushed += 1;
                self.signal.finish(self.complete);
            }
        }
        fn block(&mut self) -> bool {
            self.blocked += 1;
            true
        }
        fn unblock(&mut self) {
            self.unblocked += 1;
        }
    }

    #[test]
    fn raw_messages_map_to_session_end_notifications() {
        assert_eq!(
            SessionEndMessage::from_raw(WM_QUERYENDSESSION, 0),
            Some(SessionEndMessage::Query)
        );
        assert_eq!(
            SessionEndMessage::from_raw(WM_ENDSESSION, 1),
            Some(SessionEndMessage::End { ending: true })
        );
        assert_eq!(
            SessionEndMessage::from_raw(WM_ENDSESSION, 0),
            Some(SessionEndMessage::End { ending: false })
        );
        assert_eq!(SessionEndMessage::from_raw(WM_CLOSE, 0), None);
    }

    #[test]
    fn dirty_query_blocks_flushes_synchronously_and_never_vetoes() {
        let signal = SessionEndSignal::default();
        signal.set_dirty(true);
        let mut host = Host::new(&signal);
        assert_eq!(signal.respond(&mut host, SessionEndMessage::Query), 1);
        assert_eq!((host.blocked, host.delivered, host.flushed), (1, 1, 1));
        // A completed flush is not repeated; the block reason is released.
        assert_eq!(signal.respond(&mut host, SessionEndMessage::End { ending: true }), 0);
        assert_eq!((host.delivered, host.flushed, host.unblocked), (1, 1, 1));
        // The routed request was consumed, so a later close is an ordinary one.
        assert!(!signal.take_request());
    }

    #[test]
    fn clean_query_flushes_without_a_block_reason() {
        let signal = SessionEndSignal::default();
        let mut host = Host::new(&signal);
        assert_eq!(signal.respond(&mut host, SessionEndMessage::Query), 1);
        assert_eq!((host.blocked, host.flushed), (0, 1));
        signal.respond(&mut host, SessionEndMessage::End { ending: true });
        assert_eq!(host.unblocked, 0);
    }

    #[test]
    fn end_session_finishes_an_incomplete_flush() {
        let signal = SessionEndSignal::default();
        signal.set_dirty(true);
        let mut host = Host::new(&signal);
        host.complete = false;
        signal.respond(&mut host, SessionEndMessage::Query);
        host.complete = true;
        signal.respond(&mut host, SessionEndMessage::End { ending: true });
        assert_eq!((host.delivered, host.flushed, host.unblocked), (2, 2, 1));
    }

    #[test]
    fn cancelled_end_session_releases_the_block_without_flushing_again() {
        let signal = SessionEndSignal::default();
        signal.set_dirty(true);
        let mut host = Host::new(&signal);
        host.complete = false;
        signal.respond(&mut host, SessionEndMessage::Query);
        signal.respond(&mut host, SessionEndMessage::End { ending: false });
        assert_eq!((host.delivered, host.flushed, host.unblocked), (1, 1, 1));
    }

    #[test]
    fn busy_handler_claims_buffered_requests_later_and_ordinary_closes_stay_closes() {
        let signal = SessionEndSignal::default();
        let mut host = Host::new(&signal);
        host.idle = false;
        assert_eq!(signal.respond(&mut host, SessionEndMessage::Query), 1);
        signal.respond(&mut host, SessionEndMessage::End { ending: true });
        assert_eq!((host.delivered, host.flushed), (2, 0));
        // winit replays both buffered closes once the handler returns.
        assert!(signal.take_request());
        assert!(signal.take_request());
        assert!(!signal.take_request());
    }
}
