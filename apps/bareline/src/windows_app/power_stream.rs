// SPDX-License-Identifier: MPL-2.0
//! Native admission/completion only; all source reading and staging runs off-thread.
use super::*;
use bareline_app::workspace::WorkspaceEditor;
use bareline_document::{TextOffset, paged::{PagedSnapshot,PreparedSourceTransaction},history::{EditMetadata,EditOrigin}};
use bareline_editor_surface::{TrackedEditReceipt,power::captured::{self,StagingOptions}};
use bareline_file_io::cancellation::Cancellation;
use bareline_app::macros::{PowerReplayRequest,PowerReplayCompletion,PowerReplayTarget};
use std::sync::mpsc::{Receiver,TryRecvError};

#[derive(Clone)]
enum Operation {Transform(String),Clipboard(bool)}
#[derive(Clone)]
struct Target {index:usize,secondary:bool,source:PagedSnapshot,selection:(TextOffset,TextOffset)}
struct Promotion {index:usize,secondary:bool,identity:(u64,u64),id:String,selections:power::SelectionSet}
enum Output {Prepared(PreparedSourceTransaction),Clipboard(String)}
struct Worker {target:Target,operation:Operation,cancel:Cancellation,result:Receiver<Result<Output,String>>}
struct Replay {id:u64,index:usize,document:u64,cancelled:bool,resident:Option<TrackedEditReceipt>,terminal:Option<Result<(),String>>}
impl Replay {fn complete_once(&mut self,result:Result<(),String>){if self.terminal.is_none(){self.terminal=Some(result);}}}
impl PowerRuntime {
    fn stream_failed(&mut self,error:String){if let Some(replay)=self.stream.replay.as_mut(){replay.complete_once(Err(error.clone()));}self.status=error;}
}
impl Drop for Worker {fn drop(&mut self){self.cancel.cancel();}}
#[derive(Default)]
pub(super) struct StreamRuntime {worker:Option<Worker>,promotion:Option<Promotion>,receipt:Option<(Target,Operation,TrackedEditReceipt)>,replay:Option<Replay>}

