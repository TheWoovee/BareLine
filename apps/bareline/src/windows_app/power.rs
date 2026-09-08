// SPDX-License-Identifier: MPL-2.0
//! Native Column Editor and ephemeral clipboard-history consumers.
use super::*;
use bareline_editor_surface::power::{self, Rectangle, ClipboardHistory, consumer::Arguments};
use bareline_renderer::{DrawOp, Rect, LayoutError};
use bareline_ui::{rect, text, text_field::TextField};
#[path="power_stream.rs"]
mod stream;

#[derive(Clone, Debug, Default)]
struct PowerLayout {
    bounds: Rect,
    fields: Vec<Rect>,
    history_rows: Vec<Rect>,
    apply: Rect,
    cancel: Rect,
}
impl PowerLayout {
    fn new(width: f32, height: f32) -> Self {
        let bounds = rect((width - 440.0).max(0.0) / 2.0, (height - 400.0).max(48.0) / 2.0, width.min(440.0), height.min(400.0));
        let content_end = bounds.y + bounds.height - 70.0;
        let fields = (0..7).map(|index| {
            let y = bounds.y + 44.0 + index as f32 * 37.0;
            rect(bounds.x + 190.0, y, (bounds.width - 206.0).max(0.0), (content_end - y).clamp(0.0, 30.0))
        }).collect();
        let row_count = ((content_end - bounds.y - 44.0) / 27.0).floor().clamp(0.0, 10.0) as usize;
        let history_rows = (0..row_count).map(|row| rect(bounds.x + 10.0, bounds.y + 44.0 + row as f32 * 27.0, (bounds.width - 20.0).max(0.0), 27.0)).collect();
        let button_width = ((bounds.width - 44.0).max(0.0) / 2.0).min(120.0);
        let y = bounds.y + bounds.height - 42.0;
        Self { bounds, fields, history_rows, apply: rect(bounds.x + 16.0, y, button_width, 30.0), cancel: rect(bounds.x + 28.0 + button_width, y, button_width, 30.0) }
    }
    fn history_start(&self, selected: usize) -> usize { selected.saturating_sub(self.history_rows.len().saturating_sub(1)) }
    fn history_hit(&self, point: Point, selected: usize, count: usize) -> Option<usize> {
        self.history_rows.iter().position(|bounds| bounds.contains(point)).map(|row| self.history_start(selected) + row).filter(|index| *index < count)
    }
}
fn accessible_bounds(bounds: Rect) -> [f64; 4] { [bounds.x as f64, bounds.y as f64, bounds.width as f64, bounds.height as f64] }

