// SPDX-License-Identifier: MPL-2.0
//! Settings conflict resolution: external edits found by the save worker,
//! and handing a scope to a newly installed document without losing a write
//! that is still in flight.
use super::storage::{DiskVersion, SaveOwner};
use super::*;
struct DeferredWorkspace {
    awaited: SaveOwner,
    path: PathBuf,
    requested: SettingsDocument,
}
struct ExternalChange {
    scope: Scope,
    owner_epoch: u64,
    path: PathBuf,
    disk: DiskVersion,
}
impl SettingsController {
    pub fn set_workspace_document(&mut self, path: PathBuf, document: SettingsDocument) {
        let active = self
            .storage
            .as_ref()
            .and_then(|storage| storage.active.as_ref())
            .filter(|owner| owner.scope == Scope::Workspace && owner.path == path)
            .cloned();
        if let Some(awaited) = active {
            self.deferred_workspace = Some(DeferredWorkspace {
                awaited,
                path,
                requested: document,
            });
            self.error = Some("Workspace settings are waiting for the current save to finish".into());
            return;
        }
        self.deferred_workspace = None;
        self.error = None;
        self.install_workspace_document(path, document);
    }
    fn install_workspace_document(&mut self, path: PathBuf, document: SettingsDocument) {
        self.revision = self.revision.wrapping_add(1);
        if let Some(storage) = &mut self.storage {
            storage.workspace_epoch = storage.workspace_epoch.wrapping_add(1);
            storage.workspace_disk = Some(DiskVersion::Unread);
            storage.request_baseline(
                Scope::Workspace,
                storage.workspace_epoch,
                path.clone(),
                Some(DiskVersion::Bytes(document.to_toml().into_bytes())),
            );
            storage.workspace = Some(path);
            storage.pending.retain(|job| job.scope != Scope::Workspace);
        }
        if self
            .external_change
            .as_ref()
            .is_some_and(|change| change.scope == Scope::Workspace)
        {
            self.external_change = None;
        }
        let mut editor = SettingsEditor::new(document);
        if self.open {
            editor.begin_session();
        }
        self.workspace = Some(editor);
        self.popup = None;
        if let Some(edit) = self.value_edit.take() {
            self.retired_fields.push(edit.field);
            self.focus.close_layer();
        }
    }
    fn apply_deferred_workspace(&mut self, owner: &SaveOwner, settled: SettingsDocument) -> bool {
        let matches = self.deferred_workspace.as_ref().is_some_and(|deferred| {
            deferred.awaited == *owner
                && !self.storage.as_ref().is_some_and(|storage| {
                    storage.pending.iter().any(|job| {
                        job.scope == owner.scope && job.owner_epoch == owner.owner_epoch && job.path == owner.path
                    })
                })
        });
        if !matches {
            return false;
        }
        let deferred = self.deferred_workspace.take().unwrap();
        debug_assert_eq!(deferred.path, owner.path);
        let _requested = deferred.requested;
        self.error = None;
        self.install_workspace_document(deferred.path, settled);
        true
    }
    /// Install a newly available user document without replacing the live tab,
    /// its search/navigation state, workspace settings, or storage worker.
    pub fn reconcile_user_document(&mut self, document: SettingsDocument, expected_revision: u64) -> bool {
        if self.revision != expected_revision || self.editing_value() {
            return false;
        }
        if self.storage.as_ref().is_some_and(|storage| {
            storage.active.as_ref().is_some_and(|owner| owner.scope == Scope::User)
                || storage.pending.iter().any(|job| job.scope == Scope::User)
        }) {
            return false;
        }
        self.revision = self.revision.wrapping_add(1);
        self.workspace_opted_in = config::resolve(&document, None, false, None)
            .values
            .workspace_preferences_enabled;
        if let Some(storage) = &mut self.storage {
            storage.user_epoch = storage.user_epoch.wrapping_add(1);
            storage.user_disk = DiskVersion::Unread;
            storage.request_baseline(
                Scope::User,
                storage.user_epoch,
                storage.user.clone(),
                Some(DiskVersion::Bytes(document.to_toml().into_bytes())),
            );
        }
        if self
            .external_change
            .as_ref()
            .is_some_and(|change| change.scope == Scope::User)
        {
            self.external_change = None;
        }
        let mut editor = SettingsEditor::new(document);
        if self.open {
            editor.begin_session();
        }
        self.user = editor;
        self.popup = None;
        if let Some(edit) = self.value_edit.take() {
            self.retired_fields.push(edit.field);
            self.focus.close_layer();
        }
        true
    }
    pub fn clear_workspace_document(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        self.workspace = None;
        self.scope = Scope::User;
        self.popup = None;
        self.deferred_workspace = None;
        self.error = None;
        if let Some(storage) = &mut self.storage {
            storage.workspace_epoch = storage.workspace_epoch.wrapping_add(1);
            storage.workspace = None;
            storage.workspace_disk = None;
            storage.pending.retain(|job| job.scope != Scope::Workspace);
        }
        if self
            .external_change
            .as_ref()
            .is_some_and(|change| change.scope == Scope::Workspace)
        {
            self.external_change = None;
        }
        if let Some(edit) = self.value_edit.take() {
            self.retired_fields.push(edit.field);
            self.focus.close_layer();
        }
    }
    pub fn has_external_change(&self) -> bool {
        self.external_change.is_some()
    }
    fn owns_external_change(&self, change: &ExternalChange) -> bool {
        self.storage.as_ref().is_some_and(|storage| {
            if change.scope == Scope::Workspace {
                storage.workspace.as_ref() == Some(&change.path) && storage.workspace_epoch == change.owner_epoch
            } else {
                storage.user == change.path && storage.user_epoch == change.owner_epoch
            }
        }) && self.editor_for_scope(change.scope).is_some()
    }
    pub fn reload_external_change(&mut self) -> bool {
        let Some(change) = self.external_change.take() else {
            return false;
        };
        if !self.owns_external_change(&change) {
            self.error = Some("The settings file awaiting a decision is no longer open".into());
            self.start_save();
            return false;
        }
        let document = match &change.disk {
            // A conflict always carries what the worker read, never `Unread`.
            DiskVersion::Absent | DiskVersion::Unread => SettingsDocument::empty(change.scope),
            DiskVersion::Bytes(bytes) => match SettingsDocument::parse(bytes, change.scope) {
                Ok(document) => document,
                Err(error) => {
                    self.error = Some(format!("Cannot reload changed settings: {error}"));
                    self.external_change = Some(change);
                    return false;
                }
            },
        };
        let settled = document.clone();
        let Some(editor) = self.editor_for_scope_mut(change.scope) else {
            self.error = Some("The changed settings scope is no longer available".into());
            return false;
        };
        editor.replace_from_disk(document);
        if let Some(storage) = &mut self.storage {
            storage.pending.retain(|job| job.scope != change.scope);
            if change.scope == Scope::Workspace {
                storage.workspace_disk = Some(change.disk);
            } else {
                storage.user_disk = change.disk;
            }
        }
        if change.scope == Scope::User {
            self.workspace_opted_in = config::resolve(&self.user.document, None, false, None)
                .values
                .workspace_preferences_enabled;
        }
        if change.scope == Scope::Workspace {
            self.apply_deferred_workspace(
                &SaveOwner {
                    scope: change.scope,
                    owner_epoch: change.owner_epoch,
                    path: change.path.clone(),
                },
                settled,
            );
        }
        self.revision = self.revision.wrapping_add(1);
        self.error = None;
        self.start_save();
        true
    }
    pub fn keep_after_external_change(&mut self) -> bool {
        let Some(change) = self.external_change.take() else {
            return false;
        };
        if !self.owns_external_change(&change) {
            self.error = Some("The settings file awaiting a decision is no longer open".into());
            self.start_save();
            return false;
        }
        if let Some(storage) = &mut self.storage {
            storage.pending.retain(|job| job.scope != change.scope);
            if change.scope == Scope::Workspace {
                storage.workspace_disk = Some(change.disk);
            } else {
                storage.user_disk = change.disk;
            }
        }
        self.error = None;
        self.queue_scope(change.scope);
        true
    }
}
