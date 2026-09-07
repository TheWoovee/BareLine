// SPDX-License-Identifier: MPL-2.0
//! Bounded outline metadata. Rust declaration names and TOML table headings are
//! navigation anchors, not an AST or claims about complete semantic scope.
use bareline_document::{DocumentSnapshot, TextOffset};
use bareline_renderer::{DrawOp, Point, Rect};
use bareline_syntax::{Cancellation, Language, MAX_REQUEST_BYTES, StyleKind, lex};
use bareline_ui::{
    controls::{Key, visible_rows},
    widgets::Theme,
};
use std::{
    path::Path,
    sync::{
        Arc,
        mpsc::{self, Receiver},
    },
};
#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub kind: &'static str,
    pub offset: TextOffset,
}
struct Batch {
    symbols: Vec<Symbol>,
    finished: bool,
    status: String,
}
pub struct OutlinePanel {
    pub open: bool,
    source: Option<DocumentSnapshot>,
    symbols: Vec<Symbol>,
    filtered: Vec<usize>,
    filter: String,
    pending: Option<Receiver<Batch>>,
    cancel: Cancellation,
    selected: usize,
    offset: f64,
    bounds: Rect,
    pub status: String,
    pub title: String,
}
impl Default for OutlinePanel {
    fn default() -> Self {
        Self {
            open: false,
            source: None,
            symbols: Vec::new(),
            filtered: Vec::new(),
            filter: String::new(),
            pending: None,
            cancel: Cancellation::default(),
            selected: 0,
            offset: 0.0,
            bounds: Rect::default(),
            status: String::new(),
            title: String::new(),
        }
    }
}
impl Drop for OutlinePanel {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl OutlinePanel {
    pub fn clear(&mut self) {
        self.cancel.cancel();
        self.pending = None;
        self.source = None;
        self.symbols.clear();
        self.filtered.clear();
        self.title.clear();
        self.status = "No active document".into();
    }
    pub fn refresh(
        &mut self,
        source: &DocumentSnapshot,
        path: Option<&Path>,
        title: &str,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        if !self.open {
            self.cancel.cancel();
            self.pending = None;
            self.source = None;
            return;
        }
        if self.source.as_ref().is_some_and(|old| {
            old.same_document(source)
                && old.revision == source.revision
                && old.len() == source.len()
                && old.is_complete() == source.is_complete()
        }) {
            return;
        }
        self.cancel.cancel();
        self.pending = None;
        self.symbols.clear();
        self.filtered.clear();
        self.selected = 0;
        self.offset = 0.0;
        self.source = Some(source.clone());
        self.title = title.into();
        let ext = path
            .and_then(Path::extension)
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ext != "rs" && ext != "toml" {
            self.status = "No outline provider for this language".into();
            return;
        }
        let source = source.clone();
        self.cancel = Cancellation::default();
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        self.status = "Indexing…".into();
        match std::thread::Builder::new()
            .name("outline".into())
            .spawn(move || {
                let mut start = 0;
                let mut checkpoint = None;
                let mut total = 0;
                let mut toml_quote = None;
                while start < source.len() && !cancel.is_cancelled() {
                    let mut end = source.len().min(start + MAX_REQUEST_BYTES);
                    while end > start && !source.is_boundary(TextOffset(end)) {
                        end -= 1;
                    }
                    let Ok(mut text) =
                        source.read(TextOffset(start)..TextOffset(end), MAX_REQUEST_BYTES)
                    else {
                        let _ = tx.send(Batch {
                            symbols: vec![],
                            finished: true,
                            status: "Outline unavailable: source changed or bytes pending".into(),
                        });
                        notify();
                        return;
                    };
                    if end < source.len() {
                        let Some(newline) = text.rfind('\n') else {
                            let _ = tx.send(Batch {
                                symbols: vec![],
                                finished: true,
                                status: "Partial outline: line exceeds indexing budget".into(),
                            });
                            notify();
                            return;
                        };
                        text.truncate(newline + 1);
                        end = start + text.len();
                    }
                    let symbols = if ext == "rs" {
                        let Ok(result) = lex(
                            source.clone(),
                            Language::Rust,
                            TextOffset(start)..TextOffset(end),
                            checkpoint.as_ref(),
                            &cancel,
                        ) else {
                            return;
                        };
                        let found = rust_symbols(&text, start, &result.spans);
                        checkpoint = result.checkpoint;
                        found
                    } else {
                        toml_symbols(&text, start, &mut toml_quote)
                    };
                    let symbols: Vec<_> = symbols
                        .into_iter()
                        .take(8192usize.saturating_sub(total))
                        .collect();
                    total += symbols.len();
                    start = end;
                    let finished = start == source.len() || total >= 8192;
                    let status = if total >= 8192 {
                        "Partial outline: symbol budget reached"
                    } else if finished && source.is_complete() {
                        ""
                    } else if finished {
                        "Partial outline: document loading"
                    } else {
                        "Indexing…"
                    };
                    if tx
                        .send(Batch {
                            symbols,
                            finished,
                            status: status.into(),
                        })
                        .is_err()
                    {
                        return;
                    }
                    notify();
                    if finished {
                        return;
                    }
                }
                if source.is_empty() {
                    let _ = tx.send(Batch {
                        symbols: vec![],
                        finished: true,
                        status: String::new(),
                    });
                    notify();
                }
            }) {
            Ok(_) => self.pending = Some(rx),
            Err(e) => self.status = e.to_string(),
        }
    }
    pub fn pump(&mut self) -> bool {
        let Some(rx) = &self.pending else {
            return false;
        };
        match rx.try_recv() {
            Ok(batch) => {
                self.symbols.extend(batch.symbols);
                self.status = batch.status;
                if batch.finished {
                    self.pending = None;
                }
                self.rebuild();
                true
            }
            Err(mpsc::TryRecvError::Empty) => false,
            Err(_) => {
                self.pending = None;
                self.status = "Outline indexing stopped".into();
                true
            }
        }
    }
    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.to_lowercase();
        self.rebuild();
    }
    fn rebuild(&mut self) {
        self.filtered = self
            .symbols
            .iter()
            .enumerate()
            .filter(|(_, s)| s.name.to_lowercase().contains(&self.filter))
            .map(|(i, _)| i)
            .collect();
        self.selected = self.selected.min(self.filtered.len().saturating_sub(1));
    }
    pub fn follow_caret(&mut self, caret: TextOffset) {
        if let Some(index) = self
            .filtered
            .iter()
            .rposition(|i| self.symbols[*i].offset <= caret)
        {
            self.selected = index;
            let top = index as f64 * 28.0;
            if top < self.offset {
                self.offset = top;
            } else if top + 28.0 > self.offset + self.bounds.height as f64 {
                self.offset = (top + 28.0 - self.bounds.height as f64).max(0.0);
            }
        }
    }
    fn activate(&self, current: &DocumentSnapshot) -> Option<TextOffset> {
        let old = self.source.as_ref()?;
        if !old.same_document(current) || old.revision != current.revision {
            return None;
        }
        let symbol = self.symbols.get(*self.filtered.get(self.selected)?)?;
        let end = symbol.offset.0.checked_add(symbol.name.len())?;
        if current.read(symbol.offset..TextOffset(end), 4096).ok()? != symbol.name {
            return None;
        }
        Some(symbol.offset)
    }
    pub fn key(&mut self, key: Key, current: &DocumentSnapshot) -> Option<TextOffset> {
        match key {
            Key::Up => self.selected = self.selected.saturating_sub(1),
            Key::Down => {
                self.selected = (self.selected + 1).min(self.filtered.len().saturating_sub(1))
            }
            Key::Home => self.selected = 0,
            Key::End => self.selected = self.filtered.len().saturating_sub(1),
            Key::Enter => return self.activate(current),
            _ => {}
        }
        let top = self.selected as f64 * 28.0;
        if top < self.offset {
            self.offset = top;
        } else if top + 28.0 > self.offset + self.bounds.height as f64 {
            self.offset = (top + 28.0 - self.bounds.height as f64).max(0.0);
        }
        None
    }
    pub fn pointer(&mut self, point: Point, current: &DocumentSnapshot) -> Option<TextOffset> {
        if !self.bounds.contains(point) {
            return None;
        }
        self.selected = ((point.y - self.bounds.y) as f64 / 28.0 + self.offset / 28.0) as usize;
        self.activate(current)
    }
    pub fn draw(&mut self, bounds: Rect, ops: &mut Vec<DrawOp>) {
        if !self.open {
            return;
        }
        let theme = Theme::default();
        ops.push(DrawOp::Fill(bounds, theme.surface));
        ops.push(DrawOp::PushClip(bounds));
        ops.push(DrawOp::Text {
            origin: Point {
                x: bounds.x + 12.0,
                y: bounds.y + 8.0,
            },
            text: if self.filter.is_empty() {
                format!("Outline · {}", self.title)
            } else {
                format!("Outline · {} · {}", self.title, self.filter)
            },
            size: 13.0,
            color: theme.text,
        });
        self.bounds = Rect {
            y: bounds.y + 34.0,
            height: (bounds.height - 62.0).max(0.0),
            ..bounds
        };
        for row in visible_rows(
            self.offset,
            self.bounds.height as f64,
            28.0,
            Some(self.filtered.len()),
            1,
        ) {
            let symbol = &self.symbols[self.filtered[row]];
            let y = self.bounds.y + row as f32 * 28.0 - self.offset as f32;
            if row == self.selected {
                ops.push(DrawOp::Fill(
                    Rect {
                        y,
                        height: 28.0,
                        ..self.bounds
                    },
                    theme.selection,
                ));
            }
            ops.push(DrawOp::Text {
                origin: Point {
                    x: bounds.x + 16.0,
                    y: y + 6.0,
                },
                text: format!("{} {}", symbol.kind, symbol.name),
                size: 13.0,
                color: theme.text,
            });
        }
        ops.push(DrawOp::Text {
            origin: Point {
                x: bounds.x + 10.0,
                y: bounds.y + bounds.height - 22.0,
            },
            text: self.status.clone(),
            size: 12.0,
            color: theme.muted,
        });
        ops.push(DrawOp::PopClip);
    }
}
fn rust_symbols(text: &str, base: usize, spans: &[bareline_syntax::StyleSpan]) -> Vec<Symbol> {
    let mut result = Vec::new();
    for span in spans.iter().filter(|s| s.kind == StyleKind::Keyword) {
        let start = span.range.start.0 - base;
        let end = span.range.end.0 - base;
        let Some(kind) = text.get(start..end) else {
            continue;
        };
        let kind = match kind {
            "fn" => "fn",
            "struct" => "struct",
            "enum" => "enum",
            "trait" => "trait",
            "mod" => "mod",
            "const" => "const",
            "type" => "type",
            _ => continue,
        };
        let rest = &text[end..];
        let skip = rest.len() - rest.trim_start().len();
        let name_start = end + skip;
        let len = text[name_start..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .map(char::len_utf8)
            .sum::<usize>();
        if len == 0 || len > 4096 {
            continue;
        }
        if spans[spans.partition_point(|s| s.range.end.0 <= base + name_start)..]
            .iter()
            .take_while(|s| s.range.start.0 < base + name_start + len)
            .any(|s| {
                s.range.start.0 < base + name_start + len
                    && s.range.end.0 > base + name_start
                    && matches!(s.kind, StyleKind::Comment | StyleKind::String)
            })
        {
            continue;
        }
        result.push(Symbol {
            name: text[name_start..name_start + len].into(),
            kind,
            offset: TextOffset(base + name_start),
        });
    }
    result
}
fn toml_symbols(text: &str, base: usize, multiline: &mut Option<u8>) -> Vec<Symbol> {
    let mut offset = base;
    let mut out = Vec::new();
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if multiline.is_none()
            && trimmed.starts_with('[')
            && let Some(end) = trimmed.find(']')
        {
            let skip = if trimmed.starts_with("[[") { 2 } else { 1 };
            if end > skip {
                let name = &trimmed[skip..end];
                if name.len() <= 4096 {
                    out.push(Symbol {
                        name: name.into(),
                        kind: "table",
                        offset: TextOffset(offset + line.len() - trimmed.len() + skip),
                    });
                }
            }
        }
        // Track multiline strings across worker chunks. Quotes/comments in ordinary
        // strings do not start or finish a multiline literal.
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if let Some(quote) = *multiline {
                if bytes
                    .get(i..i + 3)
                    .is_some_and(|s| s.iter().all(|b| *b == quote))
                {
                    *multiline = None;
                    i += 3;
                } else if quote == b'"' && bytes[i] == b'\\' {
                    i += 2;
                } else {
                    i += 1;
                }
            } else if bytes[i] == b'#' {
                break;
            } else if bytes[i] == b'"' || bytes[i] == b'\'' {
                let quote = bytes[i];
                if bytes
                    .get(i..i + 3)
                    .is_some_and(|s| s.iter().all(|b| *b == quote))
                {
                    *multiline = Some(quote);
                    i += 3;
                } else {
                    i += 1;
                    while i < bytes.len() {
                        if bytes[i] == quote {
                            i += 1;
                            break;
                        }
                        if quote == b'"' && bytes[i] == b'\\' {
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                }
            } else {
                i += 1;
            }
        }
        offset += line.len();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    #[test]
    fn lexical_outline_ignores_comment_functions_and_rejects_other_document() {
        let source = Document::from_utf8(
            "// fn fake() {}\nfn real() {}\n",
            Budget::new(4096),
            Budget::new(4096),
        )
        .unwrap()
        .snapshot();
        let syntax = lex(
            source.clone(),
            Language::Rust,
            TextOffset(0)..TextOffset(source.len()),
            None,
            &Cancellation::default(),
        )
        .unwrap();
        let text = source
            .read(TextOffset(0)..TextOffset(source.len()), 4096)
            .unwrap();
        let symbols = rust_symbols(&text, 0, &syntax.spans);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "real");
        let mut panel = OutlinePanel::default();
        panel.source = Some(source.clone());
        panel.symbols = symbols;
        panel.rebuild();
        assert!(panel.activate(&source).is_some());
        let other = Document::from_utf8(&text, Budget::new(4096), Budget::new(4096))
            .unwrap()
            .snapshot();
        assert_eq!(panel.activate(&other), None);
    }
    #[test]
    fn toml_multiline_state_survives_chunks() {
        let mut quote = None;
        assert!(toml_symbols("text = \"\"\"\n[not_a_table]\n", 0, &mut quote).is_empty());
        let symbols = toml_symbols("\"\"\"\n[real]\n", 30, &mut quote);
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "real");
    }
}