pub(super) struct PowerRuntime {
    stream: stream::StreamRuntime,
    pub open: bool,
    history_open: bool,
    pub history: ClipboardHistory,
    history_limits: (usize,usize,usize),
    fields: Vec<TextField>,
    focus: usize,
    selected: usize,
    accessibility_focus: Option<u64>,
    layout: PowerLayout,
    status: String,
    rectangle: Option<Rectangle>,
    target: Option<bareline_document::DocumentSnapshot>,
    global_target:Option<bareline_document::paged::PagedSnapshot>,
    rectangle_drag: Option<(usize,usize)>,
    paged_rectangle_drag:Option<(usize,usize,bareline_document::paged::PagedSnapshot,usize)>,
    drag: Option<(usize,u32,bareline_document::DocumentSnapshot,bareline_editor_surface::Selection)>,
    group: Option<(bareline_editor_surface::group_view::SurfaceGroup,usize)>,
    metric_job: Option<(bareline_document::DocumentSnapshot,Arguments,usize,usize,Option<Rectangle>)>,
}
impl Default for PowerRuntime {
    fn default() -> Self {
        let fields = ["text","","0","1","0","10","1"].into_iter().map(|value| { let mut field = TextField::default(); field.insert(value); field }).collect();
        Self { paged_rectangle_drag:None,global_target:None,stream:stream::StreamRuntime::default(),open:false,history_open:false,history:ClipboardHistory::default(),history_limits:(20,16<<20,4<<20),fields,focus:0,selected:0,accessibility_focus:None,layout:PowerLayout::default(),status:String::new(),rectangle:None,target:None,rectangle_drag:None,drag:None,group:None,metric_job:None }
    }
}
pub(super) fn register(registry: &mut bareline_commands::CommandRegistry) {
    use bareline_commands::{CommandId,CommandSpec};
    for (id,title) in [("editor.bookmark.copyLines","Copy Bookmarked Lines"),("editor.bookmark.cutLines","Cut Bookmarked Lines"),("editor.bookmark.deleteLines","Delete Bookmarked Lines"),("editor.rectangle.paste","Paste into Rectangle"),("editor.rectangle.delete","Delete Rectangle")] {
        if registry.dispatch(CommandId(id)).is_none() { let _ = registry.register(CommandSpec { id:CommandId(id),title,category:"Edit",shortcut:"",action:Action::Contributed(CommandId(id)) }); }
    }
}
impl PowerRuntime {
    pub(super) fn configure_history(&mut self, enabled:bool,count:usize,total:usize,entry:usize) {
        let limits=(count.min(20),total.min(16<<20),entry.min(4<<20));
        if self.history_limits!=limits {self.history.set_enabled(false);self.history_limits=limits;}
        self.history.set_enabled(enabled);
    }
    pub(super) fn copied(&mut self, text: &str) { if let Err(error) = self.history.admit_with_limits(text,self.history_limits.0,self.history_limits.1,self.history_limits.2) { self.status = format!("Clipboard history: {error:?}"); } }
    pub(super) fn draw(&mut self, renderer: &mut WindowsRenderer, width: f32, height: f32, ops: &mut Vec<DrawOp>) -> Result<Option<Rect>,LayoutError> {
        if !self.open { return Ok(None); }
        let theme = bareline_ui::theme::UiTheme::default();
        self.layout = PowerLayout::new(width, height);
        let b = self.layout.bounds;
        ops.push(DrawOp::Fill(b,theme.chrome)); ops.push(DrawOp::Stroke(b,theme.border,1.0)); ops.push(DrawOp::PushClip(b));
        text(ops,b.x+16.0,b.y+12.0,if self.history_open { "Paste from History" } else { "Column Editor" },17.0,theme.text);
        let mut caret = None;
        if self.history_open {
            let start = self.layout.history_start(self.selected);
            for ((index,entry), bounds) in self.history.entries().enumerate().skip(start).zip(&self.layout.history_rows) {
                if index==self.selected { ops.push(DrawOp::Fill(*bounds,theme.interactive)); }
                let preview: String=entry.chars().map(|c| if c=='\n'||c=='\r' {' '}else{c}).take(52).collect(); text(ops,bounds.x+6.0,bounds.y+5.0,&preview,13.0,theme.text);
            }
            if self.history.entries().next().is_none() { text(ops,b.x+16.0,b.y+52.0,"No copied text in this session.",13.0,theme.muted); }
        } else {
            for (index,label) in ["Mode: text / numbers","Repeated text","Initial number","Increment","Zero-padding width","Base: 10 / 16 / 8 / 2","Repeat each number"].iter().enumerate() {
                let bounds = self.layout.fields[index];
                if bounds.width == 0.0 || bounds.height == 0.0 { continue; }
                text(ops,b.x+16.0,bounds.y+7.0,*label,12.0,theme.text);
                let focused = self.accessibility_focus.unwrap_or(34000 + self.focus as u64) == 34000 + index as u64;
                let current=self.fields[index].draw(renderer,bounds,focused,ops)?; if focused { caret=Some(current); }
            }
        }
        text(ops,b.x+16.0,b.y+b.height-70.0,&self.status,12.0,theme.muted);
        for (id, bounds, label) in [(34020, self.layout.apply, "Apply (Enter)"), (34021, self.layout.cancel, "Cancel (Esc)")] {
            ops.push(DrawOp::Fill(bounds, theme.editor));
            ops.push(DrawOp::Stroke(bounds, if self.accessibility_focus == Some(id) { theme.interactive } else { theme.border }, 1.0));
            text(ops, bounds.x + 8.0, bounds.y + 7.0, label, 13.0, theme.text);
        }
        ops.push(DrawOp::PopClip); Ok(caret)
    }
}
impl Shell {
    pub(super) fn power_dispatch(&mut self, _el:&ActiveEventLoop,id:&str)->bool {
        if self.power_stream_dispatch(id) {if !self.power.status.is_empty(){if let Some(workspace)=self.workspace.as_mut(){workspace.message=Some(self.power.status.clone());}}if let Some(window)=&self.window{window.request_redraw();}return true;}
        if self.power_paged_dispatch(id){return true;}
        if id=="editor.clipboard.toggleHistory" {
            let enabled=!self.settings.controller.effective().clipboard_history_enabled;
            let scope=self.settings.controller.scope;self.settings.controller.scope=bareline_settings::Scope::User;
            let result=self.settings.controller.edit("editor.clipboard.history_enabled",bareline_settings::SettingValue::Bool(enabled));self.settings.controller.scope=scope;
            match result{Ok(())=>self.power.history.set_enabled(enabled),Err(error)=>self.power.status=error}
            if let Some(window)=&self.window{window.request_redraw();}return true;
        }
        let Some(workspace)=self.workspace.as_mut() else { return false; };
        let Some(editor)=self.views.active_editor_mut(workspace,self.app.active) else { return false; };
        match id {
            "editor.column.insert" | "editor.paste.fromHistory" => {
                self.power.global_target=None;
                self.power.open=true; self.power.history_open=id.ends_with("fromHistory"); self.power.selected=0; self.power.target=Some(editor.snapshot().clone()); self.power.status.clear();
                if self.power.rectangle.is_none() {
                    let snapshot=editor.snapshot(); let selection=editor.selection; let first=snapshot.line_at(bareline_document::TextOffset(selection.anchor.min(selection.caret))).unwrap_or(0); let last=snapshot.line_at(bareline_document::TextOffset(selection.anchor.max(selection.caret))).unwrap_or(first);
                    self.power.rectangle=Some(Rectangle { first_line:first,last_line:last,start_column:editor.caret_display_position().map_or(0,|(_,column)|column),end_column:editor.caret_display_position().map_or(0,|(_,column)|column) });
                }
                if self.power.history_open && !self.power.history.enabled() { self.power.status="Enable Clipboard History to retain copied text.".into(); }
            }
            "editor.paste.plainText" => {
                match self.platform.as_ref().unwrap().clipboard_text() { Ok(text)=> { let args=Arguments::from([("text".into(),text)]); if let Err(e)=editor.execute_power_recorded(id,&args) { editor.error=Some(e); } },Err(e)=>editor.error=Some(e.to_string()) }
            }
            "editor.bookmark.copyLines" | "editor.bookmark.cutLines" => {
                match editor.copy_bookmarked_lines() {
                    Ok(text)=>match self.platform.as_ref().unwrap().set_clipboard_text(&text) { Ok(())=> { self.power.copied(&text); if id.ends_with("cutLines") { if let Err(e)=editor.execute_power_recorded(id,&Arguments::new()) { editor.error=Some(e); } } },Err(e)=>editor.error=Some(e.to_string()) },
                    Err(e)=>editor.error=Some(e),
                }
            }
            "editor.lines.hide" | "editor.lines.showAll" | "editor.bookmark.deleteLines" => { if let Err(e)=editor.execute_power_recorded(id,&Arguments::new()) { editor.error=Some(e); } }
            "editor.rectangle.paste" | "editor.rectangle.delete" => {
                if let Some(rectangle)=self.power.rectangle { let mut args=rectangle_arguments(rectangle); if id.ends_with("paste") { match self.platform.as_ref().unwrap().clipboard_text() { Ok(text)=>{args.insert("text".into(),text);},Err(e)=>{editor.error=Some(e.to_string());return true;} } } if let Err(e)=editor.execute_power_recorded(id,&args) {editor.error=Some(e);} }
                else { editor.error=Some("Select a rectangle first with Alt+Shift and the arrow keys.".into()); }
            }
            _=>return false,
        }
        if let Some(window)=&self.window {window.request_redraw();} true
    }
    fn power_apply(&mut self) {
        let id=if self.power.history_open {"editor.paste.fromHistory"}else{"editor.column.insert"};
        let mut args=Arguments::new();
        if self.power.history_open { let Some(text)=self.power.history.entries().nth(self.power.selected) else{return;};args.insert("text".into(),text.into()); }
        else { let Some(rectangle)=self.power.rectangle else{return;};args=rectangle_arguments(rectangle);for (key,field) in ["mode","text","start","step","width","base","repeat"].iter().zip(&self.power.fields) { args.insert((*key).into(),field.value().into()); } }
        if self.power.global_target.is_some(){if self.power_paged_literal(id,args){self.power.open=false;}return;}
        let Some(workspace)=self.workspace.as_mut() else{return;};let Some(editor)=self.views.active_editor_mut(workspace,self.app.active) else{return;};
        if !self.power.target.as_ref().is_some_and(|snapshot| snapshot.same_document(editor.snapshot()) && snapshot.revision==editor.snapshot().revision) {self.power.status="Document changed; reopen this dialog.".into();return;}
        if !self.power.history_open {
            let rectangle=self.power.rectangle.unwrap();editor.clear_column_metrics();
            if let Err(error)=editor.begin_column_measurement(){self.power.status=error;return;}
            self.power.metric_job=Some((editor.snapshot().clone(),args,rectangle.first_line,rectangle.last_line,None));
            self.power.status="Measuring selected rows… Escape cancels.".into();(self.notify)();return;
        }
        match editor.execute_power_recorded(id,&args) {Ok(())=>self.power.open=false,Err(e)=>self.power.status=e}
    }
    pub(super) fn power_action(&mut self,action:Action)->bool {
        if !self.power.open {return self.power_clipboard_action(action);}
        if self.power.history_open {return false;}
        let field=&mut self.power.fields[self.power.focus];
        match action {Action::Paste=>{if let Ok(text)=self.platform.as_ref().unwrap().clipboard_text(){field.commit(&text);}},Action::Copy|Action::Cut=>{if self.platform.as_ref().unwrap().set_clipboard_text(field.selected()).is_ok()&&action==Action::Cut{field.insert("");}},Action::SelectAll=>field.select_all(),Action::Undo=>field.undo(false),Action::Redo=>field.undo(true),_=>return false} true
    }
    pub(super) fn power_event(&mut self,_el:&ActiveEventLoop,event:&WindowEvent)->bool {
        if matches!(event,WindowEvent::KeyboardInput{event,..} if event.state==ElementState::Pressed&&event.logical_key==Key::Named(NamedKey::Escape))&&self.power_stream_cancel(){return true;}
        if self.palette.open{return false;}
        if !self.power.open { return self.power_gesture(event); }
        match event {
            WindowEvent::KeyboardInput{event,..} if event.state==ElementState::Pressed=>match &event.logical_key {
                Key::Named(NamedKey::Escape)=>self.power.open=false,
                Key::Named(NamedKey::Enter)=>{if self.power.accessibility_focus==Some(34021){self.power.open=false;}else{self.power_apply();}},
                Key::Named(NamedKey::Tab)=>{self.power.focus=(self.power.focus+1)%self.power.fields.len();if !self.power.history_open{self.power.accessibility_focus=Some(34000+self.power.focus as u64);}},
                Key::Named(NamedKey::ArrowUp) if self.power.history_open=>{self.power.selected=self.power.selected.saturating_sub(1);self.power.accessibility_focus=Some(34100+self.power.selected as u64);},
                Key::Named(NamedKey::ArrowDown) if self.power.history_open=>{self.power.selected=(self.power.selected+1).min(self.power.history.entries().count().saturating_sub(1));self.power.accessibility_focus=Some(34100+self.power.selected as u64);},
                key if !self.power.history_open=> {let field=&mut self.power.fields[self.power.focus]; match key {Key::Named(NamedKey::Backspace)=>{field.delete(false);},Key::Named(NamedKey::Delete)=>{field.delete(true);},Key::Named(NamedKey::ArrowLeft)=>field.horizontal(false,self.modifiers.shift_key()),Key::Named(NamedKey::ArrowRight)=>field.horizontal(true,self.modifiers.shift_key()),_=>{if !self.modifiers.control_key()&&!self.modifiers.alt_key(){if let Some(text)=&event.text{field.commit(text);}}}}},_=>{}
            },
            WindowEvent::Ime(Ime::Commit(text)) if !self.power.history_open=>{self.power.fields[self.power.focus].commit(text);},
            WindowEvent::MouseInput{state:ElementState::Pressed,button:MouseButton::Left,..}=> {
                if self.power.layout.apply.contains(self.pointer) { self.power.accessibility_focus=Some(34020); self.power_apply(); }
                else if self.power.layout.cancel.contains(self.pointer) { self.power.accessibility_focus=Some(34021); self.power.open=false; }
                else if self.power.history_open {
                    if let Some(index)=self.power.layout.history_hit(self.pointer,self.power.selected,self.power.history.entries().count()) { self.power.selected=index; self.power.accessibility_focus=Some(34100+index as u64); self.power_apply(); }
                } else if let Some(index)=self.power.layout.fields.iter().position(|bounds|bounds.contains(self.pointer)) { self.power.focus=index; self.power.accessibility_focus=Some(34000+index as u64); }
            },
            _=>return false,
        }
        if let Some(window)=&self.window{window.request_redraw();}true
    }
    fn power_gesture(&mut self,event:&WindowEvent)->bool {
        if self.power_pointer(event) { return true; }
        let WindowEvent::KeyboardInput{event,..}=event else{return false;};if event.state!=ElementState::Pressed||!self.modifiers.alt_key()||!self.modifiers.shift_key(){return false;}
        let (dx,dy)=match &event.logical_key {Key::Named(NamedKey::ArrowLeft)=>(-1,0),Key::Named(NamedKey::ArrowRight)=>(1,0),Key::Named(NamedKey::ArrowUp)=>(0,-1),Key::Named(NamedKey::ArrowDown)=>(0,1),_=>return false};
        if self.power_paged_literal("editor.rectangle.extend",[("dx".into(),dx.to_string()),("dy".into(),dy.to_string())].into_iter().collect()){return true;}
        let Some(workspace)=self.workspace.as_mut()else{return false;};let Some(editor)=self.views.active_editor_mut(workspace,self.app.active)else{return false;};
        let (line,column)=editor.caret_display_position().unwrap_or((0,0));
        let mut rectangle=self.power.rectangle.unwrap_or(Rectangle{first_line:line,last_line:line,start_column:column,end_column:column});rectangle.last_line=rectangle.last_line.saturating_add_signed(dy).min(editor.snapshot().line_count().saturating_sub(1));rectangle.end_column=rectangle.end_column.saturating_add_signed(dx);if rectangle.last_line<rectangle.first_line{rectangle.first_line=rectangle.last_line;}
        if let Err(e)=editor.select_rectangle(rectangle){editor.error=Some(e);}else{
            self.power.rectangle=Some(rectangle);
            if let Err(error)=editor.begin_column_measurement(){editor.error=Some(error);}else{self.power.metric_job=Some((editor.snapshot().clone(),rectangle_arguments(rectangle),rectangle.first_line,rectangle.last_line,Some(rectangle)));(self.notify)();}
        }if let Some(window)=&self.window{window.request_redraw();}true
    }
    pub(super) fn power_pump(&mut self)->bool {
        let streaming_changed=self.power_stream_pump();
        if streaming_changed&&!self.power.status.is_empty(){if let Some(workspace)=self.workspace.as_mut(){workspace.message=Some(self.power.status.clone());}}
        if let Some((snapshot,args,mut next,last,selection))=self.power.metric_job.take() {
            let Some(workspace)=self.workspace.as_mut()else{return true;};
            let same=self.views.active_editor(workspace,self.app.active).is_some_and(|editor|snapshot.same_document(editor.snapshot())&&snapshot.revision==editor.snapshot().revision);
            if !same || (selection.is_none()&&!self.power.open) {
                for editor in &mut workspace.editors{if snapshot.same_document(editor.snapshot()){editor.finish_column_measurement();}}
                if let Some(editor)=self.views.secondary.as_mut(){if snapshot.same_document(editor.snapshot()){editor.finish_column_measurement();}}
                self.power.status="Column measurement cancelled.".into();return true;
            }
            let Some(editor)=self.views.active_editor_mut(workspace,self.app.active)else{return true;};let Some(renderer)=self.renderer.as_mut()else{editor.finish_column_measurement();return true;};
            let stop=next.saturating_add(8).min(last.saturating_add(1));
            while next<stop {if let Err(error)=editor.measure_column_row(renderer,next){self.power.status=error;editor.clear_column_metrics();editor.finish_column_measurement();return true;}next+=1;}
            if next<=last{self.power.metric_job=Some((snapshot,args,next,last,selection));(self.notify)();}
            else {
                editor.finish_column_measurement();
                if let Some(rectangle)=selection {if let Err(error)=editor.select_rectangle(rectangle){editor.error=Some(error);}editor.pump();}
                else {match editor.execute_power_recorded("editor.column.insert",&args){Ok(())=>self.power.open=false,Err(error)=>self.power.status=error}}
            }
            if let Some(window)=&self.window{window.request_redraw();}return true;
        }
        let Some((mut group,index))=self.power.group.take() else{return streaming_changed;};
        let Some(workspace)=self.workspace.as_mut() else{self.power.group=Some((group,index));return false;};
        let Some(primary)=workspace.editors.get_mut(index) else{self.power.group=Some((group,index));return false;};
        let Some(secondary)=self.views.secondary.as_mut() else{self.power.group=Some((group,index));return false;};
        match group.pump(&mut [&mut **primary,&mut **secondary]) {Ok(Some(_))=>true,Ok(None)=>{self.power.group=Some((group,index));false},Err(error)=>{primary.error=Some(error);self.power.group=Some((group,index));false}}
    }
    fn power_pointer(&mut self,event:&WindowEvent)->bool {
        let relevant=matches!(event,WindowEvent::MouseInput{button:MouseButton::Left,..}|WindowEvent::CursorMoved{..});
        if !relevant{return false;}
        let point=self.pointer;
        let pane=self.views.bounds.iter().position(|bounds|bounds.is_some_and(|b|b.contains(point))).unwrap_or(self.views.pane() as usize);
        let bounds=self.views.bounds[pane].unwrap_or(self.editor_bounds());
        let local=Point{x:point.x-bounds.x,y:point.y-bounds.y};
        if self.power_paged_pointer(event,pane,local){return true;}
        let Some(workspace)=self.workspace.as_mut()else{return false;};let Some(renderer)=self.renderer.as_ref()else{return false;};
        if workspace.editors.get(self.app.active).is_some_and(|e| e.paged()) || self.views.secondary.as_ref().is_some_and(|e| e.paged()) {
            let power_gesture = self.modifiers.alt_key() || self.modifiers.control_key() || self.power.drag.is_some() || self.power.rectangle_drag.is_some();
            if power_gesture { workspace.message=Some("Power pointer editing is unavailable for paged views.".into()); }
            return power_gesture && matches!(event,WindowEvent::MouseInput{..});
        }
        let editor=if pane==1{self.views.secondary.as_mut().map(|e|&mut **e)}else{workspace.editors.get_mut(self.app.active).map(|e|&mut **e)};
        let Some(editor)=editor else{return false;};let Some((offset,line,column))=editor.power_hit_position(renderer,local)else{return false;};
        match event {
            WindowEvent::MouseInput{state:ElementState::Pressed,..} if self.modifiers.alt_key()=>{
                self.power.rectangle_drag=Some((line,column));self.power.rectangle=Some(Rectangle{first_line:line,last_line:line,start_column:column,end_column:column});
            }
            WindowEvent::MouseInput{state:ElementState::Pressed,..} if self.modifiers.control_key()=>{if let Err(e)=editor.toggle_power_caret(offset){editor.error=Some(e);}}
            WindowEvent::MouseInput{state:ElementState::Pressed,..} if (editor.selection.anchor.min(editor.selection.caret)..editor.selection.anchor.max(editor.selection.caret)).contains(&offset)=>{self.power.drag=Some((self.app.active,pane as u32,editor.snapshot().clone(),editor.selection));}
            WindowEvent::CursorMoved{..} if self.power.rectangle_drag.is_some()=>{
                let (anchor_line,anchor_column)=self.power.rectangle_drag.unwrap();let rectangle=Rectangle{first_line:anchor_line.min(line),last_line:anchor_line.max(line),start_column:anchor_column,end_column:column};
                if let Err(e)=editor.select_rectangle(rectangle){editor.error=Some(e);}else{
            self.power.rectangle=Some(rectangle);
            if let Err(error)=editor.begin_column_measurement(){editor.error=Some(error);}else{self.power.metric_job=Some((editor.snapshot().clone(),rectangle_arguments(rectangle),rectangle.first_line,rectangle.last_line,Some(rectangle)));(self.notify)();}
        }
            }
            WindowEvent::MouseInput{state:ElementState::Released,..} if self.power.rectangle_drag.take().is_some()=>{},
            WindowEvent::MouseInput{state:ElementState::Released,..} if self.power.drag.is_some()=>{
                let (index,source_pane,snapshot,selection)=self.power.drag.take().unwrap();
                if source_pane==pane as u32 {
                    if snapshot.same_document(editor.snapshot())&&snapshot.revision==editor.snapshot().revision {
                        match power::drag_text(editor.snapshot(),selection,offset,self.modifiers.control_key(),power::Limits::default()) {Ok(edit)=>{if let Err(e)=editor.apply_power(edit){editor.error=Some(e);}},Err(e)=>editor.error=Some(format!("{e:?}"))}
                    }
                } else {
                    let (scheduler,editors)=workspace.scheduler_and_editors();
                    let Some(primary)=editors.get_mut(index)else{return true;};let Some(secondary)=self.views.secondary.as_mut()else{return true;};
                    let (source,target)=if source_pane==0{(&mut **primary,&mut **secondary)}else{(&mut **secondary,&mut **primary)};
                    if !snapshot.same_document(source.snapshot())||snapshot.revision!=source.snapshot().revision{source.error=Some("Drag source changed; select the text again.".into());return true;}
                    source.selection=selection;
                    match power::consumer::drag_between(scheduler,source,target,offset,self.modifiers.control_key()){Ok(Some(group))=>self.power.group=Some((group,index)),Ok(None)=>{},Err(e)=>source.error=Some(e)}
                }
            }
            _=>return false,
        }
        if let Some(window)=&self.window{window.request_redraw();}true
    }
}
fn rectangle_arguments(r:Rectangle)->Arguments {[("first_line",r.first_line),("last_line",r.last_line),("start_column",r.start_column),("end_column",r.end_column)].into_iter().map(|(k,v)|(k.into(),v.to_string())).collect()}
impl PowerRuntime {
    pub(super) fn accessibility_nodes(&self)->Vec<bareline_platform::accessibility::AccessibilityNode> {
        use bareline_platform::accessibility::{AccessibilityNode,AccessibilityRole};
        if !self.open{return Vec::new();}
        let mut nodes=Vec::new();
        if !self.history_open {
            for ((index,label),bounds) in ["Mode: text or numbers","Repeated text","Initial number","Increment","Zero-padding width","Base","Repeat count"].iter().enumerate().zip(&self.layout.fields) {
                if bounds.width==0.0||bounds.height==0.0{continue;}
                nodes.push(AccessibilityNode{id:34000+index as u64,parent:1,role:AccessibilityRole::TextField,name:(*label).into(),value:Some(self.fields[index].value().into()),bounds:accessible_bounds(*bounds),disabled:false,selected:self.accessibility_focus.unwrap_or(34000+self.focus as u64)==34000+index as u64,expanded:None,focusable:true,invokable:false});
            }
        } else {
            let start=self.layout.history_start(self.selected);
            for ((index,entry),bounds) in self.history.entries().enumerate().skip(start).zip(&self.layout.history_rows) {
                nodes.push(AccessibilityNode{id:34100+index as u64,parent:1,role:AccessibilityRole::ListItem,name:entry.chars().take(80).collect(),value:None,bounds:accessible_bounds(*bounds),disabled:false,selected:self.accessibility_focus.unwrap_or(34100+self.selected as u64)==34100+index as u64,expanded:None,focusable:true,invokable:true});
            }
        }
        for (id,name,bounds) in [(34020,"Apply",self.layout.apply),(34021,"Cancel",self.layout.cancel)] {
            if bounds.width==0.0||bounds.height==0.0{continue;}
            nodes.push(AccessibilityNode{id,parent:1,role:AccessibilityRole::Button,name:name.into(),value:None,bounds:accessible_bounds(bounds),disabled:false,selected:self.accessibility_focus==Some(id),expanded:None,focusable:true,invokable:true});
        }
        nodes
    }
}
impl Shell {
    pub(super) fn power_accessibility(&mut self,action:&bareline_platform::accessibility::AccessibilityAction)->bool {
        use bareline_platform::accessibility::AccessibilityAction;
        if !self.power.open||self.palette.open{return false;}
        match action {
            AccessibilityAction::Focus(id) if (34000..34007).contains(id)=>{self.power.focus=(*id-34000)as usize;self.power.accessibility_focus=Some(*id);},
            AccessibilityAction::Focus(id) if *id==34020||*id==34021=>self.power.accessibility_focus=Some(*id),
            AccessibilityAction::Focus(id) if (34100..34120).contains(id)&&self.power.history_open=>{self.power.selected=(*id-34100)as usize;self.power.accessibility_focus=Some(*id);},
            AccessibilityAction::SetValue{id,value} if (34000..34007).contains(id)=>{let field=&mut self.power.fields[(*id-34000)as usize];field.select_all();field.insert(value);},
            AccessibilityAction::Invoke(34020)=>self.power_apply(),
            AccessibilityAction::Invoke(34021)=>self.power.open=false,
            AccessibilityAction::Invoke(id) if (34100..34120).contains(id)=>{self.power.selected=(*id-34100)as usize;self.power_apply();},
            _=>return false,
        }
        if let Some(window)=&self.window{window.request_redraw();}true
    }
}

