// SPDX-License-Identifier: MPL-2.0
//! View consumers for power transactions. Receipts are published only after success.
use super::*;
use crate::{EditorSurface, group_view::SurfaceGroup};
use std::collections::BTreeMap;
use bareline_document::service::Scheduler;

pub type Arguments = BTreeMap<String, String>;
static RECEIPT_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
/// One process-wide clock shared by editor and verified search completions.
pub fn next_receipt_sequence() -> u64 {
    RECEIPT_SEQUENCE.fetch_add(1,std::sync::atomic::Ordering::Relaxed)
}
#[derive(Clone)]
pub enum ReceiptEvent { Input(crate::Input), Command(String,Arguments) }
#[derive(Clone)]
pub struct OrderedReceipt { pub sequence:u64, pub event:ReceiptEvent }

fn parameter<T: std::str::FromStr>(args: &Arguments, key: &str) -> Result<T, String> {
    args.get(key).ok_or_else(|| format!("Missing {key}."))?.parse().map_err(|_| format!("Invalid {key}."))
}
fn rectangle(args: &Arguments) -> Result<Rectangle, String> {
    Ok(Rectangle { first_line: parameter(args,"first_line")?, last_line: parameter(args,"last_line")?, start_column: parameter(args,"start_column")?, end_column: parameter(args,"end_column")? })
}
impl EditorSurface {
    pub fn take_ordered_receipts(&mut self) -> Vec<OrderedReceipt> {
        self.acknowledged.clear();self.acknowledged_commands.clear();
        self.ordered_receipts.drain(..).collect()
    }
    pub(crate) fn acknowledge_event(&mut self,event:ReceiptEvent) {
        if self.ordered_receipts.len()==256 {self.ordered_receipts.pop_front();}
        self.ordered_receipts.push_back(OrderedReceipt {sequence:next_receipt_sequence(),event});
    }
    pub(crate) fn acknowledge_command(&mut self, receipt: (String, Arguments)) {
        self.acknowledge_event(ReceiptEvent::Command(receipt.0.clone(),receipt.1.clone()));
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
            "editor.indent" | "editor.unindent" if self.power_rectangle.is_some() => {
                let rectangle=self.power_rectangle.unwrap();
                self.apply_power(rectangle_indent(&self.snapshot,rectangle,id=="editor.unindent",limits).map_err(err)?)?;
            }
            "editor.rectangle.select" => {self.select_rectangle(rectangle(args)?)?;}
            "editor.column.insert" => {
                let insert = match args.get("mode").map(String::as_str) {
                    Some("text") => ColumnInsert::Text(args.get("text").cloned().ok_or("Missing text.")?),
                    Some("numbers") => ColumnInsert::Numbers { start: parameter(args,"start")?, step: parameter(args,"step")?, width: parameter(args,"width")?, base: parameter(args,"base")?, repeat: parameter(args,"repeat")? },
                    _ => return Err("Invalid column mode.".into()),
                };
                self.apply_power(column_insert_mapped(&self.snapshot, rectangle(args)?, insert, limits,self.current_column_maps()).map_err(err)?)?;
            }
            "editor.rectangle.paste" | "editor.rectangle.delete" => {
                let text = if id.ends_with("delete") { "" } else { args.get("text").ok_or("Missing text.")? };
                self.apply_power(self.prepare_rectangle_paste(rectangle(args)?,text).map_err(err)?)?;
            }
            "editor.paste.plainText" | "editor.paste.fromHistory" => {
                let text = args.get("text").ok_or("Missing plain text.")?;
                let prepared=if let Some(rectangle)=self.power_rectangle {self.prepare_rectangle_paste(rectangle,text)}else{replace(&self.snapshot,&self.selection_set(),text,limits)};
                self.apply_power(prepared.map_err(err)?)?;
            }
            "editor.lines.hide" => {
                let set = self.selection_set();
                for selected in set.selections {
                    let range = selected.range();
                    let first = self.snapshot.line_at(TextOffset(range.start)).map_err(err)?;
                    let mut end = if range.end > range.start {range.end-1}else{range.end};
                    while !self.snapshot.is_boundary(TextOffset(end)) {end=end.saturating_sub(1);}
                    let last = self.snapshot.line_at(TextOffset(end)).map_err(err)?;
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
            let fallback;
            let map=if let Some(map)=self.rectangle_maps(rectangle).and_then(|maps|maps.get(&number)){map}else{fallback=DisplayColumnMap::new(content(&text),limits.tab_width);&fallback};
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
        let hit=backend.hit_test(layout.id,bareline_renderer::Point { x:point.x-crate::LEFT+(self.scroll_x-layout.x_origin) as f32,y:((row-(self.visual_line(number)+layout.row_origin) as f64)*self.line_height() as f64) as f32 }).ok()?;
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

impl EditorSurface {
    pub fn font_family(&self) -> &str { &self.font_family }
    pub fn set_font_family(&mut self, family: &str) -> Result<(),String> {
        if !bareline_renderer::valid_font_family(family) { return Err("Invalid editor font family.".into()); }
        if self.font_family != family { self.font_family = family.into(); self.layout_revision = None; self.clear_column_metrics(); }
        Ok(())
    }
    /// Sorted view-only rows inserted before logical lines; no bytes or history are changed.
    pub fn set_view_spacers(&mut self, rows: &[(u64,u64)]) -> Result<(),String> {
        if rows.len()>100_000 { return Err("Too many view spacers.".into()); }
        let mut spacers: Vec<(usize,usize)> = Vec::with_capacity(rows.len());
        let mut total=0usize;
        for &(line,count) in rows {
            let line=usize::try_from(line).map_err(|_|"Spacer line is out of range.")?;
            let count=usize::try_from(count).map_err(|_|"Spacer count is out of range.")?;
            total=total.checked_add(count).filter(|n|*n<=100_000).ok_or("Too many spacer rows.")?;
            if line>self.snapshot.line_count() || spacers.last().is_some_and(|(previous,_)|*previous>=line) { return Err("Spacer lines must be sorted and unique.".into()); }
            if count>0 {spacers.push((line,count));}
        }
        self.view_spacers=spacers;
        self.reveal_caret=false;
        Ok(())
    }
    pub fn migrate_clean_spill(&self, captured:&DocumentSnapshot, source:bareline_document::source::MemorySource)->Result<bareline_document::paged::PagedDocument,Error> {
        if self.busy()||self.dirty() {return Err(Error::ActorBusy);}
        self.service.as_ref().ok_or(Error::ActorBusy)?.migrate_clean_spill(captured,source)
    }
    pub fn cancel_clean_spill(&self, captured:&DocumentSnapshot)->Result<(),Error> {
        self.service.as_ref().ok_or(Error::ActorBusy)?.cancel_clean_spill(captured)
    }
}

impl EditorSurface {
    /// Copy view preferences during storage migration without replacing document ownership.
    pub fn copy_presentation_to(&self, view: &mut EditorSurface) {
        view.eol_status_override = self.eol_status_override.clone();
        view.theme=self.theme;
        view.language=self.language;view.language_override=self.language_override;
        view.detected_language=self.detected_language;view.syntax_preference=self.syntax_preference;
        view.udl=self.udl.clone();view.smart_typing=self.smart_typing;view.smart_pairs=self.smart_pairs;view.smart_indent=self.smart_indent;
        view.search_marks=self.search_marks.clone();
        view.manual_hidden=self.manual_hidden.clone();view.known_folds=self.known_folds.clone();view.fold_state=self.fold_state.clone();view.hidden_lines=self.hidden_lines.clone();view.fold_revision=self.fold_revision;view.folds_incomplete=self.folds_incomplete;view.pending_folds=self.pending_folds.clone();
        view.encoding_label=self.encoding_label.clone();view.font_pixels=self.font_pixels;view.font_family=self.font_family.clone();view.tab_width=self.tab_width;view.line_numbers=self.line_numbers;view.highlight_current_line=self.highlight_current_line;view.whitespace=self.whitespace.clone();
        view.scroll_y=self.scroll_y;view.scroll_x=self.scroll_x;view.top_inset=self.top_inset;view.bottom_inset=self.bottom_inset;view.view_spacers=self.view_spacers.clone();view.layout_revision=None;
    }
}

/// Rectangle indentation touches only the insertion column or whitespace immediately before it.
pub fn rectangle_indent(snapshot:&DocumentSnapshot, rectangle:Rectangle, backward:bool, limits:Limits)->Result<PowerEdit,Error> {
    let left=rectangle.start_column.min(rectangle.end_column);
    if !backward {
        if limits.tab_width>limits.max_bytes{return Err(Error::BudgetExceeded);}
        return column_insert(snapshot,Rectangle {start_column:left,end_column:left,..rectangle},ColumnInsert::Text(" ".repeat(limits.tab_width)),limits);
    }
    if rectangle.first_line>rectangle.last_line||rectangle.last_line-rectangle.first_line>=limits.max_selections{return Err(Error::BudgetExceeded);}
    let mut edits=Vec::new();
    for number in rectangle.first_line..=rectangle.last_line {
        let (start,text)=line(snapshot,number,limits)?;
        let body=content(&text);let map=DisplayColumnMap::new(body,limits.tab_width);let end=map.at(left).0;
        let mut begin=end;
        for (index,character) in body[..end].char_indices().rev().take(limits.tab_width) {
            if character==' ' {begin=index;} else if character=='\t' {begin=index;break;} else {break;}
        }
        if begin<end {edits.push(Edit {range:TextOffset(start+begin)..TextOffset(start+end),insert:String::new()});}
    }
    finish(snapshot,edits,limits)
}
#[cfg(test)]
mod presentation_tests {
    use super::*;
    #[test]
    fn spacer_mapping_hits_next_real_line_without_changing_document() {
        let document=bareline_document::Document::from_utf8("a\nb\nc",bareline_document::Budget::new(1024),bareline_document::Budget::new(1024)).unwrap();
        let mut editor=EditorSurface::loading(document.snapshot(),std::sync::Arc::new(||{}));
        editor.set_view_spacers(&[(1,2)]).unwrap();
        assert_eq!(editor.visual_line(1),3);
        assert_eq!(editor.logical_line(1),1);
        assert_eq!(editor.logical_line(2),1);
        assert_eq!(editor.logical_line(3),1);
        assert!(editor.set_view_spacers(&[(2,1),(1,1)]).is_err());
        assert_eq!(editor.visual_line(1),3);
        editor.set_view_spacers(&[]).unwrap();
        assert_eq!(editor.visual_line(1),1);
        assert_eq!(editor.snapshot.len(),5);
    }
}
/// Private clipboard metadata is advisory. Plain text remains the complete payload.
#[derive(Clone,Debug,PartialEq,Eq)]
pub struct RectangleClipboardMetadata { pub row_widths:Vec<u32> }
impl RectangleClipboardMetadata {
    pub const FORMAT: &'static str = "Bareline.Rectangle.v1";
    pub fn encode(&self,text:&str)->Result<Vec<u8>,Error> {
        if self.row_widths.len()>65_531 || self.row_widths.len()!=clipboard_rows(text).len() || text.len()>4<<20 {return Err(Error::BudgetExceeded);}
        let mut bytes=b"BLRC\x01\0\0\0".to_vec();
        bytes.extend_from_slice(&(self.row_widths.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&clipboard_fingerprint(text).to_le_bytes());
        for width in &self.row_widths {bytes.extend_from_slice(&width.to_le_bytes());}
        Ok(bytes)
    }
    pub fn decode(bytes:&[u8],text:&str)->Option<Self> {
        if bytes.len()<20 || bytes.len()>262_144 || &bytes[..8]!=b"BLRC\x01\0\0\0" || text.len()>4<<20{return None;}
        let count=u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
        if count>65_531 || bytes.len()!=20+count*4 || count!=clipboard_rows(text).len() || u64::from_le_bytes(bytes[12..20].try_into().ok()?)!=clipboard_fingerprint(text){return None;}
        Some(Self {row_widths:bytes[20..].chunks_exact(4).map(|chunk|u32::from_le_bytes(chunk.try_into().unwrap())).collect()})
    }
}
fn clipboard_fingerprint(text:&str)->u64 {text.bytes().fold(0xcbf29ce484222325,|hash,byte|(hash^byte as u64).wrapping_mul(0x100000001b3))}
impl EditorSurface {
    pub fn active_rectangle(&self)->Option<Rectangle> {self.power_rectangle}
    pub fn rectangle_clipboard_metadata(&self,text:&str)->Result<Option<Vec<u8>>,Error> {
        let Some(rectangle)=self.power_rectangle else{return Ok(None);};
        let rows=rectangle.last_line.checked_sub(rectangle.first_line).and_then(|n|n.checked_add(1)).ok_or(Error::OutOfBounds)?;
        if rows>100_000{return Err(Error::BudgetExceeded);}
        let width=u32::try_from(rectangle.start_column.abs_diff(rectangle.end_column)).map_err(|_|Error::BudgetExceeded)?;
        RectangleClipboardMetadata {row_widths:vec![width;rows]}.encode(text).map(Some)
    }
}

impl EditorSurface {
    pub fn set_search_marks(&mut self,style:u8,ranges:Vec<std::ops::Range<TextOffset>>)->Result<(),String> {
        if ranges.iter().any(|range|range.end.0>self.snapshot.len()||!self.snapshot.is_boundary(range.start)||!self.snapshot.is_boundary(range.end)){return Err("Invalid search mark range.".into());}
        self.search_marks.set(style,ranges)
    }
    pub fn clear_search_marks(&mut self,style:Option<u8>){self.search_marks.clear(style);}
}

pub(crate) fn history_selections(set:&SelectionSet)->Vec<bareline_document::history::Selection> {
    set.selections.iter().map(|selection|bareline_document::history::Selection {anchor:TextOffset(selection.anchor),caret:TextOffset(selection.caret)}).collect()
}
pub(crate) fn monotonic_ms()->u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis().min(u64::MAX as u128)as u64
}

impl EditorSurface {
    fn current_column_maps(&self)->Option<&BTreeMap<usize,DisplayColumnMap>> {
        (self.column_maps_revision==Some(self.snapshot.revision)&&!self.column_maps.is_empty()).then_some(&self.column_maps)
    }
    pub fn clear_column_metrics(&mut self){self.column_maps.clear();self.column_map_bytes=0;self.column_maps_revision=None;}
    /// One bounded row per call; native jobs yield between batches and validate revision.
    pub fn measure_column_row(&mut self,backend:&mut impl bareline_renderer::TextBackend,number:usize)->Result<(),String> {
        if self.column_maps_revision!=Some(self.snapshot.revision){self.clear_column_metrics();self.column_maps_revision=Some(self.snapshot.revision);}
        if self.column_maps.contains_key(&number){return Ok(());}
        let (_,text)=line(&self.snapshot,number,Limits {max_bytes:1<<20,..self.power_limits()}).map_err(|e|format!("Column measurement: {e:?}"))?;
        let body=content(&text);
        let charge=body.graphemes(true).count().checked_add(1).and_then(|n|n.checked_mul(2*std::mem::size_of::<usize>())).ok_or("Column metrics exceed budget.")?;
        if self.column_map_bytes.checked_add(charge).is_none_or(|n|n>self.power_limits().max_bytes){return Err("Column metrics exceed budget.".into());}
        let space=backend.shape_with_font_family(" ",self.font_pixels,1_000_000.0,&self.font_family).map_err(|e|format!("{e:?}"))?;
        let unit=backend.caret(space,1).map(|r|r.x).map_err(|e|format!("{e:?}"));backend.release_layout(space);let unit=unit?.max(0.1);
        let layout=backend.shape_with_font_family(body,self.font_pixels,1_000_000.0,&self.font_family).map_err(|e|format!("{e:?}"))?;
        let mut widths=Vec::new();
        let measured=(||{for (index,grapheme) in body.grapheme_indices(true){
            let a=backend.caret(layout,index).map_err(|e|format!("{e:?}"))?;let b=backend.caret(layout,index+grapheme.len()).map_err(|e|format!("{e:?}"))?;
            if (a.y-b.y).abs()>0.1{return Err("Column row exceeded the layout width.".into());}
            widths.push(((b.x-a.x).abs()/unit).round().max(1.0)as usize);
        }Ok::<(),String>(())})();backend.release_layout(layout);measured?;
        let mut widths=widths.into_iter();
        // Consume one measured width per cluster, including tabs, whose stops remain configured.
        let mut column=0usize;let mut stops=vec![(0,0)];
        for (index,grapheme) in body.grapheme_indices(true){let width=widths.next().unwrap_or(1);column=column.checked_add(if grapheme=="\t"{self.tab_width.max(1)-column%self.tab_width.max(1)}else{width}).ok_or("Column width overflow.")?;stops.push((index+grapheme.len(),column));}
        self.column_maps.insert(number,DisplayColumnMap{stops});self.column_map_bytes+=charge;Ok(())
    }
}
#[cfg(test)]
mod typing_metadata_tests {
    use super::*;
    fn finish(editor:&mut EditorSurface){
        let until=std::time::Instant::now()+std::time::Duration::from_secs(3);
        while editor.busy(){assert!(std::time::Instant::now()<until);editor.pump();std::thread::yield_now();}
    }
    #[test]
    fn typing_coalesces_and_paste_keeps_an_explicit_boundary(){
        let scheduler=Scheduler::new(1,8).unwrap();
        let document=bareline_document::Document::from_utf8("",bareline_document::Budget::new(1<<20),bareline_document::Budget::new(1<<20)).unwrap();
        let snapshot=document.snapshot();let mut editor=EditorSurface::new(scheduler.document(document,8),snapshot,std::sync::Arc::new(||{}));
        editor.enqueue(crate::Input::Insert("a".into()));finish(&mut editor);
        editor.enqueue(crate::Input::Insert("b".into()));finish(&mut editor);
        assert_eq!(editor.undo_selection.len(),1);
        editor.enqueue_with_origin(crate::Input::Insert("c".into()),bareline_document::history::EditOrigin::Paste);finish(&mut editor);
        assert_eq!(editor.undo_selection.len(),2);
        editor.enqueue(crate::Input::Undo);finish(&mut editor);
        assert_eq!(editor.snapshot.read(TextOffset(0)..TextOffset(editor.snapshot.len()),1024).unwrap(),"ab");
        assert_eq!(editor.selection.caret,2);
        editor.enqueue(crate::Input::Undo);finish(&mut editor);
        assert_eq!(editor.snapshot.len(),0);assert_eq!(editor.selection.caret,0);
        editor.enqueue(crate::Input::Redo);finish(&mut editor);
        assert_eq!(editor.snapshot.read(TextOffset(0)..TextOffset(editor.snapshot.len()),1024).unwrap(),"ab");
    }
    #[test]
    fn private_rectangle_metadata_is_versioned_and_bound_to_plain_text(){
        let metadata=RectangleClipboardMetadata{row_widths:vec![2,2]};let bytes=metadata.encode("界\nab").unwrap();
        assert_eq!(RectangleClipboardMetadata::decode(&bytes,"界\nab"),Some(metadata));
        assert!(RectangleClipboardMetadata::decode(&bytes,"xy\nab").is_none());
        let mut corrupt=bytes;corrupt[4]=2;assert!(RectangleClipboardMetadata::decode(&corrupt,"界\nab").is_none());
    }
}
impl EditorSurface {
    /// Clone the owning actor handle for revision-checked coordinated jobs.
    pub fn document_service(&self)->Option<bareline_document::service::DocumentService>{self.service.clone()}
}

impl EditorSurface {
    fn rectangle_maps(&self,rectangle:Rectangle)->Option<&BTreeMap<usize,DisplayColumnMap>> {
        let maps=self.current_column_maps()?;
        (rectangle.first_line..=rectangle.last_line).all(|number|maps.contains_key(&number)).then_some(maps)
    }
    pub(crate) fn prepare_rectangle_paste(&self,rectangle:Rectangle,text:&str)->Result<PowerEdit,Error>{
        rectangle_paste_mapped(&self.snapshot,rectangle,text,self.power_limits(),self.rectangle_maps(rectangle))
    }
    pub(crate) fn copy_rectangle(&self,rectangle:Rectangle,limit:usize)->Result<String,Error>{
        rectangle_copy_mapped(&self.snapshot,rectangle,Limits{max_bytes:limit,..self.power_limits()},self.rectangle_maps(rectangle))
    }
}
impl EditorSurface {
    pub fn saved_content_state(&self)->bareline_document::ContentStateId{self.initial_state}
}
impl EditorSurface {
    pub fn begin_column_measurement(&mut self)->Result<(),String>{
        if self.pending.is_some()||self.group_pending||self.composition.is_some(){return Err("Wait for the current edit before measuring columns.".into());}
        self.column_measurement_pending=true;Ok(())
    }
    pub fn finish_column_measurement(&mut self){self.column_measurement_pending=false;}
}
