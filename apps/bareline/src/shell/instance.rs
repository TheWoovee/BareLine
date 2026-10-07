// SPDX-License-Identifier: MPL-2.0
//! Startup handoff and native request consumer. Only the elected owner saves the shared session.
use super::*;
use crate::shell::native::instance::{InstanceServer, OpenRequest, Outcome};

#[derive(Default)]
pub(super) struct InstanceRuntime {
    server: Option<InstanceServer>,
    message: Option<String>,
}

/// None means that another instance accepted this launch and no window should be created.
pub(super) fn prepare(
    config: &mut launch::LaunchConfig,
    notify: std::sync::Arc<dyn Fn() + Send + Sync>,
) -> Result<Option<InstanceRuntime>, Box<dyn std::error::Error>> {
    if bypass_single_instance(
        config.mode,
        config.smoke,
        config.perf,
        config.prototype,
        config.diag_handles,
    ) {
        return Ok(Some(InstanceRuntime::default()));
    }
    // Piped text with no files opens its own window without the handoff: an
    // empty forwarded request would only raise a running instance to compete
    // with this window for focus. Like a forwarded launch with piped text, it
    // neither restores nor writes the shared session, even with no instance
    // running, as `launch::HELP` states (APP-06, APP-09).
    if config.stdin.is_some() && config.paths.is_empty() {
        config.session_path = None;
        config.no_session = true;
        return Ok(Some(InstanceRuntime::default()));
    }
    let scope = config.settings_path.clone().unwrap_or(std::env::current_exe()?);
    let profile = config.settings_path.as_deref().and_then(std::path::Path::parent);
    let request = OpenRequest {
        paths: config.paths.clone(),
        line: config.line,
        column: config.column,
        read_only: config.read_only,
        monitor: config.monitor,
    };
    // A running extension-enabled process cannot honor --no-extensions for just one request.
    let outcome = crate::shell::native::instance::coordinate(
        &scope,
        profile,
        request,
        config.new_instance || config.no_extensions || config.no_session,
        notify,
    )?;
    Ok(match outcome {
        // Piped text cannot cross the handoff: the running instance took the
        // files, and this window shows the text on its own (APP-09).
        Outcome::Forwarded if config.stdin.is_some() => {
            config.paths.clear();
            config.session_path = None;
            config.no_session = true;
            Some(InstanceRuntime::default())
        }
        Outcome::Forwarded => None,
        Outcome::Primary(server) => Some(InstanceRuntime {
            server: Some(server),
            message: None,
        }),
        Outcome::Independent(message) => {
            // Journals stay in the shared recovery root: their directory names already
            // carry this process id, and discovery only offers journals of ended processes.
            config.session_path = None;
            config.no_session = true;
            Some(InstanceRuntime {
                server: None,
                message: Some(message),
            })
        }
    })
}

