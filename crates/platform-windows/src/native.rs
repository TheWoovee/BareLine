// SPDX-License-Identifier: MPL-2.0
//! All handles stay on the owning UI thread; the winit window outlives this adapter.
use super::renderer::WindowsRenderer;
use bareline_commands::{Action, CommandContext, CommandId, CommandRegistry, Keymap, MenuItem, MenuModel};
use bareline_platform::PlatformServices;
use std::path::{Path, PathBuf};
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
    window_icons: Vec<HICON>,
    menu_bar: std::cell::RefCell<Option<Box<crate::menu_bar::MenuBar>>>,
    /// The full curated menu; the visible subset is rebuilt from it on demand.
    model: MenuModel,
    /// The top-level menu currently owned by the window (freed on rebuild).
    menu: HMENU,
    /// Command order of the menu as currently built, so structural rebuilds only
    /// happen when a contextual command appears or disappears.
    built: Vec<CommandId>,
    commands: Vec<Action>,
    command_ids: Vec<CommandId>,
    item_menus: Vec<HMENU>,
    submenu_labels: Vec<(HMENU, u32, String)>,
    localized_commands: std::cell::RefCell<std::collections::BTreeMap<&'static str, String>>,
    applied_menu: std::cell::RefCell<Option<MenuProjection>>,
    dark: std::cell::Cell<bool>,
}

#[derive(Clone, PartialEq, Eq)]
struct MenuCommandProjection {
    index: usize,
    label: Vec<u16>,
    item_type: u32,
    state: u32,
}

