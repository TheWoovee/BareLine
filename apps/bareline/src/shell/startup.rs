// SPDX-License-Identifier: MPL-2.0
//! Startup sequencing as one explicit state machine (ARC-01).
//!
//! A launch moves through these phases in order and never back:
//!
//! 1. [`StartupPhase::AwaitingFirstFrame`]: nothing is on screen yet. Nothing
//!    that reads the profile, the session or the recovery root runs before the
//!    first frame (ADR-33).
//! 2. [`StartupPhase::SettlingProfile`]: the profile migration worker runs.
//!    Session restore, recovery discovery, macros and extensions wait for it.
//! 3. [`StartupPhase::RestoringSession`]: the previous session is restored.
//! 4. [`StartupPhase::OpeningLaunchRequests`]: command-line files, standard
//!    input and launches forwarded by other instances open on top of the
//!    restored session, never in place of it (APP-06).
//! 5. [`StartupPhase::OfferingRecovery`]: recovery discovery reports what it
//!    found, through the Recovery Center or a notice.
//! 6. [`StartupPhase::Ready`].
//!
//! A phase with nothing to do is passed on the same step, and a failure settles
//! its phase like a success: a profile worker that fails or cannot start, a
//! session file that is missing or refused, and a recovery discovery that fails
//! each let the launch continue with a notice.
//!
//! The live gates the phases are read from stay with their owners because they
//! also apply after startup: a profile migration retry makes profile storage
//! busy again, and loading a named session holds launch files exactly as the
//! startup restore does. The sequence owns what only startup has: whether a
//! frame was presented, whether command-line files still wait, which startup
//! documents a frame opens, and when a close may write the session file.
use super::*;

/// Where a launch is in its startup sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum StartupPhase {
    AwaitingFirstFrame,
    SettlingProfile,
    RestoringSession,
    OpeningLaunchRequests,
    OfferingRecovery,
    Ready,
}
impl StartupPhase {
    /// The name written to the diagnostic stream when the phase is entered.
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::AwaitingFirstFrame => "awaiting-first-frame",
            Self::SettlingProfile => "settling-profile",
            Self::RestoringSession => "restoring-session",
            Self::OpeningLaunchRequests => "opening-launch-requests",
            Self::OfferingRecovery => "offering-recovery",
            Self::Ready => "ready",
        }
    }
}

/// What the live runtimes report on one step of the sequence.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct StartupSignals {
    /// The profile worker finished, failed, or had nothing to do.
    pub(super) profile_settled: bool,
    /// The session file is being read or its documents are being opened.
    pub(super) session_loading: bool,
    /// The previous session is restored, failed to load, or is not restored by
    /// this launch.
    pub(super) session_settled: bool,
    /// Standard input has not been opened as a document yet.
    pub(super) stdin_pending: bool,
    /// Queued launch requests have not finished opening.
    pub(super) launch_requests_pending: bool,
    /// Recovery discovery reported its list or its failure.
    pub(super) recovery_reported: bool,
    /// A smoke, prototype or performance launch, which opens no startup files.
    pub(super) unattended: bool,
    /// A workspace with its document list exists.
    pub(super) workspace_open: bool,
}

/// What a frame does about the startup documents.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StartupDocuments {
    /// Nothing on this frame.
    Wait,
    /// Open the command-line files and standard input on top of the session.
    OpenLaunchRequests,
    /// No file was requested and no document is open: start with a blank one.
    OpenBlank,
}

