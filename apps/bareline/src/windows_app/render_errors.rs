// SPDX-License-Identifier: MPL-2.0
//! Render and layout failures surface once per kind as a non-modal notice
//! (APP-08). They are reported from the redraw path, where a modal dialog
//! re-enters WM_PAINT and fails again, so none is ever opened here.
use super::*;

/// The kinds of render failure already reported this session.
#[derive(Default)]
pub(super) struct RenderErrorLatch {
    reported: Vec<&'static str>,
}
impl RenderErrorLatch {
    /// True the first time `kind` fails; later failures of that kind stay quiet.
    pub(super) fn latch(&mut self, kind: &'static str) -> bool {
        if self.reported.contains(&kind) {
            return false;
        }
        self.reported.push(kind);
        true
    }
    /// True while no layer has failed this session.
    pub(super) fn is_clear(&self) -> bool {
        self.reported.is_empty()
    }
}
impl Shell {
    /// A layer that cannot be laid out or drawn is skipped for this frame and
    /// the rest of the window still draws. The first failure of each kind shows
    /// a persistent notice; repeats on later frames add nothing.
    pub(super) fn layer_failed(&mut self, el: &ActiveEventLoop, kind: &'static str, error: impl std::fmt::Display) {
        if self.smoke {
            // The smoke run exists to catch exactly this: fail it.
            self.fail(el, format!("{kind}: {error}"));
            return;
        }
        self.report_layer_failure(kind, &error.to_string());
    }
    pub(super) fn report_layer_failure(&mut self, kind: &'static str, error: &str) {
        if !self.render_errors.latch(kind) {
            return;
        }
        eprintln!("event=render_layer_failed kind={kind} error={error}");
        self.toasts.push_typed(
            format!("render-error:{kind}"),
            toast::next_revision(),
            bareline_ui::theme::ToastLevel::Error,
            toast::NotificationKind::Outcome,
            "Part of the window could not be drawn.",
            Some(format!(
                "{kind}: {error}\nThe rest of Bareline keeps working and your documents remain open. \
                 Save your work and restart Bareline if this persists."
            )),
            None,
            toast::NotificationLifetime::Persistent,
            Instant::now(),
        );
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn each_kind_latches_once() {
        let mut latch = RenderErrorLatch::default();
        assert!(latch.latch("editor layout"));
        assert!(!latch.latch("editor layout"));
        assert!(latch.latch("panel layout"));
        assert!(!latch.latch("panel layout"));
    }
    /// APP-08: a layout error that repeats on every frame yields one non-modal
    /// notice, not one per frame and never a modal dialog.
    #[test]
    fn a_persistent_layout_error_shows_one_notice() {
        let mut shell = super::super::accessibility::tests::headless_shell();
        for _ in 0..50 {
            shell.report_layer_failure("editor layout", "BackendFailure");
        }
        assert_eq!(shell.toasts.persistent_len(), 1);
        shell.report_layer_failure("toolbar layout", "BackendFailure");
        assert_eq!(shell.toasts.persistent_len(), 2);
        assert!(!shell.failed, "a layout error never ends the session");
    }
    /// A frame that presents with a skipped layer must not acknowledge the
    /// running release as healthy, or a broken update loses its rollback.
    #[test]
    fn a_latched_layer_failure_does_not_acknowledge_the_update() {
        let mut shell = super::super::accessibility::tests::headless_shell();
        shell.first_frame = true;
        let mut workspace = bareline_app::workspace::Workspace::new(
            std::sync::Arc::new(|| {}),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        shell.workspace = Some(workspace);
        assert!(shell.frame_acknowledges_update(), "a clean frame acknowledges");
        shell.report_layer_failure("editor layout", "BackendFailure");
        assert!(!shell.frame_acknowledges_update());
        // Later frames of the same failure stay quiet but still do not count.
        shell.report_layer_failure("editor layout", "BackendFailure");
        assert!(!shell.frame_acknowledges_update());
    }
}
