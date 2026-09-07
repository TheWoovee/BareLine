// SPDX-License-Identifier: MPL-2.0
//! View consumers for power transactions. Receipts are published only after success.
use super::*;
use crate::{EditorSurface, group_view::SurfaceGroup};
use std::collections::BTreeMap;
use bareline_document::service::Scheduler;

pub type Arguments = BTreeMap<String, String>;
fn parameter<T: std::str::FromStr>(args: &Arguments, key: &str) -> Result<T, String> {
    args.get(key).ok_or_else(|| format!("Missing {key}."))?.parse().map_err(|_| format!("Invalid {key}."))
}
fn rectangle(args: &Arguments) -> Result<Rectangle, String> {
    Ok(Rectangle { first_line: parameter(args,"first_line")?, last_line: parameter(args,"last_line")?, start_column: parameter(args,"start_column")?, end_column: parameter(args,"end_column")? })
}
impl EditorSurface {
    pub(crate) fn acknowledge_command(&mut self, receipt: (String, Arguments)) {
        if self.acknowledged_commands.len() == 256 { self.acknowledged_commands.pop_front(); }
        self.acknowledged_commands.push_back(receipt);
    }
    pub fn take_acknowledged_commands(&mut self) -> Vec<(String, Arguments)> {
        self.acknowledged_commands.drain(..).collect()
    }
    /// Replay takes explicit plain-text parameters; it never opens a dialog or reads a clipboard.
    pub fn execute_power_recorded(&mut self, id: &str, args: &Arguments) -> Result<(), String> {
        if self.busy() || self.read_only() || self.composition.is_some() { return Err("Document is busy or read only.".into()); }
        if args.len() > 16 || args.iter().try_fold(0usize, |n,(k,v)| n.checked_add(k.len())?.checked_add(v.len())).is_none_or(|n| n > self.power_limits().max_bytes) { return Err("Command arguments exceed the edit budget.".into()); }
        let limits = self.power_limits();
        let err = |e| format!("Command was not applied: {e:?}");
        match id {
            "editor.column.insert" => {
                let insert = match args.get("mode").map(String::as_str) {
                    Some("text") => ColumnInsert::Text(args.get("text").cloned().ok_or("Missing text.")?),
                    Some("numbers") => ColumnInsert::Numbers { start: parameter(args,"start")?, step: parameter(args,"step")?, width: parameter(args,"width")?, base: parameter(args,"base")?, repeat: parameter(args,"repeat")? },
                    _ => return Err("Invalid column mode.".into()),
                };
                self.apply_power(column_insert(&self.snapshot, rectangle(args)?, insert, limits).map_err(err)?)?;
            }
            "editor.rectangle.paste" | "editor.rectangle.delete" => {
                let text = if id.ends_with("delete") { "" } else { args.get("text").ok_or("Missing text.")? };
                self.apply_power(rectangle_paste(&self.snapshot,rectangle(args)?,text,limits).map_err(err)?)?;
            }
            "editor.paste.plainText" | "editor.paste.fromHistory" => {
                let text = args.get("text").ok_or("Missing plain text.")?;
                self.apply_power(replace(&self.snapshot,&self.selection_set(),text,limits).map_err(err)?)?;
            }
            "editor.lines.hide" => {
                let set = self.selection_set();
                for selected in set.selections {
                    let range = selected.range();
                    let first = self.snapshot.line_at(TextOffset(range.start)).map_err(err)?;
                    let last = self.snapshot.line_at(TextOffset(if range.end > range.start { range.end - 1 } else { range.end })).map_err(err)?;
                    // Keep one visible line so view navigation always has an anchor.
                    if first > 0 { self.manual_hidden.push(first..=last); }
                    else if last > 0 { self.manual_hidden.push(1..=last); }
                }
                self.refresh_hidden_lines();
            }
            "editor.lines.showAll" => { self.manual_hidden.clear(); self.refresh_hidden_lines(); }
            "editor.bookmark.deleteLines" | "editor.bookmark.cutLines" => {
                let selections = self.bookmarks.selections(&self.snapshot,limits).map_err(err)?;
                self.apply_power(replace(&self.snapshot,&selections,"",limits).map_err(err)?)?;
            }
            _ => {
                if !args.is_empty() { return Err("This command does not accept arguments.".into()); }
                self.execute_power(id)?;
            }
        }
        let receipt = (id.to_owned(),args.clone());
        if self.pending.is_some() { self.pending_command = Some(receipt); } else { self.acknowledge_command(receipt); }
        Ok(())
    }
    pub fn logical_scroll(&self) -> (u64, f64, f64) {
        let row = self.scroll_y / self.line_height() as f64;
        (self.logical_line(row.floor() as usize) as u64,row.fract(),self.scroll_x)
    }
    pub fn set_logical_scroll(&mut self, line: u64, fraction: f64, x: f64) {
        let line = usize::try_from(line).unwrap_or(usize::MAX).min(self.snapshot.line_count().saturating_sub(1));
        self.scroll_y = (self.visual_line(line) as f64 + if fraction.is_finite() { fraction.clamp(0.0,0.999999) } else { 0.0 }) * self.line_height() as f64;
        self.scroll_x = if x.is_finite() { x.max(0.0) } else { 0.0 };
        self.reveal_caret = false;
    }
    pub fn horizontal_scroll(&mut self, delta: f64) {
        if delta.is_finite() { self.scroll_x = (self.scroll_x + delta).max(0.0); self.reveal_caret = false; }
    }
    pub fn copy_bookmarked_lines(&self) -> Result<String,String> {
        let mut output = String::new();
        let limits = self.power_limits();
        for selection in self.bookmarks.selections(&self.snapshot,limits).map_err(|e|format!("{e:?}"))?.selections {
            let range = selection.range();
            let text = self.snapshot.read(TextOffset(range.start)..TextOffset(range.end),limits.max_bytes.saturating_sub(output.len())).map_err(|e|format!("{e:?}"))?;
            output.push_str(&text);
        }
        Ok(output)
    }
    pub fn select_rectangle(&mut self, rectangle: Rectangle) -> Result<(),String> {
        let limits = self.power_limits();
        if rectangle.first_line > rectangle.last_line || rectangle.last_line.saturating_sub(rectangle.first_line) >= limits.max_selections { return Err("Rectangle exceeds the selection budget.".into()); }
        let mut selections = Vec::new();
        for number in rectangle.first_line..=rectangle.last_line {
            let (start,text) = line(&self.snapshot,number,limits).map_err(|e|format!("{e:?}"))?;
            let map = DisplayColumnMap::new(content(&text),limits.tab_width);
            selections.push(Selection { anchor: start + map.at(rectangle.start_column).0, caret: start + map.at(rectangle.end_column).0 });
        }
        self.set_selections(SelectionSet { selections,primary:0 })?;
        self.power_rectangle = Some(rectangle);
        Ok(())
    }
    pub fn power_hit_position(&self, backend: &impl bareline_renderer::TextBackend, point: bareline_renderer::Point) -> Option<(usize, usize, usize)> {
        if point.x < crate::LEFT || point.y < self.top() { return None; }
        let row = ((point.y-self.top()) as f64+self.scroll_y)/self.line_height() as f64;
        let number=self.logical_line(row.floor() as usize);
        let layout=self.layouts.get(&number)?;
        let hit=backend.hit_test(layout.id,bareline_renderer::Point { x:point.x-crate::LEFT+self.scroll_x as f32,y:(row.fract()*self.line_height() as f64) as f32 }).ok()?;
        let offset=(layout.start+hit.byte_offset).min(layout.end);
        let (start,text)=line(&self.snapshot,number,self.power_limits()).ok()?;
        let map=DisplayColumnMap::new(content(&text),self.tab_width);
        Some((offset,number,map.column(offset.saturating_sub(start))))
    }
    pub fn caret_display_position(&self) -> Result<(usize,usize),String> {
        let number=self.snapshot.line_at(TextOffset(self.selection.caret)).map_err(|e|format!("{e:?}"))?;
        let (start,text)=line(&self.snapshot,number,self.power_limits()).map_err(|e|format!("{e:?}"))?;
        Ok((number,DisplayColumnMap::new(content(&text),self.tab_width).column(self.selection.caret-start)))
    }
    pub fn toggle_power_caret(&mut self, offset: usize) -> Result<(),String> {
        self.set_selections(toggle_caret(&self.snapshot,&self.selection_set(),offset,self.power_limits()).map_err(|e|format!("{e:?}"))?)
    }
}
/// Target copy changes only the destination. Moves retain the coordinator ticket until completion.
pub fn drag_between(scheduler: &Scheduler, source: &mut EditorSurface, target: &mut EditorSurface, offset: usize, copy: bool) -> Result<Option<SurfaceGroup>,String> {
    let limits = source.power_limits();
    if source.snapshot.same_document(&target.snapshot) {
        let edit = drag_text(&source.snapshot,source.selection,offset,copy,limits).map_err(|e|format!("{e:?}"))?;
        source.apply_power(edit)?;
        return Ok(None);
    }
    if copy {
        let range = source.selection.range();
        let text = source.snapshot.read(TextOffset(range.start)..TextOffset(range.end),limits.max_bytes).map_err(|e|format!("{e:?}"))?;
        let edit = replace(&target.snapshot,&Selection { anchor:offset,caret:offset }.into(),&text,limits).map_err(|e|format!("{e:?}"))?;
        target.apply_power(edit)?;
        return Ok(None);
    }
    let prepared = cross_document_drag(&source.snapshot,source.selection,&target.snapshot,offset,limits).map_err(|e|format!("{e:?}"))?;
    SurfaceGroup::apply(scheduler,&mut [source,target],vec![(prepared.source_snapshot,prepared.source),(prepared.target_snapshot,prepared.target)]).map(Some)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn surface(text: &str) -> EditorSurface {
        let document = bareline_document::Document::from_utf8(text,bareline_document::Budget::new(1<<20),bareline_document::Budget::new(1<<20)).unwrap();
        let snapshot=document.snapshot();
        let scheduler=Scheduler::new(1,8).unwrap();
        EditorSurface::new(scheduler.document(document,8),snapshot,std::sync::Arc::new(||{}))
    }
    #[test]
    fn hidden_lines_are_independent_of_folds_and_bytes() {
        let mut editor=surface("zero\none\ntwo\nthree");
        editor.set_selections(Selection{anchor:5,caret:12}.into()).unwrap();
        let revision=editor.snapshot.revision;
        editor.execute_power_recorded("editor.lines.hide",&Arguments::new()).unwrap();
        assert_eq!(editor.logical_line(1),3);
        assert_eq!(editor.snapshot.revision,revision);
        editor.unfold_all();
        editor.refresh_hidden_lines();
        assert_eq!(editor.logical_line(1),3);
        editor.execute_power_recorded("editor.lines.showAll",&Arguments::new()).unwrap();
        assert_eq!(editor.logical_line(1),1);
    }
    #[test]
    fn receipts_are_bounded_and_invalid_replay_is_silent() {
        let mut editor=surface("word");
        for _ in 0..300 {editor.execute_power_recorded("editor.selection.rotatePrimary",&Arguments::new()).unwrap();}
        assert_eq!(editor.take_acknowledged_commands().len(),256);
        assert!(editor.execute_power_recorded("editor.column.insert",&Arguments::new()).is_err());
        assert!(editor.take_acknowledged_commands().is_empty());
    }
    #[test]
    fn logical_scroll_round_trips_hidden_rows_and_horizontal_offset() {
        let mut editor=surface("a\nb\nc\nd");
        editor.manual_hidden.push(1..=2);editor.refresh_hidden_lines();
        editor.set_logical_scroll(3,0.25,48.0);
        assert_eq!(editor.logical_scroll(),(3,0.25,48.0));
        editor.horizontal_scroll(-100.0);
        assert_eq!(editor.logical_scroll().2,0.0);
    }
}
