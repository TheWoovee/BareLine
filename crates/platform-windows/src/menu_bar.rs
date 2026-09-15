// SPDX-License-Identifier: MPL-2.0
//! Documented HMENU owner drawing; navigation and accessibility remain owned by Windows.
use std::cell::{Cell, RefCell};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        UI::{
            Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW, MSAA_MENU_SIG, MSAAMENUINFO},
            Controls::*,
            HiDpi::*,
            Shell::*,
            WindowsAndMessaging::*,
        },
    },
    core::PWSTR,
};

#[repr(C)]
struct Item {
    // MSAA requires this structure to be the first field addressed by dwItemData.
    accessible: MSAAMENUINFO,
    text: Vec<u16>,
    menu: HMENU,
    position: u32,
    id: u32,
    original_type: MENU_ITEM_TYPE,
    original_data: usize,
    top_level: bool,
    separator: bool,
    submenu: bool,
    radio: Cell<bool>,
}

#[derive(Clone, Copy)]
struct StyledMenu {
    menu: HMENU,
    original_background: HBRUSH,
}

pub(crate) struct MenuBar {
    hwnd: HWND,
    items: Vec<Box<Item>>,
    menus: Vec<StyledMenu>,
    colors: Cell<(u32, u32, u32)>,
    background_brush: Cell<HBRUSH>,
    selection_brush: Cell<HBRUSH>,
    brushes_owned: Cell<bool>,
    retained_brushes: RefCell<Vec<HBRUSH>>,
    font: Cell<HFONT>,
    dpi: Cell<u32>,
    high_contrast: Cell<bool>,
}

impl MenuBar {
    pub(crate) fn attach(hwnd: HWND) -> windows::core::Result<Box<Self>> {
        let mut state = Box::new(Self {
            hwnd,
            items: Vec::new(),
            menus: Vec::new(),
            colors: Cell::new((0, 0, 0)),
            background_brush: Cell::new(HBRUSH::default()),
            selection_brush: Cell::new(HBRUSH::default()),
            brushes_owned: Cell::new(false),
            retained_brushes: RefCell::new(Vec::new()),
            font: Cell::new(HFONT::default()),
            dpi: Cell::new(0),
            high_contrast: Cell::new(false),
        });
        state.refresh_resources(true);
        unsafe {
            // A subtree that cannot be styled is deliberately left as a native
            // system menu. Other independently valid subtrees remain themed.
            state.style_menu(GetMenu(hwnd), true);
            SetWindowSubclass(hwnd, Some(callback), 0xBAAE1001, (&*state as *const Self) as usize).ok()?;
            state.apply_backgrounds();
        }
        Ok(state)
    }

    pub(crate) fn theme(&self) -> (u32, u32, u32) {
        self.colors.get()
    }

    pub(crate) fn owns(&self, menu: HMENU, value: u32, by_position: bool) -> bool {
        self.items.iter().any(|item| {
            item.menu.0 == menu.0
                && if by_position {
                    item.position == value
                } else {
                    item.id == value
                }
        })
    }

    /// Keep owner-draw text and MSAA text synchronized with native label updates.
    pub(crate) fn item(&mut self, menu: HMENU, value: u32, by_position: bool, text: &[u16], radio: bool) {
        if let Some(item) = self.items.iter_mut().find(|item| {
            item.menu.0 == menu.0
                && if by_position {
                    item.position == value
                } else {
                    item.id == value
                }
        }) {
            if item.text != text {
                item.text = text.to_vec();
                item.accessible.cchWText = text.len().saturating_sub(1) as u32;
                item.accessible.pszWText = PWSTR(item.text.as_mut_ptr());
            }
            item.radio.set(radio);
        }
    }

    pub(crate) fn colors(&self, background: u32, text: u32, selection: u32) {
        let colors = (background, text, selection);
        let resources_changed = self.refresh_resources(false);
        if self.colors.replace(colors) == colors && !resources_changed {
            return;
        }
        unsafe {
            self.rebuild_brushes();
            self.apply_backgrounds();
            let _ = DrawMenuBar(self.hwnd);
        }
    }