#[derive(Clone, PartialEq, Eq)]
struct MenuProjection {
    commands: Vec<MenuCommandProjection>,
    submenus: Vec<Vec<u16>>,
}
/// Answer to a "save changes?" prompt. `Cancel` also covers Escape and the
/// title bar close button, so callers can treat it as "do nothing".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveChoice {
    Save,
    DontSave,
    Cancel,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SavePromptOutcome {
    Choice { choice: SaveChoice, selected: i32 },
    Failure(i32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandMessage {
    pub hwnd: isize,
    pub id: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AboutAction {
    License,
    ThirdPartyNotices,
    CopyDiagnostics,
}
impl WindowsPlatform {
    pub fn confirm_external_command(
        &self,
        program: &std::path::Path,
        arguments: &[std::ffi::OsString],
        shell: bool,
    ) -> bool {
        crate::process::confirm_external_command(self.hwnd, program, arguments, shell)
    }

    pub fn menu_colors(&self, background: u32, text: u32, selection: u32) {
        if let Some(bar) = self.menu_bar.borrow().as_ref() {
            bar.colors(background, text, selection);
        }
    }

    /// Follows the theme in the window chrome Windows paints itself. Cheap to
    /// call every frame: the native work only runs when the theme changed.
    pub fn set_dark_mode(&self, dark: bool) {
        if self.dark.get() == dark {
            return;
        }
        self.dark.set(dark);
        crate::dark_mode::apply(self.hwnd, dark);
    }

    /// Displays metadata supplied by the application, never document contents.
    /// Buttons return an action so file opening stays in the normal workspace pipeline.
    pub fn about_details(&self, details: &str) -> windows::core::Result<Option<AboutAction>> {
        use windows::Win32::UI::Controls::*;
        if details.len() > 16 * 1024 || details.contains('\0') {
            return Err(windows::core::Error::new(E_INVALIDARG, "Invalid About metadata"));
        }
        let content = wide(details);
        let buttons = [
            TASKDIALOG_BUTTON {
                nButtonID: 1001,
                pszButtonText: w!("License"),
            },
            TASKDIALOG_BUTTON {
                nButtonID: 1002,
                pszButtonText: w!("Third-party notices"),
            },
            TASKDIALOG_BUTTON {
                nButtonID: 1003,
                pszButtonText: w!("Copy diagnostics"),
            },
        ];
        let config = TASKDIALOGCONFIG {
            cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
            hwndParent: self.hwnd,
            dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT | TDF_USE_HICON_MAIN,
            dwCommonButtons: TDCBF_CLOSE_BUTTON,
            pszWindowTitle: w!("About Bareline"),
            // The product mark, not a generic system glyph.
            Anonymous1: TASKDIALOGCONFIG_0 {
                hMainIcon: self.window_icons.last().copied().unwrap_or_default(),
            },
            pszMainInstruction: w!("Bareline"),
            pszContent: PCWSTR(content.as_ptr()),
            cButtons: buttons.len() as u32,
            pButtons: buttons.as_ptr(),
            nDefaultButton: IDCLOSE.0,
            pszFooter: w!("Plain text. Full power. No weight.\nCore: MPL-2.0 · Extension SDK: MIT OR Apache-2.0"),
            pfCallback: Some(task_dialog_visibility_callback),
            ..Default::default()
        };
        let mut selected = 0;
        // SAFETY: all strings and button records remain live for the synchronous modal call.
        unsafe {
            TaskDialogIndirect(&config, Some(&mut selected), None, None)?;
        }
        Ok(match selected {
            1001 => Some(AboutAction::License),
            1002 => Some(AboutAction::ThirdPartyNotices),
            1003 => Some(AboutAction::CopyDiagnostics),
            _ => None,
        })
    }
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
                    if registry.presentation(*id).is_some_and(|metadata| metadata.internal) {
                        continue;
                    }
                    if let Some(spec) = registry.entries().find(|spec| spec.id == *id) {
                        self.commands.push(spec.action);
                        self.command_ids.push(*id);
                        self.item_menus.push(menu);
                        let label = wide(&format!("{}\t{}", spec.title, spec.shortcut));
                        unsafe {
                            AppendMenuW(menu, MF_STRING, self.commands.len(), PCWSTR(label.as_ptr()))?;
                        }
                    }
                }
                MenuItem::Submenu { title, items } => unsafe {
                    let position = GetMenuItemCount(Some(menu)) as u32;
                    let child = CreatePopupMenu()?;
                    let label = wide(title);
                    if let Err(error) = AppendMenuW(menu, MF_POPUP, child.0 as usize, PCWSTR(label.as_ptr())) {
                        let _ = DestroyMenu(child);
                        return Err(error);
                    }
                    self.submenu_labels.push((menu, position, title.to_string()));
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
        self.sync_commands_localized(registry, context, keymap, |_, fallback| fallback.to_owned())
    }
    /// Stable command IDs and `menu.<English title>` IDs share one data-only label resolver.
    pub fn sync_commands_localized(
        &self,
        registry: &CommandRegistry,
        context: &CommandContext,
        keymap: &Keymap,
        label_for: impl Fn(&str, &str) -> String,
    ) -> windows::core::Result<()> {
        // One entry per registered command, replaced on every locale/state refresh.
        *self.localized_commands.borrow_mut() = registry
            .entries()
            .map(|spec| (spec.id.0, label_for(spec.id.0, spec.title)))
            .collect();
        let mut projection = MenuProjection {
            commands: Vec::with_capacity(self.command_ids.len()),
            submenus: Vec::with_capacity(self.submenu_labels.len()),
        };
        for (index, id) in self.command_ids.iter().enumerate() {
            let Some(spec) = registry.entries().find(|spec| spec.id == *id) else {
                continue;
            };
            let Some(state) = registry.state(*id, context) else {
                continue;
            };
            let label = wide(&format!(
                "{}\t{}",
                label_for(id.0, state.label.as_deref().unwrap_or(spec.title)),
                keymap.shortcut_label(*id)
            ));
            let owner_draw = self
                .menu_bar
                .borrow()
                .as_ref()
                .is_some_and(|bar| bar.owns(self.item_menus[index], (index + 1) as u32, false));
            projection.commands.push(MenuCommandProjection {
                index,
                label,
                item_type: projected_item_type(state.radio, owner_draw).0,
                state: ((if state.enabled { MFS_ENABLED } else { MFS_DISABLED })
                    | if state.checked { MFS_CHECKED } else { MFS_UNCHECKED })
                .0,
            });
        }
        for (_, _, title) in &self.submenu_labels {
            projection
                .submenus
                .push(wide(&label_for(&format!("menu.{title}"), title)));
        }
        self.apply_menu_projection(projection).map(|_| ())
    }

    fn apply_menu_projection(&self, mut projection: MenuProjection) -> windows::core::Result<bool> {
        // DrawMenuBar can schedule another frame. Reapplying an unchanged menu
        // from that frame creates a repaint loop, even while the editor is idle.
        if self.applied_menu.borrow().as_ref() == Some(&projection) {
            return Ok(false);
        }
        for command in &mut projection.commands {
            let menu = self.item_menus[command.index];
            let id = (command.index + 1) as u32;
            let info = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STATE | MIIM_STRING | MIIM_FTYPE,
                // The active document in the Window menu shows an exclusive radio
                // dot rather than a check box.
                fType: MENU_ITEM_TYPE(command.item_type),
                fState: MENU_ITEM_STATE(command.state),
                dwTypeData: windows::core::PWSTR(command.label.as_mut_ptr()),
                ..Default::default()
            };
            unsafe {
                SetMenuItemInfoW(menu, id, false, &info)?;
                if let Some(bar) = self.menu_bar.borrow_mut().as_mut() {
                    bar.item(
                        menu,
                        id,
                        false,
                        &command.label,
                        command.item_type & MFT_RADIOCHECK.0 != 0,
                    );
                }
            }
        }
        for ((menu, position, _), label) in self.submenu_labels.iter().zip(&mut projection.submenus) {
            let info = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STRING,
                dwTypeData: windows::core::PWSTR(label.as_mut_ptr()),
                ..Default::default()
            };
            unsafe {
                SetMenuItemInfoW(*menu, *position, true, &info)?;
                if let Some(bar) = self.menu_bar.borrow_mut().as_mut() {
                    bar.item(*menu, *position, true, label, false);
                }
            }
        }
        unsafe { DrawMenuBar(self.hwnd)? };
        // Cache only after all native updates and the redraw request succeed.
        *self.applied_menu.borrow_mut() = Some(projection);
        Ok(true)
    }
    pub fn confirm_discard_document(&self, name: &str) -> bool {
        let message = wide(&format!("Discard unsaved changes to {name} and close this tab?"));
        unsafe {
            MessageBoxW(
                Some(self.hwnd),
                PCWSTR(message.as_ptr()),
                w!("Bareline"),
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING | MB_SETFOREGROUND,
            ) == IDYES
        }
    }
    /// Shared modal for the product's own decisions. `buttons` are custom
    /// buttons in display order; a Cancel button is always added so Escape and
    /// the close button return `IDCANCEL`.
    ///
    /// Returns the selected button id or the native failure HRESULT.
    fn task_dialog_result(
        &self,
        title: &str,
        instruction: &str,
        content: &str,
        buttons: &[(i32, &str)],
        default_button: i32,
    ) -> Result<i32, i32> {
        use windows::Win32::UI::Controls::*;
        let title = wide(title);
        let instruction = wide(instruction);
        let content = wide(content);
        let labels: Vec<Vec<u16>> = buttons.iter().map(|(_, text)| wide(text)).collect();
        let records: Vec<TASKDIALOG_BUTTON> = buttons
            .iter()
            .zip(&labels)
            .map(|((id, _), label)| TASKDIALOG_BUTTON {
                nButtonID: *id,
                pszButtonText: PCWSTR(label.as_ptr()),
            })
            .collect();
        let config = TASKDIALOGCONFIG {
            cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
            hwndParent: self.hwnd,
            dwFlags: TDF_ALLOW_DIALOG_CANCELLATION | TDF_SIZE_TO_CONTENT | TDF_POSITION_RELATIVE_TO_WINDOW,
            dwCommonButtons: TDCBF_CANCEL_BUTTON,
            pszWindowTitle: PCWSTR(title.as_ptr()),
            pszMainInstruction: PCWSTR(instruction.as_ptr()),
            pszContent: PCWSTR(content.as_ptr()),
            cButtons: records.len() as u32,
            pButtons: records.as_ptr(),
            nDefaultButton: default_button,
            pfCallback: Some(task_dialog_visibility_callback),
            ..Default::default()
        };
        let mut selected = IDCANCEL.0;
        // SAFETY: every string and button record stays live for the synchronous
        // modal call on the owning UI thread.
        unsafe { TaskDialogIndirect(&config, Some(&mut selected), None, None) }
            .map(|_| selected)
            .map_err(|error| error.code().0)
    }
    pub fn task_dialog(
        &self,
        title: &str,
        instruction: &str,
        content: &str,
        buttons: &[(i32, &str)],
        default_button: i32,
    ) -> i32 {
        self.task_dialog_result(title, instruction, content, buttons, default_button)
            .unwrap_or(IDCANCEL.0)
    }
    /// Save / Don't Save / Cancel for one document that is about to close.
    pub fn confirm_save_document(&self, name: &str) -> SavePromptOutcome {
        let name = display_title(name);
        let selected = self.task_dialog_result(
            "Bareline",
            &format!("Save changes to {name}?"),
            "Your changes will be lost if you don't save them.",
            &[(SAVE_ID, "&Save"), (DONT_SAVE_ID, "Do&n't Save")],
            SAVE_ID,
        );
        selected
            .map(|selected| SavePromptOutcome::Choice {
                choice: SaveChoice::from_id(selected),
                selected,
            })
            .unwrap_or_else(SavePromptOutcome::Failure)
    }
    /// Save All / Don't Save / Cancel on exit, listing every unsaved document.
    pub fn confirm_save_all(&self, names: &[String]) -> SavePromptOutcome {
        let instruction = match names.len() {
            1 => format!("Save changes to {}?", display_title(&names[0])),
            count => format!("Save changes to {count} documents?"),
        };
        let list = names
            .iter()
            .map(|name| format!("\u{2022} {}", display_title(name)))
            .collect::<Vec<_>>()
            .join("\n");
        let content = format!(
            "These documents have unsaved changes:\n\n{list}\n\nYour changes will be lost if you don't save them."
        );
        let selected = self.task_dialog_result(
            "Bareline",
            &instruction,
            &content,
            &[(SAVE_ID, "Save &All"), (DONT_SAVE_ID, "Do&n't Save")],
            SAVE_ID,
        );
        selected
            .map(|selected| SavePromptOutcome::Choice {
                choice: SaveChoice::from_id(selected),
                selected,
            })
            .unwrap_or_else(SavePromptOutcome::Failure)
    }
    pub fn confirm_stop_monitoring(&self) -> bool {
        unsafe {
            MessageBoxW(
                Some(self.hwnd),
                w!("Stop monitoring and load the current file for editing?\n\nThe tab will remain open."),
                w!("Unlock monitored file"),
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
            ) == IDYES
        }
    }
    /// Reinterpretation reloads original bytes and therefore discards unsaved edits.
    pub fn confirm_encoding_reinterpret(&self, name: &str, target: &str) -> bool {
        let display = |value: &str| value.chars().filter(|c| !c.is_control()).take(256).collect::<String>();
        let message = wide(&format!(
            "Reopen {} using {}?\n\nUnsaved changes in this document will be discarded. The file on disk will not be changed.\n\nChoose No to keep the current document and its edits.",
            display(name),
            display(target)
        ));
        unsafe {
            MessageBoxW(
                Some(self.hwnd),
                PCWSTR(message.as_ptr()),
                w!("Reopen with encoding"),
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING,
            ) == IDYES
        }
    }
    pub fn confirm_overwrite(&self, path: &Path) -> bool {
        const OVERWRITE_ID: i32 = 1103;
        self.task_dialog(
            "Bareline",
            "Replace the existing file with this document?",
            &format!(
                "{}\n\nReplacing it will overwrite its current contents.",
                path.display()
            ),
            &[(OVERWRITE_ID, "&Replace")],
            windows::Win32::UI::WindowsAndMessaging::IDCANCEL.0,
        ) == OVERWRITE_ID
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
    pub unsafe fn new(raw: isize, registry: &CommandRegistry, model: MenuModel) -> windows::core::Result<Self> {
        let hwnd = HWND(raw as *mut _);
        initialize_common_controls()?;
        // The title bar and the menus follow the editor theme. The first frame
        // corrects this once the resolved theme is known.
        crate::dark_mode::apply(hwnd, true);
        // SAFETY: UI thread initializes an STA; COM dialogs are created and released here.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        let mut platform = Self {
            hwnd,
            window_icons: Vec::new(),
            menu_bar: Default::default(),
            model,
            menu: HMENU(std::ptr::null_mut()),
            built: Vec::new(),
            commands: Vec::new(),
            command_ids: Vec::new(),
            item_menus: Vec::new(),
            submenu_labels: Vec::new(),
            localized_commands: Default::default(),
            applied_menu: Default::default(),
            dark: std::cell::Cell::new(true),
        };
        // Embed the approved artwork so portable launches never depend on a working directory.
        let artwork = include_bytes!("../../../docs/blueprint/mockups/logo-brand/bareline.ico");
        for (kind, size) in [(ICON_SMALL, 16u8), (ICON_BIG, 32u8)] {
            let count = u16::from_le_bytes([artwork[4], artwork[5]]) as usize;
            for entry in artwork[6..6 + count * 16].chunks_exact(16) {
                if entry[0] != size {
                    continue;
                }
                let length = u32::from_le_bytes(entry[8..12].try_into().unwrap()) as usize;
                let offset = u32::from_le_bytes(entry[12..16].try_into().unwrap()) as usize;
                // SAFETY: immutable embedded ICO image bytes; owned handles live with the adapter.
                if let Ok(icon) = unsafe {
                    CreateIconFromResourceEx(
                        &artwork[offset..offset + length],
                        true,
                        0x00030000,
                        size.into(),
                        size.into(),
                        LR_DEFAULTCOLOR,
                    )
                } {
                    unsafe {
                        SendMessageW(
                            hwnd,
                            WM_SETICON,
                            Some(WPARAM(kind as usize)),
                            Some(LPARAM(icon.0 as isize)),
                        );
                    }
                    platform.window_icons.push(icon);
                }
                break;
            }
        }
        // The initial build shows everything; the first frame's refresh trims any
        // contextual commands that do not apply yet.
        platform.build_menu(registry, &CommandContext::default())?;
        *platform.menu_bar.borrow_mut() = Some(crate::menu_bar::MenuBar::attach(hwnd)?);
        Ok(platform)
    }
    /// Rebuild the native menu from the curated model, keeping only the commands
    /// that should be visible in the given context. Frees the previous menu.
    fn build_menu(&mut self, registry: &CommandRegistry, context: &CommandContext) -> windows::core::Result<()> {
        let visible = self.model.visible(registry, context);
        let previous_theme = self.menu_bar.get_mut().as_ref().map(|bar| bar.theme());
        // SAFETY: the new menu handle is transferred to the window on success and
        // the previous one is destroyed only after the swap.
        unsafe {
            let menu = CreateMenu()?;
            self.applied_menu.get_mut().take();
            self.commands.clear();
            self.command_ids.clear();
            self.item_menus.clear();
            self.submenu_labels.clear();
            let result = (|| -> windows::core::Result<()> {
                self.append_model(menu, &visible.items, registry)?;
                // Restore item data before the old HMENU is detached/destroyed.
                // The replacement receives fresh stable owner-draw metadata below.
                self.menu_bar.get_mut().take();
                SetMenu(self.hwnd, Some(menu))?;
                DrawMenuBar(self.hwnd)?;
                Ok(())
            })();
            if result.is_err() {
                let _ = DestroyMenu(menu);
                return result;
            }
            if !self.menu.0.is_null() {
                let _ = DestroyMenu(self.menu);
            }
            self.menu = menu;
        }
        if let Some((background, text, selection)) = previous_theme
            && let Ok(bar) = crate::menu_bar::MenuBar::attach(self.hwnd)
        {
            bar.colors(background, text, selection);
            *self.menu_bar.get_mut() = Some(bar);
        }
        self.built = visible.command_order();
        Ok(())
    }
    /// Rebuild the menu structure only when the set/order of visible commands has
    /// changed (a document finished loading, a tab opened, the Window list grew).
    /// Cheap to call every frame: it compares a fingerprint and usually no-ops.
    pub fn refresh_structure(
        &mut self,
        registry: &CommandRegistry,
        context: &CommandContext,
    ) -> windows::core::Result<()> {
        if self.model.visible(registry, context).command_order() == self.built {
            return Ok(());
        }
        self.build_menu(registry, context)
    }
    /// # Safety
    /// `raw` points to a live Win32 MSG for the duration of this call.
    pub unsafe fn command_message(raw: *const std::ffi::c_void) -> Option<CommandMessage> {
        // SAFETY: guaranteed by the winit Windows message-hook contract.
        let msg = unsafe { &*(raw as *const MSG) };
        if msg.message != WM_COMMAND || msg.lParam.0 != 0 {
            return None;
        }
        Some(CommandMessage {
            hwnd: msg.hwnd.0 as isize,
            id: msg.wParam.0 & 0xffff,
        })
    }
    pub fn accepts_command(&self, message: &CommandMessage) -> bool {
        self.hwnd.0 as isize == message.hwnd
    }
    pub fn action(&self, menu_id: usize) -> Option<Action> {
        menu_id.checked_sub(1).and_then(|i| self.commands.get(i)).copied()
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
    pub fn set_clipboard_text_with_metadata(
        &self,
        text: &str,
        format: &str,
        bytes: &[u8],
    ) -> windows::core::Result<()> {
        super::clipboard::write_with_metadata(self.hwnd, text, format, bytes)
    }
    pub fn clipboard_metadata(&self, format: &str, max_bytes: usize) -> windows::core::Result<Option<Vec<u8>>> {
        super::clipboard::metadata(self.hwnd, format, max_bytes)
    }
    pub fn clipboard_text_with_metadata(
        &self,
        format: &str,
        max_bytes: usize,
    ) -> windows::core::Result<bareline_platform::clipboard::ClipboardContents> {
        super::clipboard::read_with_metadata(self.hwnd, format, max_bytes)
    }
    pub fn confirm_discard(&self) -> bool {
        unsafe {
            MessageBoxW(
                Some(self.hwnd),
                w!("Discard unsaved changes and close Bareline?"),
                w!("Bareline"),
                MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING | MB_SETFOREGROUND,
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
    pub fn context_menu(&self, x: i32, y: i32, registry: &CommandRegistry) -> windows::core::Result<Option<Action>> {
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
                // Resolve to the visible items in order, keeping `"-"` markers as
                // separators; unknown or internal commands are dropped.
                let mut entries: Vec<Option<&bareline_commands::CommandSpec>> = Vec::new();
                for id in commands {
                    if id.0 == "-" {
                        if matches!(entries.last(), Some(Some(_))) {
                            entries.push(None);
                        }
                        continue;
                    }
                    let Some(spec) = registry.entries().find(|spec| spec.id == *id) else {
                        continue;
                    };
                    if registry.presentation(spec.id).is_some_and(|metadata| metadata.internal) {
                        continue;
                    }
                    entries.push(Some(spec));
                }
                while matches!(entries.last(), Some(None)) {
                    entries.pop();
                }
                // Menu ids are 1-based positions into `specs` (real commands only);
                // separators carry id 0 and are never returned by TrackPopupMenu.
                let mut specs: Vec<&bareline_commands::CommandSpec> = Vec::new();
                for entry in &entries {
                    let Some(spec) = entry else {
                        AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null())?;
                        continue;
                    };
                    let state = registry.state(spec.id, context).unwrap();
                    let translated = self.localized_commands.borrow();
                    let label = wide(&format!(
                        "{}\t{}",
                        state
                            .label
                            .as_deref()
                            .unwrap_or_else(|| translated.get(spec.id.0).map(String::as_str).unwrap_or(spec.title)),
                        keymap.shortcut_label(spec.id)
                    ));
                    specs.push(spec);
                    AppendMenuW(
                        menu,
                        MF_STRING
                            | if state.enabled { MF_ENABLED } else { MF_GRAYED }
                            | if state.checked { MF_CHECKED } else { MF_UNCHECKED },
                        specs.len(),
                        PCWSTR(label.as_ptr()),
                    )?;
                }
                let selected = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON, x, y, None, self.hwnd, None).0;
                Ok(selected
                    .checked_sub(1)
                    .and_then(|index| specs.get(index as usize))
                    .and_then(|spec| registry.dispatch_in(spec.id, context).ok()))
            })();
            let _ = DestroyMenu(menu);
            result
        }
    }
    pub fn open_files(&self) -> Result<Vec<PathBuf>, String> {
        // SAFETY: synchronous STA dialog; shell-allocated paths are freed after copying.
        let result = unsafe {
            (|| -> windows::core::Result<Vec<PathBuf>> {
                let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?;
                dialog.SetOptions(
                    dialog.GetOptions()?
                        | FOS_FORCEFILESYSTEM
                        | FOS_NOCHANGEDIR
                        | FOS_ALLOWMULTISELECT
                        | FOS_FILEMUSTEXIST,
                )?;
                if let Err(error) = dialog.Show(Some(self.hwnd)) {
                    if error.code() == windows::core::HRESULT::from_win32(ERROR_CANCELLED.0) {
                        return Ok(Vec::new());
                    }
                    return Err(error);
                }
                let items = dialog.GetResults()?;
                let mut paths = Vec::new();
                for index in 0..items.GetCount()? {
                    let raw = items.GetItemAt(index)?.GetDisplayName(SIGDN_FILESYSPATH)?;
                    use std::os::windows::ffi::OsStringExt;
                    let path = std::ffi::OsString::from_wide(raw.as_wide());
                    CoTaskMemFree(Some(raw.0.cast()));
                    paths.push(PathBuf::from(path));
                }
                Ok(paths)
            })()
        };
        result.map_err(|error| error.to_string())
    }
    fn dialog(
        &self,
        save: bool,
        folder: bool,
        default_name: Option<&str>,
        default_directory: Option<&Path>,
        shell_overwrite_prompt: bool,
    ) -> windows::core::Result<Option<PathBuf>> {
        // SAFETY: STA initialized by new; COM objects and allocated path freed in this scope.
        unsafe {
            let dialog: IFileDialog = if save {
                CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)?
            } else {
                CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)?
            };
            let mut options = dialog.GetOptions()? | FOS_FORCEFILESYSTEM | FOS_NOCHANGEDIR;
            if save && !shell_overwrite_prompt {
                // The shell confirms once after an asynchronous fingerprint capture.
                options &= !FOS_OVERWRITEPROMPT;
            }
            dialog.SetOptions(if folder { options | FOS_PICKFOLDERS } else { options })?;
            // File-type filters and a starting name for file (not folder) dialogs.
            // The wide buffers live until the calls that copy them return.
            let text_label = wide("Text files");
            let text_spec = wide("*.txt;*.md;*.markdown;*.log;*.json;*.xml;*.csv;*.ini;*.toml;*.yaml;*.yml");
            let all_label = wide("All files");
            let all_spec = wide("*.*");
            let default_wide = default_name.map(wide);
            if !folder {
                let filters = [
                    windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC {
                        pszName: PCWSTR(text_label.as_ptr()),
                        pszSpec: PCWSTR(text_spec.as_ptr()),
                    },
                    windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC {
                        pszName: PCWSTR(all_label.as_ptr()),
                        pszSpec: PCWSTR(all_spec.as_ptr()),
                    },
                ];
                dialog.SetFileTypes(&filters)?;
                dialog.SetFileTypeIndex(1)?;
                if save {
                    dialog.SetDefaultExtension(w!("txt"))?;
                }
                if let Some(name) = &default_wide {
                    dialog.SetFileName(PCWSTR(name.as_ptr()))?;
                }
            }
            let fallback;
            let directory = match default_directory {
                Some(path) if path.is_dir() => Some(path),
                _ if save && !shell_overwrite_prompt => {
                    fallback = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None)
                        .ok()
                        .map(|raw| {
                            use std::os::windows::ffi::OsStringExt;
                            let path = PathBuf::from(std::ffi::OsString::from_wide(raw.as_wide()));
                            CoTaskMemFree(Some(raw.0.cast()));
                            path
                        });
                    fallback.as_deref()
                }
                _ => None,
            };
            if let Some(directory) = directory {
                use std::os::windows::ffi::OsStrExt;
                let directory: Vec<u16> = directory.as_os_str().encode_wide().chain(Some(0)).collect();
                let item: windows::core::Result<IShellItem> =
                    SHCreateItemFromParsingName(PCWSTR(directory.as_ptr()), None);
                if let Ok(item) = item {
                    dialog.SetDefaultFolder(&item)?;
                }
            }
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

