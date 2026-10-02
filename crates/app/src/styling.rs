// SPDX-License-Identifier: MPL-2.0
//! A bounded syntax cache per view, reusing the resident and paged workers.
mod paged;
use bareline_document::{DocumentSnapshot, TextOffset};
use bareline_syntax::{Checkpoint, Language, MAX_REQUEST_BYTES, SyntaxResult, SyntaxTicket, SyntaxWorker};
use std::{
    ops::Range,
    sync::{Arc, mpsc::TryRecvError},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StylingReceipt {
    pub identity: (u64, u64),
    pub language: Language,
    pub range: Range<TextOffset>,
    pub ready: bool,
    pub unavailable: bool,
}
#[derive(Default)]
pub struct Styling {
    view_label: String,
    paged: Option<paged::Job>,
    pub paged_folds: Option<(Vec<bareline_syntax::folding::AnchoredFold>, usize, bool)>,
    worker: Option<SyntaxWorker>,
    pending: Option<SyntaxTicket>,
    source: Option<DocumentSnapshot>,
    language: Option<Language>,
    definition: Option<Arc<bareline_syntax::udl::Definition>>,
    preference: bareline_syntax::LexerPreference,
    requested: Option<Range<TextOffset>>,
    /// The resident range last asked for; fresh windows short of it are laid
    /// over the stand-in carried from the previous revision.
    visible: Option<Range<TextOffset>>,
    checkpoints: Vec<Checkpoint>,
    pub result: Option<SyntaxResult>,
    pub unavailable: bool,
}

impl Styling {
    /// Prepare one pane before drawing (invalidate old identity/configuration)
    /// and after drawing (request its newly visible range). Each pane owns a
    /// separate instance; cloned documents never share this pending-result slot.
    pub fn prepare_view(
        &mut self,
        editor: &mut crate::workspace::WorkspaceEditor,
        path: Option<&std::path::Path>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        use crate::workspace::WorkspaceEditor;
        let definition = editor.viewport().udl.clone();
        let language = if definition.is_some() {
            Language::PlainText
        } else {
            editor
                .viewport()
                .language_override
                .or(editor.viewport().detected_language)
                .unwrap_or_else(|| path.map_or(Language::PlainText, Language::detect))
        };
        editor.viewport_mut().language = language;
        self.view_label = definition
            .as_ref()
            .map_or(language.label(), |definition| definition.name.as_str())
            .to_owned();
        match editor {
            WorkspaceEditor::Paged(paged) => {
                if !paged.viewport_ready() {
                    self.paged = None;
                    self.pending = None;
                    self.result = None;
                    self.paged_folds = None;
                    return;
                }
                let mut policy = bareline_settings::LanguagePolicy::default();
                policy.lexer = match paged.viewport().syntax_preference {
                    bareline_syntax::LexerPreference::Lexilla => bareline_settings::LexerPreference::Primary,
                    bareline_syntax::LexerPreference::Native => bareline_settings::LexerPreference::Native,
                };
                self.refresh_paged_mapped(
                    paged.read_handle(),
                    paged.viewport().snapshot(),
                    paged.viewport_start(),
                    paged.source_segments().to_vec(),
                    language,
                    crate::language::LanguageConfiguration { policy, definition },
                    notify,
                );
                if self.paged.as_ref().is_some_and(|job| {
                    job.identity == paged.snapshot().identity_token() && job.origin == paged.viewport_start()
                }) && self
                    .result
                    .as_ref()
                    .is_some_and(|result| result.is_current(paged.viewport().snapshot()))
                    && let Some((folds, first_line, partial)) = self.paged_folds.take()
                {
                    if let Err(error) = paged.set_known_anchored_folds(folds, 0, partial, first_line) {
                        paged.viewport_mut().error = Some(error);
                    }
                }
            }
            WorkspaceEditor::Resident(resident) => {
                if let Some(definition) = definition {
                    self.refresh_udl(resident.snapshot(), definition, resident.visible_text.clone(), notify);
                } else {
                    self.refresh_preferred(
                        resident.snapshot(),
                        language,
                        resident.visible_text.clone(),
                        notify,
                        resident.syntax_preference,
                    );
                }
            }
        }
    }
    /// The return lifetime belongs only to this cache, so callers can draw the
    /// editor mutably. The editor is consulted only while validating identity.
    pub fn syntax_view<'a>(
        &'a self,
        editor: &crate::workspace::WorkspaceEditor,
    ) -> bareline_editor_surface::SyntaxView<'a> {
        let same_definition =
            |cached: &Option<Arc<bareline_syntax::udl::Definition>>| match (cached, &editor.viewport().udl) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            };
        let current = match editor {
            crate::workspace::WorkspaceEditor::Paged(paged) => {
                paged.viewport_ready()
                    && self.paged.as_ref().is_some_and(|job| {
                        job.identity == paged.snapshot().identity_token()
                            && job.origin == paged.viewport_start()
                            && job.segments == paged.source_segments()
                            && job.local.same_document(paged.viewport().snapshot())
                            && job.local.revision == paged.viewport().snapshot().revision
                            && job.language == editor.viewport().language
                            && job.preference == editor.viewport().syntax_preference
                            && same_definition(&job.definition)
                    })
            }
            crate::workspace::WorkspaceEditor::Resident(resident) => {
                self.paged.is_none()
                    && self.source.as_ref().is_some_and(|source| {
                        source.same_document(resident.snapshot()) && source.revision == resident.snapshot().revision
                    })
                    && self.language == Some(editor.viewport().language)
                    && (editor.viewport().udl.is_some() || self.preference == editor.viewport().syntax_preference)
                    && same_definition(&self.definition)
            }
        };
        bareline_editor_surface::SyntaxView {
            result: self.result.as_ref().filter(|result| {
                current && result.is_current(editor.snapshot()) && result.language == editor.viewport().language
            }),
            language: &self.view_label,
            unavailable: self.unavailable && current,
        }
    }
    pub fn receipt(&self) -> Option<StylingReceipt> {
        if let Some(job) = &self.paged {
            return Some(StylingReceipt {
                identity: job.identity,
                language: job.language,
                range: job.segments.first().map_or(job.origin, |segment| segment.source.start)
                    ..job
                        .segments
                        .last()
                        .map_or(TextOffset(job.origin.0 + job.local.len()), |segment| segment.source.end),
                ready: self.result.as_ref().is_some_and(|result| {
                    result.is_current(&job.local) && result.status == bareline_syntax::Status::Complete
                }),
                unavailable: self.unavailable,
            });
        }
        let source = self.source.as_ref()?;
        Some(StylingReceipt {
            identity: source.identity_token(),
            language: self.language?,
            range: self.requested.clone()?,
            ready: self.result.as_ref().is_some_and(|result| {
                result.is_current(source)
                    && result.status == bareline_syntax::Status::Complete
                    && self
                        .requested
                        .as_ref()
                        .is_some_and(|range| result.range.start <= range.start && range.end <= result.range.end)
            }),
            unavailable: self.unavailable,
        })
    }
    pub fn refresh_paged(
        &mut self,
        handle: bareline_editor_surface::paged_view::PagedReadHandle,
        local: &DocumentSnapshot,
        origin: TextOffset,
        language: Language,
        config: crate::language::LanguageConfiguration,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.refresh_paged_mapped(handle, local, origin, Vec::new(), language, config, notify);
    }
    #[allow(clippy::too_many_arguments)]
    pub fn refresh_paged_mapped(
        &mut self,
        handle: bareline_editor_surface::paged_view::PagedReadHandle,
        local: &DocumentSnapshot,
        origin: TextOffset,
        segments: Vec<bareline_editor_surface::paged_view::ViewportSegment>,
        language: Language,
        config: crate::language::LanguageConfiguration,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        let current = self.paged.as_ref().is_some_and(|job| {
            job.identity == handle.snapshot().identity_token()
                && job.local.same_document(local)
                && job.local.revision == local.revision
                && job.origin == origin
                && job.segments == segments
                && job.language == language
                && job.preference == config.lexer()
                && match (&job.definition, &config.definition) {
                    (None, None) => true,
                    (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                    _ => false,
                }
        });
        if current {
            return;
        }
        self.paged = None;
        self.source = None;
        self.requested = None;
        self.pending = None;
        self.result = None;
        self.paged_folds = None;
        self.unavailable = false;
        match paged::spawn(
            handle,
            local.clone(),
            origin,
            segments,
            language,
            config.lexer(),
            config.definition,
            notify,
        ) {
            Ok(job) => self.paged = Some(job),
            Err(_) => self.unavailable = true,
        }
    }
    pub fn pump(&mut self) -> bool {
        if let Some(job) = &self.paged {
            return match job.receiver.try_recv() {
                Ok(Ok(value)) => {
                    if let Some(syntax) = value.syntax {
                        self.result = Some(syntax);
                    }
                    self.paged_folds = Some((value.folds, value.first_line, value.partial));
                    true
                }
                Ok(Err(_)) => {
                    self.unavailable = true;
                    true
                }
                Err(_) => false,
            };
        }

        let Some(ticket) = &self.pending else {
            return false;
        };
        let received = match ticket.try_recv() {
            Err(TryRecvError::Empty) => return false,
            received => received,
        };
        self.pending = None;
        match received {
            Ok(Ok(result)) => {
                for checkpoint in result.checkpoints.iter().chain(result.checkpoint.iter()) {
                    self.checkpoints.retain(|c| c.offset() != checkpoint.offset());
                    self.checkpoints.push(checkpoint.clone());
                }
                self.checkpoints.sort_by_key(|c| c.offset());
                let excess = self.checkpoints.len().saturating_sub(256);
                self.checkpoints.drain(..excess);
                // Keep a stand-in's colors wherever this window has not reached. A
                // window short of the view only leaves its checkpoints, so windows
                // walking toward the view never accumulate spans.
                let (covers, touches, advances) = self.visible.as_ref().map_or((false, false, false), |visible| {
                    (
                        result.range.start <= visible.start && visible.end <= result.range.end,
                        result.range.start < visible.end && visible.start < result.range.end,
                        // The next refresh restarts nearer the view, so another
                        // window will replace what lies past this one.
                        self.checkpoints
                            .iter()
                            .any(|c| result.range.start < c.offset() && c.offset() <= visible.start),
                    )
                });
                self.result = Some(match self.result.take() {
                    Some(mut stand_in) if !covers && stand_in.status == bareline_syntax::Status::Provisional => {
                        if touches && !advances {
                            // This window ends inside the view and is the last one
                            // requested there, so carried colors past it would
                            // stand in for good; leave that text plain instead.
                            // Unless the stand-in still colors text before the
                            // window, the window alone is the verified result.
                            let (start, end) = (result.range.start, result.range.end);
                            if stand_in.spans.iter().any(|span| span.range.start < start) {
                                stand_in.spans.retain_mut(|span| {
                                    span.range.end = span.range.end.min(end);
                                    span.range.start < span.range.end
                                });
                                stand_in.range = stand_in.range.start.min(end)..stand_in.range.end.min(end);
                                result.overlay(&stand_in)
                            } else {
                                result
                            }
                        } else if touches {
                            result.overlay(&stand_in)
                        } else {
                            stand_in
                        }
                    }
                    _ => result,
                });
            }
            _ => {
                self.unavailable = true;
                // A failed refresh must not leave carried colors standing in for it.
                self.result = self
                    .result
                    .take()
                    .filter(|result| result.status == bareline_syntax::Status::Complete);
            }
        }
        true
    }
    #[cfg(test)]
    pub fn refresh(
        &mut self,
        source: &DocumentSnapshot,
        language: Language,
        visible: Range<TextOffset>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.refresh_preferred(
            source,
            language,
            visible,
            notify,
            bareline_syntax::LexerPreference::Lexilla,
        );
    }
    pub fn refresh_preferred(
        &mut self,
        source: &DocumentSnapshot,
        language: Language,
        visible: Range<TextOffset>,
        notify: Arc<dyn Fn() + Send + Sync>,
        preference: bareline_syntax::LexerPreference,
    ) {
        self.paged = None;
        if self.preference != preference {
            self.source = None;
            self.preference = preference;
        }
        self.refresh_configured(source, language, visible, notify, None);
    }
    pub fn refresh_udl(
        &mut self,
        source: &DocumentSnapshot,
        definition: Arc<bareline_syntax::udl::Definition>,
        visible: Range<TextOffset>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.paged = None;
        self.refresh_configured(source, Language::PlainText, visible, notify, Some(definition));
    }
    /// Carry the cached colors and checkpoints to `source` before a frame is
    /// drawn, without requesting anything. The previous revision's result is
    /// stale to the editor, so a frame drawn before this would paint the text
    /// plain; the refresh after drawing then asks for the newly visible range.
    pub fn carry(
        &mut self,
        source: &DocumentSnapshot,
        language: Language,
        definition: Option<Arc<bareline_syntax::udl::Definition>>,
        preference: bareline_syntax::LexerPreference,
    ) {
        self.paged = None;
        if definition.is_none() && self.preference != preference {
            self.source = None;
            self.preference = preference;
        }
        self.sync(source, language, definition);
    }
    fn refresh_configured(
        &mut self,
        source: &DocumentSnapshot,
        language: Language,
        visible: Range<TextOffset>,
        notify: Arc<dyn Fn() + Send + Sync>,
        definition: Option<Arc<bareline_syntax::udl::Definition>>,
    ) {
        self.sync(source, language, definition.clone());
        self.visible = Some(visible.clone());
        if (language == Language::PlainText && definition.is_none()) || visible.is_empty() || self.unavailable {
            return;
        }
        if self.result.as_ref().is_some_and(|r| {
            r.status == bareline_syntax::Status::Complete
                && r.range.start <= visible.start
                && r.range.end >= visible.end
        }) {
            return;
        }
        // Restart at an actual verified state. Each worker request is bounded;
        // completions schedule the next slice through the normal redraw event.
        let checkpoint = self
            .checkpoints
            .iter()
            .rev()
            .find(|c| c.offset() <= visible.start)
            .cloned();
        let start = checkpoint.as_ref().map_or(TextOffset(0), |c| c.offset());
        let mut end = visible.end.0.min(start.0.saturating_add(MAX_REQUEST_BYTES));
        if end < visible.end.0
            && let Ok(line) = source.line_at(TextOffset(end))
            && let Ok(range) = source.line_range(line)
            && range.start.0 > start.0
        {
            end = range.start.0;
        }
        while !source.is_boundary(TextOffset(end)) {
            end -= 1;
        }
        let range = start..TextOffset(end);
        if self.requested.as_ref() == Some(&range) {
            return;
        }
        self.pending = None;
        self.requested = Some(range.clone());
        if self.worker.is_none() {
            self.worker = SyntaxWorker::new().ok();
        }
        self.pending = self.worker.as_ref().and_then(|worker| {
            if let Some(definition) = definition {
                worker
                    .submit_udl(source.clone(), definition, range, checkpoint, notify)
                    .ok()
            } else {
                worker
                    .submit_preferred(source.clone(), language, range, checkpoint, notify, self.preference)
                    .ok()
            }
        });
        self.unavailable = self.pending.is_none();
    }
    /// Adopt `source` and configuration, keeping only what an edit leaves valid.
    fn sync(
        &mut self,
        source: &DocumentSnapshot,
        language: Language,
        definition: Option<Arc<bareline_syntax::udl::Definition>>,
    ) {
        let same_configuration = self.language == Some(language)
            && match (&self.definition, &definition) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            };
        let same_document = self.source.as_ref().is_some_and(|s| s.same_document(source));
        if !same_configuration || !same_document || self.source.as_ref().is_some_and(|s| s.revision != source.revision)
        {
            self.pending = None;
            self.source = Some(source.clone());
            self.language = Some(language);
            self.definition = definition;
            self.requested = None;
            self.unavailable = false;
            if same_configuration && same_document {
                // PR-008: an edit invalidates only state at or after it. Verified
                // checkpoints before the edit carry over, and the previous colors
                // stay on screen, mapped through the edit, until the fresh result.
                self.checkpoints = std::mem::take(&mut self.checkpoints)
                    .iter()
                    .filter_map(|checkpoint| checkpoint.rebase(source))
                    .collect();
                self.result = self.result.take().and_then(|result| result.rebase(source));
            } else {
                self.checkpoints.clear();
                self.result = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document, Edit, EditTransaction};
    #[test]
    fn pane_caches_do_not_supersede_each_other_and_reject_foreign_views() {
        use crate::workspace::WorkspaceEditor;
        use bareline_editor_surface::EditorSurface;
        let rust = Document::from_utf8("fn main() {}", Budget::new(1 << 20), Budget::new(1 << 20))
            .unwrap()
            .snapshot();
        let json = Document::from_utf8("{\"value\":true}", Budget::new(1 << 20), Budget::new(1 << 20))
            .unwrap()
            .snapshot();
        let (tx0, rx0) = std::sync::mpsc::channel();
        let (tx1, rx1) = std::sync::mpsc::channel();
        let notify0: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = tx0.send(());
        });
        let notify1: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = tx1.send(());
        });
        let mut first = WorkspaceEditor::Resident(EditorSurface::loading(rust.clone(), notify0.clone()));
        let mut second = WorkspaceEditor::Resident(EditorSurface::loading(json.clone(), notify1.clone()));
        first.viewport_mut().visible_text = TextOffset(0)..TextOffset(rust.len());
        second.viewport_mut().visible_text = TextOffset(0)..TextOffset(json.len());
        let mut caches = [Styling::default(), Styling::default()];
        caches[0].prepare_view(&mut first, Some(std::path::Path::new("first.rs")), notify0.clone());
        caches[1].prepare_view(&mut second, Some(std::path::Path::new("second.json")), notify1.clone());
        rx0.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        rx1.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert!(caches[0].pump());
        assert!(caches[1].pump());
        assert!(caches[0].syntax_view(&first).result.is_some());
        assert!(caches[1].syntax_view(&second).result.is_some());
        assert!(caches[0].syntax_view(&second).result.is_none());
        assert!(caches[1].syntax_view(&first).result.is_none());
        first.viewport_mut().syntax_preference = bareline_syntax::LexerPreference::Native;
        assert!(caches[0].syntax_view(&first).result.is_none());
        caches[0].prepare_view(&mut first, Some(std::path::Path::new("first.rs")), notify0);
        assert!(caches[0].syntax_view(&first).result.is_none());
        assert!(caches[1].syntax_view(&second).result.is_some());
        // Both panes can reference one document while retaining independent jobs.
        second = WorkspaceEditor::Resident(EditorSurface::loading(rust.clone(), notify1.clone()));
        second.viewport_mut().visible_text = TextOffset(0)..TextOffset(rust.len());
        caches[1].prepare_view(&mut second, Some(std::path::Path::new("clone.rs")), notify1);
        rx0.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        rx1.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert!(caches[0].pump());
        assert!(caches[1].pump());
        assert!(caches[0].syntax_view(&first).result.is_some());
        assert!(caches[1].syntax_view(&second).result.is_some());
    }
    #[test]
    fn bounded_checkpoint_progress_and_revision_invalidation() {
        let text = format!("/*\n{}*/\nlet crab = \"🦀\";\n", "comment\n".repeat(40_000));
        let mut document = Document::from_utf8(&text, Budget::new(4 << 20), Budget::new(4 << 20)).unwrap();
        let source = document.snapshot();
        let start = source.line_range(source.line_count() - 2).unwrap().start;
        let visible = start..TextOffset(source.len());
        let (sent, received) = std::sync::mpsc::channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = sent.send(());
        });
        let mut styling = Styling::default();
        for _ in 0..3 {
            styling.refresh(&source, Language::Rust, visible.clone(), notify.clone());
            received.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
            assert!(styling.pump());
            let result = styling.result.as_ref().unwrap();
            assert!(result.range.end.0 - result.range.start.0 <= MAX_REQUEST_BYTES);
            if result.range.end == visible.end {
                break;
            }
        }
        let result = styling.result.as_ref().unwrap();
        assert_eq!(result.range.end, visible.end);
        assert!(
            result
                .spans
                .iter()
                .any(|s| s.kind == bareline_syntax::StyleKind::Keyword && s.range.start >= start)
        );
        document
            .apply(EditTransaction {
                base_revision: source.revision,
                edits: vec![Edit {
                    range: TextOffset(0)..TextOffset(2),
                    insert: "//".into(),
                }],
            })
            .unwrap();
        styling.refresh(&document.snapshot(), Language::Rust, visible, notify);
        // An edit at byte 0 invalidates every checkpoint; the previous colors
        // stay only as a provisional stand-in for the new revision.
        assert!(styling.checkpoints.is_empty());
        let stand_in = styling.result.as_ref().unwrap();
        assert!(stand_in.is_current(&document.snapshot()));
        assert_eq!(stand_in.status, bareline_syntax::Status::Provisional);
        assert!(!styling.receipt().unwrap().ready);
    }
    #[test]
    fn edit_resumes_at_a_prior_checkpoint_and_keeps_colors_until_the_fresh_window() {
        // SRC-14 (PR-008): an edit near the viewport keeps the checkpoints before
        // it, re-lexes only from the nearest one, and never flashes plain text.
        let line = "let x = 1; /* c */\n";
        let text = format!("{}let tail = \"t\";\n", line.repeat(40_000));
        let mut document = Document::from_utf8(&text, Budget::new(4 << 20), Budget::new(4 << 20)).unwrap();
        let source = document.snapshot();
        let start = source.line_range(source.line_count() - 2).unwrap().start;
        let (sent, received) = std::sync::mpsc::channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = sent.send(());
        });
        let native = bareline_syntax::LexerPreference::Native;
        let mut styling = Styling::default();
        let visible = start..TextOffset(source.len());
        for _ in 0..6 {
            styling.refresh_preferred(&source, Language::Rust, visible.clone(), notify.clone(), native);
            received.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
            assert!(styling.pump());
            if styling.result.as_ref().unwrap().range.end == visible.end {
                break;
            }
        }
        assert_eq!(styling.result.as_ref().unwrap().range.end, visible.end);
        let lexed = styling.worker.as_ref().unwrap().lexed_bytes();
        document
            .apply(EditTransaction {
                base_revision: source.revision,
                edits: vec![Edit {
                    range: start..start,
                    insert: "// ".into(),
                }],
            })
            .unwrap();
        let edited = document.snapshot();
        let visible = start..TextOffset(edited.len());
        styling.refresh_preferred(&edited, Language::Rust, visible.clone(), notify.clone(), native);
        let stand_in = styling.result.as_ref().unwrap();
        assert!(stand_in.is_current(&edited));
        assert_eq!(stand_in.status, bareline_syntax::Status::Provisional);
        assert!(
            stand_in
                .spans
                .iter()
                .any(|s| s.kind == bareline_syntax::StyleKind::Keyword && s.range.start.0 == start.0 + 3)
        );
        assert!(!styling.checkpoints.is_empty());
        assert!(styling.checkpoints.iter().all(|c| c.offset() < start));
        let resume = styling.checkpoints.last().unwrap().offset();
        assert!(resume.0 > 0 && start.0 - resume.0 <= 256 * line.len());
        assert_eq!(styling.requested.as_ref().unwrap().start, resume);
        received.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert!(styling.pump());
        let result = styling.result.as_ref().unwrap();
        assert_eq!(result.status, bareline_syntax::Status::Complete);
        assert_eq!(result.range, resume..visible.end);
        assert!(
            result
                .spans
                .iter()
                .any(|s| s.kind == bareline_syntax::StyleKind::Comment && s.range.start == start)
        );
        assert!(styling.receipt().unwrap().ready);
        // Only the resumed window was lexed, never the document before it.
        assert_eq!(
            styling.worker.as_ref().unwrap().lexed_bytes() - lexed,
            (visible.end.0 - resume.0) as u64
        );
    }
    /// Refresh and pump until no further window is requested for `visible`.
    fn settle(
        styling: &mut Styling,
        source: &DocumentSnapshot,
        visible: &Range<TextOffset>,
        notify: &Arc<dyn Fn() + Send + Sync>,
        received: &std::sync::mpsc::Receiver<()>,
    ) {
        for _ in 0..8 {
            styling.refresh_preferred(
                source,
                Language::Rust,
                visible.clone(),
                notify.clone(),
                bareline_syntax::LexerPreference::Native,
            );
            if styling.pending.is_none() {
                return;
            }
            received.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
            assert!(styling.pump());
        }
        panic!("styling kept requesting windows");
    }
    #[test]
    fn skipped_revisions_drop_carried_state_and_restart_at_zero() {
        // SRC-14 fallback: a snapshot carries only its own receipt, so when
        // several edits land between two refreshes nothing can be rebased. The
        // checkpoints and the stand-in are dropped and lexing restarts at byte 0.
        let line = "let x = 1; /* c */\n";
        let mut document =
            Document::from_utf8(&line.repeat(2_000), Budget::new(4 << 20), Budget::new(4 << 20)).unwrap();
        let source = document.snapshot();
        let (sent, received) = std::sync::mpsc::channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = sent.send(());
        });
        let mut styling = Styling::default();
        let top = source.line_range(1_500).unwrap().start;
        settle(
            &mut styling,
            &source,
            &(top..TextOffset(source.len())),
            &notify,
            &received,
        );
        assert!(styling.receipt().unwrap().ready);
        assert!(!styling.checkpoints.is_empty());
        let at = source.line_range(1_800).unwrap().start;
        for _ in 0..2 {
            let base_revision = document.snapshot().revision;
            document
                .apply(EditTransaction {
                    base_revision,
                    edits: vec![Edit {
                        range: at..at,
                        insert: "x".into(),
                    }],
                })
                .unwrap();
        }
        let edited = document.snapshot();
        styling.refresh_preferred(
            &edited,
            Language::Rust,
            top..TextOffset(edited.len()),
            notify,
            bareline_syntax::LexerPreference::Native,
        );
        assert!(styling.checkpoints.is_empty());
        assert!(styling.result.is_none());
        assert_eq!(styling.requested.as_ref().unwrap().start, TextOffset(0));
    }
    #[test]
    fn a_window_ending_inside_the_view_drops_carried_colors_past_it() {
        // SRC-14: in a view taller than one window, the last window requested
        // there ends inside it. Colors carried through an edit stop at that
        // window instead of standing in for good past it, and the window stays
        // a complete result rather than a provisional one.
        let line = "let x = 1; /* c */\n";
        let mut document =
            Document::from_utf8(&line.repeat(20_000), Budget::new(8 << 20), Budget::new(8 << 20)).unwrap();
        let source = document.snapshot();
        let (sent, received) = std::sync::mpsc::channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = sent.send(());
        });
        let mut styling = Styling::default();
        let top = source.line_range(300).unwrap().start;
        let visible = top..TextOffset(source.len());
        assert!(visible.end.0 - visible.start.0 > MAX_REQUEST_BYTES);
        settle(&mut styling, &source, &visible, &notify, &received);
        let settled = styling.result.as_ref().unwrap().range.clone();
        assert!(settled.start.0 > 0 && settled.start <= top && settled.end < visible.end);
        // A paste inside the view opens a comment that runs to the end.
        let at = source.line_range(1_000).unwrap().start;
        document
            .apply(EditTransaction {
                base_revision: source.revision,
                edits: vec![Edit {
                    range: at..at,
                    insert: format!("/* {}", "x\n".repeat(60_000)),
                }],
            })
            .unwrap();
        let edited = document.snapshot();
        let visible = top..TextOffset(edited.len());
        styling.refresh_preferred(
            &edited,
            Language::Rust,
            visible.clone(),
            notify.clone(),
            bareline_syntax::LexerPreference::Native,
        );
        let requested = styling.requested.clone().unwrap();
        assert_eq!(requested.start, settled.start);
        assert!(requested.end < visible.end);
        let stand_in = styling.result.as_ref().unwrap();
        assert_eq!(stand_in.status, bareline_syntax::Status::Provisional);
        assert!(stand_in.spans.iter().any(|s| s.range.start >= requested.end));
        received.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert!(styling.pump());
        // Nothing carried lies before the window, so it stands as the verified
        // result there and syntax-aware typing keeps working.
        let merged = styling.result.as_ref().unwrap();
        assert_eq!(merged.status, bareline_syntax::Status::Complete);
        assert_eq!(merged.range, requested);
        assert!(styling.receipt().unwrap().ready);
        assert!(merged.spans.iter().all(|s| s.range.end <= requested.end));
        assert!(
            merged
                .spans
                .iter()
                .any(|s| s.kind == bareline_syntax::StyleKind::Comment && s.range.start == at)
        );
        // No later window is asked for there.
        styling.refresh_preferred(
            &edited,
            Language::Rust,
            visible,
            notify,
            bareline_syntax::LexerPreference::Native,
        );
        assert!(styling.pending.is_none());
        assert_eq!(styling.requested.as_ref(), Some(&requested));
    }
    #[test]
    fn dense_pretty_json_stays_highlighted_to_eof() {
        // SRC-13: pretty JSON exceeded the old per-window span cap and latched
        // the whole view unavailable.
        let mut text = String::from("[\n");
        let mut id = 0;
        while text.len() < 400 * 1024 {
            text.push_str(&format!(
                "  {{\n    \"id\": {id},\n    \"name\": \"item-{id}\",\n    \"tags\": [\"a\", \"b\"],\n    \"active\": true\n  }},\n"
            ));
            id += 1;
        }
        text.push_str("  {\n    \"last\": \"tail\"\n  }\n]\n");
        let document = Document::from_utf8(&text, Budget::new(4 << 20), Budget::new(4 << 20)).unwrap();
        let source = document.snapshot();
        let start = source.line_range(source.line_count() - 4).unwrap().start;
        let visible = start..TextOffset(source.len());
        let (sent, received) = std::sync::mpsc::channel();
        let notify: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = sent.send(());
        });
        let mut styling = Styling::default();
        for _ in 0..4 {
            styling.refresh(&source, Language::Json, visible.clone(), notify.clone());
            received.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            assert!(styling.pump());
            assert!(!styling.unavailable);
            if styling.result.as_ref().unwrap().range.end == visible.end {
                break;
            }
        }
        let result = styling.result.as_ref().unwrap();
        assert_eq!(result.range.end, visible.end);
        assert!(
            result
                .spans
                .iter()
                .any(|s| s.kind == bareline_syntax::StyleKind::String && s.range.start >= start)
        );
    }
}