impl Shell {
    pub(super) fn power_stream_dispatch(&mut self,id:&str)->bool {
        if power::transform_for_command(id).is_none(){return false;}
        if self.palette.open{return false;}
        if self.power.stream.worker.is_some()||self.power.stream.promotion.is_some()||self.power.stream.receipt.is_some()||self.power.stream.replay.is_some(){self.power.status="A power edit is already pending.".into();return true;}
        self.power.status.clear();
        let secondary=self.views.pane()==1;
        let Some(workspace)=self.workspace.as_mut()else{return false;};
        let selected=if secondary{self.views.secondary.as_mut()}else{workspace.editors.get_mut(self.app.active)};
        let Some(editor)=selected else{return false;};
        if editor.busy()||editor.read_only(){workspace.message=Some("Document is busy or read-only.".into());return true;}
        if let WorkspaceEditor::Resident(resident)=editor {
            match resident.execute_power_recorded(id,&Arguments::new()) {
                Ok(())=>return true,
                Err(error) if error.contains("BudgetExceeded")=>{},
                Err(error)=>{resident.error=Some(error);return true;}
            }
            let identity=resident.snapshot().identity_token();let selections=resident.selection_set();
            let index=workspace.editors.iter().position(|editor|matches!(editor,WorkspaceEditor::Resident(view) if view.snapshot().identity_token()==identity));
            let Some(index)=index else{workspace.message=Some("The source tab must remain open for staging.".into());return true;};
            self.power.stream.promotion=Some(Promotion{index,secondary,identity,id:id.into(),selections});
            (self.notify)();return true;
        }
        let index=self.app.active;
        if let Err(error)=self.start_power_worker(index,secondary,Operation::Transform(id.into()),None){self.power.status=error;}
        true
    }
    pub(super) fn power_global_clipboard(&mut self,cut:bool)->bool {
        let secondary=self.views.pane()==1;let Some(workspace)=self.workspace.as_ref()else{return false;};
        let editor=if secondary{self.views.secondary.as_ref()}else{workspace.editors.get(self.app.active)};
        if !matches!(editor,Some(WorkspaceEditor::Paged(paged)) if !paged.selection_fully_in_viewport()){return false;}
        if self.power.stream.worker.is_some()||self.power.stream.promotion.is_some()||self.power.stream.receipt.is_some()||self.power.stream.replay.is_some(){self.power.status="A power edit is already pending.".into();return true;}
        if let Err(error)=self.start_power_worker(self.app.active,secondary,Operation::Clipboard(cut),None){self.power.status=error;}
        true
    }
    fn start_power_worker(&mut self,index:usize,secondary:bool,operation:Operation,selections:Option<power::SelectionSet>)->Result<(),String> {
        let workspace=self.workspace.as_ref().ok_or("Workspace closed")?;
        let editor=if secondary{self.views.secondary.as_ref()}else{workspace.editors.get(index)};
        let Some(WorkspaceEditor::Paged(paged))=editor else{return Err("Paged source is unavailable".into());};
        if paged.busy()||paged.surface.user_read_only&&!matches!(operation,Operation::Clipboard(false)){return Err("Document is busy or read-only".into());}
        let selection=paged.global_selection();let captured=paged.read_handle();
        let target=Target{index,secondary,source:paged.snapshot().clone(),selection};
        let ranges=selections.as_ref().map(|set|set.selections.iter().map(|selection|TextOffset(selection.anchor.min(selection.caret))..TextOffset(selection.anchor.max(selection.caret))).collect::<Vec<_>>()).unwrap_or_else(||vec![selection.0.min(selection.1)..selection.0.max(selection.1)]);
        if matches!(operation,Operation::Clipboard(_))&&ranges.iter().all(|range|range.is_empty()){return Ok(());}
        let cancel=Cancellation::default();
        let options=StagingOptions{cache:std::env::temp_dir().join("Bareline-power-staging"),quota:workspace.transcode_quota_bytes,platform:std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),source_options:bareline_file_io::source::SourceOptions::default(),budget:workspace.source_edit_budget(),memory:16<<20,cancellation:cancel.clone()};
        let before=selections.map(|set|set.selections.into_iter().map(|selection|bareline_document::history::Selection{anchor:TextOffset(selection.anchor),caret:TextOffset(selection.caret)}).collect()).unwrap_or_else(||vec![bareline_document::history::Selection{anchor:selection.0,caret:selection.1}]);
        let metadata=EditMetadata{before,origin:EditOrigin::Command,boundary:power::consumer::next_receipt_sequence(),..Default::default()};
        let tab_width=paged.surface.configured_tab_width();let work=operation.clone();let notify=self.notify.clone();let (send,result)=std::sync::mpsc::sync_channel(1);
        std::thread::Builder::new().name("power-staging".into()).spawn(move||{
            let outcome=match work {
                Operation::Transform(id)=>captured::prepare_transform(captured,&ranges,power::transform_for_command(&id).expect("admitted transform"),tab_width,metadata,&options).map(Output::Prepared),
                Operation::Clipboard(_)=>captured::clipboard_text(captured,&ranges,4<<20,options.budget,options.cancellation).map(Output::Clipboard),
            }.map_err(|error|error.to_string());
            let _=send.send(outcome);notify();
        }).map_err(|error|error.to_string())?;
        self.power.stream.worker=Some(Worker{target,operation,cancel,result});
        self.power.status="Preparing selected text…".into();Ok(())
    }
    pub(super) fn power_stream_pump(&mut self)->bool {
        if let Some(promotion)=self.power.stream.promotion.take(){
            let Some(workspace)=self.workspace.as_mut()else{self.power.stream_failed("Workspace closed during preparation".into());return true;};
            let current=if promotion.secondary{self.views.secondary.as_ref()}else{workspace.editors.get(promotion.index)};
            if matches!(current,Some(WorkspaceEditor::Resident(view)) if view.selection_set()!=promotion.selections){self.power.stream_failed("Selection changed during staging promotion.".into());return true;}
            match workspace.promote_resident_for_source_edit(promotion.index,promotion.identity){
                Ok(false)=>{self.power.stream.promotion=Some(promotion);return false;},
                Err(error)=>{self.power.stream_failed(error);return true;},
                Ok(true)=>{
                    if promotion.secondary {
                        let current=self.views.secondary.as_ref();
                        if !matches!(current,Some(WorkspaceEditor::Paged(view)) if view.snapshot().identity_token()==promotion.identity){
                            if !matches!(current,Some(WorkspaceEditor::Resident(view)) if view.snapshot().identity_token()==promotion.identity&&view.selection_set()==promotion.selections){self.power.stream_failed("Selection changed during staging promotion.".into());return true;}
                            let Some(WorkspaceEditor::Paged(source))=workspace.editors.get(promotion.index)else{self.power.stream_failed("Promoted source unavailable".into());return true;};
                            match source.clone_view(){Ok(mut clone)=>{if let Some(current)=current{current.copy_presentation_to(&mut clone.surface);}self.views.secondary=Some(WorkspaceEditor::Paged(clone));},Err(error)=>{self.power.stream_failed(error);return true;}}
                        }
                    }
                    let target=if promotion.secondary{self.views.secondary.as_ref()}else{workspace.editors.get(promotion.index)};
                    if target.is_some_and(|editor|editor.busy()){self.power.stream.promotion=Some(promotion);return false;}
                    let primary=promotion.selections.primary();
                    if matches!(target,Some(WorkspaceEditor::Paged(view)) if view.global_selection()!=(TextOffset(primary.anchor),TextOffset(primary.caret))){self.power.stream_failed("Selection changed during staging promotion.".into());return true;}
                    if let Err(error)=self.start_power_worker(promotion.index,promotion.secondary,Operation::Transform(promotion.id),Some(promotion.selections)){self.power.stream_failed(error);}
                    return true;
                }
            }
        }
        if let Some((target,operation,receipt))=self.power.stream.receipt.take(){
            match receipt.terminal(){
                None=>{self.power.stream.receipt=Some((target,operation,receipt));},
                Some(Err(error))=>{self.power.stream_failed(error);return true;},
                Some(Ok(_))=>{
                    if let Some(replay)=self.power.stream.replay.as_mut(){replay.complete_once(Ok(()));}
                    if let Some(workspace)=self.workspace.as_ref(){let editor=if target.secondary{self.views.secondary.as_ref()}else{workspace.editors.get(target.index)};
                        if editor.is_some_and(|editor|editor.busy()){self.power.stream.receipt=Some((target,operation,receipt));return false;}
                    }
                    if let Operation::Transform(id)=operation && self.power.stream.replay.is_none() {
                        if let Some(workspace)=self.workspace.as_mut(){let editor=if target.secondary{self.views.secondary.as_mut()}else{workspace.editors.get_mut(target.index)};
                            if let Some(WorkspaceEditor::Paged(paged))=editor {if let Err(error)=paged.acknowledge_tracked_power(&receipt,&id,&Arguments::new()){self.power.status=error;return true;}}
                        }
                    }
                    self.power.status.clear();return true;
                }
            }
        }
        let Some(worker)=self.power.stream.worker.take()else{return false;};
        let Some(workspace)=self.workspace.as_mut()else{self.power.stream_failed("Workspace closed during preparation".into());return true;};
        let editor=if worker.target.secondary{self.views.secondary.as_mut()}else{workspace.editors.get_mut(worker.target.index)};
        let Some(WorkspaceEditor::Paged(paged))=editor else{self.power.stream_failed("Source view closed during staging.".into());return true;};
        if paged.snapshot().identity_token()!=worker.target.source.identity_token()||paged.snapshot().content_state!=worker.target.source.content_state||paged.global_selection()!=worker.target.selection{self.power.stream_failed("Document or selection changed; power edit cancelled.".into());return true;}
        let outcome=match worker.result.try_recv(){Ok(outcome)=>outcome,Err(TryRecvError::Empty)=>{self.power.stream.worker=Some(worker);return false;},Err(TryRecvError::Disconnected)=>Err("Power worker stopped".into())};
        match outcome {
            Err(error)=>self.power.stream_failed(error),
            Ok(Output::Prepared(prepared))=>match paged.apply_prepared_source_tracked(&worker.target.source,prepared){Ok(receipt)=>{self.power.stream.receipt=Some((worker.target.clone(),worker.operation.clone(),receipt));self.power.status="Applying selected transform…".into();},Err(error)=>self.power.stream_failed(error)},
            Ok(Output::Clipboard(text))=>{
                let result=self.platform.as_ref().ok_or_else(||"Clipboard unavailable".to_string()).and_then(|platform|platform.set_clipboard_text(&text).map_err(|error|error.to_string()));
                match result {
                    Err(error)=>self.power.status=error,
                    Ok(())=>{
                        self.power.copied(&text);
                        if matches!(worker.operation,Operation::Clipboard(true)) {
                            let range=worker.target.selection.0.min(worker.target.selection.1)..worker.target.selection.0.max(worker.target.selection.1);
                            let transaction=bareline_document::EditTransaction{base_revision:worker.target.source.revision,edits:vec![bareline_document::Edit{range,insert:String::new()}]};
                            match paged.apply_prepared_tracked(&worker.target.source,transaction){Ok(receipt)=>self.power.stream.receipt=Some((worker.target.clone(),worker.operation.clone(),receipt)),Err(error)=>self.power.status=error}
                        }
                    }
                }
            }
        }
        true
    }
    pub(super) fn power_stream_cancel(&mut self)->bool {
        if self.power.stream.worker.is_none()&&self.power.stream.promotion.is_none(){return false;}
        self.power.stream.worker=None;self.power.stream.promotion=None;
        self.power.stream_failed("Power preparation cancelled.".into());
        if let Some(workspace)=self.workspace.as_mut(){workspace.message=Some(self.power.status.clone());}
        true
    }
}

