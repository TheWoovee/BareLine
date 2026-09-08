// SPDX-License-Identifier: MPL-2.0
//! Nonpersistent authority created only after an explicit native consent decision.
use std::{io,path::{Path,PathBuf},sync::{Arc,atomic::{AtomicBool,Ordering}},time::{Duration,Instant}};
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub enum RemoteReadAction{Open,Reload,Follow}
#[derive(Clone)]
pub struct RemoteReadGrant{path:PathBuf,action:RemoteReadAction,expires:Instant,state:Arc<State>}
struct State{claimed:AtomicBool,revoked:AtomicBool}
impl RemoteReadGrant{
    /// No filesystem or network classification occurs here. The caller must have
    /// received affirmative consent for this exact displayed path and action.
    pub fn after_consent(path:PathBuf,action:RemoteReadAction,admission_lifetime:Duration)->io::Result<Self>{
        if path.as_os_str().is_empty()||path.as_os_str().len()>32768||admission_lifetime.is_zero()||admission_lifetime>Duration::from_secs(300){return Err(denied());}
        Ok(Self{path,action,expires:Instant::now()+admission_lifetime,state:Arc::new(State{claimed:AtomicBool::new(false),revoked:AtomicBool::new(false)})})
    }
    pub fn is_revoked(&self)->bool{self.state.revoked.load(Ordering::Acquire)}
    pub fn revoke(&self){self.state.revoked.store(true,Ordering::Release);}
    pub fn claim(&self,path:&Path,action:RemoteReadAction,cancelled:Arc<dyn Fn()->bool+Send+Sync>)->io::Result<RemoteReadAccess>{
        if path!=self.path||action!=self.action||Instant::now()>=self.expires||self.state.revoked.load(Ordering::Acquire)||cancelled(){return Err(denied());}
        self.state.claimed.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).map_err(|_|denied())?;
        Ok(RemoteReadAccess{path:self.path.clone(),action,state:self.state.clone(),cancelled})
    }
}
/// One admitted operation. Follow retains this capability until stopped/revoked;
/// admission expiry does not interrupt a live follow operation halfway through a read.
#[derive(Clone)]
pub struct RemoteReadAccess{path:PathBuf,action:RemoteReadAction,state:Arc<State>,cancelled:Arc<dyn Fn()->bool+Send+Sync>}
impl RemoteReadAccess{
    pub fn is_revoked(&self)->bool{self.state.revoked.load(Ordering::Acquire)||(self.cancelled)()}
    pub fn path(&self)->&Path{&self.path}
    pub fn action(&self)->RemoteReadAction{self.action}
    pub fn check(&self,path:&Path)->io::Result<()>{if path!=self.path||self.state.revoked.load(Ordering::Acquire)||(self.cancelled)(){Err(denied())}else{Ok(())}}
}
fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"remote read grant is missing, expired, revoked, or outside its action")}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn exact_action_once_and_revocation_before_any_io(){let path=PathBuf::from("approved-fixture");let grant=RemoteReadGrant::after_consent(path.clone(),RemoteReadAction::Open,Duration::from_secs(1)).unwrap();assert!(grant.claim(Path::new("other"),RemoteReadAction::Open,Arc::new(||false)).is_err());assert!(grant.claim(&path,RemoteReadAction::Follow,Arc::new(||false)).is_err());let access=grant.claim(&path,RemoteReadAction::Open,Arc::new(||false)).unwrap();assert!(grant.claim(&path,RemoteReadAction::Open,Arc::new(||false)).is_err());assert!(access.check(&path).is_ok());grant.revoke();assert!(access.check(&path).is_err());}
    #[test]fn expired_and_cancelled_admission_refuses(){let mut grant=RemoteReadGrant::after_consent("fixture".into(),RemoteReadAction::Reload,Duration::from_secs(1)).unwrap();grant.expires=Instant::now();assert!(grant.claim(Path::new("fixture"),RemoteReadAction::Reload,Arc::new(||false)).is_err());let grant=RemoteReadGrant::after_consent("fixture".into(),RemoteReadAction::Reload,Duration::from_secs(1)).unwrap();assert!(grant.claim(Path::new("fixture"),RemoteReadAction::Reload,Arc::new(||true)).is_err());}
}
