Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
using System.Text;
public class W32 {
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern void keybd_event(byte k, byte s, uint f, UIntPtr e);
  public static void WheelEv(int d) { mouse_event(0x800, 0, 0, unchecked((uint)d), UIntPtr.Zero); }
  public static void ForceFg(IntPtr h) { keybd_event(0x12, 0, 0, UIntPtr.Zero); keybd_event(0x12, 0, 2, UIntPtr.Zero); SetForegroundWindow(h); }
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint x, uint y, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr h, int x, int y, int w, int hh, bool r);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static System.Collections.Generic.List<string> ListWindows(uint pid) {
    var r = new System.Collections.Generic.List<string>();
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); if (p == pid) { var sb = new StringBuilder(256); GetWindowText(h, sb, 256); var cn = new StringBuilder(256); GetClassName(h, cn, 256); RECT rc; GetWindowRect(h, out rc); r.Add(h.ToInt64() + "|" + cn + "|" + sb + "|" + IsWindowVisible(h) + "|" + rc.L + "," + rc.T + "," + rc.R + "," + rc.B); } return true; }, IntPtr.Zero);
    return r;
  }
}
"@
function Shot($name) {
  $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
  $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
  $p = Join-Path $PSScriptRoot "shots\$name.png"
  New-Item -ItemType Directory -Force (Split-Path $p) | Out-Null
  $bmp.Save($p, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  Write-Output $p
}
function ShotWin($name, $h) {
  $r = New-Object W32+RECT; [W32]::GetWindowRect($h, [ref]$r) | Out-Null
  $w = $r.R - $r.L; $hh = $r.B - $r.T
  $bmp = New-Object System.Drawing.Bitmap $w, $hh
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($r.L, $r.T, 0, 0, (New-Object System.Drawing.Size $w, $hh))
  $p = Join-Path $PSScriptRoot "shots\$name.png"
  New-Item -ItemType Directory -Force (Split-Path $p) | Out-Null
  $bmp.Save($p, [System.Drawing.Imaging.ImageFormat]::Png)
  $g.Dispose(); $bmp.Dispose()
  Write-Output $p
}
function EnsureApp { $procs = Get-Process bareline -ErrorAction SilentlyContinue; if (-not $procs) { throw "bareline not running" }; $fg = [W32]::GetForegroundWindow(); $fpid = 0; [W32]::GetWindowThreadProcessId($fg, [ref]$fpid) | Out-Null; if (-not ($procs.Id -contains $fpid)) { $h = ($procs | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1).MainWindowHandle; Focus $h; $fg2 = [W32]::GetForegroundWindow(); $fpid2 = 0; [W32]::GetWindowThreadProcessId($fg2, [ref]$fpid2) | Out-Null; if (-not ($procs.Id -contains $fpid2)) { throw "could not focus bareline (fg pid $fpid2)" } } }
function Keys($k, $ms = 400) { EnsureApp; [System.Windows.Forms.SendKeys]::SendWait($k); Start-Sleep -Milliseconds $ms }
function Click($x, $y, $ms = 400) { EnsureApp; [W32]::SetCursorPos($x, $y) | Out-Null; Start-Sleep -Milliseconds 60; [W32]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); [W32]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds $ms }
function RClick($x, $y, $ms = 400) { EnsureApp; [W32]::SetCursorPos($x, $y) | Out-Null; Start-Sleep -Milliseconds 60; [W32]::mouse_event(8, 0, 0, 0, [UIntPtr]::Zero); [W32]::mouse_event(16, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds $ms }
function DblClick($x, $y, $ms = 400) { Click $x $y 80; Click $x $y $ms }
function Wheel($x, $y, $delta, $ms = 300) { EnsureApp; [W32]::SetCursorPos($x, $y) | Out-Null; Start-Sleep -Milliseconds 60; [W32]::WheelEv([int]$delta); Start-Sleep -Milliseconds $ms }
function Drag($x1, $y1, $x2, $y2, $ms = 400) { EnsureApp; [W32]::SetCursorPos($x1, $y1) | Out-Null; Start-Sleep -Milliseconds 60; [W32]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 80; $steps = 10; for ($i = 1; $i -le $steps; $i++) { [W32]::SetCursorPos([int]($x1 + ($x2 - $x1) * $i / $steps), [int]($y1 + ($y2 - $y1) * $i / $steps)) | Out-Null; Start-Sleep -Milliseconds 30 }; [W32]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds $ms }
function Focus($h) { [W32]::ShowWindow($h, 8) | Out-Null; [W32]::ForceFg($h); Start-Sleep -Milliseconds 300 }
function Wins($procId) { [W32]::ListWindows([uint32]$procId) }
function AppProcs { Get-Process bareline* -ErrorAction SilentlyContinue | Select-Object Id, ProcessName, MainWindowTitle, @{n='WS_MB';e={[math]::Round($_.WorkingSet64/1MB,1)}}, @{n='Priv_MB';e={[math]::Round($_.PrivateMemorySize64/1MB,1)}}, Handles, Threads }

function Max { EnsureApp; $h = (Get-Process bareline | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1).MainWindowHandle; $r = New-Object W32+RECT; [W32]::GetWindowRect($h, [ref]$r) | Out-Null; if ($r.L -ne -8) { [W32]::SetCursorPos(($r.R - 78), ($r.T + 15)) | Out-Null; Start-Sleep -Milliseconds 60; [W32]::mouse_event(2,0,0,0,[UIntPtr]::Zero); [W32]::mouse_event(4,0,0,0,[UIntPtr]::Zero); Start-Sleep -Milliseconds 600 } }
function ShotApp($name) { EnsureApp; Start-Sleep -Milliseconds 150; Shot $name }
function ClipText { Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Clipboard]::GetText() }
