// SPDX-License-Identifier: MPL-2.0
use super::*;
use bareline_file_io::codecs::{Encoding, state::EncodingState};
impl Workspace {
    pub fn encoding_state(&self, index: usize) -> Option<EncodingState> {
        let editor=self.editors.get(index)?;
        let fallback=|| bareline_file_io::codecs::state::EncodingState::new(bareline_file_io::codecs::Detection {encoding:Encoding::Utf8,confidence:bareline_file_io::codecs::Confidence::Utf8Sample,bom:self.files.get(index).and_then(Option::as_ref).is_some_and(|file|file.bom),binary_warning:false});
        Some(match editor {
            WorkspaceEditor::Paged(editor)=>editor.encoding_state()?,
            WorkspaceEditor::Resident(editor)=>bareline_file_io::codecs::state::metadata_encoding(editor.snapshot().metadata()).or_else(||self.files.get(index).and_then(Option::as_ref).and_then(|file|file.encoding.as_ref()).map(|encoding|encoding.state.clone())).unwrap_or_else(fallback),
        })
    }
    pub fn binary_warning_pending(&self, index: usize) -> bool {
        self.files.get(index).and_then(Option::as_ref).is_some_and(|file| !file.binary_accepted)
            && self.encoding_state(index).is_some_and(|state| state.binary_warning)
    }
    pub fn encoding_convert(&mut self,index:usize,target:Encoding,bom:bool)->Result<(),String>{
        if bom&&target.bom().is_empty(){return Err("This encoding does not support a byte-order mark".into());}
        let mut state=self.encoding_state(index).ok_or("Encoding state is unavailable")?;
        state.convert_to(target);state.bom=bom;
        let editor=self.editors.get_mut(index).ok_or("Document unavailable")?;
        match editor {
            WorkspaceEditor::Resident(editor)=>{let metadata=bareline_file_io::codecs::state::with_encoding(editor.snapshot().metadata(),&state).map_err(|error|format!("{error:?}"))?;editor.apply_document_metadata(metadata)},
            WorkspaceEditor::Paged(editor)=>{let metadata=bareline_file_io::codecs::state::with_encoding(editor.snapshot().metadata(),&state).map_err(|error|format!("{error:?}"))?;editor.apply_document_metadata(metadata)},
        }
    }
    pub fn encoding_accept_binary(&mut self, index: usize, read_only: bool) -> Result<(), String> {
        let file = self.files.get_mut(index).and_then(Option::as_mut).ok_or("Document is unavailable")?;
        file.binary_accepted = true;
        self.editors.get_mut(index).ok_or("Document is unavailable")?.set_read_only(read_only);
        Ok(())
    }
    pub(super) fn refresh_encoding_open(&mut self, index: usize) {
        if let Some(state) = self.encoding_state(index) {
            self.editors[index].encoding_label = format!("{:?}", state.save_target);
            if self.binary_warning_pending(index) { self.editors[index].set_read_only(true); }
        }
    }
    pub fn encoding_interpret(&mut self, index: usize, target: Encoding, discard_confirmed: bool) -> Result<(), String> {
        let editor = self.editors.get(index).ok_or("Document is unavailable")?;
        if editor.busy() || (editor.dirty() && !discard_confirmed) { return Err("Confirm discarding edits before interpreting original bytes".into()); }
        let source = self.files.get(index).and_then(Option::as_ref).ok_or("No original byte source")?;
        if let WorkspaceEditor::Paged(paged) = editor {
            if self.interpreting_paged.is_some() {return Err("An interpretation is already running".into());}
            let captured=paged.snapshot().clone();
            let path=source.path.clone();
            let request=bareline_file_io::lifecycle::InterpretPagedRequest {source:paged.read_handle().original_store()?,target,path:path.clone(),fingerprint:source.fingerprint.clone(),cache:std::env::temp_dir().join("Bareline-transcode"),quota:self.transcode_quota_bytes,options:bareline_file_io::source::SourceOptions::default(),bytes:self.bytes.clone(),history:self.history.clone()};
            if !self.ensure_io(){return Err("File service unavailable".into());}
            let receiver=self.io.as_ref().unwrap().submit(IoRequest::InterpretPaged(Box::new(request)),self.notify.clone()).map_err(|_|"File queue is full")?;
            self.interpreting_paged=Some((captured,path.clone()));
            self.pending_io.push(PendingIo {completion:None,receiver,save:None,copy_only:false,open_path:Some(path),preview:None,reload:None});
            self.message=Some("Interpreting sealed original bytes…".into());
            return Ok(());
        }
        let encoding = source.encoding.clone().ok_or("Original bytes are unavailable")?;
        let request = bareline_file_io::lifecycle::InterpretRequest { source: encoding, target, dirty: editor.dirty(), discard_confirmed, bytes:self.bytes.clone(), history:self.history.clone(), path:source.path.clone(), fingerprint:source.fingerprint.clone() };
        let captured = editor.snapshot().clone();
        let path = source.path.clone();
        if !self.ensure_io() { return Err("File service unavailable".into()); }
        let receiver = self.io.as_ref().unwrap().submit(IoRequest::Interpret(Box::new(request)),self.notify.clone()).map_err(|_| "File queue is full")?;
        self.pending_io.push(PendingIo {completion:None,receiver,save:None,copy_only:false,open_path:Some(path),preview:None,reload:Some(captured)});
        self.message = Some("Interpreting retained original bytes…".into());
        Ok(())
    }
}