    fn refresh_resources(&self, force: bool) -> bool {
        unsafe {
            let dpi = GetDpiForWindow(self.hwnd).max(96);
            let high_contrast = system_high_contrast();
            let dpi_changed = self.dpi.replace(dpi) != dpi;
            let contrast_changed = self.high_contrast.replace(high_contrast) != high_contrast;
            if force || dpi_changed {
                let mut metrics = NONCLIENTMETRICSW {
                    cbSize: std::mem::size_of::<NONCLIENTMETRICSW>() as u32,
                    ..Default::default()
                };
                if SystemParametersInfoForDpi(
                    SPI_GETNONCLIENTMETRICS.0,
                    metrics.cbSize,
                    Some((&mut metrics as *mut NONCLIENTMETRICSW).cast()),
                    0,
                    dpi,
                )
                .is_ok()
                {
                    let font = CreateFontIndirectW(&metrics.lfMenuFont);
                    if !font.0.is_null() {
                        delete_font(self.font.replace(font));
                    }
                }
            }
            if force || dpi_changed || contrast_changed {
                self.rebuild_brushes();
            }
            force || dpi_changed || contrast_changed
        }
    }

    unsafe fn rebuild_brushes(&self) {
        unsafe {
            let (background, _, selection) = self.colors.get();
            let high_contrast = self.high_contrast.get();
            let (background_brush, selection_brush) = if high_contrast {
                (
                    HBRUSH(GetSysColorBrush(COLOR_MENU).0),
                    HBRUSH(GetSysColorBrush(COLOR_HIGHLIGHT).0),
                )
            } else {
                (CreateSolidBrush(rgb(background)), CreateSolidBrush(rgb(selection)))
            };
            if background_brush.0.is_null() || selection_brush.0.is_null() {
                delete_brush(background_brush, !high_contrast);
                delete_brush(selection_brush, !high_contrast);
                return;
            }
            if !self.apply_background(background_brush) {
                // Some menus may already reference the candidate. Repoint every
                // menu to the still-live old brush, and retain the candidate if
                // any rollback also fails so no HMENU can dangle.
                let rollback_complete = self.apply_background(self.background_brush.get());
                if !high_contrast && !rollback_complete {
                    self.retained_brushes
                        .borrow_mut()
                        .extend([background_brush, selection_brush]);
                } else {
                    delete_brush(background_brush, !high_contrast);
                    delete_brush(selection_brush, !high_contrast);
                }
                return;
            }
            let old_owned = self.brushes_owned.replace(!high_contrast);
            let old_background = self.background_brush.replace(background_brush);
            let old_selection = self.selection_brush.replace(selection_brush);
            delete_brush(old_background, old_owned);
            delete_brush(old_selection, old_owned);
        }
    }

    fn font_object(&self) -> HGDIOBJ {
        if self.font.get().0.is_null() {
            unsafe { GetStockObject(DEFAULT_GUI_FONT) }
        } else {
            self.font.get().into()
        }
    }

