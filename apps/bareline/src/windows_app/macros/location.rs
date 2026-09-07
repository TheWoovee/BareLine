// SPDX-License-Identifier: MPL-2.0
//! Output locations resolve after open; paged line scans stay on one cancellable worker.
use super::*;
use bareline_app::macros::model::process::OutputLink;
use bareline_document::{TextOffset, paged::WindowPoll};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn paged_line_column(
    source: &bareline_editor_surface::paged_view::PagedReadHandle,
    offset: usize,
    cancel: &AtomicBool,
) -> Result<(u64, u64), String> {
    let snapshot = source.snapshot();
    if offset > snapshot.len() {
        return Err("Command position is outside the document".into());
    }
    let budget = bareline_document::Budget::new(64 * 1024);
    let (mut cursor, mut line, mut column, mut cr) = (0usize, 1u64, 1u64, false);
    while cursor < offset {
        if cancel.load(Ordering::Relaxed) {
            return Err("Command preparation cancelled".into());
        }
        let mut request = snapshot
            .begin_viewport(TextOffset(cursor), 64 * 1024, &budget)
            .map_err(|error| format!("Command source: {error:?}"))?;
        let window = loop {
            match request.poll() {
                WindowPoll::Ready(window) => break window,
                WindowPoll::Pending(ticket) => {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("Command preparation cancelled".into());
                    }
                    if !source.resolve_page(ticket)? {
                        return Err("Document is busy; run the command again".into());
                    }
                }
                _ => return Err("Command source is unavailable".into()),
            }
        };
        for (byte, ch) in window.text().char_indices() {
            if window.range().start.0 + byte >= offset {
                return Ok((line, column));
            }
            if cr && ch == '\n' {
                cr = false;
                continue;
            }
            if ch == '\r' || ch == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
            cr = ch == '\r';
        }
        let next = window.range().end.0;
        if next <= cursor {
            return Err("Command source made no progress".into());
        }
        cursor = next;
    }
    Ok((line, column))
}

