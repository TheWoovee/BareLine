// SPDX-License-Identifier: MPL-2.0
//! Profile storage: the migration and cleanup worker that runs after the first
//! frame, and the profile and legacy paths whose read authority it decides for
//! the session, recovery, extensions and macros (ARC-01).
use super::*;

/// The profile initialization worker and the paths it chooses between.
#[derive(Default)]
pub(super) struct ProfileRuntime {
    initialization: super::launch::ProfileInitializationRuntime,
    settings_path: Option<PathBuf>,
    /// The settings revision the migrated settings are reconciled against.
    pub(super) settings_revision: u64,
    extensions_path: Option<PathBuf>,
    legacy_settings_path: Option<PathBuf>,
    legacy_session_path: Option<PathBuf>,
    legacy_recovery_path: Option<PathBuf>,
    legacy_extensions_path: Option<PathBuf>,
}
impl ProfileRuntime {
    pub(super) fn new(launch: &mut super::launch::LaunchConfig) -> Self {
        Self {
            initialization: super::launch::ProfileInitializationRuntime::new(launch.profile_initialization.take()),
            settings_path: launch.settings_path.clone(),
            settings_revision: 0,
            extensions_path: launch.extensions_path.clone(),
            legacy_settings_path: launch.legacy_settings_path.clone(),
            legacy_session_path: launch.legacy_session_path.clone(),
            legacy_recovery_path: launch.legacy_recovery_path.clone(),
            legacy_extensions_path: launch.legacy_extensions_path.clone(),
        }
    }
    /// Profile storage is not being migrated or reconciled. Readers of the
    /// session, recovery, macros and extensions wait while it is, including
    /// during a Retry Profile Migration after startup.
    pub(super) fn settled(&self) -> bool {
        self.initialization.settled()
    }
    /// Starts the profile worker after the first frame (ADR-33).
    pub(super) fn schedule(&mut self, notify: std::sync::Arc<dyn Fn() + Send + Sync>) -> Result<bool, String> {
        self.initialization.schedule(notify)
    }
}

impl Shell {
    /// Reconciles the profile worker's result once it arrives.
    pub(super) fn profile_pump(&mut self) {
        if self.profile.initialization.pump() {
            match self.profile.initialization.take_completion() {
                Some(Ok(result)) => self.reconcile_profile_initialization(result),
                Some(Err(error)) => self.profile_initialization_message(error),
                None => {}
            }
        }
    }

    /// `profile.migration.retry`: runs the profile worker again.
    pub(super) fn profile_retry_migration(&mut self) {
        let result = self
            .profile
            .initialization
            .retry()
            .and_then(|()| self.profile.initialization.schedule(self.notify.clone()).map(|_| ()));
        match result {
            Ok(()) => self.profile_initialization_message("Profile migration retry started".into()),
            Err(error) => self.profile_initialization_message(error),
        }
    }

    /// Without APPDATA or LOCALAPPDATA there is no legacy/local pair to migrate
    /// and the report is empty: the one profile root in use owns every item,
    /// so session, recovery, extensions and macros stay enabled (APP-16).
    fn item_authority(
        report: &bareline_file_io::profile_migration::MigrationReport,
        name: &str,
    ) -> Option<bareline_file_io::profile_migration::ReadAuthority> {
        if report.items.is_empty() {
            return Some(bareline_file_io::profile_migration::ReadAuthority::Local);
        }
        report.authority(name)
    }

    fn migrated_item_path(
        report: &bareline_file_io::profile_migration::MigrationReport,
        name: &str,
        local: Option<PathBuf>,
        legacy: Option<PathBuf>,
    ) -> Option<PathBuf> {
        match Self::item_authority(report, name) {
            Some(bareline_file_io::profile_migration::ReadAuthority::Local) => local,
            Some(bareline_file_io::profile_migration::ReadAuthority::Legacy) => legacy,
            _ => None,
        }
    }

