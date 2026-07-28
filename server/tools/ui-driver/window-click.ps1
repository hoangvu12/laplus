# Click a point inside laplus's window, and say what the window did.
#
# The other half of `window-drag.ps1`: the three buttons the page draws are the
# only way to minimise, maximise or close a window with no frame, and a button
# denied by the capability looks exactly like a button that works. This presses
# one and then reads the window's state back from Windows rather than from the
# page, so the answer does not come from the thing being tested.
#
#   powershell -File tools/ui-driver/window-click.ps1 -FromRight 115 -Y 20
#
# -FromRight is client pixels in from the window's right edge, because that is
# where the controls are and the window's width is not this script's business.

param(
  [string]$Title = "laplus",
  [int]$FromRight = 115,
  [int]$Y = 20,
  [switch]$DoubleClick
)

. "$PSScriptRoot/window-find.ps1"

Add-Type @'
using System;
using System.Runtime.InteropServices;

public static class Click {
  [StructLayout(LayoutKind.Sequential)]
  public struct RECT { public int Left, Top, Right, Bottom; }

  [StructLayout(LayoutKind.Sequential)]
  public struct WINDOWPLACEMENT {
    public int length, flags, showCmd;
    public int minX, minY, maxX, maxY;
    public int normalLeft, normalTop, normalRight, normalBottom;
  }

  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, IntPtr extra);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool GetWindowPlacement(IntPtr hWnd, ref WINDOWPLACEMENT p);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr hWnd, int attribute, out RECT value, int size);

  public const uint LEFTDOWN = 0x0002;
  public const uint LEFTUP = 0x0004;
  public const int FRAME_BOUNDS = 9;
}
'@

function Report($handle, $label) {
  if (-not [Click]::IsWindowVisible($handle)) { Write-Output "${label}: gone"; return }
  $state = "restored"
  if ([Click]::IsIconic($handle)) { $state = "minimised" }
  elseif ([Click]::IsZoomed($handle)) { $state = "maximised" }
  Write-Output "${label}: $state"
}

$handle = Find-LaplusWindow $Title
if (-not $handle) { Write-Error "no window titled '$Title'"; exit 1 }

Report $handle "before"

# A minimised window has no on-screen buttons to press, so bring it back first
# — after reporting, so the run still says what it started from. SW_RESTORE
# only, and only when minimised: on a maximised window it would un-maximise,
# which is a state change this script did not ask for and would then blame on
# the button.
if ([Click]::IsIconic($handle)) {
  [void][Click]::ShowWindow($handle, 9)
  Start-Sleep -Milliseconds 500
}
[void][Click]::SetForegroundWindow($handle)
Start-Sleep -Milliseconds 400

$rect = New-Object Click+RECT
[void][Click]::DwmGetWindowAttribute($handle, [Click]::FRAME_BOUNDS, [ref]$rect, 16)
$x = $rect.Right - $FromRight
$y = $rect.Top + $Y

[void][Click]::SetCursorPos($x, $y)
Start-Sleep -Milliseconds 250
[Click]::mouse_event([Click]::LEFTDOWN, 0, 0, 0, [IntPtr]::Zero)
[Click]::mouse_event([Click]::LEFTUP, 0, 0, 0, [IntPtr]::Zero)
if ($DoubleClick) {
  Start-Sleep -Milliseconds 60
  [Click]::mouse_event([Click]::LEFTDOWN, 0, 0, 0, [IntPtr]::Zero)
  [Click]::mouse_event([Click]::LEFTUP, 0, 0, 0, [IntPtr]::Zero)
}
Start-Sleep -Milliseconds 900

Report $handle "after"
