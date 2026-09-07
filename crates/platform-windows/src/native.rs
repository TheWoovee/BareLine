// SPDX-License-Identifier: MPL-2.0
//! All handles stay on the owning UI thread; the winit window outlives this adapter.
use super::renderer::WindowsRenderer;
use bareline_commands::{
    Action, CommandContext, CommandId, CommandRegistry, Keymap, MenuItem, MenuModel,
};
use bareline_platform::PlatformServices;
use std::path::PathBuf;
use windows::{
    Win32::{
        Foundation::*,
        System::Com::*,
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, w},
};

pub struct WindowsPlatform {
    hwnd: HWND,
    commands: Vec<Action>,
    command_ids: Vec<CommandId>,
    item_menus: Vec<HMENU>,
}
impl WindowsPlatform {
    fn append_model(
        &mut self,
        menu: HMENU,
        items: &[MenuItem],
        registry: &CommandRegistry,
    ) -> windows::core::Result<()> {
        for item in items {
            match item {
                MenuItem::Separator => unsafe {
                    AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
                },
                MenuItem::Command(id) => {
                    if registry
                        .presentation(*id)
                        .is_some_and(|metadata| metadata.internal)
                    {
                        continue;
                    }
                    if let Some(spec) = registry.entries().find(|spec| spec.id == *id) {
                        self.commands.push(spec.action);
                        self.command_ids.push(*id);
                        self.item_menus.push(menu);
                        let label = wide(&format!("{}\t{}", spec.title, spec.shortcut));
                        unsafe {
                            AppendMenuW(
                                menu,
                                MF_STRING,
                                self.commands.len(),
                                PCWSTR(label.as_ptr()),
                            )?;
                        }
                    }
                }
                MenuItem::Submenu { title, items } => unsafe {
                    let child = CreatePopupMenu()?;
                    let label = wide(title);
                    if let Err(error) =
                        AppendMenuW(menu, MF_POPUP, child.0 as usize, PCWSTR(label.as_ptr()))
                    {
                        let _ = DestroyMenu(child);
                        return Err(error);
                    }
                    self.append_model(child, items, registry)?;
                },
            }
        }
        Ok(())
    }
    pub fn command_id(&self, menu_id: usize) -> Option<CommandId> {
        menu_id
            .checked_sub(1)
            .and_then(|index| self.command_ids.get(index))
            .copied()
    }
    /// Update native command projections from the same immutable state and effective keymap as palette.
    pub fn sync_commands(
        &self,
        registry: &CommandRegistry,
        context: &CommandContext,
        keymap: &Keymap,
    ) -> windows::core::Result<()> {
        for (index, id) in self.command_ids.iter().enumerate() {
            let Some(spec) = registry.entries().find(|spec| spec.id == *id) else {
                continue;
            };
            let Some(state) = registry.state(*id, context) else {
                continue;
            };
            let mut label = wide(&format!(
                "{}\t{}",
                state.label.as_deref().unwrap_or(spec.title),
                keymap.shortcut_label(*id)
            ));
            let info = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STATE | MIIM_STRING,
                fState: (if state.enabled {
                    MFS_ENABLED
                } else {
                    MFS_DISABLED
                }) | if state.checked {
                    MFS_CHECKED
                } else {
                    MFS_UNCHECKED
                },
                dwTypeData: windows::core::PWSTR(label.as_mut_ptr()),
                ..Default::default()
            };
            unsafe {
                SetMenuItemInfoW(self.item_menus[index], (index + 1) as u32, false, &info)?;
            }
        }
        unsafe { DrawMenuBar(self.hwnd) }
    }
    pub fn confirm_discard_document(&self, name: &str) -> bool {
        let message = wide(&format!(
            "Discard unsaved changes to {name} and close this tab?"
        ));
        unsafe {
            MessageBoxW(
                Some(self.hwnd),
                PCWSTR(message.as_ptr()),
                w!("Bareline"),
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING,
            ) == IDYES
        }
    }
    pub fn operation_failed(&self, details: &str) {
        let message = wide(&format!(
            "The operation could not finish. Your documents remain open.\n\n{details}\n\nFile commands remain available to save your work."
        ));
        // SAFETY: owned UTF-16 remains live for the modal call on the owning UI thread.
        unsafe {
            MessageBoxW(
                Some(self.hwnd),
                PCWSTR(message.as_ptr()),
                w!("Bareline"),
                MB_OK | MB_ICONERROR,
            );
        }
    }
    /// The raw handle must belong to a live window on the current thread.
    ///
    /// # Safety
    /// Caller must retain that window until this adapter and renderer are dropped.
    pub unsafe fn new(raw: isize, registry: &CommandRegistry) -> windows::core::Result<Self> {
        let hwnd = HWND(raw as *mut _);
        // This supported DWM attribute follows the dark editor theme on Windows 10/11.
        // Older builds may ignore it; window creation remains usable.
        unsafe {
            use windows::Win32::Graphics::Dwm::*;
            let dark = 1i32;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_IMMERSIVE_DARK_MODE,
                (&dark as *const i32).cast(),
                4,
            );
        }
        // SAFETY: UI thread initializes an STA; COM dialogs are created and released here.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        let mut platform = Self {
            hwnd,
            commands: Vec::new(),
            command_ids: Vec::new(),
            item_menus: Vec::new(),
        };
        // SAFETY: menu handles are transferred to the live window after successful SetMenu.
        unsafe {
            let menu = CreateMenu()?;
            let result = (|| -> windows::core::Result<()> {
                platform.append_model(menu, &MenuModel::from_registry(registry).items, registry)?;
                SetMenu(hwnd, Some(menu))?;
                DrawMenuBar(hwnd)?;
                Ok(())
            })();
            if result.is_err() {
                let _ = SetMenu(hwnd, None);
                let _ = DestroyMenu(menu);
            }
            result?;
        }
        Ok(platform)
    }
    /// # Safety
    /// `raw` points to a live Win32 MSG for the duration of this call.
    pub unsafe fn command_message(raw: *const std::ffi::c_void) -> Option<usize> {
        // SAFETY: guaranteed by the winit Windows message-hook contract.
        let msg = unsafe { &*(raw as *const MSG) };
        (msg.message == WM_COMMAND && msg.lParam.0 == 0).then_some(msg.wParam.0 & 0xffff)
    }
    pub fn action(&self, menu_id: usize) -> Option<Action> {
        menu_id
            .checked_sub(1)
            .and_then(|i| self.commands.get(i))
            .copied()
    }
    pub fn renderer(&self, software: bool) -> windows::core::Result<WindowsRenderer> {
        WindowsRenderer::new(self.hwnd, software)
    }
    pub fn clipboard_text(&self) -> windows::core::Result<String> {
        super::clipboard::read(self.hwnd)
    }
    pub fn set_clipboard_text(&self, text: &str) -> windows::core::Result<()> {
        super::clipboard::write(self.hwnd, text)
    }
    pub fn confirm_discard(&self) -> bool {
        unsafe {
            MessageBoxW(
                Some(self.hwnd),
                w!("Discard unsaved changes and close Bareline?"),
                w!("Bareline"),
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING,
            ) == IDYES
        }
    }
    pub fn pending_operation_notice(&self) {
        unsafe {
            MessageBoxW(
                Some(self.hwnd),
                w!("A file operation is still running. Wait for it to finish before closing."),
                w!("Bareline"),
                MB_OK | MB_ICONINFORMATION,
            );
        }
    }
    pub fn context_menu(
        &self,
        x: i32,
        y: i32,
        registry: &CommandRegistry,
    ) -> windows::core::Result<Option<Action>> {
        self.context_menu_in(
            x,
            y,
            registry,
            &CommandContext::default(),
            &Keymap::defaults(registry),
            &[
                CommandId("edit.undo"),
                CommandId("edit.redo"),
                CommandId("edit.cut"),
                CommandId("edit.copy"),
                CommandId("edit.paste"),
                CommandId("edit.select_all"),
                CommandId("search.find"),
            ],
        )
    }
    pub fn context_menu_in(
        &self,
        x: i32,
        y: i32,
        registry: &CommandRegistry,
        context: &CommandContext,
        keymap: &Keymap,
        commands: &[CommandId],
    ) -> windows::core::Result<Option<Action>> {
        // SAFETY: popup menu is scoped to the owning UI thread and destroyed after tracking.
        unsafe {
            let menu = CreatePopupMenu()?;
            let result = (|| {
                let specs: Vec<_> = commands
                    .iter()
                    .filter_map(|id| registry.entries().find(|spec| spec.id == *id))
                    .filter(|spec| {
                        !registry
                            .presentation(spec.id)
                            .is_some_and(|metadata| metadata.internal)
                    })
                    .collect();
                for (index, spec) in specs.iter().enumerate() {
                    let state = registry.state(spec.id, context).unwrap();
                    let label = wide(&format!(
                        "{}\t{}",
                        state.label.as_deref().unwrap_or(spec.title),
                        keymap.shortcut_label(spec.id)
                    ));
                    AppendMenuW(
                        menu,
                        MF_STRING
                            | if state.enabled { MF_ENABLED } else { MF_GRAYED }
                            | if state.checked {
                                MF_CHECKED
                            } else {
                                MF_UNCHECKED
                            },
                        index + 1,
                        PCWSTR(label.as_ptr()),
                    )?;
                }
                let selected = TrackPopupMenu(
                    menu,
                    TPM_RETURNCMD | TPM_RIGHTBUTTON,
                    x,
                    y,
                    None,
                    self.hwnd,
                    None,
                )
                .0;
                Ok(selected
                    .checked_sub(1)
                    .and_then(|index| specs.get(index as usize))
                    .and_then(|spec| registry.dispatch_in(spec.id, context).ok()))
            })();
            let _ = DestroyMenu(menu);
            result
        }
    }
    fn dialog(&self, save: bool, folder: bool) -> windows::core::Result<Option<PathBuf>> {
        // SAFETY: STA initialized by new; COM objects and allocated path freed in this scope.
        unsafe {
            let dialog: IFileDialog = if save {
                CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?
            } else {
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?
            };
            let options = dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR;
            dialog.SetOptions(if folder {
                options | FOS_PICKFOLDERS
            } else {
                options
            })?;
            if let Err(error) = dialog.Show(Some(self.hwnd)) {
                if error.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0) {
                    return Ok(None);
                }
                return Err(error);
            }
            let result = dialog.GetResult()?;
            let raw = result.GetDisplayName(SIGDN_FILESYSPATH)?;
            use std::os::windows::ffi::OsStringExt;
            let path = PathBuf::from(std::ffi::OsString::from_wide(raw.as_wide()));
            CoTaskMemFree(Some(raw.0.cast()));
            Ok(Some(path))
        }
    }
}

