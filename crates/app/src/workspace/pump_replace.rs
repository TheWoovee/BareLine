// SPDX-License-Identifier: MPL-2.0
//! Replace completions for resident and paged documents (ARC-01).
use super::*;
use std::sync::mpsc::TryRecvError;
impl Workspace {
    /// Apply a prepared paged replacement once its worker answers.
    pub(super) fn pump_paged_replace(&mut self) -> bool {
        let mut changed = false;
        if let Some(ticket) = &self.pending_paged_replace {
            match ticket.try_recv() {
                Err(TryRecvError::Empty) => {}
                received => {
                    let cancelled = ticket.job.is_cancelled();
                    self.pending_paged_replace = None;
                    changed = true;
                    self.message = Some(match received {
                        Ok(Ok(prepared)) if !cancelled => {
                            let target = self.editors.iter_mut().find_map(|editor| match editor {
                                WorkspaceEditor::Paged(paged) if paged.snapshot().same_document(&prepared.source) => {
                                    Some(paged)
                                }
                                _ => None,
                            });
                            match target {
                                Some(paged) => match paged.apply_prepared(&prepared.source, prepared.transaction) {
                                    Ok(()) => "Applying paged replacements…".into(),
                                    Err(error) => error,
                                },
                                None => "Document closed; replacement not applied".into(),
                            }
                        }
                        Ok(Err(error)) => format!("Replacement not applied: {error}."),
                        _ => "Replacement cancelled".into(),
                    });
                    self.find.status = self.message.clone().unwrap_or_default();
                }
            }
        }
        changed
    }
    /// Apply a prepared resident replacement once its worker answers.
    pub(super) fn pump_replace(&mut self) -> bool {
        let mut changed = false;
        if let Some(ticket) = &self.pending_replace {
            match ticket.try_recv() {
                Err(TryRecvError::Empty) => {}
                received => {
                    let cancelled = ticket.job.is_cancelled();
                    self.pending_replace = None;
                    changed = true;
                    self.message = Some(match received {
                        Ok(Ok(prepared)) if !cancelled => {
                            let count = prepared.transaction.edits.len();
                            match self
                                .editors
                                .iter_mut()
                                .find(|editor| editor.snapshot().same_document(&prepared.source))
                            {
                                Some(editor) => match editor.apply_prepared(&prepared.source, prepared.transaction) {
                                    Ok(()) => {
                                        self.applying_replace = Some(prepared.source.identity_token().0);
                                        format!("Applying {count} replacements…")
                                    }
                                    Err(error) => error,
                                },
                                None => "Document was closed; replacement was not applied.".into(),
                            }
                        }
                        Ok(Err(error)) => format!("Replacement was not applied: {error}."),
                        _ => "Replacement cancelled.".into(),
                    });
                    // The find bar showed "Preparing replacement…"; never leave it there.
                    self.find.status = self.message.clone().unwrap_or_default();
                }
            }
        }
        changed
    }
    /// Report a replacement complete, or its document limit, once no
    /// document is still applying it.
    pub(super) fn finish_applied_replace(&mut self) {
        if self
            .message
            .as_deref()
            .is_some_and(|message| message.starts_with("Applying "))
            && !self.editors.iter().any(|editor| editor.busy())
        {
            // A document limit reached while applying is reported, not called complete.
            let failure = self.applying_replace.take().and_then(|document| {
                self.editors.iter().find_map(|editor| match editor {
                    WorkspaceEditor::Resident(editor) if editor.snapshot().identity_token().0 == document => {
                        editor.error.clone()
                    }
                    _ => None,
                })
            });
            let message = failure.unwrap_or_else(|| "Replacement complete.".into());
            if self.find.status.starts_with("Applying ") {
                self.find.status = message.clone();
            }
            self.message = Some(message);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::tests::PagedFileSystem;

    #[test]
    fn an_applied_replacement_is_reported_once_no_document_is_busy() {
        let mut workspace = Workspace::new(Arc::new(|| {}), Arc::new(PagedFileSystem)).unwrap();
        assert!(!workspace.pump_replace());
        assert!(!workspace.pump_paged_replace());
        workspace.message = Some("Applying 3 replacements…".into());
        workspace.find.status = "Applying 3 replacements…".into();
        workspace.finish_applied_replace();
        assert_eq!(workspace.message.as_deref(), Some("Replacement complete."));
        assert_eq!(workspace.find.status, "Replacement complete.");

        // The find bar keeps a status that is not about the replacement.
        workspace.message = Some("Applying paged replacements…".into());
        workspace.find.status = "3 matches".into();
        workspace.finish_applied_replace();
        assert_eq!(workspace.message.as_deref(), Some("Replacement complete."));
        assert_eq!(workspace.find.status, "3 matches");

        // Any other message is left alone.
        workspace.message = Some("Saved.".into());
        workspace.finish_applied_replace();
        assert_eq!(workspace.message.as_deref(), Some("Saved."));
    }
}