    unsafe fn style_menu(&mut self, menu: HMENU, top_level: bool) -> bool {
        unsafe {
            let count = GetMenuItemCount(Some(menu));
            if count < 0 {
                return false;
            }
            let mut pending = Vec::with_capacity(count as usize);
            let mut original_background = MENUINFO {
                cbSize: std::mem::size_of::<MENUINFO>() as u32,
                fMask: MIM_BACKGROUND,
                ..Default::default()
            };
            if GetMenuInfo(menu, &mut original_background).is_err() {
                return false;
            }
            for position in 0..count as u32 {
                let mut info = MENUITEMINFOW {
                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_FTYPE | MIIM_DATA | MIIM_SUBMENU | MIIM_ID,
                    ..Default::default()
                };
                if GetMenuItemInfoW(menu, position, true, &mut info).is_err() {
                    return false;
                }
                let mut text = vec![0u16; 512];
                let length = GetMenuStringW(menu, position, Some(&mut text), MF_BYPOSITION).max(0) as usize;
                text.truncate(length + 1);
                let mut item = Box::new(Item {
                    accessible: MSAAMENUINFO {
                        dwMSAASignature: MSAA_MENU_SIG as u32,
                        cchWText: length as u32,
                        pszWText: PWSTR::null(),
                    },
                    text,
                    menu,
                    position,
                    id: info.wID,
                    original_type: info.fType,
                    original_data: info.dwItemData,
                    top_level,
                    separator: info.fType.0 & MFT_SEPARATOR.0 != 0,
                    submenu: !info.hSubMenu.0.is_null(),
                    radio: Cell::new(info.fType.0 & MFT_RADIOCHECK.0 != 0),
                });
                item.accessible.pszWText = PWSTR(item.text.as_mut_ptr());
                pending.push((item, info.hSubMenu));
            }

            for applied in 0..pending.len() {
                let item = &pending[applied].0;
                let info = MENUITEMINFOW {
                    cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                    fMask: MIIM_FTYPE | MIIM_DATA,
                    fType: MENU_ITEM_TYPE(item.original_type.0 | MFT_OWNERDRAW.0),
                    dwItemData: (item.as_ref() as *const Item) as usize,
                    ..Default::default()
                };
                if SetMenuItemInfoW(menu, item.position, true, &info).is_err() {
                    let failed: Vec<_> = pending
                        .iter()
                        .take(applied)
                        .enumerate()
                        .filter_map(|(index, (rollback, _))| {
                            (!restore_item(rollback) && IsMenu(rollback.menu).as_bool()).then_some(index)
                        })
                        .collect();
                    for (index, (record, _)) in pending.into_iter().enumerate() {
                        if failed.contains(&index) {
                            // A live HMENU still has this exact dwItemData.
                            Box::leak(record);
                        }
                    }
                    return false;
                }
            }

            self.menus.push(StyledMenu {
                menu,
                original_background: original_background.hbrBack,
            });
            let children: Vec<_> = pending
                .iter()
                .filter_map(|(_, submenu)| (!submenu.0.is_null()).then_some(*submenu))
                .collect();
            self.items.extend(pending.into_iter().map(|(item, _)| item));
            for child in children {
                self.style_menu(child, false);
            }
            true
        }
    }

    unsafe fn apply_backgrounds(&self) {
        unsafe {
            self.apply_background(self.background_brush.get());
        }
    }

    unsafe fn apply_background(&self, brush: HBRUSH) -> bool {
        unsafe {
            let mut complete = true;
            for styled in &self.menus {
                let info = MENUINFO {
                    cbSize: std::mem::size_of::<MENUINFO>() as u32,
                    fMask: MIM_BACKGROUND,
                    hbrBack: brush,
                    ..Default::default()
                };
                complete &= SetMenuInfo(styled.menu, &info).is_ok();
            }
            complete
        }
    }
}

impl Drop for MenuBar {
    fn drop(&mut self) {
        unsafe {
            for item in std::mem::take(&mut self.items) {
                if !restore_item(&item) && IsMenu(item.menu).as_bool() {
                    // A live HMENU still references this address. Leaking one
                    // failed record is safer than leaving a dangling itemData.
                    Box::leak(item);
                }
            }
            let mut backgrounds_restored = true;
            for styled in &self.menus {
                let info = MENUINFO {
                    cbSize: std::mem::size_of::<MENUINFO>() as u32,
                    fMask: MIM_BACKGROUND,
                    hbrBack: styled.original_background,
                    ..Default::default()
                };
                if SetMenuInfo(styled.menu, &info).is_err() && IsMenu(styled.menu).as_bool() {
                    backgrounds_restored = false;
                }
            }
            let _ = RemoveWindowSubclass(self.hwnd, Some(callback), 0xBAAE1001);
            delete_font(self.font.replace(HFONT::default()));
            if backgrounds_restored {
                let owned = self.brushes_owned.replace(false);
                delete_brush(self.background_brush.replace(HBRUSH::default()), owned);
                delete_brush(self.selection_brush.replace(HBRUSH::default()), owned);
                for brush in self.retained_brushes.get_mut().drain(..) {
                    delete_brush(brush, true);
                }
            }
        }
    }
}

unsafe fn restore_item(item: &Item) -> bool {
    unsafe {
        let info = MENUITEMINFOW {
            cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE | MIIM_DATA,
            fType: item.original_type,
            dwItemData: item.original_data,
            ..Default::default()
        };
        SetMenuItemInfoW(item.menu, item.position, true, &info).is_ok()
    }
}

