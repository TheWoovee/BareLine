# QA-FIX-001 — startup chrome and Output

Scope: manual QA ISSUE-001, authorized follow-up. Authority: ADR-04 (native HMENU), PR-011 menu projection contract, UI shared chrome, inspected `docs/blueprint/mockups/bareline-main-editor-dark.png` and existing `logo-brand/bareline.ico`.

Plan: prevent successful background macro-library load/save from opening an empty Output pane; assign the existing branded icon to the native window. Preserve native menus. The supported DWM dark-caption attribute does not theme HMENU: white native menu remains a tracked visual discrepancy; no undocumented theme hooks or menu redesign.

Verification: static source review only. User explicitly deferred builds and tests until all fixes land. Native rendering and matching-dimension comparison remain pending. Historical QA observations are unchanged.

Implemented (partial): successful Library/Saved macro results preserve Output visibility; errors and explicit operation results still open it. Native small/large window icons use embedded approved ICO entries, owned and released by WindowsPlatform. Files: apps/bareline/src/windows_app/macros.rs; crates/platform-windows/src/native.rs. White native HMENU remains unresolved under ADR-04. No build/test/native verification executed.

Supported theming follow-up: native top-level HMENU items now use documented WM_MEASUREITEM/WM_DRAWITEM and MIM_BACKGROUND with current UI colors; popup items/navigation remain Windows-owned. MSAAMENUINFO exposes owner-drawn item names and WM_MENUCHAR preserves mnemonic activation. Names refresh after localization. Subclass/item data/brush lifetime belongs to WindowsPlatform. Authoritative docs: https://learn.microsoft.com/en-us/windows/win32/api/oleacc/ns-oleacc-msaamenuinfo and https://learn.microsoft.com/en-us/windows/win32/menurc/using-menus . This supersedes the prior claim that ADR04 alone blocked theming. Native visual/accessibility/DPI checks remain for parent validation.

Font/lifetime review: owner-drawn labels use NONCLIENTMETRICS.lfMenuFont at GetDpiForWindow DPI, refreshed on system setting/DPI changes, with system disabled text color. WindowsPlatform drops MenuBar first; it restores ordinary item types/clears item-data, removes its subclass, and releases brush/font before data is freed. Native menu handles remain window-owned.
