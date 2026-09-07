// SPDX-License-Identifier: MPL-2.0
use super::*;
#[derive(Default)] pub(super) struct ShellIntegrationRuntime { tray:Option<bareline_platform_windows::shell_integration::TrayIcon>, pub keep_in_tray:bool, recent:std::collections::BTreeSet<PathBuf>, pub portable:bool }
pub(super) fn commands()->Vec<bareline_commands::CommandSpec>{[("file.reveal","Open Containing Folder"),("file.terminal","Open Terminal Here"),("tray.toggle","Keep Running in Tray"),("tray.hide","Minimize to Tray"),("tray.restore","Restore Window")].into_iter().map(|(id,title)|bareline_commands::CommandSpec{id:bareline_commands::CommandId(id),title,category:"File",shortcut:"",action:Action::Contributed(bareline_commands::CommandId(id))}).collect()}
impl Shell {
    pub(super) fn shell_integration_command(&mut self,id:&str)->bool{
        let result=match id {
            "file.reveal"|"file.terminal"=>{
                let path=self.workspace.as_ref().and_then(|w|w.path(self.app.active)).map(|p|p.to_owned());
                match path {Some(path)=>if id=="file.reveal"{bareline_platform_windows::shell_integration::reveal(&path)}else{path.parent().ok_or_else(||"File has no parent folder".to_owned()).and_then(bareline_platform_windows::shell_integration::open_terminal)},None=>Err("Save the current document first".into())}
            },
            "tray.toggle"=>{self.shell_integration.keep_in_tray=!self.shell_integration.keep_in_tray;if !self.shell_integration.keep_in_tray {self.shell_integration.tray=None;}Ok(())},
            "tray.hide"=>self.hide_to_tray(),
            "tray.restore"=>{if let Some(window)=&self.window{window.set_visible(true);window.set_minimized(false);window.focus_window();}Ok(())},
            _=>return false,
        };
        if let Some(w)=&mut self.workspace{w.message=Some(match result{Ok(())=>"Shell action completed".into(),Err(e)=>e});}true
    }
    pub(super) fn hide_to_tray(&mut self)->Result<(),String>{
        let window=self.window.as_ref().ok_or("Window unavailable")?;
        if self.shell_integration.tray.is_none(){let handle=window.window_handle().map_err(|e|e.to_string())?;let RawWindowHandle::Win32(handle)=handle.as_raw() else{return Err("Windows handle unavailable".into());};self.shell_integration.tray=Some(bareline_platform_windows::shell_integration::TrayIcon::new(handle.hwnd.get())?);}
        window.set_visible(false);Ok(())
    }
    pub(super) fn shell_recent_pump(&mut self){
        if self.shell_integration.portable{return;}
        if let Some(workspace)=&self.workspace{for i in 0..workspace.editors.len(){if let Some(path)=workspace.path(i){if self.shell_integration.recent.len()<256 && self.shell_integration.recent.insert(path.to_owned()){bareline_platform_windows::shell_integration::add_recent(path,false);}}}}
    }
}

impl ShellIntegrationRuntime {
    pub(super) fn annotate_context(&self,context:&mut bareline_commands::CommandContext,has_path:bool,window_visible:bool){
        use bareline_commands::{CommandId,CommandState};
        context.states.insert(CommandId("tray.toggle"),CommandState{checked:self.keep_in_tray,..Default::default()});
        if !has_path{for id in ["file.reveal","file.terminal"]{context.states.insert(CommandId(id),CommandState::disabled("Save the current document first"));}}
        let(id,reason)=if window_visible{("tray.restore","Window is already visible")}else{("tray.hide","Window is already hidden")};
        context.states.insert(CommandId(id),CommandState::disabled(reason));
    }
}