fn bypass_single_instance(
    mode: launch::LaunchMode,
    smoke: bool,
    perf: bool,
    prototype: bool,
    diag_handles: bool,
) -> bool {
    smoke
        || perf
        || prototype
        || diag_handles
        || matches!(mode, launch::LaunchMode::Diagnostic | launch::LaunchMode::Performance)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LNX-EDIT-001: the notice went into the shared message slot, which the
    /// spelling notice overwrote at every launch.
    #[test]
    fn the_separate_window_notice_is_a_toast_that_later_messages_do_not_replace() {
        let mut shell = crate::shell::accessibility::tests::headless_shell();
        let mut workspace = bareline_app::workspace::Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(crate::shell::native::FileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        shell.workspace = Some(workspace);
        shell.instance.message = Some(
            "This window runs separately from other Bareline windows; its tabs are not restored next time.".into(),
        );
        let before = shell.toasts.persistent_len();
        shell.instance_notice();
        assert!(shell.instance.message.is_none());
        assert_eq!(shell.toasts.persistent_len(), before + 1);
        assert_eq!(shell.workspace.as_ref().unwrap().message, None);
        // A later status line leaves the notice on screen.
        shell.workspace.as_mut().unwrap().message = Some("This system does not support spell checking yet".into());
        assert_eq!(shell.toasts.persistent_len(), before + 1);
    }

    #[test]
    fn portable_handle_diagnostics_stay_out_of_single_instance_forwarding() {
        assert!(bypass_single_instance(
            launch::LaunchMode::Portable,
            false,
            false,
            false,
            true,
        ));
    }
}

impl Shell {
    /// Call where the application decides to exit, before any prompt and again right
    /// before exiting. From here on, new launches are turned away and open on their
    /// own. Launches the pipe workers already acknowledged would be lost with this
    /// process, so while any remains this returns false and the exit must not happen.
    pub(super) fn instance_exit_ready(&self) -> bool {
        self.instance.server.as_ref().is_none_or(|server| server.quiesce() == 0)
    }
    /// Accepts launches again unless an application close is still under way: its
    /// saves or discards are pending, or the session is saving for the exit.
    pub(super) fn instance_resume(&self) {
        let closing = self.session.closing() || !matches!(self.pending_close, None | Some(PendingClose::Document(_)));
        if let Some(server) = &self.instance.server {
            server.set_accepting(!closing);
        }
    }
    /// Cancels an exit that `instance_exit_ready` refused and opens the acknowledged
    /// launches instead. The caller has already ended any session save for the exit.
    pub(super) fn instance_exit_cancelled(&mut self, el: &ActiveEventLoop) {
        // Accepts again and drains, as no close is under way any more.
        self.instance_pump(el);
        let message = "Files were opened while Bareline was closing. Close again to exit.".to_string();
        match &mut self.workspace {
            Some(workspace) => workspace.message = Some(message),
            None => self.instance.message = Some(message),
        }
    }
    /// Shows the instance notice (this window runs separately and its tabs are
    /// not restored, or files arrived while closing) as a toast that stays until
    /// dismissed: in the shared message slot the next status, such as the
    /// spelling notice at startup, replaced it before anyone read it.
    fn instance_notice(&mut self) {
        if let Some(message) = self.instance.message.take() {
            self.startup_notice(
                "instance:notice",
                bareline_ui::theme::ToastLevel::Warning,
                message.clone(),
                message,
            );
        }
    }
    pub(super) fn instance_pump(&mut self, el: &ActiveEventLoop) {
        // A closing owner turns new launches away at once so they open on their own.
        // Requests it already acknowledged stay queued: the exit waits for them.
        // Once the exit is under way, the refusal it set lasts until the process ends.
        if el.exiting() {
            return;
        }
        self.instance_resume();
        if !self.startup.presented() || self.session.closing() {
            return;
        }
        self.instance_notice();
        // A prompt or file dialog waits for its answer (Linux): the requests stay
        // queued, as they do behind a modal dialog on Windows, so the command
        // that asked runs again on the document it asked about.
        if self.interaction_hold() {
            return;
        }
        // Every queued request was acknowledged by the pipe worker, so each one is
        // acted on here; none is dropped for having waited (APP-03).
        while let Some(request) = self.instance.server.as_ref().and_then(InstanceServer::try_recv) {
            if !request.paths.is_empty() {
                if !self.ensure_workspace(el) {
                    continue;
                }
                if self.launch.queue(&request).is_none() {
                    if let Some(workspace) = &mut self.workspace {
                        workspace.message =
                            Some("Open request rejected: 256 launch operations are still outstanding.".into());
                    }
                    continue;
                }
                self.launch_pump();
            }
            if let Some(window) = &self.window {
                // Unhide before un-minimizing: a window parked in the tray is
                // hidden, and focus does not restore a hidden window.
                window.set_visible(true);
                window.set_minimized(false);
                window.focus_window();
                window.request_redraw();
            }
        }
    }
}
