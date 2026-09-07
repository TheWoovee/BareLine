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
    if config.smoke || config.perf || config.prototype {
        return Ok(Some(InstanceRuntime::default()));
    }
    let scope = config
        .settings_path
        .clone()
        .unwrap_or(std::env::current_exe()?);
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
        config.new_instance || config.no_extensions,
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
            .filter_map(|_| {
                self.instance
                    .server
                    .as_ref()
                    .and_then(InstanceServer::try_recv)
            })
            .collect();
        for pending in requests {
            if !pending.live() || self.session.closing() {
                continue;
            }
            if !pending.request.paths.is_empty() {
                if !self.ensure_workspace(el) || !self.launch.queue(&pending.request) {
                    continue;
                }
                let workspace = self.workspace.as_mut().unwrap();
                for path in &pending.request.paths {
                    if let Some(index) = (0..workspace.editors.len())
                        .find(|&index| workspace.path(index) == Some(path.as_path()))
                    {
                        self.app.active = index;
                    } else if !workspace.path_loading(path) {
                        workspace.open(path.clone());
                    }
                }
            }
            if let Some(window) = &self.window {
                window.set_minimized(false);
                window.focus_window();
                window.request_redraw();
            }
            pending.accept();
        }
    }
}