unsafe fn delete_font(font: HFONT) {
    unsafe {
        if !font.0.is_null() {
            let _ = DeleteObject(font.into());
        }
    }
}

unsafe fn delete_brush(brush: HBRUSH, owned: bool) {
    unsafe {
        if owned && !brush.0.is_null() {
            let _ = DeleteObject(brush.into());
        }
    }
}

fn rgb(argb: u32) -> COLORREF {
    COLORREF(((argb >> 16) & 255) | (argb & 0x00ff00) | ((argb & 255) << 16))
}

fn muted(foreground: u32, background: u32) -> u32 {
    let channel =
        |shift: u32| ((((foreground >> shift) & 255u32) + 2u32 * ((background >> shift) & 255u32)) / 3u32) << shift;
    channel(16) | channel(8) | channel(0)
}

fn system_high_contrast() -> bool {
    let mut contrast = HIGHCONTRASTW {
        cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            contrast.cbSize,
            Some((&mut contrast as *mut HIGHCONTRASTW).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
        .is_ok()
            && contrast.dwFlags.contains(HCF_HIGHCONTRASTON)
    }
}

fn item_text(item: &Item) -> (&[u16], &[u16]) {
    let text = &item.text[..item.text.len().saturating_sub(1)];
    text.iter()
        .position(|character| *character == b'\t' as u16)
        .map_or((text, &[]), |tab| (&text[..tab], &text[tab + 1..]))
}

fn mnemonic(item: &Item) -> Option<char> {
    let label = String::from_utf16_lossy(item_text(item).0);
    let mut characters = label.chars();
    let mut contains_marker = false;
    while let Some(character) = characters.next() {
        if character != '&' {
            continue;
        }
        contains_marker = true;
        match characters.next() {
            Some('&') => continue,
            key => return key,
        }
    }
    // Bareline's canonical root titles are plain "File", "Search", etc.
    // Preserve their native Alt+first-letter contract without treating escaped
    // ampersands in popup labels as access-key markers.
    (!contains_marker && item.top_level)
        .then(|| label.chars().next())
        .flatten()
}

fn menu_char_lresult(position: usize, action: u32) -> LRESULT {
    LRESULT((position | ((action as usize) << 16)) as isize)
}

unsafe fn menu_char_result(state: &MenuBar, menu: HMENU, pressed: char) -> Option<LRESULT> {
    unsafe {
        if !state.menus.iter().any(|styled| styled.menu.0 == menu.0) {
            return None;
        }
        let matches: Vec<_> = state
            .items
            .iter()
            .filter(|item| item.menu.0 == menu.0 && !item.separator)
            .filter(|item| {
                let flags = GetMenuState(menu, item.position, MF_BYPOSITION);
                flags != u32::MAX && flags & (MF_DISABLED.0 | MF_GRAYED.0) == 0
            })
            .filter(|item| mnemonic(item).is_some_and(|key| key.to_lowercase().eq(pressed.to_lowercase())))
            .collect();
        if matches.is_empty() {
            return Some(menu_char_lresult(0, MNC_IGNORE));
        }
        let highlighted = matches
            .iter()
            .position(|item| GetMenuState(menu, item.position, MF_BYPOSITION) & MF_HILITE.0 != 0);
        let selected = highlighted.map_or(0, |index| (index + 1) % matches.len());
        Some(menu_char_lresult(
            matches[selected].position as usize,
            if matches.len() == 1 { MNC_EXECUTE } else { MNC_SELECT },
        ))
    }
}

#[derive(Clone, Copy)]
enum Glyph {
    Check,
    Radio,
    Arrow,
    Separator,
}

unsafe fn draw_glyph(dc: HDC, rect: RECT, glyph: Glyph, color: COLORREF, dpi: u32) {
    unsafe {
        let width = ((dpi + 95) / 96).max(1) as i32;
        let pen = CreatePen(PS_SOLID, width, color);
        if pen.0.is_null() {
            return;
        }
        let old_pen = SelectObject(dc, pen.into());
        let center_x = (rect.left + rect.right) / 2;
        let center_y = (rect.top + rect.bottom) / 2;
        match glyph {
            Glyph::Check => {
                let radius = ((5 * dpi) / 96).max(3) as i32;
                let _ = MoveToEx(dc, center_x - radius, center_y, None);
                let _ = LineTo(dc, center_x - radius / 3, center_y + radius / 2);
                let _ = LineTo(dc, center_x + radius, center_y - radius);
            }
            Glyph::Radio => {
                let radius = ((3 * dpi) / 96).max(2) as i32;
                let brush = CreateSolidBrush(color);
                if !brush.0.is_null() {
                    let old_brush = SelectObject(dc, brush.into());
                    let _ = Ellipse(
                        dc,
                        center_x - radius,
                        center_y - radius,
                        center_x + radius + 1,
                        center_y + radius + 1,
                    );
                    SelectObject(dc, old_brush);
                    let _ = DeleteObject(brush.into());
                }
            }
            Glyph::Arrow => {
                let radius = ((4 * dpi) / 96).max(2) as i32;
                let _ = MoveToEx(dc, center_x - radius / 2, center_y - radius, None);
                let _ = LineTo(dc, center_x + radius / 2, center_y);
                let _ = LineTo(dc, center_x - radius / 2, center_y + radius);
            }
            Glyph::Separator => {
                let _ = MoveToEx(dc, rect.left, center_y, None);
                let _ = LineTo(dc, rect.right, center_y);
            }
        }
        SelectObject(dc, old_pen);
        let _ = DeleteObject(pen.into());
    }
}

unsafe extern "system" fn callback(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    // The adapter owns this pinned state and removes the subclass before releasing it.
    let state = unsafe { &*(data as *const MenuBar) };
    unsafe {
        if matches!(
            message,
            WM_SETTINGCHANGE | WM_THEMECHANGED | WM_SYSCOLORCHANGE | WM_DPICHANGED
        ) {
            state.refresh_resources(true);
            state.apply_backgrounds();
            let _ = DrawMenuBar(hwnd);
        } else if message == WM_INITMENUPOPUP && state.refresh_resources(false) {
            state.apply_backgrounds();
            let _ = DrawMenuBar(hwnd);
        }
        if message == WM_MENUCHAR {
            if let Some(pressed) = char::from_u32((wparam.0 & 0xffff) as u32) {
                let active_menu = HMENU(lparam.0 as *mut std::ffi::c_void);
                if let Some(result) = menu_char_result(state, active_menu, pressed) {
                    return result;
                }
            }
        }
        if message == WM_MEASUREITEM && lparam.0 != 0 {
            let measure = &mut *(lparam.0 as *mut MEASUREITEMSTRUCT);
            if measure.CtlType == ODT_MENU {
                if let Some(item) = state
                    .items
                    .iter()
                    .find(|item| (&***item as *const Item) as usize == measure.itemData)
                {
                    let dpi = state.dpi.get().max(96);
                    if item.separator {
                        measure.itemWidth = 1;
                        measure.itemHeight = ((7 * dpi) / 96).max(1);
                        return LRESULT(1);
                    }
                    let dc = GetDC(Some(hwnd));
                    let old = SelectObject(dc, state.font_object());
                    let mut size = SIZE::default();
                    let _ = GetTextExtentPoint32W(dc, &item.text[..item.text.len().saturating_sub(1)], &mut size);
                    SelectObject(dc, old);
                    ReleaseDC(Some(hwnd), dc);
                    let gutters = if item.top_level { 24 } else { 58 };
                    measure.itemWidth = (size.cx + ((gutters * dpi) / 96) as i32).max(1) as u32;
                    let metric = if item.top_level { SM_CYMENU } else { SM_CYMENUSIZE };
                    measure.itemHeight = GetSystemMetricsForDpi(metric, dpi).max(size.cy + 4) as u32;
                    return LRESULT(1);
                }
            }
        }
        if message == WM_DRAWITEM && lparam.0 != 0 {
            let draw = &*(lparam.0 as *const DRAWITEMSTRUCT);
            if draw.CtlType == ODT_MENU {
                if let Some(item) = state
                    .items
                    .iter()
                    .find(|item| (&***item as *const Item) as usize == draw.itemData)
                {
                    let selected = draw.itemState.0 & (ODS_SELECTED.0 | ODS_HOTLIGHT.0) != 0;
                    FillRect(
                        draw.hDC,
                        &draw.rcItem,
                        if selected {
                            state.selection_brush.get()
                        } else {
                            state.background_brush.get()
                        },
                    );
                    if item.separator {
                        let mut separator = draw.rcItem;
                        separator.left += ((28 * state.dpi.get()) / 96) as i32;
                        let (background, text, _) = state.colors.get();
                        let color = if state.high_contrast.get() {
                            GetSysColor(COLOR_GRAYTEXT)
                        } else {
                            rgb(muted(text, background)).0
                        };
                        draw_glyph(draw.hDC, separator, Glyph::Separator, COLORREF(color), state.dpi.get());
                        return LRESULT(1);
                    }

                    let saved = SaveDC(draw.hDC);
                    SelectObject(draw.hDC, state.font_object());
                    SetBkMode(draw.hDC, TRANSPARENT);
                    let disabled = draw.itemState.0 & (ODS_DISABLED.0 | ODS_GRAYED.0) != 0;
                    let (background, text, selection) = state.colors.get();
                    let text_color = if state.high_contrast.get() {
                        GetSysColor(if selected {
                            COLOR_HIGHLIGHTTEXT
                        } else if disabled {
                            COLOR_GRAYTEXT
                        } else {
                            COLOR_MENUTEXT
                        })
                    } else if disabled {
                        rgb(muted(text, if selected { selection } else { background })).0
                    } else {
                        rgb(text).0
                    };
                    SetTextColor(draw.hDC, COLORREF(text_color));
                    let mut flags = DT_VCENTER | DT_SINGLELINE;
                    if draw.itemState.0 & ODS_NOACCEL.0 != 0 {
                        flags |= DT_HIDEPREFIX;
                    }
                    let (label, shortcut) = item_text(item);
                    if item.top_level {
                        let mut label = label.to_vec();
                        let mut rect = draw.rcItem;
                        DrawTextW(draw.hDC, &mut label, &mut rect, flags | DT_CENTER);
                    } else {
                        let scale = state.dpi.get() as i32;
                        let gutter = 28 * scale / 96;
                        let arrow = 18 * scale / 96;
                        let mut label_rect = draw.rcItem;
                        label_rect.left += gutter;
                        label_rect.right -= arrow;
                        let mut label = label.to_vec();
                        DrawTextW(draw.hDC, &mut label, &mut label_rect, flags | DT_LEFT);
                        if !shortcut.is_empty() {
                            let mut shortcut_rect = label_rect;
                            let mut shortcut = shortcut.to_vec();
                            DrawTextW(draw.hDC, &mut shortcut, &mut shortcut_rect, flags | DT_RIGHT);
                        }
                        if draw.itemState.0 & ODS_CHECKED.0 != 0 {
                            let mut mark = draw.rcItem;
                            mark.right = mark.left + gutter;
                            draw_glyph(
                                draw.hDC,
                                mark,
                                if item.radio.get() { Glyph::Radio } else { Glyph::Check },
                                COLORREF(text_color),
                                state.dpi.get(),
                            );
                        }
                        if item.submenu {
                            let mut arrow_rect = draw.rcItem;
                            arrow_rect.left = arrow_rect.right - arrow;
                            draw_glyph(
                                draw.hDC,
                                arrow_rect,
                                Glyph::Arrow,
                                COLORREF(text_color),
                                state.dpi.get(),
                            );
                        }
                    }
                    let _ = RestoreDC(draw.hDC, saved);
                    return LRESULT(1);
                }
            }
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_and_shortcut_are_separate_native_columns() {
        let mut item = Item {
            accessible: MSAAMENUINFO::default(),
            text: "&Save\tCtrl+S\0".encode_utf16().collect(),
            menu: HMENU::default(),
            position: 0,
            id: 1,
            original_type: MENU_ITEM_TYPE(0),
            original_data: 0,
            top_level: false,
            separator: false,
            submenu: false,
            radio: Cell::new(false),
        };
        item.accessible.pszWText = PWSTR(item.text.as_mut_ptr());
        let (label, shortcut) = item_text(&item);
        assert_eq!(String::from_utf16_lossy(label), "&Save");
        assert_eq!(String::from_utf16_lossy(shortcut), "Ctrl+S");
        assert_ne!(muted(0x00ffffff, 0x00000000), 0x00ffffff);
    }

    #[test]
    fn recursive_owner_draw_enumerates_and_restores_three_levels() -> windows::core::Result<()> {
        unsafe {
            let root = CreateMenu()?;
            let first = CreatePopupMenu()?;
            let second = CreatePopupMenu()?;
            AppendMenuW(second, MF_STRING, 41, windows::core::w!("&Alpha"))?;
            AppendMenuW(second, MF_STRING, 42, windows::core::w!("&Again"))?;
            AppendMenuW(second, MF_STRING, 43, windows::core::w!("Rock && Roll"))?;
            AppendMenuW(first, MF_POPUP, second.0 as usize, windows::core::w!("Nested"))?;
            AppendMenuW(root, MF_POPUP, first.0 as usize, windows::core::w!("File"))?;

            let mut state = MenuBar {
                hwnd: HWND::default(),
                items: Vec::new(),
                menus: Vec::new(),
                colors: Cell::new((0, 0, 0)),
                background_brush: Cell::new(HBRUSH::default()),
                selection_brush: Cell::new(HBRUSH::default()),
                brushes_owned: Cell::new(false),
                retained_brushes: RefCell::new(Vec::new()),
                font: Cell::new(HFONT::default()),
                dpi: Cell::new(96),
                high_contrast: Cell::new(false),
            };
            assert!(state.style_menu(root, true));
            assert_eq!(state.menus.len(), 3);
            assert_eq!(state.items.len(), 5);
            assert!(state.items[0].top_level);
            assert!(state.items[1].submenu);
            assert_eq!(state.items[2].id, 41);

            let root_file = menu_char_result(&state, root, 'f').unwrap().0 as usize;
            assert_eq!(root_file >> 16, MNC_EXECUTE as usize);
            assert_eq!(root_file & 0xffff, 0);

            let _ = EnableMenuItem(second, 1, MF_BYPOSITION | MF_DISABLED);
            let unique = menu_char_result(&state, second, 'a').unwrap().0 as usize;
            assert_eq!(unique >> 16, MNC_EXECUTE as usize);
            assert_eq!(unique & 0xffff, 0);
            let escaped = menu_char_result(&state, second, 'r').unwrap().0 as usize;
            assert_eq!(escaped >> 16, MNC_IGNORE as usize);

            let _ = EnableMenuItem(second, 1, MF_BYPOSITION | MF_ENABLED);
            let duplicate = menu_char_result(&state, second, 'a').unwrap().0 as usize;
            assert_eq!(duplicate >> 16, MNC_SELECT as usize);
            assert_eq!(duplicate & 0xffff, 0);
            let highlighted = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_STATE,
                fState: MFS_HILITE,
                ..Default::default()
            };
            SetMenuItemInfoW(second, 0, true, &highlighted)?;
            let cycled = menu_char_result(&state, second, 'a').unwrap().0 as usize;
            assert_eq!(cycled >> 16, MNC_SELECT as usize);
            assert_eq!(cycled & 0xffff, 1);

            let changed: Vec<_> = "&Changed\tCtrl+K\0".encode_utf16().collect();
            state.item(second, 41, false, &changed, true);
            let radio = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_DATA,
                fType: MFT_OWNERDRAW | MFT_RADIOCHECK,
                dwItemData: (state.items[2].as_ref() as *const Item) as usize,
                ..Default::default()
            };
            SetMenuItemInfoW(second, 0, true, &radio)?;
            assert!(state.items[2].radio.get());

            for item in &state.items {
                restore_item(item);
            }
            state.items.clear();
            state.menus.clear();
            let mut leaf = MENUITEMINFOW {
                cbSize: std::mem::size_of::<MENUITEMINFOW>() as u32,
                fMask: MIIM_FTYPE | MIIM_DATA,
                ..Default::default()
            };
            GetMenuItemInfoW(second, 0, true, &mut leaf)?;
            assert_eq!(leaf.fType.0 & MFT_OWNERDRAW.0, 0);
            assert_eq!(leaf.fType.0 & MFT_RADIOCHECK.0, 0);
            assert_eq!(leaf.dwItemData, 0);
            DestroyMenu(root)?;
        }
        Ok(())
    }
}