enum EolSource { Resident(bareline_document::DocumentSnapshot), Paged(bareline_editor_surface::paged_view::PagedReadHandle) }
pub(super) struct EolJob { cancellation: bareline_file_io::cancellation::Cancellation, receiver: std::sync::mpsc::Receiver<(EolSource, Result<bareline_document::EditTransaction,String>)> }
impl Drop for EolJob {fn drop(&mut self){self.cancellation.cancel();}}
impl Workspace {
    pub fn encoding_eol(&mut self,index:usize,target:bareline_file_io::codecs::state::Eol,selection_only:bool)->Result<(),String>{
        if self.eol_job.is_some(){return Err("A newline conversion is already running".into());}
        let editor=self.editors.get(index).ok_or("Document unavailable")?;
        if editor.busy()||editor.read_only(){return Err("Document is busy or read only".into());}
        let (source,length,origin)=match editor { WorkspaceEditor::Resident(editor)=>(EolSource::Resident(editor.snapshot().clone()),editor.snapshot().len(),0),WorkspaceEditor::Paged(editor)=>(EolSource::Paged(editor.read_handle()),editor.snapshot().len(),editor.viewport_start().0)};
        let selected=editor.selection.anchor.min(editor.selection.caret)..editor.selection.anchor.max(editor.selection.caret);
        if selection_only&&selected.is_empty(){return Err("Select text before converting selection newlines".into());}
        let range=if selection_only {origin+selected.start..origin+selected.end}else{0..length};
        let budget=self.bytes.clone();let notify=self.notify.clone();let (sender,receiver)=std::sync::mpsc::sync_channel(1);let cancellation=bareline_file_io::cancellation::Cancellation::default();let worker_cancel=cancellation.clone();
        std::thread::Builder::new().name("encoding-eol".into()).spawn(move||{
            let result=plan_eol(&source,range,target,&budget,&worker_cancel);let _=sender.send((source,result));notify();
        }).map_err(|error|error.to_string())?;
        self.eol_job=Some(EolJob{receiver,cancellation});self.message=Some("Preparing newline conversion…".into());Ok(())
    }
    pub(super) fn pump_encoding(&mut self)->bool{
        let Some(job)=&self.eol_job else{return false;};
        let result=match job.receiver.try_recv(){Ok(value)=>value,Err(std::sync::mpsc::TryRecvError::Empty)=>return false,Err(_)=>{self.eol_job=None;self.message=Some("Newline worker stopped".into());return true;}};
        self.eol_job=None;
        let (source,result)=result;
        let applied=result.and_then(|transaction|{
            for editor in &mut self.editors {match (editor,&source){
                (WorkspaceEditor::Resident(editor),EolSource::Resident(snapshot)) if editor.snapshot().same_document(snapshot)=>return editor.apply_prepared(snapshot,transaction).map_err(str::to_owned),
                (WorkspaceEditor::Paged(editor),EolSource::Paged(handle)) if editor.snapshot().same_document(handle.snapshot())=>return editor.apply_prepared(handle.snapshot(),transaction),
                _=>{}
            }}Err("Document closed before newline conversion completed".into())
        });
        self.message=Some(match applied{Ok(())=>"Applying newline conversion…".into(),Err(error)=>error});true
    }
}
fn plan_eol(source:&EolSource,range:std::ops::Range<usize>,target:bareline_file_io::codecs::state::Eol,budget:&Budget,cancel:&bareline_file_io::cancellation::Cancellation)->Result<bareline_document::EditTransaction,String>{
    use bareline_document::{TextOffset,Edit,EditTransaction,paged::WindowPoll};
    use bareline_file_io::codecs::state::Eol;
    if let EolSource::Resident(snapshot)=source {return bareline_file_io::codecs::state::plan_eol_conversion(snapshot,TextOffset(range.start)..TextOffset(range.end),target,131072).map_err(|error|format!("{error:?}"));}
    let EolSource::Paged(handle)=source else{unreachable!()};let snapshot=handle.snapshot();
    let mut edits=Vec::new();let mut cr=None;let mut offset=range.start;
    let mut emit=|start:usize,len:usize,current:Eol|->Result<(),String>{if current!=target {if edits.len()>=131072{return Err("Newline conversion exceeds the bounded transaction limit".into());}edits.push(Edit{range:TextOffset(start)..TextOffset(start+len),insert:target.text().into()});}Ok(())};
    while offset<range.end {
        cancel.check().map_err(|_|"Newline conversion cancelled")?;
        let mut request=snapshot.begin_viewport(TextOffset(offset),(range.end-offset).min(64*1024),budget).map_err(|error|format!("{error:?}"))?;
        let window=loop{cancel.check().map_err(|_|"Newline conversion cancelled")?;match request.poll(){WindowPoll::Ready(window)=>break window,WindowPoll::Pending(ticket)=>{if !handle.resolve_captured_page(ticket)?{std::thread::yield_now();}},_=>return Err("Newline source unavailable".into())}};
        let start=window.range().start.0;let end=window.range().end.0.min(range.end);
        if end<=offset{return Err("Newline source made no progress".into());}
        for (local,byte) in window.text().bytes().enumerate(){let at=start+local;if at<offset||at>=end{continue;}if let Some(previous)=cr.take(){if byte==b'\n'{emit(previous,2,Eol::CrLf)?;continue;}emit(previous,1,Eol::Cr)?;}match byte{b'\r'=>cr=Some(at),b'\n'=>emit(at,1,Eol::Lf)?,_=>{}}}offset=end;
    }
    if let Some(previous)=cr{emit(previous,1,Eol::Cr)?;}Ok(EditTransaction{base_revision:snapshot.revision,edits})
}
