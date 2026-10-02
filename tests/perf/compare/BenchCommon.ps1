# SPDX-License-Identifier: MPL-2.0
# Shared helpers for the Bareline vs Notepad++ comparison harness (dot-source only).
# Drives both editors without injected keystrokes: menu WM_COMMAND, dialog button
# messages, Scintilla queries and UI Automation. Keep this file ASCII-only so that
# Windows PowerShell 5.1 parses it identically without a BOM.
#
# PowerShell variables are case-insensitive: never reuse a name that differs only in
# case (the review scripts lost their fixture root when $f overwrote $F).

Set-StrictMode -Version 2.0

# Scintilla messages (Scintilla.h) and Notepad++ Find/Replace dialog control IDs
# (FindReplaceDlg_rc.h, v8.9.8.1).
$script:SCI_GETLENGTH = 2006
$script:SCI_GETLINECOUNT = 2154
$script:SCI_GETMODIFY = 2159
$script:NPP_FIND_WHAT = 1601
$script:NPP_REPLACE_WITH = 1602
$script:NPP_MODE_REGEX = 1605
$script:NPP_REPLACE_ALL = 1609
$script:NPP_MODE_NORMAL = 1625
$script:NPP_MODE_EXTENDED = 1626
$script:NPP_IN_SELECTION = 1632
$script:NPP_DOT_MATCHES_NEWLINE = 1703
$script:WM_NULL = 0x0000
$script:WM_SETTEXT = 0x000C
$script:WM_CLOSE = 0x0010
$script:WM_COMMAND = 0x0111
$script:BM_CLICK = 0x00F5
$script:BM_GETCHECK = 0x00F0
$script:ELLIPSIS = [string][char]0x2026

function Initialize-BenchInterop {
    # Loaded lazily so the pure helpers below need no desktop session.
    if ('BenchNative' -as [type]) { return }
    Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
    Add-Type @'
using System;
using System.Text;
using System.Diagnostics;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class BenchNative {
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumChildWindows(IntPtr p, EnumProc cb, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, UIntPtr w, IntPtr l);
    [DllImport("user32.dll")] static extern IntPtr SendMessageTimeoutW(IntPtr h, uint m, UIntPtr w, IntPtr l, uint f, uint t, out UIntPtr r);
    [DllImport("user32.dll", CharSet = CharSet.Unicode, EntryPoint = "SendMessageTimeoutW")] static extern IntPtr SendTextTimeoutW(IntPtr h, uint m, UIntPtr w, string l, uint f, uint t, out UIntPtr r);
    [DllImport("user32.dll")] public static extern IntPtr GetDlgItem(IntPtr d, int id);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindowExW(IntPtr p, IntPtr after, string cls, string title);
    [DllImport("user32.dll")] public static extern IntPtr GetMenu(IntPtr h);
    [DllImport("user32.dll")] public static extern int GetMenuItemCount(IntPtr m);
    [DllImport("user32.dll")] public static extern IntPtr GetSubMenu(IntPtr m, int p);
    [DllImport("user32.dll")] public static extern uint GetMenuItemID(IntPtr m, int p);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetMenuStringW(IntPtr m, uint p, StringBuilder s, int n, uint f);
    const uint SMTO_ABORTIFHUNG = 2;
    public static string Text(IntPtr h) { var s = new StringBuilder(1024); GetWindowTextW(h, s, 1024); return s.ToString(); }
    public static string Cls(IntPtr h) { var s = new StringBuilder(256); GetClassNameW(h, s, 256); return s.ToString(); }
    public static string MenuLabel(IntPtr m, int p) { var s = new StringBuilder(512); GetMenuStringW(m, (uint)p, s, 512, 0x400); return s.ToString(); }
    public static List<IntPtr> Windows(uint pid, bool visibleOnly) {
        var r = new List<IntPtr>();
        EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid && (!visibleOnly || IsWindowVisible(h))) r.Add(h); return true; }, IntPtr.Zero);
        return r;
    }
    public static List<IntPtr> Children(IntPtr parent) {
        var r = new List<IntPtr>();
        EnumChildWindows(parent, (h, l) => { r.Add(h); return true; }, IntPtr.Zero);
        return r;
    }
    // Message value, or -1 when the target is hung or the timeout expires.
    public static long Ask(IntPtr h, uint m, uint timeoutMs) {
        UIntPtr r;
        if (SendMessageTimeoutW(h, m, UIntPtr.Zero, IntPtr.Zero, SMTO_ABORTIFHUNG, timeoutMs, out r) == IntPtr.Zero) return -1;
        return (long)r.ToUInt64();
    }
    // Round trip of a synchronous message in ms, or -1 on timeout.
    public static long Send(IntPtr h, uint m, uint w, uint timeoutMs) {
        var sw = Stopwatch.StartNew(); UIntPtr r;
        if (SendMessageTimeoutW(h, m, (UIntPtr)w, IntPtr.Zero, 0, timeoutMs, out r) == IntPtr.Zero) return -1;
        return sw.ElapsedMilliseconds;
    }
    public static bool SetText(IntPtr h, string s) { UIntPtr r; return SendTextTimeoutW(h, 0x000C, UIntPtr.Zero, s, 0, 2000, out r) != IntPtr.Zero; }
}
'@
}