    fn reconcile_profile_initialization(&mut self, result: super::launch::ProfileInitializationResult) {
        eprintln!(
            "event=profile_cleanup roots={} candidates={} removed={} visited={} limit={} cancelled={}",
            result.cleanup.roots,
            result.cleanup.candidates,
            result.cleanup.removed,
            result.cleanup.visited_entries,
            result.cleanup.limit_reached,
            result.cleanup.cancelled
        );
        let authorities = result.authorities;
        let report = match result.migration {
            Ok(report) => report,
            Err(error) => {
                self.profile_initialization_message(format!(
                    "Profile migration paused: {error}. Use Retry Profile Migration; legacy data was retained."
                ));
                Default::default()
            }
        };

        // The worker read the settings file migration published (APP-12).
        if let Some(migrated) = result.migrated_settings {
            match migrated.map(|document| {
                self.settings
                    .reconcile_migrated_user(document, self.profile.settings_revision)
            }) {
                Ok(true) => self.profile.settings_revision = self.settings.controller.revision,
                Ok(false) => self.profile_initialization_message(
                    "Migrated settings were retained, but settings changed after startup; the newer live settings remain active."
                        .into(),
                ),
                Err(error) => self.profile_initialization_message(format!(
                    "Migrated settings could not be applied: {error}. Use Retry Profile Migration."
                )),
            }
        }

        let local_session = self
            .profile
            .settings_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|root| root.join("session.json"));
        let session_path = Self::migrated_item_path(
            &authorities,
            "session.json",
            local_session,
            self.profile.legacy_session_path.clone(),
        );
        if !self.session.set_restore_path(session_path) {
            self.profile_initialization_message(
                "Profile session migration completed after session restore began; the retained session will be considered on restart."
                    .into(),
            );
        }

        let recovery_root = Self::migrated_item_path(
            &authorities,
            "recovery",
            self.recovery_root.clone(),
            self.profile.legacy_recovery_path.clone(),
        );
        let recovery_mutation_allowed = Self::item_authority(&authorities, "recovery")
            == Some(bareline_file_io::profile_migration::ReadAuthority::Local);
        self.recovery.configure(recovery_root, recovery_mutation_allowed);

        let extensions_root = Self::migrated_item_path(
            &authorities,
            "extensions",
            self.profile.extensions_path.clone(),
            self.profile.legacy_extensions_path.clone(),
        );
        let extensions_local = Self::item_authority(&authorities, "extensions")
            == Some(bareline_file_io::profile_migration::ReadAuthority::Local);
        self.extensions
            .set_profile_root_before_restore(extensions_root, extensions_local);

        let local_macros = self
            .profile
            .settings_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|root| root.join("macros"));
        let legacy_macros = self
            .profile
            .legacy_settings_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(|root| root.join("macros"));
        let macros_root = Self::migrated_item_path(&authorities, "macros", local_macros, legacy_macros);
        self.macros.set_profile_read_directory(macros_root);

        if report.retryable || report.conflicts {
            let retained = report
                .items
                .iter()
                .filter_map(|item| item.retained_source.as_ref())
                .next()
                .map_or_else(|| "the legacy profile".into(), |path| path.display().to_string());
            self.profile_initialization_message(format!(
                "Profile migration needs attention. Use Retry Profile Migration; source data was retained at {retained}."
            ));
        }
        // Readers that were gated on the maintenance receipt get a fresh pump
        // only after their per-item read authority has been installed.
        (self.notify)();
    }

    pub(super) fn profile_initialization_message(&mut self, message: String) {
        eprintln!("event=profile_migration_notice message={message}");
        let revision = toast::next_revision();
        self.toasts.push_typed(
            format!("profile-initialization-{revision}"),
            revision,
            bareline_ui::theme::ToastLevel::Error,
            toast::NotificationKind::Outcome,
            message,
            None,
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
mod profile_authority_tests {
    use super::super::Shell;
    use bareline_file_io::profile_migration::{MigrationReport, ReadAuthority};
    use std::path::PathBuf;

    #[test]
    fn missing_appdata_roots_keep_every_profile_item_local() {
        // With APPDATA or LOCALAPPDATA unset the worker reports no items; the
        // single profile root must still own recovery, session and extensions.
        let report = MigrationReport::default();
        let local = PathBuf::from(r"C:\profile\item");
        for name in ["settings.toml", "session.json", "recovery", "macros", "extensions"] {
            assert_eq!(Shell::item_authority(&report, name), Some(ReadAuthority::Local));
            assert_eq!(
                Shell::migrated_item_path(&report, name, Some(local.clone()), None),
                Some(local.clone())
            );
        }
    }
}
