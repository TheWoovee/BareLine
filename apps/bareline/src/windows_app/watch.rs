// SPDX-License-Identifier: MPL-2.0
//! Native composition only. Bounded background metadata reconciliation never replaces buffers.
use super::*;
use bareline_platform::{FileIdentity, LocalFileSystem, PathOrigin, PathTrustProvider};
use bareline_platform_windows::{WindowsFileSystem, WindowsPathTrustProvider, WindowsWatchService};
use std::{
    collections::{BTreeSet, VecDeque},
    sync::mpsc::{self, Receiver},
};

type Registration = Result<Option<WindowsWatchService>, String>;
type Checked = (PathBuf, FileIdentity, Result<bool, String>);
pub(super) fn draw_banner(editor: &bareline_app::workspace::WorkspaceEditor, bounds: bareline_renderer::Rect, ops: &mut Vec<bareline_renderer::DrawOp>) -> Vec<(bareline_renderer::Rect, bareline_commands::CommandId)> {
    use bareline_renderer::DrawOp;
    use bareline_ui::{rect, text, ACCENT, CHROME, TEXT};
    let bareline_app::workspace::WorkspaceEditor::Paged(editor) = editor else { return Vec::new(); };
    let Some((paused, changed)) = editor.follow_status() else { return Vec::new(); };
    let changed = changed || editor.source_changed();
    let banner = rect(bounds.x + 8.0, bounds.y + 4.0, (bounds.width - 16.0).max(0.0), 34.0);
    ops.push(DrawOp::FillRounded(banner, CHROME, 4.0));
    ops.push(DrawOp::StrokeRounded(banner, ACCENT, 4.0, 1.0));
    let actions = if changed { vec![("Reopen and follow", "file.monitor.reopen"), ("Unlock captured content", "file.monitor.unlock")] }
        else { vec![(if paused { "Resume ↓" } else { "Pause" }, if paused { "file.monitor.resume" } else { "file.monitor.pause" }), ("Unlock to edit", "file.monitor.unlock")] };
    let action_width = (if changed { 175.0_f32 } else { 135.0_f32 }).min(((banner.width - 16.0) / 2.0).max(0.0));
    let actions_x = (banner.x + banner.width - action_width * 2.0 - 8.0).max(banner.x + 8.0);
    ops.push(DrawOp::PushClip(rect(banner.x + 12.0, banner.y, (actions_x - banner.x - 20.0).max(0.0), banner.height)));
    let name = editor.path.display();
    let label = if changed { format!("{name} · Source changed") } else { format!("Following {name} · {}", if paused { "Paused (scrolled up)" } else { "Following new content" }) };
    text(ops, banner.x + 12.0, banner.y + 8.0, label, 14.0, TEXT); ops.push(DrawOp::PopClip);
    let mut hits = Vec::new();
    for (index, (label, command)) in actions.into_iter().enumerate() {
        let bounds = rect(actions_x + index as f32 * action_width, banner.y, action_width, banner.height);
        ops.push(DrawOp::PushClip(bounds)); text(ops, bounds.x + 6.0, bounds.y + 8.0, label, 14.0, ACCENT); ops.push(DrawOp::PopClip);
        hits.push((bounds, bareline_commands::CommandId(command)));
    }
    hits
}
#[derive(Default)]
pub(super) struct WatchRuntime {
    pub(super) hits: Vec<(bareline_renderer::Rect,u32,usize,bareline_commands::CommandId)>,
    service: Option<WindowsWatchService>,
    setup: Option<Receiver<Registration>>,
    registered: Vec<PathBuf>,
    desired: Vec<PathBuf>,
    queue: VecDeque<(PathBuf, FileIdentity)>,
    checking: Option<Receiver<Vec<Checked>>>,
    conflicts: BTreeSet<PathBuf>,
    requested: bool,
    auto_reload_clean: bool,
    reopen_follow: BTreeSet<PathBuf>,
    reopen_pane: std::collections::BTreeMap<PathBuf,u32>,
    remote_wake: Option<Receiver<()>>,
    remote_follow: std::collections::BTreeMap<PathBuf,(bareline_platform::RemoteReadGrant,std::sync::Arc<dyn LocalFileSystem>)>,
}
pub(super) fn commands() -> Vec<bareline_commands::CommandSpec> {
    [
        ("file.remote.open", "Open Remote File with Permission…"),
        ("file.remote.reload", "Reload Remote File with Permission…"),
        ("file.remote.follow", "Follow Remote File with Permission…"),
        ("file.external.auto_reload", "Automatically Reload Clean Local Files (This Session)"),
        ("file.external.check", "Check for External Changes"),
        ("file.external.keep", "Keep Current Buffer"),
        ("file.external.reload", "Reload External Changes"),
        ("file.monitor.start", "Follow New Content"),
        ("file.monitor.pause", "Pause Following Scroll"),
        ("file.monitor.resume", "Resume Following"),
        ("file.monitor.reopen", "Reopen and Follow"),
        ("file.monitor.unlock", "Unlock to Edit (Stop Monitoring)"),
    ]
    .into_iter()
    .map(|(id, title)| bareline_commands::CommandSpec {
        id: bareline_commands::CommandId(id),
        title,
        category: "File",
        shortcut: "",
        action: Action::Contributed(bareline_commands::CommandId(id)),
    })
    .collect()
}
impl Shell {
    pub(super) fn watch_start_follow(&mut self, index: usize) -> Result<(), String> {
        let w = self.workspace.as_mut().ok_or("Workspace unavailable")?;
        if self.views.pane()==1 { if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor))=&mut self.views.secondary{return editor.start_follow(std::sync::Arc::new(WindowsFileSystem));} }
        if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) = w.editors.get_mut(index) {
            return editor.start_follow(std::sync::Arc::new(WindowsFileSystem));
        }
        let editor = w.editors.get_mut(index).ok_or("Document unavailable")?;
        if editor.dirty() { return Err("Save or discard edits before monitoring.".into()); }
        let path = w.path(index).ok_or("Save this file before monitoring")?.to_owned();
        let was_read_only = w.editors[index].user_read_only; w.editors[index].user_read_only = false;
        let threshold = w.resident_max_bytes; w.resident_max_bytes = 0;
        let result = w.reload(index, false); w.resident_max_bytes = threshold;
        w.editors[index].user_read_only = was_read_only;
        result?;
        self.watch.reopen_pane.insert(path.clone(),self.views.pane());
        self.watch.reopen_follow.insert(path);
        Ok(())
    }
    pub(super) fn watch_sync(&mut self) {
        let mut paths = self.workspace_watch_roots();
        if let Some(w) = &self.workspace {
            for index in 0..w.editors.len() {
                if let Some(path) = w.path(index)
                    && let Some(parent) = path.parent()
                {
                    if !self.watch.remote_follow.contains_key(path) && !is_network_path(path){paths.push(parent.to_owned());}
                }
            }
        }
        paths.retain(|path|!is_network_path(path));
        paths.sort();
        paths.dedup();
        if paths.len() > 63 {
            paths.truncate(63);
            if let Some(w) = &mut self.workspace { w.message = Some("Watching 63 directories. Additional directories reconcile on focus or Check for External Changes.".into()); }
        }
        self.watch.desired = paths;
        if self.watch.setup.is_some() || self.watch.desired == self.watch.registered {
            return;
        }
        let desired = self.watch.desired.clone();
        let previous = self.watch.service.take();
        let notify = self.notify.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("bareline-watch-setup".into())
            .spawn(move || {
                drop(previous);
                let result = if desired.is_empty() {
                    Ok(None)
                } else {
                    WindowsWatchService::start_notifying(desired, notify.clone())
                        .map(Some)
                        .map_err(|e| e.to_string())
                };
                let _ = tx.send(result);
                notify();
            }) {
            Ok(_) => {
                self.watch.registered = self.watch.desired.clone();
                self.watch.setup = Some(rx);
            }
            Err(e) => {
                if let Some(w) = &mut self.workspace {
                    w.message = Some(format!("File watching unavailable: {e}"));
                }
            }
        }
    }
    pub(super) fn watch_dispatch(&mut self, el: &ActiveEventLoop, id: &str) -> bool {
        match id {
            "file.remote.open"|"file.remote.reload"|"file.remote.follow" => { self.watch_remote_request(el,id); true }
            "file.monitor.start" => {
                if let Err(error) = self.watch_start_follow(self.app.active) { if let Some(w) = &mut self.workspace { w.message = Some(error); } }
                true
            }
            "file.monitor.pause" | "file.monitor.resume" => {
                if let Some(w) = &mut self.workspace && let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) = self.views.active_workspace_editor_mut(w,self.app.active) {
                    editor.set_follow_paused(id == "file.monitor.pause");
                    if id == "file.monitor.resume" { let _ = editor.request_viewport(bareline_document::TextOffset(editor.snapshot().len().saturating_sub(32768))); }
                }
                true
            }
            "file.monitor.unlock" => {
                let confirmed = self.platform.as_ref().is_some_and(|p| p.confirm_discard_document("Stop monitoring and capture a fixed generation for editing?"));
                if confirmed && let Some(w) = &mut self.workspace && let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor)) = self.views.active_workspace_editor_mut(w,self.app.active) {
                    if let Err(error) = editor.unlock_follow(true) { w.message = Some(error); }
                }
                true
            }
            "file.monitor.reopen" => {
                if self.workspace.as_ref().and_then(|w|w.path(self.app.active)).is_some_and(|path|self.watch.remote_follow.contains_key(path)||is_network_path(path)){self.watch_remote_request(el,"file.remote.follow");return true;}
                if let Some(w) = &mut self.workspace && let Some(editor) = w.editors.get_mut(self.app.active) {
                    let was_read_only = editor.user_read_only; editor.user_read_only = false;
                    let path = w.path(self.app.active).map(|p| p.to_owned());
                    let result = w.reload(self.app.active, false);
                    w.editors[self.app.active].user_read_only = was_read_only;
                    match result {
                        Err(error) => { w.message = Some(error); }
                        Ok(()) => { if let Some(path) = path { self.watch.reopen_pane.insert(path.clone(),self.views.pane());self.watch.reopen_follow.insert(path); } }
                    }
                }
                true
            }
            "file.external.auto_reload" => {self.watch.auto_reload_clean=!self.watch.auto_reload_clean;true}
            "file.external.check" => {
                self.watch.requested = true;
                if self.watch.service.is_none() && self.watch.setup.is_none() {
                    self.watch.registered.clear();
                }
                (self.notify)();
                true
            }
            "file.external.reload" => {
                let active = self.app.active;
                let dirty = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.editors.get(active))
                    .is_some_and(|e| e.dirty());
                let name = self
                    .workspace
                    .as_ref()
                    .and_then(|w| w.path(active))
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "document".into());
                let confirmed = !dirty
                    || self
                        .platform
                        .as_ref()
                        .is_some_and(|p| p.confirm_discard_document(&name));
                if confirmed
                    && let Some(w) = &mut self.workspace
                    && let Err(e) = w.reload(active, dirty)
                {
                    w.message = Some(e);
                }
                true
            }
            "file.external.keep" => {
                if let Some(w) = &mut self.workspace {
                    if let Some(path) = w.path(self.app.active) {
                        self.watch.conflicts.remove(path);
                    }
                    w.message=Some("Current buffer kept. Save will still check the disk version; use Save As to preserve both.".into());
                }
                true
            }
            _ => false,
        }
    }
    pub(super) fn watch_pump(&mut self, _: &ActiveEventLoop) {
        let mut open: BTreeSet<_> = self.workspace.as_ref().map(|w|(0..w.editors.len()).filter(|&i|w.path(i).is_some_and(|p|w.path_loading(p)||self.watch.reopen_follow.contains(p))||w.document_busy(i)||matches!(&w.editors[i],bareline_app::workspace::WorkspaceEditor::Paged(e) if e.follow_status().is_some())).filter_map(|i|w.path(i).map(|p|p.to_owned())).collect()).unwrap_or_default();
        if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor))=&self.views.secondary{if editor.follow_status().is_some()||editor.busy(){open.insert(editor.path.clone());}}
        self.watch.remote_follow.retain(|path,(grant,_)|{if !open.contains(path){grant.revoke();false}else{true}});
        if self.watch.remote_wake.as_ref().is_some_and(|rx|rx.try_recv().is_ok()){self.watch.remote_wake=None;self.watch.requested=true;}
        if !self.watch.remote_follow.is_empty() && self.watch.remote_wake.is_none(){
            let(tx,rx)=mpsc::sync_channel(1);let notify=self.notify.clone();
            if std::thread::Builder::new().name("bareline-remote-follow-wake".into()).spawn(move||{std::thread::sleep(std::time::Duration::from_secs(1));let _=tx.send(());notify();}).is_ok(){self.watch.remote_wake=Some(rx);}
        }
        let pending: Vec<_> = self.watch.reopen_follow.iter().cloned().collect();
        for path in pending {
            if !self.workspace.as_ref().is_some_and(|w|(0..w.editors.len()).any(|i|w.path(i)==Some(path.as_path()))){self.watch.reopen_follow.remove(&path);continue;}
            let index = self.workspace.as_ref().and_then(|w| (0..w.editors.len()).find(|&i| w.path(i) == Some(path.as_path()) && matches!(&w.editors[i], bareline_app::workspace::WorkspaceEditor::Paged(e) if e.follow_status().is_none() && !e.busy())));
            if let Some(index) = index {
                let provider=self.watch.remote_follow.get(&path).map(|(_,fs)|fs.clone()).unwrap_or_else(||std::sync::Arc::new(WindowsFileSystem));
                let result=if self.watch.reopen_pane.get(&path)==Some(&1){
                    if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor))=&mut self.views.secondary{if editor.path!=path||editor.busy()||editor.follow_status().is_some(){continue;}editor.start_follow(provider)}else{continue;}
                }else if let Some((_,fs))=self.watch.remote_follow.get(&path){
                    if let Some(w)=&mut self.workspace && let bareline_app::workspace::WorkspaceEditor::Paged(editor)=&mut w.editors[index]{editor.start_follow(fs.clone())}else{Err("Paged follow view unavailable".into())}
                }else{if let Some(w)=&mut self.workspace && let bareline_app::workspace::WorkspaceEditor::Paged(editor)=&mut w.editors[index]{editor.start_follow(provider)}else{Err("Paged follow view unavailable".into())}};
                if result.is_ok(){self.watch.reopen_follow.remove(&path);self.watch.reopen_pane.remove(&path);}
                else if let Err(error)=result{self.watch.reopen_follow.remove(&path);if let Some((grant,_))=self.watch.remote_follow.remove(&path){grant.revoke();}if let Some(w)=&mut self.workspace{w.message=Some(error);}}
            }
        }
        if let Some(rx) = &self.watch.setup {
            match rx.try_recv() {
                Ok(result) => {
                    self.watch.setup = None;
                    match result {
                        Ok(service) => {
                            self.watch.service = service;
                            self.watch.requested = true;
                        }
                        Err(e) => {
                            if let Some(w) = &mut self.workspace {
                                w.message = Some(format!(
                                    "File watching unavailable: {e}. Check for External Changes remains available."
                                ));
                            }
                        }
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.watch.setup = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut events = Vec::new();
        if let Some(service) = &self.watch.service {
            for _ in 0..256 {
                let Some(event) = service.try_recv() else {
                    break;
                };
                events.push(event);
                self.watch.requested = true;
            }
        }
        for event in events { self.workspace_watch_event(&event); }
        if let Some(w) = &mut self.workspace {
            for editor in &mut w.editors {
                if let bareline_app::workspace::WorkspaceEditor::Paged(editor) = editor {
                    if let Err(error) = editor.follow_tick(self.watch.remote_follow.get(&editor.path).map(|(_,fs)|fs.clone()).unwrap_or_else(||std::sync::Arc::new(WindowsFileSystem)), self.watch.requested) { editor.error = Some(error); }
                }
            }
        }
        if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor))=&mut self.views.secondary {
            let platform=self.watch.remote_follow.get(&editor.path).map(|(_,fs)|fs.clone()).unwrap_or_else(||std::sync::Arc::new(WindowsFileSystem));
            if let Err(error)=editor.follow_tick(platform,self.watch.requested){editor.error=Some(error);}
        }        if let Some(rx) = &self.watch.checking {
            match rx.try_recv() {
                Ok(results) => {
                    self.watch.checking = None;
                    if let Some(w) = &mut self.workspace {
                        for (path, expected, result) in results {
                            let index = (0..w.editors.len()).find(|&i| {
                                w.path(i) == Some(path.as_path())
                                    && w.fingerprint(i).is_some_and(|f| f.identity == expected)
                            });
                            if let Some(index) = index
                                && !matches!(&w.editors[index], bareline_app::workspace::WorkspaceEditor::Paged(e) if e.follow_status().is_some())
                                && (result.as_ref().is_err() || result == Ok(true))
                            {
                                if should_auto_reload(self.watch.auto_reload_clean,w.editors[index].dirty(),w.editors[index].read_only(),w.editors[index].busy(),result==Ok(true),is_network_path(&path)) && w.reload(index,false).is_ok(){
                                    self.watch.conflicts.remove(&path);continue;
                                }
                                self.watch.conflicts.insert(path.clone());
                                let name = path.display();
                                w.message = Some(if w.editors[index].dirty() {
                                    format!(
                                        "{name} changed outside Bareline. Your edits are preserved. Keep Current Buffer, Save As, or explicitly reload."
                                    )
                                } else {
                                    format!(
                                        "{name} changed or became unavailable outside Bareline. Current bytes are preserved; choose Keep Current Buffer or explicitly reload."
                                    )
                                });
                            }
                        }
                    }
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.watch.checking = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.watch.requested && self.watch.queue.is_empty() && self.watch.checking.is_none() {
            self.watch.requested = false;
            if let Some(w) = &self.workspace {
                for i in 0..w.editors.len() {
                    if let (Some(path), Some(f)) = (w.path(i), w.fingerprint(i)) {
                        self.watch.queue.push_back((path.to_owned(), f.identity));
                    }
                }
            }
        }
        if self.watch.checking.is_none() && !self.watch.queue.is_empty() {
            let batch: Vec<_> = (0..16)
                .filter_map(|_| self.watch.queue.pop_front())
                .collect();
            let notify = self.notify.clone();
            let (tx, rx) = mpsc::sync_channel(1);
            match std::thread::Builder::new()
                .name("bareline-watch-check".into())
                .spawn(move || {
                    let results = batch
                        .into_iter()
                        .map(|(path, expected)| {
                            let result = WindowsPathTrustProvider
                                .open_read(&path, PathOrigin::User)
                                .and_then(|opened| WindowsFileSystem.identity(&opened.file))
                                .map(|actual| actual != expected)
                                .map_err(|e| e.to_string());
                            (path, expected, result)
                        })
                        .collect();
                    let _ = tx.send(results);
                    notify();
                }) {
                Ok(_) => self.watch.checking = Some(rx),
                Err(e) => {
                    if let Some(w) = &mut self.workspace {
                        w.message = Some(format!("External-change check unavailable: {e}"));
                    }
                }
            }
        }
        self.watch_sync();
    }
}

impl Shell {
    pub(super) fn watch_scroll_away(&mut self,index:usize){
        if let Some(w)=&mut self.workspace && let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor))=w.editors.get_mut(index){
            if editor.follow_status().is_some(){editor.set_follow_paused(true);}
        }
    }
    pub(super) fn watch_focus_check(&mut self){self.watch.requested=true;(self.notify)();}
    pub(super) fn watch_annotate_context(&self,context:&mut bareline_commands::CommandContext){
        use bareline_commands::{CommandId,CommandState};
        context.states.insert(CommandId("file.external.auto_reload"),CommandState{checked:self.watch.auto_reload_clean,..Default::default()});
        let editor=self.workspace.as_ref().and_then(|w|self.views.active_workspace_editor(w,self.app.active));
        let path=self.workspace.as_ref().and_then(|w|w.path(self.app.active));
        let source_changed=matches!(editor,Some(bareline_app::workspace::WorkspaceEditor::Paged(e)) if e.source_changed());
        let follow=editor.and_then(|e|if let bareline_app::workspace::WorkspaceEditor::Paged(e)=e{e.follow_status().map(|(paused,changed)|(paused,changed||e.source_changed()))}else{None});
        let busy=editor.is_some_and(|e|e.busy());
        for(id,enabled,reason)in[
            ("file.remote.reload",path.is_some()&&!busy,"Open a document and wait for current work"),
            ("file.remote.follow",path.is_some()&&!busy&&(follow.is_none()||follow.is_some_and(|(_,changed)|changed))&&!editor.is_some_and(|e|e.dirty()),"Save edits and wait before following"),
            ("file.external.keep",path.is_some_and(|p|self.watch.conflicts.contains(p))||source_changed,"No external change to keep"),
            ("file.external.reload",path.is_some()&&!busy,"Save this document first or wait for its current operation"),
            ("file.monitor.start",path.is_some()&&!busy&&follow.is_none()&&!editor.is_some_and(|e|e.dirty()),"Save edits and wait for current work before following"),
            ("file.monitor.pause",follow==Some((false,false)),"No active following scroll to pause"),
            ("file.monitor.resume",follow==Some((true,false)),"Following is not paused, or the source changed"),
            ("file.monitor.reopen",follow.is_some_and(|(_,changed)|changed)&&!busy,"The monitored source has not changed, or work is pending"),
            ("file.monitor.unlock",follow.is_some()&&!busy,"No monitored generation ready to unlock"),
        ]{if !enabled{context.states.insert(CommandId(id),CommandState::disabled(reason));}}
    }
}
impl WatchRuntime {
    pub(super) fn draw_banner(&self,w:&bareline_app::workspace::Workspace,index:usize,bounds:bareline_renderer::Rect,ops:&mut Vec<bareline_renderer::DrawOp>)->Vec<(bareline_renderer::Rect,bareline_commands::CommandId)>{
        let Some(editor)=w.editors.get(index) else{return Vec::new();};
        let follow=draw_banner(editor,bounds,ops);if !follow.is_empty(){return follow;}
        let source_changed=matches!(editor,bareline_app::workspace::WorkspaceEditor::Paged(e) if e.source_changed());
        let Some(path)=w.path(index).filter(|p|self.conflicts.contains(*p)||source_changed) else{return Vec::new();};
        use bareline_renderer::DrawOp;use bareline_ui::{rect,text,ACCENT,CHROME,TEXT};
        let banner=rect(bounds.x+8.0,bounds.y+4.0,(bounds.width-16.0).max(0.0),62.0);
        ops.push(DrawOp::FillRounded(banner,CHROME,4.0));ops.push(DrawOp::StrokeRounded(banner,ACCENT,4.0,1.0));
        ops.push(DrawOp::PushClip(banner));text(ops,banner.x+10.0,banner.y+6.0,format!("{} · Source changed; current bytes preserved",path.display()),13.0,TEXT);ops.push(DrawOp::PopClip);
        let action_width=((banner.width-16.0)/4.0).clamp(0.0,110.0);
        let mut hits=Vec::new();for(i,(label,id))in[("Compare","compare.external"),("Reload","file.external.reload"),("Keep editing","file.external.keep"),("Save As…","file.save_as")].into_iter().enumerate(){
            let hit=rect(banner.x+8.0+i as f32*action_width,banner.y+30.0,action_width,28.0);ops.push(DrawOp::PushClip(hit));text(ops,hit.x+4.0,hit.y+5.0,label,13.0,ACCENT);ops.push(DrawOp::PopClip);hits.push((hit,bareline_commands::CommandId(id)));
        }hits
    }
}

fn is_network_path(path:&std::path::Path)->bool{matches!(path.components().next(),Some(std::path::Component::Prefix(p)) if matches!(p.kind(),std::path::Prefix::UNC(..)|std::path::Prefix::VerbatimUNC(..)))}
impl Drop for WatchRuntime{fn drop(&mut self){for(grant,_)in self.remote_follow.values(){grant.revoke();}}}
impl Shell{
    fn watch_remote_request(&mut self,el:&ActiveEventLoop,id:&str){
        if !self.ensure_workspace(el){return;}
        use bareline_platform::{RemoteReadGrant,RemoteReadAction};
        let action=match id{"file.remote.open"=>RemoteReadAction::Open,"file.remote.reload"=>RemoteReadAction::Reload,_=>RemoteReadAction::Follow};
        let path=if action==RemoteReadAction::Open{match self.platform.as_ref().map(|p|p.open_file()){Some(Ok(Some(path)))=>path,_=>return}}else{let Some(path)=self.workspace.as_ref().and_then(|w|w.path(self.app.active)).map(|p|p.to_owned())else{return;};path};
        if !WindowsWatchService::confirm_remote_read(&path,action){return;}
        let grant=match RemoteReadGrant::after_consent(path.clone(),action,std::time::Duration::from_secs(60)){Ok(grant)=>grant,Err(error)=>{if let Some(w)=&mut self.workspace{w.message=Some(error.to_string());}return;}};
        let dirty=self.workspace.as_ref().and_then(|w|w.editors.get(self.app.active)).is_some_and(|e|e.dirty());
        if action==RemoteReadAction::Reload && dirty && !self.platform.as_ref().is_some_and(|p|p.confirm_discard_document(&path.display().to_string())){grant.revoke();return;}
        if action==RemoteReadAction::Follow && self.views.pane()==1 {
            if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor))=&mut self.views.secondary && editor.follow_status().is_none(){
                let result=grant.claim(&path,action,std::sync::Arc::new(||false)).map_err(|e|e.to_string()).and_then(|access|WindowsFileSystem.scoped_remote_read(access).map_err(|e|e.to_string())).and_then(|fs|{editor.start_follow(fs.clone())?;Ok(fs)});
                match result{Ok(fs)=>{if let Some((old,_))=self.watch.remote_follow.insert(path,(grant,fs)){old.revoke();}},Err(error)=>{grant.revoke();if let Some(w)=&mut self.workspace{w.message=Some(error);}}}return;
            }
        }        let Some(w)=&mut self.workspace else{grant.revoke();return;};
        let result=match action{
            RemoteReadAction::Open=>w.open_authorized(path,grant),
            RemoteReadAction::Reload=>w.reload_authorized(self.app.active,dirty,grant),
            RemoteReadAction::Follow=>w.follow_authorized(self.app.active,grant.clone()).map(|fs|{if w.path_loading(&path){self.watch.reopen_pane.insert(path.clone(),self.views.pane());self.watch.reopen_follow.insert(path.clone());}if let Some((old,_))=self.watch.remote_follow.insert(path,(grant,fs)){old.revoke();}}),
        };if let Err(error)=result{w.message=Some(error);}
    }
}
fn should_auto_reload(enabled:bool,dirty:bool,read_only:bool,busy:bool,changed:bool,remote:bool)->bool{enabled&&!dirty&&!read_only&&!busy&&changed&&!remote}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn automatic_reload_never_discards_dirty_or_unavailable_source(){
        assert!(should_auto_reload(true,false,false,false,true,false));
        for(dirty,read_only,busy,changed,remote)in[(true,false,false,true,false),(false,true,false,true,false),(false,false,true,true,false),(false,false,false,false,false),(false,false,false,true,true)]{assert!(!should_auto_reload(true,dirty,read_only,busy,changed,remote));}
        assert!(!should_auto_reload(false,false,false,false,true,false));
    }
    #[test]fn network_path_classification_is_lexical(){assert!(is_network_path(std::path::Path::new(r"\\untrusted.example\share\log")));assert!(!is_network_path(std::path::Path::new(r"C:\local\log")));}
}
impl Shell {
    pub(super) fn watch_event(&mut self,el:&ActiveEventLoop,event:&WindowEvent)->bool{
        if self.palette.open{return false;}
        if matches!(event,WindowEvent::MouseInput{state:ElementState::Pressed,button:MouseButton::Left,..}){
            if let Some((_,pane,index,id))=self.watch.hits.iter().find(|(bounds,_,_,_)|bounds.contains(self.pointer)).copied(){
                if let Some(w)=&self.workspace{if !self.views.activate_watch_pane(w,&mut self.app,pane){return true;}}
                self.app.active=index;
                if let Some(action)=self.app.commands.dispatch_in(id,&self.command_context()){self.dispatch(el,action);}
                if let Some(window)=&self.window{window.request_redraw();}return true;
            }
        }
        let away=match event{WindowEvent::MouseWheel{delta:MouseScrollDelta::LineDelta(_,y),..}=>*y>0.0,WindowEvent::MouseWheel{delta:MouseScrollDelta::PixelDelta(p),..}=>p.y>0.0,WindowEvent::KeyboardInput{event,..}=>event.state==ElementState::Pressed&&matches!(event.logical_key,Key::Named(NamedKey::PageUp|NamedKey::Home|NamedKey::ArrowUp)),_=>false};
        if away&&!self.modifiers.control_key(){if let Some(w)=&mut self.workspace{if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor))=self.views.active_workspace_editor_mut(w,self.app.active){if editor.follow_status().is_some(){editor.set_follow_paused(true);}}}}
        false
    }
}