# ---------- Pure helpers (no desktop needed) ----------

function ConvertTo-MenuPath([string]$Label) {
    # "&Replace...\tCtrl+H" and "Replace<U+2026>" both become "Replace...".
    (($Label -split "`t")[0]) -replace '&', '' -replace [regex]::Escape($script:ELLIPSIS), '...'
}

function Get-FileSha256([string]$Path) {
    (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Get-TextCount([string]$Text, [string]$Needle) {
    if ($Needle.Length -eq 0) { throw 'needle must not be empty' }
    ($Text.Length - $Text.Replace($Needle, '').Length) / $Needle.Length
}

function Wait-Condition([scriptblock]$Condition, [int]$TimeoutMs, [int]$IntervalMs = 20) {
    # Returns elapsed ms when $Condition becomes true, or -1 at the deadline.
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt $TimeoutMs) {
        if (& $Condition) { return $watch.ElapsedMilliseconds }
        Start-Sleep -Milliseconds $IntervalMs
    }
    -1
}

# ---------- Window, menu and message helpers ----------

function Find-MainWindow([int]$ProcessId) {
    # First visible, unowned, titled top-level window of the process (or zero).
    foreach ($handle in [BenchNative]::Windows([uint32]$ProcessId, $true)) {
        if ([BenchNative]::GetWindow($handle, 4) -eq [IntPtr]::Zero -and [BenchNative]::Text($handle) -ne '') { return $handle }
    }
    [IntPtr]::Zero
}

function Get-OwnedDialogTitles([int]$ProcessId) {
    @([BenchNative]::Windows([uint32]$ProcessId, $true) | Where-Object {
            [BenchNative]::GetWindow($_, 4) -ne [IntPtr]::Zero
        } | ForEach-Object { '{0}:{1}' -f [BenchNative]::Cls($_), [BenchNative]::Text($_) })
}

function Get-MenuTree([IntPtr]$Window) {
    $bar = [BenchNative]::GetMenu($Window)
    $items = New-Object Collections.Generic.List[object]
    if ($bar -eq [IntPtr]::Zero) { return $items }
    $stack = New-Object Collections.Generic.Stack[object]
    $stack.Push(@($bar, ''))
    while ($stack.Count) {
        $frame = $stack.Pop()
        $menu = [IntPtr]$frame[0]
        for ($index = 0; $index -lt [BenchNative]::GetMenuItemCount($menu); $index++) {
            $path = $frame[1] + '/' + (ConvertTo-MenuPath ([BenchNative]::MenuLabel($menu, $index)))
            $items.Add([pscustomobject]@{ Path = $path; Id = [BenchNative]::GetMenuItemID($menu, $index) })
            $sub = [BenchNative]::GetSubMenu($menu, $index)
            if ($sub -ne [IntPtr]::Zero) { $stack.Push(@($sub, $path)) }
        }
    }
    $items
}

function Get-MenuCommandId([IntPtr]$Window, [string]$Path) {
    $item = Get-MenuTree $Window | Where-Object { $_.Path -eq $Path } | Select-Object -First 1
    if (-not $item) { throw "menu item not found: $Path" }
    [uint32]$item.Id
}

function Send-MenuCommand([IntPtr]$Window, [string]$Path) {
    # Posted, so both editors receive it through their own message loop.
    $id = Get-MenuCommandId $Window $Path
    [void][BenchNative]::PostMessageW($Window, $script:WM_COMMAND, [UIntPtr]$id, [IntPtr]::Zero)
}

function Measure-UiRoundTrip([IntPtr]$Window, [int]$TimeoutMs = 5000) {
    [BenchNative]::Send($Window, $script:WM_NULL, 0, [uint32]$TimeoutMs)
}

function Get-NppEditor([IntPtr]$Window) {
    # The largest visible Scintilla child is the active document view.
    $best = [IntPtr]::Zero; $area = 0
    foreach ($child in [BenchNative]::Children($Window)) {
        if ([BenchNative]::Cls($child) -ne 'Scintilla' -or -not [BenchNative]::IsWindowVisible($child)) { continue }
        $rect = New-Object BenchNative+RECT
        [void][BenchNative]::GetWindowRect($child, [ref]$rect)
        $size = ($rect.R - $rect.L) * ($rect.B - $rect.T)
        if ($size -gt $area) { $area = $size; $best = $child }
    }
    $best
}

function Get-SciValue([IntPtr]$Editor, [int]$Message, [int]$TimeoutMs = 2000) {
    if ($Editor -eq [IntPtr]::Zero) { return -1 }
    [BenchNative]::Ask($Editor, [uint32]$Message, [uint32]$TimeoutMs)
}

# ---------- UI Automation ----------

function Get-UiaRoot([IntPtr]$Window) { [Windows.Automation.AutomationElement]::FromHandle($Window) }

function Get-UiaNames([IntPtr]$Window, [Windows.Automation.ControlType]$Type) {
    $condition = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty, $Type)
    @((Get-UiaRoot $Window).FindAll([Windows.Automation.TreeScope]::Descendants, $condition) | ForEach-Object { $_.Current.Name })
}

