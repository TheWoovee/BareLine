// SPDX-License-Identifier: MPL-2.0
//! Exact read authority plus ordinary local-cache operations. Never remote writes.
use bareline_platform::{LocalFileSystem,RemoteReadAccess,RemoteReadAction,TrustedRead,FileIdentity};
use std::{fs::File,io,path::Path,sync::{Arc,Mutex}};
use crate::files::WindowsFileSystem;
pub(crate) struct RemoteFileSystem{access:RemoteReadAccess,opened:Mutex<Option<TrustedRead>>}
impl RemoteFileSystem{pub(crate) fn new(access:RemoteReadAccess)->Self{Self{access,opened:Mutex::new(None)}}fn local(&self,path:&Path)->io::Result<()>{if path==self.access.path(){Err(io::Error::new(io::ErrorKind::PermissionDenied,"remote grant permits reads only"))}else{use bareline_platform::PathTrustProvider;let parent=path.parent().filter(|parent|!parent.as_os_str().is_empty()).unwrap_or(path);crate::path_trust::WindowsPathTrustProvider.open_read(parent,bareline_platform::PathOrigin::User).map(|_|())}}}
impl LocalFileSystem for RemoteFileSystem{
    fn check_source_read(&self,path:&Path)->io::Result<()>{if path==self.access.path(){self.access.check(path)}else{Ok(())}}
    fn release_source_read(&self,path:&Path){if path==self.access.path(){if let Ok(mut opened)=self.opened.lock(){*opened=None;}}}
    fn validate_source(&self,path:&Path)->io::Result<()>{
        if path!=self.access.path(){return WindowsFileSystem.validate_source(path);}
        self.access.check(path)?;
        let mut held=self.opened.lock().map_err(|_|io::Error::other("remote source guard stopped"))?;
        if held.is_none(){*held=Some(crate::path_trust::WindowsPathTrustProvider.open_remote_read(path,&self.access)?);}
        self.access.check(path)
    }
    fn open_follow_read(&self,path:&Path)->io::Result<(File,Arc<dyn Send+Sync>)>{
        if path!=self.access.path()||self.access.action()!=RemoteReadAction::Follow{return Err(io::Error::new(io::ErrorKind::PermissionDenied,"outside followed path grant"));}
        let opened=crate::path_trust::WindowsPathTrustProvider.open_remote_read(path,&self.access)?;
        Ok((opened.file,Arc::new(opened.ancestors)))
    }
    fn guard_directory(&self,path:&Path)->io::Result<Arc<dyn Send+Sync>>{self.local(path)?;WindowsFileSystem.guard_directory(path)}
    fn available_space(&self,path:&Path)->io::Result<u64>{self.local(path)?;WindowsFileSystem.available_space(path)}
    fn open_sealed_read(&self,path:&Path)->io::Result<File>{self.local(path)?;WindowsFileSystem.open_sealed_read(path)}
    fn identity(&self,file:&File)->io::Result<FileIdentity>{WindowsFileSystem.identity(file)}
    fn validate_target(&self,path:&Path)->io::Result<()>{self.local(path)?;WindowsFileSystem.validate_target(path)}
    fn commit(&self,staged:&Path,target:&Path,existed:bool)->io::Result<()>{self.validate_target(target)?;self.validate_target(staged)?;WindowsFileSystem.commit(staged,target,existed)}
}
