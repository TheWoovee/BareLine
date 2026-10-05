// SPDX-License-Identifier: MPL-2.0
//! Follows the editor theme in the parts of the window Windows paints for us:
//! the title bar (documented DWM attribute) and the menu bar and popup menus
//! (the uxtheme ordinals File Explorer itself uses).
//!
//! Everything here is best effort. The ordinals are undocumented, so they are
//! resolved by number, guarded by the OS build, and every failure is ignored:
//! a build that refuses them keeps the light system chrome and stays usable.
use std::sync::OnceLock;
use windows::Win32::{
    Foundation::HWND,
    System::LibraryLoader::{GetProcAddress, LoadLibraryW},
};
use windows::core::{BOOL, PCSTR, w};

/// Ordinal 135 argument. Only `AllowDark` and `ForceLight` are used here.
#[repr(i32)]
#[derive(Clone, Copy)]
enum PreferredAppMode {
    #[allow(dead_code)]
    Default = 0,
    AllowDark = 1,
    ForceLight = 3,
}

type SetPreferredAppModeFn = unsafe extern "system" fn(i32) -> i32;
type FlushMenuThemesFn = unsafe extern "system" fn();

/// The ordinals moved between Windows 10 releases; 19045 is the first build
/// this product supports and matches the installer's `MinVersion`.
const MIN_BUILD: u32 = 19045;

fn uxtheme() -> Option<(SetPreferredAppModeFn, FlushMenuThemesFn)> {
    static ENTRIES: OnceLock<Option<(usize, usize)>> = OnceLock::new();
    let raw = (*ENTRIES.get_or_init(|| {
        if build_number() < MIN_BUILD {
            return None;
        }
        // SAFETY: the library name is a static wide literal; handles are process
        // lifetime and intentionally left loaded.
        unsafe {
            let module = LoadLibraryW(w!("uxtheme.dll")).ok()?;
            let set = GetProcAddress(module, PCSTR(135 as *const u8))?;
            let flush = GetProcAddress(module, PCSTR(136 as *const u8))?;
            Some((set as usize, flush as usize))
        }
    }))?;
    // SAFETY: both ordinals were resolved from uxtheme.dll above and keep the
    // signatures Explorer uses.
    unsafe {
        Some((
            std::mem::transmute::<usize, SetPreferredAppModeFn>(raw.0),
            std::mem::transmute::<usize, FlushMenuThemesFn>(raw.1),
        ))
    }
}

/// Real build number: `GetVersionExW` lies for unmanifested compatibility, so
/// ask ntdll directly and treat any failure as "too old".
fn build_number() -> u32 {
    #[repr(C)]
    struct OsVersionInfoW {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }
    type RtlGetVersionFn = unsafe extern "system" fn(*mut OsVersionInfoW) -> i32;
    static BUILD: OnceLock<u32> = OnceLock::new();
    *BUILD.get_or_init(|| {
        // SAFETY: ntdll is always mapped; the record is fully owned and sized here.
        unsafe {
            let Ok(module) = LoadLibraryW(w!("ntdll.dll")) else {
                return 0;
            };
            let Some(entry) = GetProcAddress(module, PCSTR(c"RtlGetVersion".as_ptr() as *const u8)) else {
                return 0;
            };
            let get: RtlGetVersionFn = std::mem::transmute(entry);
            let mut info = OsVersionInfoW {
                size: std::mem::size_of::<OsVersionInfoW>() as u32,
                major: 0,
                minor: 0,
                build: 0,
                platform: 0,
                csd: [0; 128],
            };
            if get(&mut info) != 0 { 0 } else { info.build }
        }
    })
}

/// Applies the theme to the title bar and to every menu this process shows.
pub fn apply(hwnd: HWND, dark: bool) {
    if let Some((set_mode, flush)) = uxtheme() {
        let mode = if dark {
            PreferredAppMode::AllowDark
        } else {
            PreferredAppMode::ForceLight
        };
        // SAFETY: ordinal entry points resolved above; both take no owned state.
        unsafe {
            set_mode(mode as i32);
            flush();
        }
    }
    use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    let value = BOOL::from(dark);
    // SAFETY: the caller owns `hwnd`; the attribute value is a local BOOL.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::from_ref(&value).cast(),
            std::mem::size_of::<BOOL>() as u32,
        );
        use windows::Win32::UI::WindowsAndMessaging::{
            DrawMenuBar, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetWindowPos,
        };
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
        );
        let _ = DrawMenuBar(hwnd);
    }
}