impl Shell {
    pub(crate) fn power_replay_start(&mut self,request:PowerReplayRequest)->Result<(),String> {
        if self.power.stream.worker.is_some()||self.power.stream.promotion.is_some()||self.power.stream.receipt.is_some()||self.power.stream.replay.is_some(){return Err("Power editing is busy".into());}
        if !request.arguments.is_empty()||power::transform_for_command(&request.command).is_none(){return Err("Unsupported explicit replay transform".into());}
        let workspace=self.workspace.as_mut().ok_or("Workspace closed")?;
        let editor=workspace.editors.get_mut(request.target_index).ok_or("Replay target closed")?;
        let document=match (&request.target,&*editor) {
            (PowerReplayTarget::Resident(source),WorkspaceEditor::Resident(view)) if source.identity_token()==view.snapshot().identity_token()&&source.content_state==view.snapshot().content_state=>source.identity_token().0,
            (PowerReplayTarget::Paged(source),WorkspaceEditor::Paged(view)) if source.identity_token()==view.snapshot().identity_token()&&source.content_state==view.snapshot().content_state=>source.identity_token().0,
            _=>return Err("Replay target changed before admission".into()),
        };
        if editor.busy()||editor.read_only(){return Err("Replay target is busy or read-only".into());}
        let selections=match &*editor {WorkspaceEditor::Resident(view)=>view.selection_set(),WorkspaceEditor::Paged(view)=>{let (anchor,caret)=view.global_selection();power::SelectionSet{selections:vec![bareline_editor_surface::Selection{anchor:anchor.0,caret:caret.0}],primary:0}}};
        if selections!=request.selections{return Err("Replay selection changed before admission".into());}
        self.power.status.clear();
        let mut replay=Replay{id:request.id,index:request.target_index,document,cancelled:false,resident:None,terminal:None};
        if let WorkspaceEditor::Resident(view)=editor {
            match view.execute_transform_tracked(&request.command){
                Ok(receipt)=>{replay.resident=Some(receipt);self.power.stream.replay=Some(replay);return Ok(());},
                Err(error) if error.contains("BudgetExceeded")=>{
                    self.power.stream.promotion=Some(Promotion{index:request.target_index,secondary:false,identity:view.snapshot().identity_token(),id:request.command,selections:request.selections});
                    self.power.stream.replay=Some(replay);(self.notify)();return Ok(());
                },
                Err(error)=>return Err(error),
            }
        }
        self.start_power_worker(request.target_index,false,Operation::Transform(request.command),Some(request.selections))?;
        self.power.stream.replay=Some(replay);Ok(())
    }
    pub(crate) fn power_replay_poll(&mut self,id:u64)->Option<PowerReplayCompletion> {
        let replay=self.power.stream.replay.as_ref()?;if replay.id!=id{return None;}
        let resident_result=match replay.resident.as_ref(){Some(receipt)=>Some(receipt.terminal()?),None=>None};
        if self.power.stream.worker.is_some()||self.power.stream.promotion.is_some()||self.power.stream.receipt.is_some(){return None;}
        let current=self.workspace.as_ref().and_then(|workspace|workspace.editors.get(replay.index));
        if current.is_some_and(|editor|editor.busy()){return None;}
        let target=current.and_then(|editor|match editor {
            WorkspaceEditor::Resident(view) if view.snapshot().identity_token().0==replay.document=>Some(PowerReplayTarget::Resident(view.snapshot().clone())),
            WorkspaceEditor::Paged(view) if view.snapshot().identity_token().0==replay.document=>Some(PowerReplayTarget::Paged(view.snapshot().clone())),
            _=>None,
        });
        let result=if let Some(result)=resident_result{result.map(|_|())}else if let Some(result)=replay.terminal.clone(){result}else if replay.cancelled{Err("Replay power operation cancelled".into())}else{return None;};
        self.power.stream.replay=None;
        Some(PowerReplayCompletion{target,result})
    }
    pub(crate) fn power_replay_cancel(&mut self,id:u64) {
        let Some(replay)=self.power.stream.replay.as_mut()else{return;};if replay.id!=id{return;}
        replay.cancelled=true;
        self.power.stream.worker=None;self.power.stream.promotion=None;
        // An admitted actor mutation cannot be withdrawn; retain its receipt
        // until publication/failure before reporting the final target identity.
    }
}