fn initialize_common_controls() -> windows::core::Result<()> {
    use windows::Win32::UI::Controls::*;
    let controls = INITCOMMONCONTROLSEX {
        dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
        dwICC: ICC_STANDARD_CLASSES,
    };
    // SAFETY: the immutable descriptor remains live for the synchronous call.
    if unsafe { InitCommonControlsEx(&controls) }.as_bool() {
        Ok(())
    } else {
        Err(windows::core::Error::new(E_FAIL, "InitCommonControlsEx returned FALSE"))
    }
}

unsafe extern "system" fn task_dialog_visibility_callback(
    hwnd: HWND,
    notification: windows::Win32::UI::Controls::TASKDIALOG_NOTIFICATIONS,
    _wparam: WPARAM,
    _lparam: LPARAM,
    _data: isize,
) -> windows::core::HRESULT {
    use windows::Win32::UI::Controls::TDN_CREATED;
    if notification == TDN_CREATED {
        // This notification is delivered once the dialog is fully constructed
        // but before display. Reveal only the supplied TaskDialog;
        // its own modal loop retains activation and button semantics.
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_SHOWWINDOW,
            )
        };
    }
    windows::core::HRESULT(0)
}

/// Monotonic high-resolution timestamp in nanoseconds from
/// `QueryPerformanceCounter`, the same process-independent clock
/// `perf_counter_ns` uses on Windows. Returns `None` if the counter or its
/// frequency is unavailable. Keeping the FFI here confines the Win32 surface to
/// this crate instead of a raw binding in the app crate (ARCH-19).
pub fn monotonic_ns() -> Option<u128> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn QueryPerformanceCounter(value: *mut i64) -> i32;
        fn QueryPerformanceFrequency(value: *mut i64) -> i32;
    }
    let (mut value, mut frequency) = (0i64, 0i64);
    // SAFETY: each call writes a single caller-owned i64 and returns a BOOL.
    if unsafe { QueryPerformanceCounter(&mut value) == 0 || QueryPerformanceFrequency(&mut frequency) == 0 }
        || value < 0
        || frequency <= 0
    {
        return None;
    }
    Some(value as u128 * 1_000_000_000 / frequency as u128)
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
        self.menu_bar.get_mut().take();
        unsafe {
            for kind in [ICON_SMALL, ICON_BIG] {
                SendMessageW(self.hwnd, WM_SETICON, Some(WPARAM(kind as usize)), Some(LPARAM(0)));
            }
            for icon in self.window_icons.drain(..) {
                let _ = DestroyIcon(icon);
            }
            CoUninitialize();
        }
    }
}
impl PlatformServices for WindowsPlatform {
    fn clipboard_text(&self) -> Result<String, String> {
        WindowsPlatform::clipboard_text(self).map_err(|e| e.to_string())
    }
    fn set_clipboard_text(&self, text: &str) -> Result<(), String> {
        WindowsPlatform::set_clipboard_text(self, text).map_err(|e| e.to_string())
    }
    fn set_clipboard_text_with_metadata(&self, text: &str, format: &str, bytes: &[u8]) -> Result<(), String> {
        WindowsPlatform::set_clipboard_text_with_metadata(self, text, format, bytes).map_err(|e| e.to_string())
    }
    fn clipboard_metadata(&self, format: &str, max_bytes: usize) -> Result<Option<Vec<u8>>, String> {
        WindowsPlatform::clipboard_metadata(self, format, max_bytes).map_err(|e| e.to_string())
    }
    fn clipboard_text_with_metadata(
        &self,
        format: &str,
        max_bytes: usize,
    ) -> Result<bareline_platform::clipboard::ClipboardContents, String> {
        WindowsPlatform::clipboard_text_with_metadata(self, format, max_bytes).map_err(|e| e.to_string())
    }
    fn about(&self) {
        let message = wide(&format!(
            "Bareline {}\nPlain text. Full power. No weight.\n\nCore: MPL-2.0\nExtension SDK: MIT OR Apache-2.0",
            env!("CARGO_PKG_VERSION")
        ));
        unsafe {
            MessageBoxW(Some(self.hwnd), PCWSTR(message.as_ptr()), w!("About Bareline"), MB_OK);
        }
    }
    fn open_file(&self) -> Result<Option<PathBuf>, String> {
        self.dialog(false, false, None, None, true).map_err(|e| e.to_string())
    }
    fn save_file(&self) -> Result<Option<PathBuf>, String> {
        self.dialog(true, false, None, None, true).map_err(|e| e.to_string())
    }
    fn save_file_named(&self, default_name: &str) -> Result<Option<PathBuf>, String> {
        self.dialog(true, false, Some(default_name), None, true)
            .map_err(|e| e.to_string())
    }
    fn save_document_file_at(
        &self,
        default_name: &str,
        default_directory: Option<&Path>,
    ) -> Result<Option<PathBuf>, String> {
        self.dialog(true, false, Some(default_name), default_directory, false)
            .map_err(|e| e.to_string())
    }
    fn pick_folder(&self) -> Result<Option<PathBuf>, String> {
        self.dialog(false, true, None, None, true).map_err(|e| e.to_string())
    }
}
const SAVE_ID: i32 = 1101;
const DONT_SAVE_ID: i32 = 1102;
impl SaveChoice {
    fn from_id(id: i32) -> Self {
        match id {
            SAVE_ID => SaveChoice::Save,
            DONT_SAVE_ID => SaveChoice::DontSave,
            _ => SaveChoice::Cancel,
        }
    }
}
/// Tab titles carry the unsaved marker; prose in a prompt must not repeat it.
fn display_title(name: &str) -> String {
    let trimmed = name.trim().trim_end_matches(['\u{2022}', '*', '\u{25cf}']).trim();
    if trimmed.is_empty() {
        "this document".to_string()
    } else {
        trimmed.to_string()
    }
}
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn projected_item_type(radio: bool, owner_draw: bool) -> MENU_ITEM_TYPE {
    (if owner_draw { MFT_OWNERDRAW } else { MENU_ITEM_TYPE(0) })
        | if radio { MFT_RADIOCHECK } else { MENU_ITEM_TYPE(0) }
}