pub fn private_bytes() -> windows::core::Result<u64> {
    use windows::Win32::System::{ProcessStatus::*, Threading::GetCurrentProcess};
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: extended structure begins with PROCESS_MEMORY_COUNTERS and cb provides full size.
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            counters.cb,
        )?;
    }
    Ok(counters.PrivateUsage as u64)
}
/// OS-reported lifetime peak process commit charge; includes harness and prior workloads.
pub fn peak_private_bytes() -> windows::core::Result<u64> {
    use windows::Win32::System::{ProcessStatus::*, Threading::GetCurrentProcess};
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: the initialized extended structure has the required prefix and full cb.
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            counters.cb,
        )?;
    }
    Ok(counters.PeakPagefileUsage as u64)
}
impl Drop for WindowsPlatform {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
impl PlatformServices for WindowsPlatform {
    fn about(&self) {
        unsafe {
            MessageBoxW(
                Some(self.hwnd),
                w!(
                    "Bareline 0.1.0\nPlain text. Full power. No weight.\n\nLocal foundation preview — editing is under development."
                ),
                w!("About Bareline"),
                MB_OK,
            );
        }
    }
    fn open_file(&self) -> Result<Option<PathBuf>, String> {
        self.dialog(false, false).map_err(|e| e.to_string())
    }
    fn save_file(&self) -> Result<Option<PathBuf>, String> {
        self.dialog(true, false).map_err(|e| e.to_string())
    }
    fn pick_folder(&self) -> Result<Option<PathBuf>, String> {
        self.dialog(false, true).map_err(|e| e.to_string())
    }
}
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