/// The startup state machine owned by the shell.
pub(super) struct StartupSequence {
    phase: StartupPhase,
    /// Files were named on the command line and not yet handed to the launch
    /// queue.
    launch_files: bool,
}
impl Default for StartupSequence {
    fn default() -> Self {
        Self::new(false)
    }
}
impl StartupSequence {
    pub(super) fn new(launch_files: bool) -> Self {
        Self {
            phase: StartupPhase::AwaitingFirstFrame,
            launch_files,
        }
    }
    #[cfg(test)]
    pub(super) fn phase(&self) -> StartupPhase {
        self.phase
    }
    /// A frame has been presented.
    pub(super) fn presented(&self) -> bool {
        self.phase > StartupPhase::AwaitingFirstFrame
    }
    /// Records the first presented frame; later frames change nothing.
    pub(super) fn mark_first_frame(&mut self) {
        if !self.presented() {
            self.phase = StartupPhase::SettlingProfile;
        }
    }
    /// Command-line files still wait to be handed to the launch queue.
    pub(super) fn launch_files_waiting(&self) -> bool {
        self.launch_files
    }
    /// The command-line files were handed to the launch queue.
    pub(super) fn launch_files_opened(&mut self) {
        self.launch_files = false;
    }
    /// What this frame opens. Command-line files and standard input wait for
    /// the session restore and open on top of it, as in Notepad++ (APP-06);
    /// a launch without them starts with a blank document while the restore
    /// has not begun reading.
    pub(super) fn documents(&self, signals: &StartupSignals) -> StartupDocuments {
        if !self.presented() || signals.session_loading || signals.unattended {
            StartupDocuments::Wait
        } else if self.launch_files || signals.stdin_pending {
            if signals.session_settled {
                StartupDocuments::OpenLaunchRequests
            } else {
                StartupDocuments::Wait
            }
        } else if signals.workspace_open {
            StartupDocuments::Wait
        } else {
            StartupDocuments::OpenBlank
        }
    }
    /// Moves through every phase whose work is done and returns the phase
    /// entered, or `None` when the phase did not change. Before the first frame
    /// nothing advances, so a launch that fails to create its window or
    /// renderer never starts profile, session or recovery work.
    pub(super) fn advance(&mut self, signals: &StartupSignals) -> Option<StartupPhase> {
        let before = self.phase;
        loop {
            self.phase = match self.phase {
                StartupPhase::SettlingProfile if signals.profile_settled => StartupPhase::RestoringSession,
                StartupPhase::RestoringSession if signals.session_settled => StartupPhase::OpeningLaunchRequests,
                StartupPhase::OpeningLaunchRequests
                    if signals.unattended
                        || (!self.launch_files && !signals.stdin_pending && !signals.launch_requests_pending) =>
                {
                    StartupPhase::OfferingRecovery
                }
                StartupPhase::OfferingRecovery if signals.recovery_reported => StartupPhase::Ready,
                _ => break,
            };
        }
        (self.phase != before).then_some(self.phase)
    }
    /// Whether a close may write the session file now. A close before the first
    /// frame, or before the previous session has finished restoring, exits
    /// without replacing the saved session with a partial restore or with only
    /// the command-line files (APP-06).
    pub(super) fn close_saves_session(&self, session_settled: bool) -> bool {
        self.presented() && session_settled
    }
}