impl Shell {
    /// One atomic clipboard read/write per editor action, including optional rectangle metadata.
    pub(super) fn power_clipboard_action(&mut self,action:Action)->bool {
        if !matches!(action,Action::Copy|Action::Cut|Action::Paste)||self.palette.open{return false;}
        if action==Action::Paste&&self.power_paged_paste(){return true;}
        if matches!(action,Action::Copy|Action::Cut)&&self.power_global_clipboard(action==Action::Cut){if !self.power.status.is_empty(){if let Some(workspace)=self.workspace.as_mut(){workspace.message=Some(self.power.status.clone());}}return true;}
        let Some(workspace)=self.workspace.as_mut()else{return false;};let Some(platform)=self.platform.as_ref()else{return false;};
        let secondary=self.views.pane()==1;
        let editor=if secondary {self.views.secondary.as_ref()} else {workspace.editors.get(self.app.active)};
        if matches!(editor,Some(bareline_app::workspace::WorkspaceEditor::Paged(paged)) if !paged.selection_fully_in_viewport()) {
            workspace.message=Some("Reveal the complete selection before using clipboard commands.".into());
            return true;
        }
        if action==Action::Paste {
            match platform.clipboard_text_with_metadata(power::consumer::RectangleClipboardMetadata::FORMAT,262_144) {
                Ok(contents)=>{
                    let _metadata=contents.metadata.as_deref().and_then(|bytes|power::consumer::RectangleClipboardMetadata::decode(bytes,&contents.text));
                    if secondary {if let Some(editor)=self.views.secondary.as_mut(){editor.enqueue_with_origin(Input::Insert(contents.text),bareline_document::history::EditOrigin::Paste);}}
                    else if let Some(editor)=workspace.editors.get_mut(self.app.active){editor.commit_with_origin(contents.text,bareline_document::history::EditOrigin::Paste);}
                }
                Err(error)=>workspace.message=Some(error.to_string()),
            }
        } else {
            let copied=if secondary {self.views.secondary.as_ref().map(|editor|editor.selected_text().map(|text|{let metadata=editor.rectangle_clipboard_metadata(&text).ok().flatten();(text,metadata)}))}
            else {workspace.editors.get(self.app.active).map(|editor|editor.selected_text().map(|text|{let metadata=editor.rectangle_clipboard_metadata(&text).ok().flatten();(text,metadata)}))};
            match copied {
                Some(Ok((text,metadata))) if !text.is_empty()=>{
                    let result=if let Some(metadata)=metadata {platform.set_clipboard_text_with_metadata(&text,power::consumer::RectangleClipboardMetadata::FORMAT,&metadata)}else{platform.set_clipboard_text(&text)};
                    match result {Ok(())=>{self.power.copied(&text);if action==Action::Cut {if secondary{if let Some(editor)=self.views.secondary.as_mut(){editor.enqueue_with_origin(Input::Insert(String::new()),bareline_document::history::EditOrigin::Command);}}else if let Some(editor)=workspace.editors.get_mut(self.app.active){editor.enqueue_with_origin(Input::Insert(String::new()),bareline_document::history::EditOrigin::Command);}}},Err(error)=>workspace.message=Some(error.to_string())}
                }
                Some(Err(error))=>workspace.message=Some(error.into()),
                _=>{},
            }
        }
        if let Some(window)=&self.window{window.request_redraw();}true
    }
}

