# Adds Python 3.12 user install dirs to the MACHINE (system) PATH.
# Requires: Run as Administrator (Right-click PowerShell -> Run as administrator).

$ErrorActionPreference = "Stop"
if (-not ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    Write-Error "Run this script in an elevated PowerShell (Run as administrator)."
}

$pyRoot = Join-Path $env:LOCALAPPDATA "Programs\Python\Python312"
$scripts = Join-Path $pyRoot "Scripts"
$launcher = Join-Path $env:LOCALAPPDATA "Programs\Python\Launcher"
if (-not (Test-Path (Join-Path $pyRoot "python.exe"))) {
    Write-Error "python.exe not found at: $pyRoot"
}

$toAdd = @($pyRoot, $scripts, $launcher) | ForEach-Object { (New-Object System.IO.FileInfo $_).FullName.TrimEnd('\') }
$machine = [Environment]::GetEnvironmentVariable("Path", "Machine")
$parts = @($machine -split ';' | ForEach-Object { $_.Trim().TrimEnd('\') } | Where-Object { $_ -ne '' })
$norm = [System.Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
$ordered = [System.Collections.Generic.List[string]]::new()
foreach ($p in $parts) {
    try { $k = (New-Object System.IO.FileInfo $p).FullName.TrimEnd('\') } catch { $k = $p }
    if ($norm.Add($k)) { $ordered.Add($k) }
}
foreach ($p in $toAdd) {
    if ($norm.Add($p)) { $ordered.Add($p) }
}
$newMachine = ($ordered -join ';')
[Environment]::SetEnvironmentVariable("Path", $newMachine, "Machine")
Write-Host "Updated system PATH. Open a new terminal (or sign out/in) and run: python --version"
