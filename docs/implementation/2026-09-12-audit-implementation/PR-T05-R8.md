# PR-T05 R8: Visible owned save-consent dialogs

- **Baseline:** `8eca399e663156d9ff2b74be86a57f46ee13f39a`
- **Confirmed defect:** A successfully dispatched dirty Close enters `TaskDialogIndirect`, disables the owner and creates the expected owned dialog/buttons, but the dialog and every descendant retain `visible:false` for at least 15 seconds.
- **Source finding:** The executable omitted the required `InitCommonControlsEx` call, but adding it under the product's v6 manifest did not make the dialog visible in the native rerun. No product code changes `WS_VISIBLE`, calls `ShowWindow`, swallows dialog messages, or subclasses the owned TaskDialog; the menu theme subclass is bound only to the main HWND. The native defect is therefore confined at the TaskDialog pre-display boundary rather than attributed to app command ingress or theming.
- **Scope:** Keep the required Common-Controls initialization and reveal only the TaskDialog HWND supplied by its documented creation callback, without changing focus, polling, enumerating controls, selecting a choice, or changing T05 close/discard behavior.

## Evidence

- Added `initialize_common_controls`, using a fully sized `INITCOMMONCONTROLSEX` with `ICC_STANDARD_CLASSES`, at the start of `WindowsPlatform::new`. Initialization occurs before the menu adapter, COM dialogs, or TaskDialog calls.
- Removed the platform-library idempotence test because that test executable does not embed the product's v6 Common-Controls manifest; its failure did not represent the shipped app activation context.
- Registered a TaskDialog callback and applies `SWP_SHOWWINDOW` once at `TDN_CREATED`, after Windows has created the dialog and before it displays it. `SWP_NOMOVE`, `SWP_NOSIZE`, `SWP_NOZORDER`, and `SWP_NOACTIVATE` preserve the native layout, z-order, activation, modal loop, button IDs, HRESULT, and Cancel behavior.
- Primary API contract: <https://learn.microsoft.com/en-us/windows/win32/api/commctrl/nf-commctrl-initcommoncontrolsex>.
- Callback lifecycle contract: <https://learn.microsoft.com/en-us/windows/win32/controls/tdn-created>.

Native qualification remains required: launch one fresh process, make one exact dirty edit, invoke one Close, and prove the owned `#32770` TaskDialog plus its Save / Don't Save / Cancel controls are visible and actionable without an unrelated input. Preserve the R7 command trace to show ingress, ready state, dialog entry, and selected native ID.
