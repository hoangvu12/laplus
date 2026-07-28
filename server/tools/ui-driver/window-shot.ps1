# Photograph laplus's own window, frame and all.
#
# `cdp.mjs` beside this drives the UI in a headless Chrome, which is the right
# tool for everything the *page* does and is blind to the one thing ticket 27 is
# about: what the window looks like around the page. A browser has no window
# controls to get wrong and no frame to remove.
#
# Captures from the screen rather than with PrintWindow: WebView2 composes
# through DirectComposition, and PrintWindow returns a blank client area for it
# on Windows 11. So the window is raised first, and what lands in the PNG is
# what a person sitting here would see.
#
#   powershell -File tools/ui-driver/window-shot.ps1 -Out .scratch/shot.png
#
# -Title matches the window title (laplus's is "laplus"). -SettleMs waits for
# the raise and any animation to finish before the shutter.

param(
  [string]$Title = "laplus",
  [string]$Out = "window.png",
  [int]$SettleMs = 600
)

. "$PSScriptRoot/window-find.ps1"

Add-Type -AssemblyName System.Drawing

Add-Type @'
using System;
using System.Runtime.InteropServices;

public static class Win {
  [StructLayout(LayoutKind.Sequential)]
  public struct RECT { public int Left, Top, Right, Bottom; }

  [DllImport("user32.dll")]
  public static extern bool SetForegroundWindow(IntPtr hWnd);

  [DllImport("user32.dll")]
  public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);

  [DllImport("user32.dll")]
  public static extern bool IsIconic(IntPtr hWnd);

  // The window's rectangle including any frame. DwmGetWindowAttribute rather
  // than GetWindowRect: since Windows 10 the latter reports the invisible
  // resize border too, so a capture of it carries a margin of desktop on three
  // sides — which in a screenshot of a frameless window reads as a frame.
  [DllImport("dwmapi.dll")]
  public static extern int DwmGetWindowAttribute(IntPtr hWnd, int attribute, out RECT value, int size);

  public const int DWMWA_EXTENDED_FRAME_BOUNDS = 9;
  public const int SW_RESTORE = 9;
}
'@

$handle = Find-LaplusWindow $Title
if (-not $handle) {
  Write-Error "no window titled '$Title'. Is laplus running?"
  exit 1
}

# Only if it is minimised. SW_RESTORE un-maximises a maximised window, which
# would quietly photograph a different window than the one asked about — and
# maximised is the state a frameless window is most likely to get wrong.
if ([Win]::IsIconic($handle)) {
  [void][Win]::ShowWindow($handle, [Win]::SW_RESTORE)
}
[void][Win]::SetForegroundWindow($handle)
Start-Sleep -Milliseconds $SettleMs

$rect = New-Object Win+RECT
$result = [Win]::DwmGetWindowAttribute($handle, [Win]::DWMWA_EXTENDED_FRAME_BOUNDS, [ref]$rect, 16)
if ($result -ne 0) {
  Write-Error "DwmGetWindowAttribute failed: $result"
  exit 1
}

$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
$bitmap = New-Object System.Drawing.Bitmap $width, $height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
$graphics.Dispose()

# Combine rather than Join-Path: -Out is often an absolute path, and Join-Path
# would glue it onto the working directory and produce something Windows will
# not open.
$path = [System.IO.Path]::GetFullPath([System.IO.Path]::Combine((Get-Location).Path, $Out))
$bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
$bitmap.Dispose()

Write-Output "$path  ${width}x${height} at $($rect.Left),$($rect.Top)"
