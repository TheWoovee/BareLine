// SPDX-License-Identifier: MPL-2.0
//! Caller-owned terminal results; observing a receipt never consumes it.
use bareline_document::{DocumentSnapshot, EditTransaction, Revision};
use std::sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}};

#[derive(Clone)]
pub struct TrackedEditReceipt {
    pub captured_identity_token: (u64,u64),
    pub operation_id: u64,
    result: Arc<Mutex<Option<Result<Revision,String>>>>,
}
impl TrackedEditReceipt {
    pub(crate) fn new(captured_identity_token:(u64,u64))->Self {
        static NEXT:AtomicU64=AtomicU64::new(1);
        Self{captured_identity_token,operation_id:NEXT.fetch_add(1,Ordering::Relaxed),result:Arc::new(Mutex::new(None))}
    }
    pub fn terminal(&self)->Option<Result<Revision,String>> {
        self.result.lock().unwrap_or_else(|error|error.into_inner()).clone()
    }
    pub(crate) fn complete_once(&self,result:Result<Revision,String>) {
        let mut terminal=self.result.lock().unwrap_or_else(|error|error.into_inner());
        if terminal.is_none(){*terminal=Some(result);}
    }
}
pub(crate) struct TrackedEditCompletion(pub(crate) TrackedEditReceipt);
impl TrackedEditCompletion {
    pub(crate) fn complete_once(&self,result:Result<Revision,String>){self.0.complete_once(result);}
}
impl Drop for TrackedEditCompletion {
    fn drop(&mut self){self.0.complete_once(Err("Edit completion was cancelled or disconnected".into()));}
}
impl crate::EditorSurface {
    pub fn execute_power_tracked(&mut self,id:&str,args:&crate::power::consumer::Arguments)->Result<TrackedEditReceipt,String>{
        crate::paged_power::validate_arguments(id,args)?;
        let receipt=TrackedEditReceipt::new(self.snapshot.identity_token());self.execute_power_parameters(id,args,false)?;
        if let Some(pending)=self.pending.as_mut(){pending.tracked=Some(TrackedEditCompletion(receipt.clone()));}else{receipt.complete_once(Ok(self.snapshot.revision));}Ok(receipt)
    }
    /// Explicit transform replay uses the same actor path without creating a
    /// second command-recording event for the command being replayed.
    pub fn execute_transform_tracked(&mut self,id:&str)->Result<TrackedEditReceipt,String> {
        if crate::power::transform_for_command(id).is_none(){return Err("Unsupported replay transform".into());}
        let receipt=TrackedEditReceipt::new(self.snapshot.identity_token());
        self.execute_power(id)?;
        if let Some(pending)=self.pending.as_mut(){pending.tracked=Some(TrackedEditCompletion(receipt.clone()));}
        else{receipt.complete_once(Ok(self.snapshot.revision));}
        Ok(receipt)
    }
    pub fn apply_prepared_tracked(&mut self,source:&DocumentSnapshot,transaction:EditTransaction)->Result<TrackedEditReceipt,&'static str> {
        self.apply_prepared(source,transaction)?;
        let receipt=TrackedEditReceipt::new(source.identity_token());
        self.pending.as_mut().ok_or("Prepared edit was not admitted")?.tracked=Some(TrackedEditCompletion(receipt.clone()));
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resident_receipt_reports_the_actor_result_not_revision_observation(){
        use bareline_document::{Budget,Document,Edit,TextOffset,service::Scheduler};
        let scheduler=Scheduler::new(1,8).unwrap();
        let document=Document::from_utf8("abc",Budget::new(1<<20),Budget::new(1<<20)).unwrap();
        let snapshot=document.snapshot();
        let mut view=crate::EditorSurface::new(scheduler.document(document,8),snapshot.clone(),Arc::new(||{}));
        let receipt=view.apply_prepared_tracked(&snapshot,EditTransaction{base_revision:snapshot.revision,edits:vec![Edit{range:TextOffset(1)..TextOffset(2),insert:"X".into()}]}).unwrap();
        assert!(receipt.terminal().is_none());
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(2);
        while receipt.terminal().is_none(){view.pump();assert!(std::time::Instant::now()<deadline);std::thread::yield_now();}
        assert_eq!(receipt.terminal(),Some(Ok(view.snapshot().revision)));
        let snapshot=view.snapshot().clone();
        let rejected=view.apply_prepared_tracked(&snapshot,EditTransaction{base_revision:snapshot.revision,edits:vec![Edit{range:TextOffset(9)..TextOffset(10),insert:"X".into()}]}).unwrap();
        while rejected.terminal().is_none(){view.pump();assert!(std::time::Instant::now()<deadline);std::thread::yield_now();}
        assert!(matches!(rejected.terminal(),Some(Err(_))));
        assert_eq!(view.snapshot().revision,snapshot.revision);
        assert_eq!(receipt.terminal(),Some(Ok(snapshot.revision)));
    }
    #[test]
    fn terminal_is_shared_nonconsuming_and_first_result_wins(){
        let receipt=TrackedEditReceipt::new((7,4));let observer=receipt.clone();
        assert!(observer.terminal().is_none());
        {let completion=TrackedEditCompletion(receipt);completion.complete_once(Ok(Revision(5)));}
        assert_eq!(observer.terminal(),Some(Ok(Revision(5))));
        assert_eq!(observer.terminal(),Some(Ok(Revision(5))));
    }
    #[test]
    fn replay_transform_waits_for_actor_without_recording_itself(){
        use bareline_document::{Budget,Document,TextOffset,service::Scheduler};
        let scheduler=Scheduler::new(1,8).unwrap();let document=Document::from_utf8("abc",Budget::new(1<<20),Budget::new(1<<20)).unwrap();
        let snapshot=document.snapshot();let mut view=crate::EditorSurface::new(scheduler.document(document,8),snapshot,Arc::new(||{}));
        view.set_selections(crate::power::SelectionSet{selections:vec![crate::Selection{anchor:0,caret:3}],primary:0}).unwrap();
        let receipt=view.execute_transform_tracked("editor.case.upper").unwrap();assert!(receipt.terminal().is_none());
        let deadline=std::time::Instant::now()+std::time::Duration::from_secs(2);
        while receipt.terminal().is_none(){view.pump();assert!(std::time::Instant::now()<deadline);std::thread::yield_now();}
        assert!(matches!(receipt.terminal(),Some(Ok(_))));
        assert_eq!(view.snapshot().read(TextOffset(0)..TextOffset(3),3).unwrap(),"ABC");
        assert!(view.take_acknowledged_commands().is_empty());
    }
    #[test]
    fn dropped_completion_reports_failure(){
        let receipt=TrackedEditReceipt::new((7,4));
        drop(TrackedEditCompletion(receipt.clone()));
        assert!(matches!(receipt.terminal(),Some(Err(_))));
    }
}