impl Shell {
    fn startup_signals(&self) -> StartupSignals {
        StartupSignals {
            profile_settled: self.profile.settled(),
            session_loading: self.session.startup_pending(),
            session_settled: self.session.restore_settled(),
            stdin_pending: self.launch.has_stdin(),
            launch_requests_pending: self.launch.has_requests(),
            recovery_reported: self.recovery.discovery_reported(),
            unattended: self.smoke || self.prototype.is_some() || self.performance.enabled(),
            workspace_open: self.workspace.is_some(),
        }
    }
    /// Opens the startup documents the sequence asks for on this frame.
    pub(super) fn startup_documents(&mut self, el: &ActiveEventLoop) {
        match self.startup.documents(&self.startup_signals()) {
            StartupDocuments::Wait => {}
            StartupDocuments::OpenLaunchRequests => {
                if self.ensure_workspace(el) {
                    self.startup.launch_files_opened();
                    self.launch_pump();
                    self.window.as_ref().unwrap().request_redraw();
                }
            }
            StartupDocuments::OpenBlank => self.dispatch(el, Action::New),
        }
    }
    /// Advances the startup sequence from the live runtimes and reports each
    /// phase it enters on the diagnostic stream.
    pub(super) fn advance_startup(&mut self) {
        let signals = self.startup_signals();
        if let Some(phase) = self.startup.advance(&signals) {
            eprintln!("event=startup_phase phase={}", phase.name());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settled() -> StartupSignals {
        StartupSignals {
            profile_settled: true,
            session_settled: true,
            recovery_reported: true,
            ..Default::default()
        }
    }

    #[test]
    fn phases_run_in_order_and_wait_for_each_step() {
        let mut startup = StartupSequence::new(true);
        let mut signals = StartupSignals::default();
        startup.mark_first_frame();
        assert_eq!(startup.phase(), StartupPhase::SettlingProfile);
        assert_eq!(startup.advance(&signals), None, "the profile worker is still running");
        signals.profile_settled = true;
        assert_eq!(startup.advance(&signals), Some(StartupPhase::RestoringSession));
        signals.session_loading = true;
        assert_eq!(startup.advance(&signals), None, "the session is still restoring");
        signals.session_loading = false;
        signals.session_settled = true;
        signals.launch_requests_pending = true;
        assert_eq!(startup.advance(&signals), Some(StartupPhase::OpeningLaunchRequests));
        startup.launch_files_opened();
        assert_eq!(startup.advance(&signals), None, "queued launch files are still opening");
        signals.launch_requests_pending = false;
        assert_eq!(startup.advance(&signals), Some(StartupPhase::OfferingRecovery));
        signals.recovery_reported = true;
        assert_eq!(startup.advance(&signals), Some(StartupPhase::Ready));
        assert_eq!(startup.advance(&signals), None);
    }

    #[test]
    fn a_launch_with_nothing_to_wait_for_is_ready_in_one_step() {
        let mut startup = StartupSequence::new(false);
        startup.mark_first_frame();
        assert_eq!(startup.advance(&settled()), Some(StartupPhase::Ready));
    }

    #[test]
    fn phases_never_move_back() {
        let mut startup = StartupSequence::new(false);
        startup.mark_first_frame();
        assert_eq!(startup.advance(&settled()), Some(StartupPhase::Ready));
        // A later profile migration retry or named session load reopens the
        // live gates, not startup.
        assert_eq!(startup.advance(&StartupSignals::default()), None);
        assert_eq!(startup.phase(), StartupPhase::Ready);
        startup.mark_first_frame();
        assert_eq!(startup.phase(), StartupPhase::Ready);
    }

    /// A launch whose window, accessibility provider or renderer cannot be
    /// created fails before the first frame: no startup work is started and no
    /// document is opened.
    #[test]
    fn a_launch_that_fails_before_its_first_frame_starts_nothing() {
        let startup_signals = StartupSignals {
            stdin_pending: true,
            ..settled()
        };
        let mut startup = StartupSequence::new(true);
        assert!(!startup.presented());
        assert_eq!(startup.advance(&startup_signals), None);
        assert_eq!(startup.phase(), StartupPhase::AwaitingFirstFrame);
        assert_eq!(startup.documents(&startup_signals), StartupDocuments::Wait);
        assert_eq!(startup.documents(&settled()), StartupDocuments::Wait);
    }

    /// A profile worker that fails, a session file that is missing or refused,
    /// and a recovery discovery that fails all settle their phase, so the
    /// launch continues to its files and to Ready with only a notice.
    #[test]
    fn failed_steps_settle_their_phase() {
        let mut startup = StartupSequence::new(true);
        startup.mark_first_frame();
        let failed_profile = StartupSignals {
            profile_settled: true,
            ..Default::default()
        };
        assert_eq!(startup.advance(&failed_profile), Some(StartupPhase::RestoringSession));
        let failed_restore = StartupSignals {
            session_settled: true,
            ..failed_profile
        };
        assert_eq!(startup.documents(&failed_restore), StartupDocuments::OpenLaunchRequests);
        assert_eq!(
            startup.advance(&failed_restore),
            Some(StartupPhase::OpeningLaunchRequests)
        );
        startup.launch_files_opened();
        let failed_discovery = StartupSignals {
            recovery_reported: true,
            ..failed_restore
        };
        assert_eq!(startup.advance(&failed_discovery), Some(StartupPhase::Ready));
    }

    /// APP-06: command-line files and standard input never open before the
    /// session restore has settled; without them a blank document opens while
    /// the session file has not begun loading.
    #[test]
    fn startup_documents_follow_the_session_restore() {
        let mut files = StartupSequence::new(true);
        files.mark_first_frame();
        let restoring = StartupSignals {
            profile_settled: true,
            ..Default::default()
        };
        assert_eq!(files.documents(&restoring), StartupDocuments::Wait);
        let loading = StartupSignals {
            session_loading: true,
            ..restoring
        };
        assert_eq!(files.documents(&loading), StartupDocuments::Wait);
        assert_eq!(files.documents(&settled()), StartupDocuments::OpenLaunchRequests);
        files.launch_files_opened();
        let opened = StartupSignals {
            workspace_open: true,
            ..settled()
        };
        assert_eq!(files.documents(&opened), StartupDocuments::Wait);

        let mut stdin = StartupSequence::new(false);
        stdin.mark_first_frame();
        let piped = StartupSignals {
            stdin_pending: true,
            ..restoring
        };
        assert_eq!(stdin.documents(&piped), StartupDocuments::Wait);
        let piped = StartupSignals {
            session_settled: true,
            ..piped
        };
        assert_eq!(stdin.documents(&piped), StartupDocuments::OpenLaunchRequests);

        let mut blank = StartupSequence::new(false);
        blank.mark_first_frame();
        assert_eq!(blank.documents(&restoring), StartupDocuments::OpenBlank);
        assert_eq!(blank.documents(&loading), StartupDocuments::Wait);
        let open = StartupSignals {
            workspace_open: true,
            ..restoring
        };
        assert_eq!(blank.documents(&open), StartupDocuments::Wait);
    }

    #[test]
    fn unattended_launches_open_no_startup_documents_and_still_finish() {
        let unattended = StartupSignals {
            unattended: true,
            ..settled()
        };
        let mut startup = StartupSequence::new(true);
        startup.mark_first_frame();
        assert_eq!(startup.documents(&unattended), StartupDocuments::Wait);
        assert_eq!(startup.advance(&unattended), Some(StartupPhase::Ready));
        assert!(startup.launch_files_waiting());
    }

    /// Closing early exits without writing the session file: before the first
    /// frame, and while the previous session is still restoring (APP-06).
    #[test]
    fn an_early_close_never_writes_the_session() {
        let mut startup = StartupSequence::new(true);
        assert!(!startup.close_saves_session(true), "closed before the first frame");
        startup.mark_first_frame();
        assert!(!startup.close_saves_session(false), "closed during the session restore");
        // A cancelled close leaves the sequence where it was.
        assert_eq!(startup.phase(), StartupPhase::SettlingProfile);
        assert!(startup.close_saves_session(true));
    }

    /// The shell reads the sequence's signals from its live runtimes.
    #[test]
    fn shell_holds_launch_files_until_the_session_restore_settles() {
        let mut shell = super::super::accessibility::tests::headless_shell();
        shell.startup = StartupSequence::new(true);
        let session = std::env::temp_dir().join("bareline-startup-sequence-session.json");
        shell.session.configure(Some(session.clone()), Some(session), true);
        assert_eq!(
            shell.startup.documents(&shell.startup_signals()),
            StartupDocuments::Wait
        );
        shell.advance_startup();
        assert_eq!(shell.startup.phase(), StartupPhase::AwaitingFirstFrame);
        shell.startup.mark_first_frame();
        // No profile migration in this launch, but the restore has not run.
        shell.advance_startup();
        assert_eq!(shell.startup.phase(), StartupPhase::RestoringSession);
        assert_eq!(
            shell.startup.documents(&shell.startup_signals()),
            StartupDocuments::Wait
        );
        assert!(!shell.startup.close_saves_session(shell.session.restore_settled()));
        // Profile reconciliation found no session to restore.
        assert!(shell.session.set_restore_path(None));
        assert_eq!(
            shell.startup.documents(&shell.startup_signals()),
            StartupDocuments::OpenLaunchRequests
        );
        assert!(shell.startup.close_saves_session(shell.session.restore_settled()));
        shell.startup.launch_files_opened();
        shell.advance_startup();
        // Recovery discovery has not reported yet.
        assert_eq!(shell.startup.phase(), StartupPhase::OfferingRecovery);
    }
}