#[cfg(test)]
pub(super) fn accessibility_test_cases() -> Vec<(
    &'static str,
    Vec<bareline_platform::accessibility::AccessibilityNode>,
    Option<u64>,
)> {
    // Match the runtime's 1000x800 popover placement; no native window is created.
    let width = 1000.0_f32;
    let height = 800.0_f32;
    let mut runtime = PowerRuntime::default();
    runtime.layout = PowerLayout::new(width, height);
    let mut cases = vec![("power.closed", runtime.accessibility_nodes(), None)];
    runtime.open = true;
    cases.push(("power.column", runtime.accessibility_nodes(), Some(34000)));
    runtime.focus = 2;
    runtime.accessibility_focus = Some(34002);
    cases.push(("power.column_focus", runtime.accessibility_nodes(), Some(34002)));
    runtime.fields[2].select_all();
    runtime.fields[2].insert("-24");
    cases.push(("power.column_value", runtime.accessibility_nodes(), Some(34002)));
    runtime.accessibility_focus = Some(34020);
    cases.push(("power.column_apply_focus", runtime.accessibility_nodes(), Some(34020)));
    runtime.accessibility_focus = Some(34021);
    cases.push(("power.column_cancel_focus", runtime.accessibility_nodes(), Some(34021)));
    runtime.history_open = true;
    runtime.accessibility_focus = None;
    cases.push(("power.history_disabled", runtime.accessibility_nodes(), None));
    runtime.configure_history(true, 20, 16 << 20, 4 << 20);
    runtime.copied("First copied row");
    runtime.copied("界 and emoji 🦀\nsecond row");
    runtime.selected = 0;
    cases.push(("power.history", runtime.accessibility_nodes(), Some(34100)));
    runtime.selected = 1;
    runtime.accessibility_focus = Some(34101);
    cases.push(("power.history_focus", runtime.accessibility_nodes(), Some(34101)));
    runtime.accessibility_focus = Some(34021);
    cases.push(("power.history_cancel_focus", runtime.accessibility_nodes(), Some(34021)));
    for index in 0..20 { runtime.copied(&format!("History entry {index}")); }
    runtime.selected = 14;
    runtime.accessibility_focus = Some(34114);
    cases.push(("power.history_scrolled", runtime.accessibility_nodes(), Some(34114)));
    runtime.configure_history(false, 20, 16 << 20, 4 << 20);
    runtime.accessibility_focus = None;
    cases.push(("power.history_cleared", runtime.accessibility_nodes(), None));
    cases
}