#[cfg(test)]
mod replay_terminal_tests {
    use super::*;
    #[test]
    fn unrelated_status_cannot_turn_committed_replay_into_retryable_failure(){
        let mut runtime=PowerRuntime::default();
        runtime.stream.replay=Some(Replay{id:1,index:0,document:9,cancelled:false,resident:None,terminal:None});
        runtime.status="A different control is unavailable".into();
        assert!(runtime.stream.replay.as_ref().unwrap().terminal.is_none());
        runtime.stream.replay.as_mut().unwrap().complete_once(Ok(()));
        runtime.status="A power edit is already pending".into();
        runtime.stream_failed("Late failure from another UI action".into());
        assert_eq!(runtime.stream.replay.as_ref().unwrap().terminal,Some(Ok(())));
        let delivered=runtime.stream.replay.take().unwrap().terminal;
        assert_eq!(delivered,Some(Ok(())));
        assert!(runtime.stream.replay.is_none());
    }
    #[test]
    fn failed_preparation_stays_failed_when_status_is_cleared(){
        let mut runtime=PowerRuntime::default();
        runtime.stream.replay=Some(Replay{id:1,index:0,document:9,cancelled:false,resident:None,terminal:None});
        runtime.stream_failed("Staging quota exceeded".into());runtime.status.clear();
        assert_eq!(runtime.stream.replay.as_ref().unwrap().terminal,Some(Err("Staging quota exceeded".into())));
    }
}
