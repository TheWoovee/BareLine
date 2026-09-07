// SPDX-License-Identifier: MPL-2.0
//! One lazy syntax worker per workspace; active-view context is bounded and disposable.
use bareline_document::{DocumentSnapshot, TextOffset};
use bareline_syntax::{
    Checkpoint, Language, MAX_REQUEST_BYTES, SyntaxResult, SyntaxTicket, SyntaxWorker,
};
use std::{
    ops::Range,
    sync::{Arc, mpsc::TryRecvError},
};

#[derive(Default)]
pub struct Styling {
    worker: Option<SyntaxWorker>,
    pending: Option<SyntaxTicket>,
    source: Option<DocumentSnapshot>,
    language: Option<Language>,
    definition: Option<Arc<bareline_syntax::udl::Definition>>,
    preference: bareline_syntax::LexerPreference,
    requested: Option<Range<TextOffset>>,
    checkpoints: Vec<Checkpoint>,
    pub result: Option<SyntaxResult>,
    pub unavailable: bool,
}

impl Styling {
    pub fn pump(&mut self) -> bool {
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
                    self.checkpoints
                        .retain(|c| c.offset() != checkpoint.offset());
                    self.checkpoints.push(checkpoint.clone());
                }
                self.checkpoints.sort_by_key(|c| c.offset());
                let excess = self.checkpoints.len().saturating_sub(256);
                self.checkpoints.drain(..excess);
                self.result = Some(result);
            }
            _ => self.unavailable = true,
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
        self.refresh_configured(
            source,
            Language::PlainText,
            visible,
            notify,
            Some(definition),
        );
    }
    fn refresh_configured(
        &mut self,
        source: &DocumentSnapshot,
        language: Language,
        visible: Range<TextOffset>,
        notify: Arc<dyn Fn() + Send + Sync>,
        definition: Option<Arc<bareline_syntax::udl::Definition>>,
    ) {
        if self.language != Some(language)
            || match (&self.definition, &definition) {
                (None, None) => false,
                (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
                _ => true,
            }
            || !self
                .source
                .as_ref()
                .is_some_and(|s| s.same_document(source) && s.revision == source.revision)
        {
            self.pending = None;
            self.source = Some(source.clone());
            self.language = Some(language);
            self.definition = definition.clone();
            self.requested = None;
            self.checkpoints.clear();
            self.result = None;
            self.unavailable = false;
        }
        if (language == Language::PlainText && definition.is_none())
            || visible.is_empty()
            || self.unavailable
        {
            return;
        }
        if self
            .result
            .as_ref()
            .is_some_and(|r| r.range.start <= visible.start && r.range.end >= visible.end)
        {
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
                    .submit_preferred(
                        source.clone(),
                        language,
                        range,
                        checkpoint,
                        notify,
                        self.preference,
                    )
                    .ok()
            }
        });
        self.unavailable = self.pending.is_none();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document, Edit, EditTransaction};
    #[test]
    fn bounded_checkpoint_progress_and_revision_invalidation() {
        let text = format!("/*\n{}*/\nlet crab = \"🦀\";\n", "comment\n".repeat(40_000));
        let mut document =
            Document::from_utf8(&text, Budget::new(4 << 20), Budget::new(4 << 20)).unwrap();
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
            received
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap();
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
        assert!(styling.result.is_none());
        assert!(styling.checkpoints.is_empty());
    }
}
