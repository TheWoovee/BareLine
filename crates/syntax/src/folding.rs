// SPDX-License-Identifier: MPL-2.0
//! Folds derived only from current verified syntax; no guessed or whole-file scan.
use crate::{Error, Status, StyleKind, SyntaxResult};
use bareline_document::{DocumentSnapshot, TextOffset};
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fold {
    pub header: usize,
    pub end: usize,
    pub level: usize,
}
/// Bounded carry for a single forward pass. Open headers survive byte-window
/// boundaries; only closed regions are published. Revision ownership remains
/// with the caller's immutable snapshot.
#[derive(Default)]
pub struct FoldAccumulator {
    open: Vec<(usize, i32)>,
    known: Vec<Fold>,
    next: usize,
}
impl FoldAccumulator {
    pub fn known(&self) -> &[Fold] {
        &self.known
    }
    pub fn advance(
        &mut self,
        snapshot: &DocumentSnapshot,
        syntax: &SyntaxResult,
        limit: usize,
    ) -> Result<(), Error> {
        if !syntax.is_current(snapshot)
            || syntax.status != Status::Complete
            || syntax.range.start.0 != self.next
        {
            return Err(Error::StaleCheckpoint);
        }
        let eof = syntax.range.end.0 == snapshot.len();
        if let Some(levels) = &syntax.fold_levels {
            let first = snapshot
                .line_at(syntax.range.start)
                .map_err(|_| Error::InvalidRange)?;
            let last = snapshot
                .line_at(syntax.range.end)
                .map_err(|_| Error::InvalidRange)?;
            let empty_final = eof
                && snapshot
                    .line_range(last)
                    .is_ok_and(|range| range.start == syntax.range.end);
            let count = if eof {
                levels.len().saturating_sub(usize::from(empty_final))
            } else {
                last.saturating_sub(first)
            };
            for (index, value) in levels.iter().copied().take(count).enumerate() {
                let number = value & 0xfff;
                if value & 0x1000 == 0 {
                    while self.open.last().is_some_and(|(_, level)| number <= *level) {
                        let (header, level) = self.open.pop().unwrap();
                        push_level_fold(
                            &mut self.known,
                            header,
                            (first + index).saturating_sub(1),
                            level,
                            limit,
                        )?;
                    }
                }
                if value & 0x2000 != 0 {
                    if self.open.len() >= 1024 {
                        return Err(Error::BudgetExceeded);
                    }
                    self.open.push((first + index, number));
                }
            }
            if eof {
                for (header, level) in self.open.drain(..) {
                    push_level_fold(
                        &mut self.known,
                        header,
                        last.saturating_sub(usize::from(empty_final)),
                        level,
                        limit,
                    )?;
                }
            }
        } else {
            // A fallback grammar cannot finish opaque primary-lexer headers.
            self.open.clear();
            self.known.extend(folds(
                snapshot,
                syntax,
                limit.saturating_sub(self.known.len()),
            )?);
        }
        self.known.sort_by_key(|fold| (fold.header, fold.level));
        self.next = syntax.range.end.0;
        Ok(())
    }
}
pub fn folds(
    snapshot: &DocumentSnapshot,
    syntax: &SyntaxResult,
    max_folds: usize,
) -> Result<Vec<Fold>, Error> {
    if !syntax.is_current(snapshot) || syntax.status != Status::Complete {
        return Err(Error::StaleCheckpoint);
    }
    if let Some(levels) = &syntax.fold_levels {
        let first_line = snapshot
            .line_at(syntax.range.start)
            .map_err(|_| Error::InvalidRange)?;
        let eof = syntax.range.end.0 == snapshot.len();
        let last = snapshot
            .line_at(syntax.range.end)
            .map_err(|_| Error::InvalidRange)?;
        let empty_final = eof
            && snapshot
                .line_range(last)
                .is_ok_and(|range| range.start == syntax.range.end);
        return from_levels(
            &levels[..levels.len().saturating_sub(usize::from(empty_final))],
            first_line,
            eof,
            max_folds,
        );
    }
    let text = snapshot
        .read(syntax.range.clone(), crate::MAX_REQUEST_BYTES)
        .map_err(|_| Error::InvalidRange)?;
    let mut stack = Vec::new();
    let mut folds = Vec::new();
    let mut span_index = 0;
    for (i, c) in text.char_indices() {
        let offset = syntax.range.start.0 + i;
        while span_index < syntax.spans.len() && syntax.spans[span_index].range.end.0 <= offset {
            span_index += 1;
        }
        if syntax.spans.get(span_index).is_some_and(|s| {
            s.range.start.0 <= offset && matches!(s.kind, StyleKind::Comment | StyleKind::String)
        }) {
            continue;
        }
        if c == '{' || c == '[' {
            if stack.len() >= 1024 {
                return Err(Error::BudgetExceeded);
            }
            stack.push((
                c,
                snapshot
                    .line_at(TextOffset(offset))
                    .map_err(|_| Error::InvalidRange)?,
            ));
        } else if (c == '}' || c == ']')
            && let Some((open, header)) = stack.pop()
        {
            if (open == '{' && c != '}') || (open == '[' && c != ']') {
                stack.clear();
                continue;
            }
            let end = snapshot
                .line_at(TextOffset(offset))
                .map_err(|_| Error::InvalidRange)?;
            if end > header {
                if folds.len() >= max_folds {
                    return Err(Error::BudgetExceeded);
                }
                folds.push(Fold {
                    header,
                    end,
                    level: stack.len() + 1,
                });
            }
        }
    }
    folds.sort_by_key(|f| (f.header, f.level));
    Ok(folds)
}
// Scintilla levels carry the current numeric nesting in the low 12 bits and
// a header flag. Never infer a closed fold at a truncated window boundary.
fn from_levels(levels: &[i32], first: usize, eof: bool, limit: usize) -> Result<Vec<Fold>, Error> {
    let mut stack: Vec<(usize, i32)> = Vec::new();
    let mut result = Vec::new();
    for (index, value) in levels.iter().copied().enumerate() {
        let number = value & 0xfff;
        // Blank lines do not close indentation folds.
        if value & 0x1000 == 0 {
            while stack.last().is_some_and(|(_, level)| number <= *level) {
                let (header, level) = stack.pop().unwrap();
                push_level_fold(
                    &mut result,
                    first + header,
                    first + index.saturating_sub(1),
                    level,
                    limit,
                )?;
            }
        }
        if value & 0x2000 != 0 {
            if stack.len() >= 1024 {
                return Err(Error::BudgetExceeded);
            }
            stack.push((index, number));
        }
    }
    if eof {
        for (header, level) in stack {
            push_level_fold(
                &mut result,
                first + header,
                first + levels.len().saturating_sub(1),
                level,
                limit,
            )?;
        }
    }
    result.sort_by_key(|f| (f.header, f.level));
    Ok(result)
}
fn push_level_fold(
    result: &mut Vec<Fold>,
    header: usize,
    end: usize,
    level: i32,
    limit: usize,
) -> Result<(), Error> {
    if end > header {
        if result.len() >= limit {
            return Err(Error::BudgetExceeded);
        }
        result.push(Fold {
            header,
            end,
            level: level.saturating_sub(0x400).max(0) as usize + 1,
        });
    }
    Ok(())
}
#[derive(Clone, Debug, Default)]
pub struct FoldState {
    pub collapsed: std::collections::BTreeSet<usize>,
    pub collapse_level: Option<usize>,
}
impl FoldState {
    pub fn apply_level(&mut self, folds: &[Fold], level: usize) {
        self.collapse_level = Some(level.clamp(1, 8));
        self.refresh(folds);
    }
    pub fn refresh(&mut self, folds: &[Fold]) {
        if let Some(level) = self.collapse_level {
            self.collapsed
                .extend(folds.iter().filter(|f| f.level >= level).map(|f| f.header));
        }
    }
    pub fn unfold_all(&mut self) {
        self.collapsed.clear();
        self.collapse_level = None;
    }
    pub fn toggle(&mut self, header: usize) {
        if !self.collapsed.remove(&header) {
            self.collapsed.insert(header);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bareline_document::{Budget, Document};
    #[test]
    fn forward_pass_carries_header_across_windows() {
        let first = format!("int f() {{\n{}", "int x;\n".repeat(1000));
        let text = format!("{first}return 1;\n}}\n");
        let document =
            Document::from_utf8(&text, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let snapshot = document.snapshot();
        let mut lexer = crate::ForwardLexer::new(snapshot.clone(), crate::Language::Cpp);
        let mut folds = FoldAccumulator::default();
        let initial = lexer
            .advance(TextOffset(first.len()), &crate::Cancellation::default())
            .unwrap();
        folds.advance(&snapshot, &initial, 10).unwrap();
        assert!(folds.known().is_empty());
        let final_window = lexer
            .advance(TextOffset(text.len()), &crate::Cancellation::default())
            .unwrap();
        folds.advance(&snapshot, &final_window, 10).unwrap();
        assert_eq!(
            folds.known(),
            &[Fold {
                header: 0,
                end: 1002,
                level: 1
            }]
        );
        assert_eq!(initial.checkpoints.len(), 3);
    }
    #[test]
    fn lexilla_indentation_folds_exclude_following_statement() {
        let text = "def f():\n    x = 1\n    return x\ny = 2\n";
        let document =
            Document::from_utf8(text, Budget::new(1 << 20), Budget::new(1 << 20)).unwrap();
        let snapshot = document.snapshot();
        let syntax = crate::lex(
            snapshot.clone(),
            crate::Language::Python,
            TextOffset(0)..TextOffset(text.len()),
            None,
            &crate::Cancellation::default(),
        )
        .unwrap();
        assert!(syntax.fold_levels.is_some());
        assert_eq!(
            folds(&snapshot, &syntax, 10).unwrap(),
            vec![Fold {
                header: 0,
                end: 2,
                level: 1
            }]
        );
    }
    #[test]
    fn unfinished_header_is_not_exposed_until_closed_and_limits_apply() {
        let levels = [0x2400, 0x401, 0x401];
        assert!(from_levels(&levels, 100, false, 10).unwrap().is_empty());
        assert_eq!(
            from_levels(&levels, 100, true, 10).unwrap()[0],
            Fold {
                header: 100,
                end: 102,
                level: 1
            }
        );
        assert_eq!(from_levels(&levels, 0, true, 0), Err(Error::BudgetExceeded));
    }
}