function Get-StatusText([IntPtr]$Window) {
    # Bareline status items are named "Document size: N bytes, M lines"; Notepad++'s
    # Win32 status bar exposes its parts as children of the status bar element.
    $root = Get-UiaRoot $Window
    $condition = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::ControlTypeProperty, [Windows.Automation.ControlType]::StatusBar)
    $names = New-Object Collections.Generic.List[string]
    foreach ($bar in $root.FindAll([Windows.Automation.TreeScope]::Descendants, $condition)) {
        if ($bar.Current.Name) { $names.Add($bar.Current.Name) }
        foreach ($part in $bar.FindAll([Windows.Automation.TreeScope]::Children, [Windows.Automation.Condition]::TrueCondition)) {
            if ($part.Current.Name) { $names.Add($part.Current.Name) }
        }
    }
    $names -join ' | '
}

function Get-BarelineNotices([IntPtr]$Window) {
    (Get-UiaNames $Window ([Windows.Automation.ControlType]::Text) | ForEach-Object { ($_ -split "`n")[0] }) -join ' || '
}

function Test-BarelineModified([IntPtr]$Window) {
    [bool](Get-UiaNames $Window ([Windows.Automation.ControlType]::TabItem) | Where-Object { $_ -like '*, modified' })
}

function Find-UiaByName([IntPtr]$Window, [string]$Name) {
    $condition = New-Object Windows.Automation.PropertyCondition([Windows.Automation.AutomationElement]::NameProperty, $Name)
    (Get-UiaRoot $Window).FindFirst([Windows.Automation.TreeScope]::Descendants, $condition)
}

function Set-UiaValue([IntPtr]$Window, [string]$Name, [string]$Value) {
    $element = Find-UiaByName $Window $Name
    if (-not $element) { throw "UI Automation element not found: $Name" }
    $element.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue($Value)
}

function Invoke-UiaButton([IntPtr]$Window, [string]$Name) {
    $element = Find-UiaByName $Window $Name
    if (-not $element) { throw "UI Automation element not found: $Name" }
    $element.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke()
}

# ---------- Process sampling ----------

function Get-ProcessSample([Diagnostics.Process]$Process) {
    $Process.Refresh()
    [ordered]@{
        private_mb = [math]::Round($Process.PrivateMemorySize64 / 1MB, 1)
        working_set_mb = [math]::Round($Process.WorkingSet64 / 1MB, 1)
        peak_working_set_mb = [math]::Round($Process.PeakWorkingSet64 / 1MB, 1)
        threads = $Process.Threads.Count
        handles = $Process.HandleCount
        cpu_s = [math]::Round($Process.TotalProcessorTime.TotalSeconds, 2)
    }
}

function Stop-BenchProcess([Diagnostics.Process]$Process) {
    if (-not $Process) { return }
    try {
        if (-not $Process.HasExited) { $Process.Kill(); [void]$Process.WaitForExit(10000) }
    } catch { Write-Verbose "process cleanup: $_" }
}
