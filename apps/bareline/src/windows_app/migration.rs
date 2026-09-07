// SPDX-License-Identifier: MPL-2.0
//! Explicit review/apply migration. Foreign session paths never open during parsing.
use super::*;
use bareline_distribution::importer::{ImportReport,Preference};
use std::sync::mpsc::{self,Receiver};
#[derive(Default)]pub(super) struct MigrationRuntime{pending:Option<(Vec<u8>,ImportReport)>,worker:Option<Receiver<Result<(Vec<u8>,ImportReport),String>>>,paths:Vec<PathBuf>,opening:Option<Receiver<Vec<PathBuf>>>,cancel:std::sync::Arc<std::sync::atomic::AtomicBool>}
pub(super) fn commands()->Vec<bareline_commands::CommandSpec>{[("migration.cancel","Cancel Import Work"),("migration.review","Review Notepad++ Import…"),("migration.apply","Apply Reviewed Notepad++ Import"),("migration.open_paths","Open Imported Local Files")].into_iter().map(|(id,title)|bareline_commands::CommandSpec{id:bareline_commands::CommandId(id),title,category:"Settings",shortcut:"",action:Action::Contributed(bareline_commands::CommandId(id))}).collect()}
impl Shell{
    pub(super) fn migration_dispatch(&mut self,el:&ActiveEventLoop,id:&str)->bool{
        match id{
            "migration.cancel"=>{self.migration.cancel.store(true,std::sync::atomic::Ordering::Release);self.migration.pending=None;},
            "migration.review"=>{
                if self.migration.worker.is_some()||self.migration.opening.is_some(){return true;}
                self.migration.pending=None;
                let path=match self.platform.as_ref().map(|p|p.open_file()){Some(Ok(Some(path)))=>path,Some(Err(e))=>{if let Some(w)=&mut self.workspace{w.message=Some(e);}return true;},_=>return true};
                self.migration.cancel.store(false,std::sync::atomic::Ordering::Release);let cancel=self.migration.cancel.clone();let(tx,rx)=mpsc::sync_channel(1);let notify=self.notify.clone();
                let spawn=std::thread::Builder::new().name("bareline-migration-review".into()).spawn(move||{
                    use std::io::Read;use bareline_platform::{PathTrustProvider,PathOrigin};
                    let result=(||{if cancel.load(std::sync::atomic::Ordering::Acquire){return Err("Import cancelled".into());}let opened=bareline_platform_windows::WindowsPathTrustProvider.open_read(&path,PathOrigin::User).map_err(|e|e.to_string())?;let mut bytes=Vec::new();opened.file.take(1024*1024+1).read_to_end(&mut bytes).map_err(|e|e.to_string())?;let report=bareline_distribution::importer::parse(&bytes)?;if cancel.load(std::sync::atomic::Ordering::Acquire){return Err("Import cancelled".into());}Ok((bytes,report))})();let _=tx.send(result);notify();
                });
                match spawn{Ok(_)=>self.migration.worker=Some(rx),Err(e)=>if let Some(w)=&mut self.workspace{w.message=Some(e.to_string());}}
            },
            "migration.apply"=>{
                if self.migration.worker.is_some()||self.migration.opening.is_some(){return true;}
                let Some((bytes,mut report))=self.migration.pending.take()else{return true;};
                let prior_scope=self.settings.controller.scope;self.settings.controller.scope=bareline_settings::Scope::User;
                for(key,value)in &report.preferences{
                    let value=match value{Preference::Bool(v)=>bareline_settings::SettingValue::Bool(*v),Preference::Integer(v)if key=="editor.font.size"=>bareline_settings::SettingValue::Number(*v as f64),Preference::Integer(v)=>bareline_settings::SettingValue::Integer(*v),Preference::Text(v)=>bareline_settings::SettingValue::Text(v.clone()),Preference::Colors(v)=>bareline_settings::SettingValue::Map(v.clone())};
                    if let Err(e)=self.settings.controller.edit(key,value){report.notes.push(format!("Skipped {key}: {e}"));}
                }
                self.settings.controller.scope=prior_scope;
                let mut keymap=self.settings.keymap.clone();let mut changed=false;
                for(id,chord)in &report.shortcuts{
                    let Some(command)=self.app.commands.entries().find(|c|c.id.0==*id).map(|c|c.id)else{report.notes.push(format!("Skipped unregistered command {id}"));continue;};
                    let result=bareline_commands::KeyChord::parse(chord).and_then(|chord|keymap.set_binding(bareline_commands::KeyBinding{command,sequence:vec![chord]},&self.app.commands));
                    match result{Ok(())=>changed=true,Err(e)=>report.notes.push(format!("Skipped shortcut {id}: {e}"))}
                }
                if changed{self.settings.save_keymap(keymap,None);}
                if report.user_language{self.language.controller.import_udl_bytes(bytes,self.notify.clone());}
                self.migration.paths=report.paths.iter().map(PathBuf::from).collect();
                report.notes.insert(0,"Mapped preferences and conflict-free shortcuts submitted to the normal settings save flow. UDL validation, if requested, reports through Language. Paths remain unopened until Open Imported Local Files.".into());
                self.migration_report(el,&report);
            },
            "migration.open_paths"=>{
                if self.migration.opening.is_some()||self.migration.worker.is_some(){return true;}let paths=self.migration.paths.clone();self.migration.cancel.store(false,std::sync::atomic::Ordering::Release);let cancel=self.migration.cancel.clone();let(tx,rx)=mpsc::sync_channel(1);let notify=self.notify.clone();
                match std::thread::Builder::new().name("bareline-migration-paths".into()).spawn(move||{use bareline_platform::{PathTrustProvider,PathOrigin};let paths=paths.into_iter().take(16).take_while(|_|!cancel.load(std::sync::atomic::Ordering::Acquire)).filter(|p|bareline_platform_windows::WindowsSessionPathTrustProvider.open_read(p,PathOrigin::Session).is_ok()).collect();let _=tx.send(paths);notify();}){Ok(_)=>self.migration.opening=Some(rx),Err(e)=>if let Some(w)=&mut self.workspace{w.message=Some(e.to_string());}}
            },_=>return false,
        }true
    }
    fn migration_report(&mut self,el:&ActiveEventLoop,report:&ImportReport){
        if !self.ensure_workspace(el){return;}
        let text=report.render();let budget=bareline_document::Budget::new(2*1024*1024);
        match bareline_document::Document::from_utf8(&text,budget.clone(),budget){
            Ok(doc)=>if let Some(w)=&mut self.workspace{match w.add_snapshot_preview(&doc.snapshot(),"Notepad++ Import Report".into()){Ok(index)=>self.app.active=index,Err(e)=>w.message=Some(format!("Report: {e:?}"))}},
            Err(e)=>if let Some(w)=&mut self.workspace{w.message=Some(format!("Report: {e:?}"));}
        }
    }
    pub(super) fn migration_pump(&mut self,el:&ActiveEventLoop){
        if let Some(result)=self.migration.worker.as_ref().and_then(|rx|rx.try_recv().ok()){
            self.migration.worker=None;if self.migration.cancel.load(std::sync::atomic::Ordering::Acquire){return;}match result{Ok((bytes,report))=>{self.migration_report(el,&report);self.migration.pending=Some((bytes,report));},Err(e)=>{if self.ensure_workspace(el){self.workspace.as_mut().unwrap().message=Some(e);}}}
        }
        if let Some(paths)=self.migration.opening.as_ref().and_then(|rx|rx.try_recv().ok()){
            self.migration.opening=None;if self.migration.cancel.load(std::sync::atomic::Ordering::Acquire){return;}if self.ensure_workspace(el){let w=self.workspace.as_mut().unwrap();for path in paths{w.open(path);}w.message=Some("Opened validated local import candidates; unavailable, linked or remote paths were skipped.".into());}
        }
    }
}

impl Drop for MigrationRuntime{fn drop(&mut self){self.cancel.store(true,std::sync::atomic::Ordering::Release);}}

impl MigrationRuntime {
    pub(super) fn annotate_context(&self,context:&mut bareline_commands::CommandContext){
        use bareline_commands::{CommandId,CommandState};
        let busy=self.worker.is_some()||self.opening.is_some();
        if busy {for id in ["migration.review","migration.apply","migration.open_paths"]{context.states.insert(CommandId(id),CommandState::disabled("Wait for import work or cancel it"));}}
        if self.pending.is_none(){context.states.insert(CommandId("migration.apply"),CommandState::disabled("Review an import first"));}
        if self.paths.is_empty(){context.states.insert(CommandId("migration.open_paths"),CommandState::disabled("Apply a reviewed import with local file candidates first"));}
        if !busy && self.pending.is_none(){context.states.insert(CommandId("migration.cancel"),CommandState::disabled("No import work to cancel"));}
    }
}