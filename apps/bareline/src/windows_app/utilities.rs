// SPDX-License-Identifier: MPL-2.0
//! User-triggered utility workers; completions return through the normal actor boundary.
use super::*;
use bareline_app::utilities::{self as core, HashAlgorithm, Transform, ExportFormat, Rgb};
use bareline_document::{DocumentSnapshot, EditTransaction, TextOffset};
use bareline_diff::CancelToken;
use bareline_platform::LocalFileSystem;
use std::sync::{Arc, mpsc, atomic::{AtomicBool, Ordering}};

enum UtilityResult { Text(String), Edit(DocumentSnapshot,EditTransaction) }
pub(super) struct UtilitiesRuntime {
    pending: Option<mpsc::Receiver<Result<UtilityResult,String>>>,
    cancel: CancelToken,
    print_cancel: Arc<AtomicBool>,
    pub(super) print_options: bareline_platform::printing::PrintOptions,
    pub(super) result: Option<String>,
}
impl Default for UtilitiesRuntime {
    fn default()->Self {Self{pending:None,cancel:CancelToken::default(),print_cancel:Arc::new(AtomicBool::new(false)),print_options:Default::default(),result:None}}
}
impl Drop for UtilitiesRuntime { fn drop(&mut self){self.cancel.cancel();self.print_cancel.store(true,Ordering::Release);} }
pub(super) fn register(registry:&mut bareline_commands::CommandRegistry) {
    core::register_commands(registry);
    for (id,title) in [("utilities.cancel","Cancel utility operation"),("utilities.copyResult","Copy utility result"),("utilities.print","Print document…"),("utilities.printSelection","Print selection…")] {
        let id=bareline_commands::CommandId(id);
        let _=registry.register(bareline_commands::CommandSpec{id,title,category:"Utilities",shortcut:"",action:Action::Contributed(id)});
    }
}
impl Shell {
    pub(super) fn utilities_dispatch(&mut self,_el:&ActiveEventLoop,id:&str)->bool {
        if !id.starts_with("utilities.") {return false;}
        if id=="utilities.cancel" {self.utilities.cancel.cancel();self.utilities.print_cancel.store(true,Ordering::Release);return true;}
        if id=="utilities.copyResult" {if let (Some(platform),Some(result))=(&self.platform,&self.utilities.result){let _=platform.set_clipboard_text(result);}return true;}
        let Some(workspace)=&mut self.workspace else{return true;};
        if self.utilities.pending.is_some(){workspace.message=Some("A utility is running; cancel it before starting another".into());return true;}
        let Some(editor)=workspace.editors.get(self.app.active) else{return true;};
        let selection=editor.selection;
        let snapshot=editor.snapshot().clone();
        let paged=match editor {bareline_app::workspace::WorkspaceEditor::Paged(p)=>Some((p.read_handle(),p.viewport_start().0)),_=>None};
        let offset=paged.as_ref().map_or(0,|(_,offset)|*offset);
        let length=paged.as_ref().map_or(snapshot.len(),|(source,_)|source.snapshot().len());
        let selected=selection.anchor!=selection.caret;
        let range=if selected {TextOffset(offset+selection.anchor.min(selection.caret))..TextOffset(offset+selection.anchor.max(selection.caret))}else{TextOffset(0)..TextOffset(length)};
        let language=editor.language;
        let title=workspace.titles().get(self.app.active).cloned().unwrap_or_else(||"Document".into());
        let algorithm=match id {"utilities.md5"=>Some(HashAlgorithm::Md5),"utilities.sha1"=>Some(HashAlgorithm::Sha1),"utilities.sha256"=>Some(HashAlgorithm::Sha256),"utilities.sha512"=>Some(HashAlgorithm::Sha512),_=>None};
        let transform=match id {"utilities.base64Encode"=>Some(Transform::Base64Encode),"utilities.base64Decode"=>Some(Transform::Base64Decode),"utilities.urlEncode"=>Some(Transform::UrlEncode),"utilities.urlDecode"=>Some(Transform::UrlDecode),_=>None};
        if transform.is_some() && (editor.read_only()||editor.busy()) {workspace.message=Some("The destination is read-only or has an edit pending".into());return true;}
        let export=match id {"utilities.exportHtml"=>Some(ExportFormat::Html),"utilities.exportRtf"=>Some(ExportFormat::Rtf),_=>None};
        let printing=matches!(id,"utilities.print"|"utilities.printSelection");
        let print_selection=id=="utilities.printSelection";
        if id=="utilities.printSelection"&&!selected {workspace.message=Some("Select text before printing a selection".into());return true;}
        if paged.is_some() && algorithm.is_none() {workspace.message=Some("This utility needs a full paged worker adapter; the source remains unchanged".into());return true;}
        let destination=if export.is_some(){match self.platform.as_ref().map(|p|p.save_file()){Some(Ok(Some(path)))=>Some(path),Some(Err(e))=>{workspace.message=Some(e);return true;},_=>return true}}else{None};
        let printer=if printing {match bareline_platform_windows::printing::choose_printer(){Ok(Some(p))=>Some(p),Ok(None)=>return true,Err(e)=>{workspace.message=Some(format!("Print unavailable: {e:?}. Retry Print to choose another printer."));return true;}}}else{None};
        if algorithm.is_none()&&transform.is_none()&&export.is_none()&&!printing&&id!="utilities.statistics" {return false;}
        let mut print_options=self.utilities.print_options.clone();print_options.title=title;
        let colors=[Rgb(86,156,214),Rgb(206,145,120),Rgb(181,206,168),Rgb(106,153,85),Rgb(212,212,212)];
        self.utilities.cancel=CancelToken::default();self.utilities.print_cancel=Arc::new(AtomicBool::new(false));
        let cancel=self.utilities.cancel.clone();let print_cancel=self.utilities.print_cancel.clone();
        let (send,receive)=mpsc::sync_channel(1);let notify=self.notify.clone();
        let spawned=std::thread::Builder::new().name("bareline-utility".into()).spawn(move||{
            let result=(||->Result<UtilityResult,String>{
                if let Some(algorithm)=algorithm {
                    let result=if let Some((source,_))=paged {let reader=core::PagedTextReader::new(source,range,cancel.clone()).map_err(|e|format!("{e:?}"))?;core::hash_reader(reader,algorithm,&cancel,|_|true)}else{core::hash_snapshot(&snapshot,range,algorithm,&cancel)}.map_err(|e|format!("{e:?}"))?;
                    return Ok(UtilityResult::Text(format!("{} · {} bytes\n{}",algorithm.label(),result.bytes,result.hexadecimal)));
                }
                if let Some(kind)=transform {let transaction=core::transform(&snapshot,range,kind,16*1024*1024,&cancel).map_err(|e|format!("{e:?}"))?;return Ok(UtilityResult::Edit(snapshot,transaction));}
                if let (Some(format),Some(path))=(export,destination) {
                    publish_export(&path,|writer|core::export_language(&snapshot,language,Rgb(32,32,32),colors,format,writer,&cancel))?;
                    return Ok(UtilityResult::Text(format!("Exported {}",path.display())));
                }
                if let Some(printer)=printer {
                    let job=bareline_platform_windows::printing::WindowsPrintJob::start(printer,print_options).map_err(|e|format!("{e:?}"))?;
                    let range=if print_selection {range}else{TextOffset(0)..TextOffset(snapshot.len())};
                    let summary=core::print_snapshot(&snapshot,range,language,colors,Box::new(job),&print_cancel).map_err(|e|format!("{e:?}"))?;
                    return Ok(UtilityResult::Text(format!("Sent {} pages / {} lines to the printer",summary.pages,summary.lines)));
                }
                let s=core::statistics(&snapshot,256*1024,&cancel,|_|true).map_err(|e|format!("{e:?}"))?;
                Ok(UtilityResult::Text(format!("{} bytes · {} characters · {} graphemes · {} words · {} lines",s.bytes,s.characters,s.graphemes,s.words,s.lines)))
            })();let _=send.send(result);notify();
        });
        match spawned {Ok(_)=>{self.utilities.pending=Some(receive);workspace.message=Some("Utility running · Cancel utility operation stops the worker".into());},Err(e)=>workspace.message=Some(format!("Could not start utility: {e}"))}
        true
    }
    pub(super) fn utilities_pump(&mut self,_el:&ActiveEventLoop) {
        let Some(receiver)=&self.utilities.pending else{return;};
        let result=match receiver.try_recv(){Ok(r)=>r,Err(mpsc::TryRecvError::Empty)=>return,Err(_)=>Err("Utility worker stopped; retry".into())};
        self.utilities.pending=None;
        let Some(workspace)=&mut self.workspace else{return;};
        let message=match result {
            Ok(UtilityResult::Text(text))=>{self.utilities.result=Some(text.clone());text},
            Ok(UtilityResult::Edit(snapshot,transaction))=>match workspace.editors.iter_mut().find(|e|!e.paged()&&e.snapshot().same_document(&snapshot)) {
                Some(editor)=>match editor.apply_prepared(&snapshot,transaction){Ok(())=>"Transformation queued as one undoable edit".into(),Err(e)=>format!("Transformation not applied: {e}")},
                None=>"Destination closed; transformation not applied".into(),
            },
            Err(e)=>format!("Utility stopped: {e}. Source preserved; retry the command."),
        };
        workspace.message=Some(message);if let Some(window)=&self.window{window.request_redraw();}
    }
}
fn publish_export(path:&std::path::Path,write:impl FnOnce(&mut std::fs::File)->Result<(),core::UtilityError>)->Result<(),String> {
    use std::io::Write;
    let platform=bareline_platform_windows::WindowsFileSystem;
    platform.validate_target(path).map_err(|e|e.to_string())?;
    let parent=path.parent().ok_or("Export destination has no parent")?;
    let _guard=platform.guard_directory(parent).map_err(|e|e.to_string())?;
    let existed=path.exists();
    static NEXT:std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(1);
    let stage=parent.join(format!(".bareline-export-{}-{}.tmp",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
    let mut owned=false;
    let result=(|| {let mut file=std::fs::OpenOptions::new().create_new(true).write(true).open(&stage).map_err(|e|e.to_string())?;owned=true;write(&mut file).map_err(|e|format!("{e:?}"))?;file.flush().and_then(|_|file.sync_all()).map_err(|e|e.to_string())?;drop(file);platform.commit(&stage,path,existed).map_err(|e|e.to_string())})();
    if result.is_err()&&owned{let _=std::fs::remove_file(&stage);}result
}