#[cfg(test)]
mod menu_state_tests {
    use super::*;

    #[test]
    fn unchanged_menu_skips_redraw_but_state_locale_and_rebuild_still_update() -> windows::core::Result<()> {
        struct Window(HWND);
        impl Drop for Window {
            fn drop(&mut self) {
                unsafe {
                    let _ = DestroyWindow(self.0);
                }
            }
        }
        let window = Window(unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("STATIC"),
                w!("Bareline menu regression"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                480,
                320,
                None,
                None,
                None,
                None,
            )?
        });
        let mut registry = CommandRegistry::default();
        let id = CommandId("test.new");
        registry
            .register(bareline_commands::CommandSpec {
                id,
                title: "New",
                category: "File",
                shortcut: "Ctrl+N",
                action: Action::New,
            })
            .unwrap();
        let keymap = Keymap::defaults(&registry);
        let mut context = CommandContext::default();
        // Exercise real native menus without the executable's common-controls
        // activation manifest, which the unit-test binary does not carry.
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        }
        let mut platform = WindowsPlatform {
            hwnd: window.0,
            window_icons: Vec::new(),
            menu_bar: Default::default(),
            model: MenuModel::from_registry(&registry),
            menu: HMENU(std::ptr::null_mut()),
            built: Vec::new(),
            commands: Vec::new(),
            command_ids: Vec::new(),
            item_menus: Vec::new(),
            submenu_labels: Vec::new(),
            localized_commands: Default::default(),
            applied_menu: Default::default(),
            dark: std::cell::Cell::new(false),
        };
        platform.build_menu(&registry, &context)?;
        platform.sync_commands(&registry, &context, &keymap)?;
        let initial = platform.applied_menu.borrow().as_ref().unwrap().clone();
        for _ in 0..64 {
            assert!(
                !platform.apply_menu_projection(initial.clone())?,
                "idle menu requested another repaint"
            );
        }

        let mut state = bareline_commands::CommandState::disabled("test disabled state");
        state.checked = true;
        state.radio = true;
        context.states.insert(id, state);
        platform.sync_commands_localized(&registry, &context, &keymap, |_, title| format!("Translated {title}"))?;
        let mut label = [0u16; 128];
        let mut actual = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_STATE | MIIM_FTYPE | MIIM_STRING,
            dwTypeData: windows::core::PWSTR(label.as_mut_ptr()),
            cch: label.len() as u32,
            ..Default::default()
        };
        unsafe {
            GetMenuItemInfoW(platform.item_menus[0], 1, false, &mut actual)?;
        }
        assert_ne!(actual.fState.0 & MFS_DISABLED.0, 0);
        assert_ne!(actual.fState.0 & MFS_CHECKED.0, 0);
        assert_ne!(actual.fType.0 & MFT_RADIOCHECK.0, 0);
        let text = String::from_utf16_lossy(&label[..actual.cch as usize]);
        assert!(text.starts_with("Translated New\t"), "{text}");
        assert!(text.contains("Ctrl+N"), "{text}");
        let translated = platform.applied_menu.borrow().as_ref().unwrap().clone();
        assert!(!platform.apply_menu_projection(translated.clone())?);

        platform.build_menu(&registry, &context)?;
        assert!(
            platform.applied_menu.borrow().is_none(),
            "a rebuilt HMENU reused the old projection"
        );
        assert!(platform.apply_menu_projection(translated.clone())?);
        assert!(!platform.apply_menu_projection(translated)?);
        Ok(())
    }

    #[test]
    fn command_ingress_preserves_owner_and_rejects_controls() {
        let hwnd = HWND(7usize as *mut std::ffi::c_void);
        let queued = MSG {
            hwnd,
            message: WM_COMMAND,
            wParam: WPARAM(45),
            lParam: LPARAM(0),
            ..Default::default()
        };
        assert_eq!(
            unsafe { WindowsPlatform::command_message((&queued as *const MSG).cast()) },
            Some(CommandMessage { hwnd: 7, id: 45 })
        );
        let control = MSG {
            hwnd,
            message: WM_COMMAND,
            wParam: WPARAM(45),
            lParam: LPARAM(1),
            ..Default::default()
        };
        assert!(unsafe { WindowsPlatform::command_message((&control as *const MSG).cast()) }.is_none());
    }

    #[test]
    fn native_fallback_state_update_does_not_enable_owner_draw() -> windows::core::Result<()> {
        unsafe {
            let menu = CreatePopupMenu()?;
            AppendMenuW(menu, MF_STRING, 7, w!("Fallback"))?;
            let mut label = wide("Updated fallback");
            let update = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STATE | MIIM_STRING | MIIM_FTYPE,
                fType: projected_item_type(true, false),
                fState: MFS_DISABLED | MFS_CHECKED,
                dwTypeData: windows::core::PWSTR(label.as_mut_ptr()),
                ..Default::default()
            };
            SetMenuItemInfoW(menu, 7, false, &update)?;
            let mut actual = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STATE | MIIM_FTYPE,
                ..Default::default()
            };
            GetMenuItemInfoW(menu, 7, false, &mut actual)?;
            assert_eq!(actual.fType.0 & MFT_OWNERDRAW.0, 0);
            assert_ne!(actual.fType.0 & MFT_RADIOCHECK.0, 0);
            assert_ne!(actual.fState.0 & MFS_DISABLED.0, 0);
            assert_ne!(actual.fState.0 & MFS_CHECKED.0, 0);
            DestroyMenu(menu)?;
        }

        let retained = projected_item_type(false, true);
        assert_ne!(retained.0 & MFT_OWNERDRAW.0, 0);
        assert_eq!(retained.0 & MFT_RADIOCHECK.0, 0);
        Ok(())
    }
}
