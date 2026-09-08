// SPDX-License-Identifier: MPL-2.0
//! Ordinary text drag captures source coordinates; transfer publication stays in the actor coordinator.
use super::*;
use bareline_app::workspace::{Workspace,WorkspaceEditor};
use bareline_document::TextOffset;
use bareline_editor_surface::paged_view::{SourceAffinity,transfer::{PagedTransfer,PagedTransferCapture}};
use bareline_editor_surface::power::captured::StagingOptions;
use bareline_file_io::cancellation::Cancellation;
#[derive(Clone)]
struct Endpoint {pane:usize,tab:u64,identity:(u64,u64)}
struct Capture {source:Endpoint,ranges:Vec<std::ops::Range<TextOffset>>,down:Point,moved:bool}
struct DropRequest {source:Endpoint,destination:Endpoint,ranges:Vec<std::ops::Range<TextOffset>>,at:TextOffset,copy:bool}
struct Operation {request:DropRequest,transfer:PagedTransfer,cancelled:bool}
#[derive(Default)]
pub(super) struct Runtime {capture:Option<Capture>,pending:Option<DropRequest>,operation:Option<Operation>}
impl Runtime {pub(super) fn active(&self)->bool{self.capture.is_some()}}
fn editor<'a>(workspace:&'a Workspace,views:&'a super::super::views::ViewsRuntime,pane:usize)->Option<&'a WorkspaceEditor>{
    if pane==1 {views.secondary.as_ref()} else {workspace.editors.get(views.primary_index(workspace)?)}
}
fn identity(editor:&WorkspaceEditor)->(u64,u64){bareline_app::accessibility::source_identity(editor)}
fn current(workspace:&Workspace,views:&super::super::views::ViewsRuntime,target:&Endpoint)->bool {
    views.pane_token(target.pane)==Some(target.tab)&&editor(workspace,views,target.pane).is_some_and(|editor|identity(editor)==target.identity)
}
fn hit(editor:&WorkspaceEditor,renderer:&impl bareline_renderer::TextBackend,point:Point)->Option<TextOffset>{
    if editor.busy(){return None;}
    if matches!(editor,WorkspaceEditor::Paged(paged) if !paged.paged_frame_state().ready){return None;}
    let (offset,_,_)=editor.power_hit_position(renderer,point)?;
    match editor {WorkspaceEditor::Paged(paged)=>paged.source_offset(TextOffset(offset),SourceAffinity::After),_=>Some(TextOffset(offset))}
}
fn ranges(editor:&WorkspaceEditor)->Vec<std::ops::Range<TextOffset>>{
    let selections=match editor{WorkspaceEditor::Paged(paged)=>paged.global_selection_set(),_=>editor.selection_set()};
    selections.selections.iter().map(|selection|TextOffset(selection.anchor.min(selection.caret))..TextOffset(selection.anchor.max(selection.caret))).filter(|range|range.start!=range.end).collect()
}
impl Shell {
    pub(super) fn power_drag_cancel(&mut self)->bool {
        let capture=self.power.drag_runtime.capture.take().is_some();
        let pending=self.power.drag_runtime.pending.take().is_some();
        let operation=self.power.drag_runtime.operation.as_mut();
        let active=operation.is_some();
        if let Some(operation)=operation {operation.transfer.cancel();operation.cancelled=true;}
        if capture||pending||active {self.power.status="Cancelling text transfer…".into();(self.notify)();}
        capture||pending||active
    }
    pub(super) fn power_drag_outside(&mut self,event:&WindowEvent)->bool{
        if matches!(event,WindowEvent::MouseInput{state:ElementState::Released,..}|WindowEvent::Focused(false)){
            return self.power.drag_runtime.capture.take().is_some();
        }
        self.power.drag_runtime.active()
    }
    pub(super) fn power_drag_pointer(&mut self,event:&WindowEvent,pane:usize,point:Point)->bool{
        if self.power.drag_runtime.pending.is_some()||self.power.drag_runtime.operation.is_some(){return true;}
        if let WindowEvent::CursorMoved{..}=event {
            if let Some(capture)=&mut self.power.drag_runtime.capture {
                let dx=self.pointer.x-capture.down.x;let dy=self.pointer.y-capture.down.y;
                capture.moved|=dx*dx+dy*dy>=16.0;return true;
            }
            return false;
        }
        let Some(workspace)=self.workspace.as_ref()else{return false;};
        let Some(renderer)=self.renderer.as_ref()else{return false;};
        let Some(target)=editor(workspace,&self.views,pane)else{return false;};
        let target_hit=hit(target,renderer,point);
        match event {
            WindowEvent::MouseInput{state:ElementState::Pressed,button:MouseButton::Left,..}=>{
                if self.modifiers.alt_key()||self.modifiers.control_key(){return false;}
                // Resident-only drags retain their existing path; any paged pane
                // needs source-owned staging, including a resident source pane.
                if !workspace.editors.iter().any(WorkspaceEditor::paged)&&!self.views.secondary.as_ref().is_some_and(WorkspaceEditor::paged){return false;}
                let Some(at)=target_hit else{return false;};let ranges=ranges(target);
                if !ranges.iter().any(|range|range.start<=at&&at<range.end){return false;}
                let Some(tab)=self.views.pane_token(pane)else{return false;};
                self.power.drag_runtime.capture=Some(Capture{source:Endpoint{pane,tab,identity:identity(target)},ranges,down:self.pointer,moved:false});true
            }
            WindowEvent::MouseInput{state:ElementState::Released,button:MouseButton::Left,..}=>{
                let Some(capture)=self.power.drag_runtime.capture.take()else{return false;};
                if !current(workspace,&self.views,&capture.source)||editor(workspace,&self.views,capture.source.pane).is_none_or(|source|ranges(source)!=capture.ranges){self.power.status="Drag source changed; select the text again.".into();return true;}
                let Some(at)=target_hit else{self.power.status="Drop cancelled: destination viewport is not ready.".into();return true;};
                if !capture.moved {
                    let index=self.views.primary_index(workspace);
                    let target=if pane==1 {self.views.secondary.as_mut()} else {index.and_then(|index|self.workspace.as_mut()?.editors.get_mut(index))};
                    if let Some(target)=target {
                        match target {
                            WorkspaceEditor::Paged(paged)=>{if let Err(error)=paged.restore_global_selection(at,at,false){paged.error=Some(error);}},
                            target=>target.enqueue(Input::SetCaret(at.0,false)),
                        }
                    }
                    return true;
                }
                let Some(tab)=self.views.pane_token(pane)else{return true;};
                self.power.drag_runtime.pending=Some(DropRequest{source:capture.source,destination:Endpoint{pane,tab,identity:identity(target)},ranges:capture.ranges,at,copy:self.modifiers.control_key()});
                self.power.status="Preparing text transfer…".into();(self.notify)();true
            }
            _=>false,
        }
    }
    pub(super) fn power_drag_pump(&mut self)->bool{
        let Some(workspace)=self.workspace.as_mut()else{return false;};
        self.views.pump(workspace);
        if let Some(mut operation)=self.power.drag_runtime.operation.take(){
            if !operation.cancelled&&(!current(workspace,&self.views,&operation.request.source)||!current(workspace,&self.views,&operation.request.destination)){
                operation.transfer.cancel();operation.cancelled=true;
            }
            let mut views:Vec<_>=workspace.editors.iter_mut().filter_map(|editor|match editor{WorkspaceEditor::Paged(paged)=>Some(paged),_=>None}).collect();
            if let Some(WorkspaceEditor::Paged(peer))=&mut self.views.secondary{views.push(peer);}
            match operation.transfer.pump(&mut views){
                Some(Ok(()))=>{self.power.status="Text transfer complete.".into();workspace.message=Some(self.power.status.clone());},
                Some(Err(error))=>{self.power.status=error;workspace.message=Some(self.power.status.clone());},
                None=>{self.power.drag_runtime.operation=Some(operation);return false;}
            }
            return true;
        }
        let Some(request)=self.power.drag_runtime.pending.take()else{return false;};
        if !current(workspace,&self.views,&request.source)||!current(workspace,&self.views,&request.destination){self.power.status="Text transfer cancelled because a captured view changed.".into();workspace.message=Some(self.power.status.clone());return true;}
        if let Some(source)=editor(workspace,&self.views,request.source.pane) {
            if !source.busy()&&ranges(source)!=request.ranges {self.power.status="Text transfer selection changed.".into();workspace.message=Some(self.power.status.clone());return true;}
        }
        for endpoint in [&request.source,&request.destination]{
            let Some(index)=workspace.editors.iter().position(|editor|identity(editor)==endpoint.identity)else{self.power.status="Text transfer source tab closed.".into();return true;};
            if !workspace.editors[index].paged(){
                match workspace.promote_resident_for_source_edit(index,endpoint.identity){
                    Ok(_)=>{self.power.drag_runtime.pending=Some(request);(self.notify)();return true;},
                    Err(error)=>{self.power.status=error;workspace.message=Some(self.power.status.clone());return true;}
                }
            }
        }
        let Some(WorkspaceEditor::Paged(source))=editor(workspace,&self.views,request.source.pane)else{self.power.drag_runtime.pending=Some(request);return false;};
        let Some(WorkspaceEditor::Paged(destination))=editor(workspace,&self.views,request.destination.pane)else{self.power.drag_runtime.pending=Some(request);return false;};
        if source.busy()||destination.busy(){self.power.drag_runtime.pending=Some(request);return false;}
        let captured=PagedTransferCapture{source:source.read_handle(),ranges:request.ranges.clone(),destination:destination.read_handle(),at:request.at,copy:request.copy,source_selections:source.global_selection_set(),destination_selections:destination.global_selection_set()};
        let options=StagingOptions{cache:std::env::temp_dir().join("Bareline-drag-staging"),quota:workspace.transcode_quota_bytes,platform:std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),source_options:bareline_file_io::source::SourceOptions::default(),budget:workspace.source_edit_budget(),memory:16<<20,cancellation:Cancellation::default()};
        match PagedTransfer::start(captured,options){Ok(transfer)=>self.power.drag_runtime.operation=Some(Operation{request,transfer,cancelled:false}),Err(error)=>{self.power.status=error;workspace.message=Some(self.power.status.clone());}}
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paged_drag_capture_keeps_global_selection_outside_viewport() {
        let path=std::env::temp_dir().join(format!("bareline-drag-capture-{}.txt",std::process::id()));
        std::fs::write(&path,"line with drag text\n".repeat(20_000)).unwrap();
        let mut workspace=Workspace::new(std::sync::Arc::new(||{}),std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem)).unwrap();
        workspace.resident_max_bytes=1;workspace.open(path.clone());
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(30);
        loop {assert!(std::time::Instant::now()<deadline);workspace.pump();if workspace.editors.first().is_some_and(|editor|matches!(editor,WorkspaceEditor::Paged(paged) if paged.viewport_ready())){break;}std::thread::yield_now();}
        let WorkspaceEditor::Paged(paged)=&mut workspace.editors[0] else {unreachable!()};
        let token=paged.restore_global_selection(TextOffset(2),TextOffset(9),true).unwrap();
        while paged.busy(){assert!(std::time::Instant::now()<deadline);paged.pump();std::thread::yield_now();}
        assert!(matches!(paged.selection_restore_status(token),bareline_editor_surface::paged_view::SelectionRestoreStatus::Applied));
        paged.request_viewport(TextOffset(200_000)).unwrap();
        while paged.busy(){assert!(std::time::Instant::now()<deadline);paged.pump();std::thread::yield_now();}
        assert!(!paged.selection_fully_in_viewport());
        assert_eq!(ranges(&workspace.editors[0]),vec![TextOffset(2)..TextOffset(9)]);
        drop(workspace);let _=std::fs::remove_file(path);
    }
}