#[cfg(test)]
mod power_layout_tests {
    use super::*;

    #[test]
    fn scrolled_history_hits_and_semantics_follow_visible_rows() {
        let mut runtime=PowerRuntime::default();
        runtime.open=true;runtime.history_open=true;
        runtime.layout=PowerLayout::new(1000.0,800.0);
        runtime.configure_history(true,20,16<<20,4<<20);
        for index in 0..20{runtime.copied(&format!("Entry {index}"));}
        runtime.selected=14;
        let nodes=runtime.accessibility_nodes();
        let rows:Vec<_>=nodes.iter().filter(|node|(34100..34120).contains(&node.id)).collect();
        assert_eq!(rows.len(),10);
        assert_eq!(rows.first().unwrap().id,34105);
        assert_eq!(rows.last().unwrap().id,34114);
        assert_eq!(rows[0].bounds,[290.0,244.0,420.0,27.0]);
        assert_eq!(rows[1].bounds,[290.0,271.0,420.0,27.0]);
        for node in rows{
            let point=Point{x:node.bounds[0]as f32+1.0,y:node.bounds[1]as f32+1.0};
            assert_eq!(runtime.layout.history_hit(point,runtime.selected,20),Some((node.id-34100)as usize));
        }
        assert_eq!(runtime.layout.history_hit(Point{x:289.0,y:245.0},14,20),None);
        assert_eq!(runtime.layout.history_hit(Point{x:300.0,y:570.0},14,20),None);
        assert_eq!(runtime.layout.history_hit(Point{x:291.0,y:245.0},0,0),None);
    }

