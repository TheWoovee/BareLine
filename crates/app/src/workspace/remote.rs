// SPDX-License-Identifier: MPL-2.0
//! Explicit one-action remote read admission; authority is never session metadata.
use super::*;
use bareline_platform::{RemoteReadGrant,RemoteReadAction};
impl Workspace{
    pub(super) fn remote_open_request(&self,path:PathBuf)->IoRequest{IoRequest::OpenPagedEncoded(bareline_file_io::lifecycle::PagedOpenRequest{path,bytes:self.bytes.clone(),history:self.history.clone(),cache:std::env::temp_dir().join("Bareline-transcode"),options:bareline_file_io::codecs::disk::DiskOptions{temp_quota_bytes:self.transcode_quota_bytes,interpret:None},source_options:self.source_options()})}
    pub fn open_authorized(&mut self,path:PathBuf,grant:RemoteReadGrant)->Result<(),String>{
        if self.path_loading(&path){return Err("This file is already loading".into());}
        if !self.ensure_io(){return Err("File service unavailable".into());}
        let receiver=self.io.as_ref().unwrap().submit_authorized(self.remote_open_request(path.clone()),grant,RemoteReadAction::Open,self.notify.clone()).map_err(|_|"File queue is full")?;
        self.pending_io.push(PendingIo{completion:None,receiver,save:None,copy_only:false,open_path:Some(path),preview:None,reload:None});
        self.message=Some("Opening the approved remote file…".into());Ok(())
    }
    pub fn reload_authorized(&mut self,index:usize,discard_confirmed:bool,grant:RemoteReadGrant)->Result<(),String>{
        let editor=self.editors.get(index).ok_or("Document unavailable")?;
        if editor.busy()||editor.dirty()&&!discard_confirmed{return Err("Confirm discard before reloading current edits".into());}
        let captured=editor.snapshot().clone();let path=self.path(index).ok_or("Document has no source path")?.to_path_buf();
        if self.path_loading(&path){return Err("This file is already loading".into());}
        if !self.ensure_io(){return Err("File service unavailable".into());}
        let receiver=self.io.as_ref().unwrap().submit_authorized(self.remote_open_request(path.clone()),grant,RemoteReadAction::Reload,self.notify.clone()).map_err(|_|"File queue is full")?;
        self.pending_io.push(PendingIo{completion:None,receiver,save:None,copy_only:false,open_path:Some(path),preview:None,reload:Some(captured)});self.message=Some("Reloading the approved remote file…".into());Ok(())
    }
    /// Returns the same admitted provider for subsequent follow ticks. If Resident,
    /// queues a fixed paged capture first; caller waits for it before start_follow.
    pub fn follow_authorized(&mut self,index:usize,grant:RemoteReadGrant)->Result<Arc<dyn LocalFileSystem>,String>{
        let editor=self.editors.get(index).ok_or("Document unavailable")?;
        if editor.busy()||editor.dirty(){return Err("Finish current work and save edits before following".into());}
        let path=self.path(index).ok_or("Document has no source path")?.to_path_buf();
        let access=grant.claim(&path,RemoteReadAction::Follow,Arc::new(||false)).map_err(|e|e.to_string())?;
        let provider=self.file_system.scoped_remote_read(access.clone()).map_err(|e|e.to_string())?;
        if let WorkspaceEditor::Paged(paged)=&mut self.editors[index]&&paged.follow_status().is_none(){paged.start_follow(provider.clone())?;return Ok(provider);}
        let captured=self.editors[index].snapshot().clone();
        if self.path_loading(&path){return Err("This file is already loading".into());}
        if !self.ensure_io(){return Err("File service unavailable".into());}
        let receiver=self.io.as_ref().unwrap().submit_follow_read(self.remote_open_request(path.clone()),access,self.notify.clone()).map_err(|_|"File queue is full")?;
        self.pending_io.push(PendingIo{completion:None,receiver,save:None,copy_only:false,open_path:Some(path),preview:None,reload:Some(captured)});self.message=Some("Preparing the approved remote file for follow…".into());Ok(provider)
    }
}
