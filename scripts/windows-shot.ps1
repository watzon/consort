# Starts a program in the interactive desktop session of the current user, waits, and saves a
# PNG of its main window with PrintWindow (works when the session is locked or idle).
# Usage (over ssh): powershell -File scripts\windows-shot.ps1 -Exe <path> -Args "<args>" -Out <png> [-Wait 8] [-NoLaunch]
# With -NoLaunch it captures a window of a program that is already running.
param([string]$Exe, [string]$Args = "", [string]$Out, [int]$Wait = 8, [switch]$NoLaunch)
$ErrorActionPreference = "Stop"
$name = [System.IO.Path]::GetFileNameWithoutExtension($Exe)
$capture = Join-Path $env:TEMP "consort-capture.ps1"
@"
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class W {
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint f);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  public struct RECT { public int L, T, R, B; }
}
'@
[W]::SetProcessDPIAware() | Out-Null
$(if ($NoLaunch) { '' } else { "Start-Process -FilePath '$Exe' -ArgumentList '$Args'" })
Start-Sleep -Seconds $Wait
`$p = Get-Process -Name '$name' -ErrorAction SilentlyContinue | Where-Object { `$_.MainWindowHandle -ne 0 } | Select-Object -First 1
if (-not `$p) { 'no window' | Out-File '$Out.txt'; exit 1 }
`$r = New-Object W+RECT; [W]::GetWindowRect(`$p.MainWindowHandle, [ref]`$r) | Out-Null
`$bmp = New-Object System.Drawing.Bitmap ([Math]::Max(1, `$r.R - `$r.L)), ([Math]::Max(1, `$r.B - `$r.T))
`$g = [System.Drawing.Graphics]::FromImage(`$bmp); `$dc = `$g.GetHdc()
[W]::PrintWindow(`$p.MainWindowHandle, `$dc, 2) | Out-Null
`$g.ReleaseHdc(`$dc); `$bmp.Save('$Out', [System.Drawing.Imaging.ImageFormat]::Png)
"(`$(`$r.R - `$r.L))x(`$(`$r.B - `$r.T))" | Out-File '$Out.txt'
"@ | Set-Content -Path $capture -Encoding UTF8
schtasks /create /tn ConsortShot /tr "powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File $capture" /sc once /st 00:00 /it /f | Out-Null
schtasks /run /tn ConsortShot | Out-Null
Start-Sleep -Seconds ($Wait + 6)
schtasks /delete /tn ConsortShot /f | Out-Null
if (Test-Path "$Out.txt") { Get-Content "$Out.txt" } else { "capture did not run" }
