# Drag laplus's window by its topbar, and say whether the window moved.
#
# Ticket 27's "reopen it when there is a person willing to watch a window" made
# a measurement, so that the answer is a number rather than an impression. A
# frameless window whose drag regions do not work cannot be moved at all, and
# that failure is invisible from the page: the click lands, nothing happens, and
# the console says nothing unless the IPC refused the command out loud.
#
#   powershell -File tools/ui-driver/window-drag.ps1 -X 600 -Y 26
#
# -X and -Y are client coordinates inside the window, so they name a point on
# the bar rather than a point on this monitor. The default is the middle of the
# chat topbar, clear of the buttons at either end.

param(
  [string]$Title = "laplus",
  [int]$X = 600,
  [int]$Y = 26,
  [int]$By = 120
)

. "$PSScriptRoot/window-find.ps1"

Add-Type @'
using System;
using System.Runtime.InteropServices;

public static class Drag {
  [StructLayout(LayoutKind.Sequential)]
  public struct RECT { public int Left, Top, Right, Bottom; }

  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, IntPtr extra);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr hWnd, int attribute, out RECT value, int size);

  public const uint LEFTDOWN = 0x0002;
  public const uint LEFTUP = 0x0004;
  public const int FRAME_BOUNDS = 9;
}
'@

function Get-Bounds($handle) {
  $rect = New-Object Drag+RECT
  [void][Drag]::DwmGetWindowAttribute($handle, [Drag]::FRAME_BOUNDS, [ref]$rect, 16)
  return $rect
}

$handle = Find-LaplusWindow $Title
if (-not $handle) { Write-Error "no window titled '$Title'"; exit 1 }

[void][Drag]::SetForegroundWindow($handle)
Start-Sleep -Milliseconds 400

$before = Get-Bounds $handle
$startX = $before.Left + $X
$startY = $before.Top + $Y

# Press, then move in steps. One jump from press to release is not a drag: the
# webview starts dragging on mousedown and Windows then runs its own modal move
# loop, which needs to see the pointer travel.
[void][Drag]::SetCursorPos($startX, $startY)
Start-Sleep -Milliseconds 120
[Drag]::mouse_event([Drag]::LEFTDOWN, 0, 0, 0, [IntPtr]::Zero)
for ($step = 1; $step -le 12; $step++) {
  [void][Drag]::SetCursorPos($startX + [int]($By * $step / 12), $startY + [int]($By * $step / 12))
  Start-Sleep -Milliseconds 30
}
Start-Sleep -Milliseconds 200
[Drag]::mouse_event([Drag]::LEFTUP, 0, 0, 0, [IntPtr]::Zero)
Start-Sleep -Milliseconds 400

$after = Get-Bounds $handle
$movedX = $after.Left - $before.Left
$movedY = $after.Top - $before.Top

Write-Output "asked to move by $By,$By at client $X,$Y"
Write-Output "window moved by $movedX,$movedY"
if ($movedX -eq 0 -and $movedY -eq 0) {
  Write-Output "RESULT: the topbar does not drag the window"
  exit 1
}
Write-Output "RESULT: the topbar drags the window"