    #[test]
    fn field_and_footer_bounds_match_the_drawn_controls() {
        let mut runtime=PowerRuntime::default();runtime.open=true;
        runtime.layout=PowerLayout::new(1000.0,800.0);
        let nodes=runtime.accessibility_nodes();
        for (index,bounds) in runtime.layout.fields.iter().enumerate(){
            let node=nodes.iter().find(|node|node.id==34000+index as u64).unwrap();
            assert_eq!(node.bounds,accessible_bounds(*bounds));
            assert!(bounds.y+bounds.height<=runtime.layout.apply.y);
        }
        let apply=nodes.iter().find(|node|node.id==34020).unwrap();
        let cancel=nodes.iter().find(|node|node.id==34021).unwrap();
        assert_eq!(apply.bounds,[296.0,558.0,120.0,30.0]);
        assert_eq!(cancel.bounds,[428.0,558.0,120.0,30.0]);
        let apply_point=Point{x:300.0,y:570.0};let cancel_point=Point{x:432.0,y:570.0};
        assert!(runtime.layout.apply.contains(apply_point));
        assert!(!runtime.layout.cancel.contains(apply_point));
        assert!(runtime.layout.cancel.contains(cancel_point));
        assert!(!runtime.layout.apply.contains(cancel_point));
    }
}