impl Shell {
    pub(super) fn macros_open_link(&mut self, mut link: OutputLink) {
        if !link.path.is_absolute() {
            if let Some(base) = self.macros.output_directory.as_ref() {
                link.path = base.join(link.path);
            } else {
                self.macros.controller.status =
                    "Relative output location has no captured process directory".into();
                return;
            }
        }
        self.macros.location_cancel.store(true, Ordering::Relaxed);
        if let Some(workspace) = &mut self.workspace {
            if !(0..workspace.editors.len())
                .any(|index| workspace.path(index) == Some(link.path.as_path()))
            {
                workspace.open(link.path.clone());
            }
            self.macros.controller.status = format!(
                "Opening {}:{}:{}",
                link.path.display(),
                link.line,
                link.column
            );
            self.macros.output_target = Some(link);
        }
    }
    pub(super) fn macros_poll_location(&mut self) {
        if let Some(receiver) = &self.macros.location {
            match receiver.try_recv() {
                Ok(result) => {
                    self.macros.location = None;
                    if !self.macros.location_cancel.load(Ordering::Relaxed) {
                        match result {
                            Ok((snapshot, offset)) => {
                                if let Some(workspace) = &mut self.workspace {
                                    if let Some(index)=workspace.editors.iter().position(|editor|matches!(editor,bareline_app::workspace::WorkspaceEditor::Paged(editor) if editor.snapshot().same_document(&snapshot)&&editor.snapshot().content_state==snapshot.content_state)){
                                        if let bareline_app::workspace::WorkspaceEditor::Paged(editor)=&mut workspace.editors[index]{
                                            match editor.restore_selection(offset,offset){Ok(())=>{self.app.active=index;self.macros.focused=false;self.macros.controller.status="Opened output location".into();},Err(error)=>self.macros.controller.status=error}
                                        }
                                    }else{self.macros.controller.status="Output document changed; activate the location again".into();}
                                }
                            }
                            Err(error) => self.macros.controller.status = error,
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.macros.location = None;
                    self.macros.controller.status = "Output location worker stopped".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.macros.location.is_some() {
            return;
        }
        let Some(link) = self.macros.output_target.clone() else {
            return;
        };
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        let Some(index) = (0..workspace.editors.len())
            .find(|index| workspace.path(*index) == Some(link.path.as_path()))
        else {
            if !workspace.path_loading(&link.path) {
                self.macros.output_target = None;
                self.macros.controller.status = "Output document could not be opened".into();
            }
            return;
        };
        if workspace.editors[index].busy() {
            return;
        }
        self.macros.output_target = None;
        match &mut workspace.editors[index] {
            bareline_app::workspace::WorkspaceEditor::Resident(editor) => {
                let result = (|| {
                    let range = editor
                        .snapshot()
                        .line_range(
                            usize::try_from(link.line.saturating_sub(1))
                                .map_err(|_| "Line number is too large")?,
                        )
                        .map_err(|_| "Output line is outside the document")?;
                    let mut end = range.start.0.saturating_add(1024 * 1024).min(range.end.0);
                    while !editor.snapshot().is_boundary(TextOffset(end)) {
                        end = end.saturating_sub(1);
                    }
                    let value = editor
                        .snapshot()
                        .read(range.start..TextOffset(end), 1024 * 1024)
                        .map_err(|_| "Output column exceeds the bounded line window")?;
                    let byte = value
                        .char_indices()
                        .take_while(|(_, ch)| *ch != '\r' && *ch != '\n')
                        .nth(link.column.saturating_sub(1) as usize)
                        .map(|(byte, _)| byte);
                    let byte = match byte {
                        Some(byte) => byte,
                        None if end < range.end.0 => {
                            return Err("Output column exceeds the 1 MiB navigation limit");
                        }
                        None => value.trim_end_matches(['\r', '\n']).len(),
                    };
                    Ok::<_, &str>(range.start.0 + byte)
                })();
                match result {
                    Ok(offset) => {
                        editor.enqueue(Input::SetCaret(offset, false));
                        self.app.active = index;
                        self.macros.focused = false;
                        self.macros.controller.status = "Opened output location".into();
                    }
                    Err(error) => self.macros.controller.status = error.into(),
                }
            }
            bareline_app::workspace::WorkspaceEditor::Paged(editor) => {
                let source = editor.read_handle();
                let cancel = std::sync::Arc::new(AtomicBool::new(false));
                self.macros.location_cancel = cancel.clone();
                let notify = self.notify.clone();
                let (tx, rx) = mpsc::sync_channel(1);
                match std::thread::Builder::new()
                    .name("bareline-output-location".into())
                    .spawn(move || {
                        let result = locate_paged(&source, link.line, link.column, &cancel)
                            .map(|offset| (source.snapshot().clone(), offset));
                        let _ = tx.send(result);
                        notify();
                    }) {
                    Ok(_) => self.macros.location = Some(rx),
                    Err(error) => self.macros.controller.status = error.to_string(),
                }
            }
        }
    }
}
fn locate_paged(
    source: &bareline_editor_surface::paged_view::PagedReadHandle,
    line: u64,
    column: u64,
    cancel: &AtomicBool,
) -> Result<TextOffset, String> {
    let snapshot = source.snapshot();
    let budget = bareline_document::Budget::new(64 * 1024);
    let (mut cursor, mut current_line, mut current_column, mut cr) = (0usize, 1u64, 1u64, false);
    while cursor < snapshot.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err("Output navigation cancelled".into());
        }
        let mut request = snapshot
            .begin_viewport(TextOffset(cursor), 64 * 1024, &budget)
            .map_err(|error| format!("Output source: {error:?}"))?;
        let window = loop {
            match request.poll() {
                WindowPoll::Ready(window) => break window,
                WindowPoll::Pending(ticket) => {
                    if cancel.load(Ordering::Relaxed) {
                        return Err("Output navigation cancelled".into());
                    }
                    if !source.resolve_page(ticket)? {
                        return Err("Document is busy; activate the output location again".into());
                    }
                }
                _ => return Err("Output source is unavailable".into()),
            }
        };
        for (byte, ch) in window.text().char_indices() {
            if cr && ch == '\n' {
                cr = false;
                continue;
            }
            if current_line == line && (current_column >= column || ch == '\r' || ch == '\n') {
                return Ok(TextOffset(window.range().start.0 + byte));
            }
            if ch == '\n' || ch == '\r' {
                current_line += 1;
                current_column = 1;
            } else {
                current_column += 1;
            }
            cr = ch == '\r';
        }
        let next = window.range().end.0;
        if next <= cursor {
            return Err("Output source made no progress".into());
        }
        cursor = next;
    }
    if current_line == line {
        Ok(TextOffset(snapshot.len()))
    } else {
        Err("Output line is outside the document".into())
    }
}
