// SPDX-License-Identifier: MPL-2.0
//! Startup handoff and native request consumer. Only the elected owner saves the shared session.
use super::*;
use bareline_platform_windows::instance::{InstanceServer, OpenRequest, Outcome};

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
    let scope = config.settings_path.clone().unwrap_or(std::env::current_exe()?);
    let request = OpenRequest {
        paths: config.paths.clone(),
        line: config.line,
        column: config.column,
        read_only: config.read_only,
        monitor: config.monitor,
    };
    // A running extension-enabled process cannot honor --no-extensions for just one request.
    let outcome = bareline_platform_windows::instance::coordinate(
        &scope,
        request,
        config.new_instance || config.no_extensions || config.no_session,
        notify,
    )?;
    Ok(match outcome {
        Outcome::Forwarded => None,
        Outcome::Primary(server) => Some(InstanceRuntime {
            server: Some(server),
            message: None,
        }),
        Outcome::Independent(message) => {
            config.session_path = None;
            config.no_session = true;
            config.recovery_path = config
                .recovery_path
                .take()
                .map(|root| root.join("instances").join(std::process::id().to_string()));
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
    pub(super) fn instance_pump(&mut self, el: &ActiveEventLoop) {
        if !self.first_frame {
            return;
        }
        if let Some(message) = self.instance.message.take() {
            if let Some(workspace) = &mut self.workspace {
                workspace.message = Some(message);
            } else {
                self.instance.message = Some(message);
            }
        }
        let requests: Vec<_> = (0..16)
            .filter_map(|_| self.instance.server.as_ref().and_then(InstanceServer::try_recv))
            .collect();
        for pending in requests {
            if !pending.live() || self.session.closing() {
                continue;
            }
            let mut request_ids = Vec::new();
            if !pending.request.paths.is_empty() {
                if !self.ensure_workspace(el) {
                    continue;
                }
                let Some(accepted) = self.launch.queue(&pending.request) else {
                    if let Some(workspace) = &mut self.workspace {
                        workspace.message =
                            Some("Open request rejected: 256 launch operations are still outstanding.".into());
                    }
                    continue;
                };
                request_ids = accepted;
            }
            if !pending.accept() {
                self.launch.cancel_requests(&request_ids);
                continue;
            }
            if !request_ids.is_empty() {
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
