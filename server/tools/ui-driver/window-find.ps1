# Find laplus's window, whatever state it is in.
#
# Dot-sourced by `window-shot.ps1`, `window-drag.ps1` and `window-click.ps1`,
# which all begin by needing the same handle.
#
# **Not `Get-Process().MainWindowHandle`**, which is the obvious way and is
# wrong here in two compounding ways — both of which cost a run of silent
# false passes, because a script pointed at the wrong window does not fail, it
# clicks somewhere harmless and reports that nothing happened.
#
#   - `MainWindowTitle` is empty for a minimised window, so matching on it stops
#     finding laplus the moment the minimise button works.
#   - `MainWindowHandle` is not a Win32 concept; .NET computes it by walking the
#     process's top-level windows and taking the first that looks like a main
#     one. tao (under Tauri) keeps a 16x16 helper window at 0,0, and once the
#     real window is minimised that helper is what the walk returns. Clicks then
#     land on a phantom sixteen pixels wide in the corner of the screen.
#
# EnumWindows with an explicit title match avoids both: `GetWindowText` answers
# for a minimised window, and the title is the one thing that says which of a
# process's windows is the one a person is looking at.

Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public static class WindowFinder {
  private delegate bool EnumProc(IntPtr hWnd, IntPtr param);

  [DllImport("user32.dll")] private static extern bool EnumWindows(EnumProc proc, IntPtr param);
  [DllImport("user32.dll")] private static extern int GetWindowThreadProcessId(IntPtr hWnd, out int pid);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)]
  private static extern int GetWindowTextW(IntPtr hWnd, StringBuilder text, int count);

  public static IntPtr ByTitle(string title) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr hWnd, IntPtr param) {
      StringBuilder text = new StringBuilder(512);
      GetWindowTextW(hWnd, text, text.Capacity);
      if (text.ToString() == title) {
        found = hWnd;
        return false;
      }
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
'@

function Find-LaplusWindow {
  param([string]$Title = "laplus")

  $handle = [WindowFinder]::ByTitle($Title)
  if ($handle -eq [IntPtr]::Zero) { return $null }
  return $handle
}
