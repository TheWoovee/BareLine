// SPDX-License-Identifier: MPL-2.0
//! Language/completion UI controller with one bounded on-demand worker.
use bareline_document::{DocumentSnapshot, TextOffset};
use bareline_editor_surface::{
    completion::{CompletionLimits, CompletionProvider, CompletionResult, WordIndex},
    power::{Limits, PowerEdit, SelectionSet},
};
use bareline_renderer::{DrawOp, Point, Rect};
use bareline_syntax::{Cancellation, Language};
use bareline_ui::{
    BORDER, CHROME, MUTED, TEXT,
    controls::{ControlAction, ControlState, Key, UiEvent},
    rect, text,
    widgets::{ItemSource, List, Metrics, Theme},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
};
pub enum LanguageEffect {
    Choose(Language),
    Accept(usize),
}
enum WorkerResult {
    Saved,
    Completion(CompletionResult),
    Folds(DocumentSnapshot, Vec<bareline_syntax::folding::Fold>, bool),
    Udl(
        bareline_syntax::udl::Definition,
        Vec<bareline_syntax::udl::Mapping>,
    ),
}
#[derive(Default)]
struct Rows(Vec<String>);
impl ItemSource for Rows {
    fn len(&self) -> Option<usize> {
        Some(self.0.len())
    }
    fn discovered(&self) -> usize {
        self.0.len()
    }
    fn label(&self, n: usize) -> &str {
        self.0.get(n).map_or("", String::as_str)
    }
}
pub struct LanguageController {
    pub open: bool,
    pub title: String,
    pub status: String,
    pub completion: Option<CompletionResult>,
    pub folds: Option<(DocumentSnapshot, Vec<bareline_syntax::folding::Fold>, bool)>,
    pub definition: Option<Arc<bareline_syntax::udl::Definition>>,
    pub fold_level: usize,
    rows: Rows,
    list: List,
    choosing: bool,
    receiver: Option<Receiver<Result<WorkerResult, String>>>,
    cancel: Cancellation,
}
impl Default for LanguageController {
    fn default() -> Self {
        Self {
            open: false,
            title: String::new(),
            status: String::new(),
            completion: None,
            folds: None,
            definition: None,
            fold_level: 1,
            rows: Rows::default(),
            list: List {
                bounds: Rect::default(),
                state: ControlState {
                    focused: true,
                    ..Default::default()
                },
                selected: None,
                offset: 0.0,
                metrics: Metrics::COMPACT,
            },
            choosing: false,
            receiver: None,
            cancel: Cancellation::default(),
        }
    }
}
impl LanguageController {
    pub fn busy(&self) -> bool {
        self.receiver.is_some()
    }
    pub fn validate_definition(
        &mut self,
        snapshot: DocumentSnapshot,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.launch("Language Definition", notify, move |cancel| {
            if cancel.is_cancelled() {
                return Err("Validation cancelled".into());
            }
            let text = snapshot
                .read(TextOffset(0)..TextOffset(snapshot.len()), 128 << 10)
                .map_err(|e| format!("{e:?}"))?;
            let definition = bareline_syntax::udl::Definition::from_json(&text)
                .map_err(|e| format!("Definition unchanged: {e:?}"))?;
            Ok(WorkerResult::Udl(definition, Vec::new()))
        });
        self.open = false;
    }
    pub fn export_definition(
        &mut self,
        path: PathBuf,
        file_system: Arc<dyn bareline_platform::LocalFileSystem>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        let Some(definition) = self.definition.clone() else {
            self.status = "Import a definition first".into();
            return;
        };
        self.launch("Export Language", notify, move |cancel| {
            use std::io::Write;
            let text = definition.to_json().map_err(|e| format!("{e:?}"))?;
            let stage = path.with_file_name(format!(
                ".bareline-language-{}-{}.tmp",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ));
            let result = (|| {
                file_system
                    .validate_target(&path)
                    .map_err(|e| e.to_string())?;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&stage)
                    .map_err(|e| e.to_string())?;
                file.write_all(text.as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(|e| e.to_string())?;
                drop(file);
                if cancel.is_cancelled() {
                    return Err("Export cancelled".into());
                }
                file_system
                    .commit(&stage, &path, path.exists())
                    .map_err(|e| e.to_string())?;
                Ok(WorkerResult::Saved)
            })();
            if result.is_err() {
                let _ = std::fs::remove_file(&stage);
            }
            result
        });
    }
    pub fn choose_language(&mut self) {
        self.close();
        self.open = true;
        self.choosing = true;
        self.title = "Language".into();
        self.status = "Enter applies to the current document · Escape closes".into();
        self.rows = Rows(
            std::iter::once("Plain text".into())
                .chain(
                    bareline_syntax::catalog::CATALOG
                        .iter()
                        .map(|m| m.label.into()),
                )
                .collect(),
        );
        self.list.selected = Some(0);
    }
    pub fn close(&mut self) {
        self.cancel.cancel();
        self.open = false;
        self.completion = None;
        self.rows.0.clear();
        self.list.selected = None;
    }
    fn launch(
        &mut self,
        title: &str,
        notify: Arc<dyn Fn() + Send + Sync>,
        job: impl FnOnce(Cancellation) -> Result<WorkerResult, String> + Send + 'static,
    ) {
        if self.receiver.is_some() {
            self.status = "Language worker is busy; retry after completion".into();
            return;
        }
        self.cancel = Cancellation::default();
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        self.receiver = Some(rx);
        self.title = title.into();
        self.status = "Preparing…".into();
        self.rows.0.clear();
        self.list.selected = None;
        self.open = true;
        self.choosing = false;
        if let Err(e) = std::thread::Builder::new()
            .name("bareline-language".into())
            .spawn(move || {
                let result = job(cancel);
                let _ = tx.send(result);
                notify();
            })
        {
            self.receiver = None;
            self.status = e.to_string();
        }
    }
    pub fn request_completion(
        &mut self,
        snapshot: DocumentSnapshot,
        caret: usize,
        language: Language,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.launch("Completion", notify, move |cancel| {
            let mut start = caret.saturating_sub(128 << 10);
            while !snapshot.is_boundary(TextOffset(start)) {
                start += 1;
            }
            let mut end = (start + 256 * 1024).min(snapshot.len());
            while !snapshot.is_boundary(TextOffset(end)) {
                end -= 1;
            }
            let mut words = WordIndex::default();
            words
                .update(
                    &snapshot,
                    TextOffset(start)..TextOffset(end),
                    CompletionLimits::default(),
                    &cancel,
                )
                .map_err(|e| format!("{e:?}"))?;
            let syntax = if start == 0 {
                bareline_syntax::lex(
                    snapshot.clone(),
                    language,
                    TextOffset(0)..TextOffset(end),
                    None,
                    &cancel,
                )
                .ok()
            } else {
                None
            };
            words
                .complete(
                    &snapshot,
                    TextOffset(caret),
                    language,
                    syntax.as_ref(),
                    CompletionLimits::default(),
                )
                .map(WorkerResult::Completion)
                .map_err(|e| format!("{e:?}"))
        });
    }
    pub fn request_folds(
        &mut self,
        snapshot: DocumentSnapshot,
        language: Language,
        level: usize,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        self.fold_level = level.clamp(1, 8);
        if self.receiver.is_some() {
            self.status = "Language worker is busy".into();
            return;
        }
        self.cancel = Cancellation::default();
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        self.receiver = Some(rx);
        self.status = "Discovering folds…".into();
        let spawn = std::thread::Builder::new()
            .name("bareline-folds".into())
            .spawn(move || {
                let mut lexer = bareline_syntax::ForwardLexer::new(snapshot.clone(), language);
                let mut accumulator = bareline_syntax::folding::FoldAccumulator::default();
                let mut start = 0;
                loop {
                    if cancel.is_cancelled() {
                        return;
                    }
                    let mut end = snapshot
                        .len()
                        .min(start + bareline_syntax::MAX_REQUEST_BYTES);
                    if end < snapshot.len()
                        && let Ok(line) = snapshot.line_at(TextOffset(end))
                        && let Ok(range) = snapshot.line_range(line)
                        && range.start.0 > start
                    {
                        end = range.start.0;
                    }
                    while !snapshot.is_boundary(TextOffset(end)) {
                        end -= 1;
                    }
                    let result = lexer
                        .advance(TextOffset(end), &cancel)
                        .and_then(|result| accumulator.advance(&snapshot, &result, 8192));
                    if let Err(error) = result {
                        let _ = tx.send(Err(format!("Fold indexing stopped: {error:?}")));
                        notify();
                        return;
                    }
                    let partial = end < snapshot.len();
                    let result = Ok(WorkerResult::Folds(
                        snapshot.clone(),
                        accumulator.known().to_vec(),
                        partial,
                    ));
                    if partial {
                        let _ = tx.try_send(result);
                    } else {
                        let _ = tx.send(result);
                    }
                    notify();
                    if !partial {
                        return;
                    }
                    start = end;
                }
            });
        if let Err(error) = spawn {
            self.receiver = None;
            self.status = error.to_string();
        }
    }
    pub fn import_udl(&mut self, path: PathBuf, notify: Arc<dyn Fn() + Send + Sync>) {
        self.launch("Import Language", notify, move |cancel| {
            use std::io::Read;
            let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let mut bytes = Vec::new();
            file.take(128 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if cancel.is_cancelled() {
                return Err("Import cancelled".into());
            }
            if bytes.len() > 128 * 1024 {
                return Err("Language definition exceeds 128 KiB".into());
            }
            let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
            let (definition, report) = if text.trim_start().starts_with('<') {
                bareline_syntax::udl::import_notepad_xml(&text).map_err(|e| format!("{e:?}"))?
            } else {
                (
                    bareline_syntax::udl::Definition::from_json(&text)
                        .map_err(|e| format!("{e:?}"))?,
                    Vec::new(),
                )
            };
            Ok(WorkerResult::Udl(definition, report))
        });
    }
    pub fn poll(&mut self) -> bool {
        let Some(result) = self.receiver.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return false;
        };
        if !matches!(&result, Ok(WorkerResult::Folds(_, _, true))) {
            self.receiver = None;
        }
        if self.cancel.is_cancelled() {
            return false;
        }
        match result {
            Ok(WorkerResult::Saved) => self.status = "Language definition exported".into(),
            Ok(WorkerResult::Completion(result)) => {
                self.rows = Rows(result.items.iter().map(|i| i.text.clone()).collect());
                self.status = if result.partial {
                    "Partial local index · select a suggestion before Enter/Tab"
                } else {
                    "Select a suggestion before Enter/Tab"
                }
                .into();
                self.completion = Some(result);
            }
            Ok(WorkerResult::Folds(snapshot, folds, partial)) => {
                self.status = format!(
                    "{} known folds{}",
                    folds.len(),
                    if partial {
                        " · indexing incomplete"
                    } else {
                        ""
                    }
                );
                self.folds = Some((snapshot, folds, partial));
                self.open = false;
            }
            Ok(WorkerResult::Udl(definition, report)) => {
                self.status = format!(
                    "Imported {} · {} mapping notes",
                    definition.name,
                    report.len()
                );
                self.rows = Rows(
                    report
                        .into_iter()
                        .map(|r| format!("{:?}: {} — {}", r.kind, r.field, r.reason))
                        .collect(),
                );
                self.definition = Some(Arc::new(definition));
            }
            Err(error) => self.status = error,
        }
        true
    }
    pub fn event(&mut self, event: UiEvent) -> Option<LanguageEffect> {
        if !self.open {
            return None;
        }
        if matches!(event, UiEvent::Key(Key::Escape)) {
            self.close();
            return None;
        }
        if self.list.event(event, &self.rows) == Some(ControlAction::Activated) {
            let n = self.list.selected?;
            if self.choosing {
                let language = if n == 0 {
                    Language::PlainText
                } else {
                    bareline_syntax::catalog::CATALOG.get(n - 1)?.language
                };
                self.close();
                Some(LanguageEffect::Choose(language))
            } else if self.completion.is_some() {
                Some(LanguageEffect::Accept(n))
            } else {
                None
            }
        } else {
            None
        }
    }
    pub fn pointer(&mut self, p: Point) -> Option<LanguageEffect> {
        self.event(UiEvent::PointerDown(p))
    }
    pub fn accept(
        &mut self,
        snapshot: &DocumentSnapshot,
        set: &SelectionSet,
        index: usize,
    ) -> Result<PowerEdit, String> {
        let result = self
            .completion
            .as_ref()
            .ok_or("Completion is unavailable")?;
        let edit = bareline_editor_surface::completion::accept(
            snapshot,
            result,
            index,
            set,
            Limits::default(),
        )
        .map_err(|e| format!("{e:?}"))?;
        self.close();
        Ok(edit)
    }
    pub fn draw(&mut self, width: f32, height: f32, ops: &mut Vec<DrawOp>) {
        if !self.open {
            return;
        }
        let w = 520.0f32.min((width - 32.0).max(80.0));
        let h = (100.0 + self.rows.0.len().min(9) as f32 * 28.0).min((height - 100.0).max(80.0));
        let bounds = rect((width - w) / 2.0, 72.0, w, h);
        ops.push(DrawOp::Fill(bounds, CHROME));
        ops.push(DrawOp::Stroke(bounds, BORDER, 1.0));
        text(
            ops,
            bounds.x + 12.0,
            bounds.y + 10.0,
            &self.title,
            16.0,
            TEXT,
        );
        self.list.bounds = rect(
            bounds.x + 10.0,
            bounds.y + 38.0,
            w - 20.0,
            (h - 76.0).max(0.0),
        );
        self.list.paint(&self.rows, Theme::default(), ops);
        text(
            ops,
            bounds.x + 12.0,
            bounds.y + h - 28.0,
            &self.status,
            13.0,
            MUTED,
        );
    }
}
pub fn register_commands(registry: &mut bareline_commands::CommandRegistry) {
    use bareline_commands::{Action, CommandId, CommandSpec};
    for (id, title, key) in [
        ("language.choose", "Choose Language…", ""),
        ("language.udl.import", "Import User-defined Language…", ""),
        ("language.udl.edit", "Edit Imported Language Definition", ""),
        ("language.udl.export", "Export User-defined Language…", ""),
        ("language.udl.preview", "Preview User-defined Language", ""),
        ("editor.completion.show", "Show Completion", "Ctrl+Space"),
        ("view.fold.all", "Fold All Known Regions", ""),
        ("view.fold.unfoldAll", "Unfold All", ""),
        ("view.fold.toggleCurrent", "Toggle Current Fold", ""),
        ("view.fold.level1", "Fold Level 1", ""),
        ("view.fold.level2", "Fold Level 2", ""),
        ("view.fold.level3", "Fold Level 3", ""),
        ("view.fold.level4", "Fold Level 4", ""),
        ("view.fold.level5", "Fold Level 5", ""),
        ("view.fold.level6", "Fold Level 6", ""),
        ("view.fold.level7", "Fold Level 7", ""),
        ("view.fold.level8", "Fold Level 8", ""),
    ] {
        let id = CommandId(id);
        if registry.dispatch(id).is_none() {
            let _ = registry.register(CommandSpec {
                id,
                title,
                category: "Language",
                shortcut: key,
                action: Action::Contributed(id),
            });
        }
    }
}
