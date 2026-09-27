# Per-user install of the Bastion desktop shell (BMD-28): no administrator
# rights, installs under %LOCALAPPDATA%, a Start-Menu shortcut, and optional
# autostart. Signing is a separate release step (see README.md); this script
# installs an already-built (ideally signed) bastion-shell.exe.
#
#   powershell -ExecutionPolicy Bypass -File install.ps1 `
#       -Exe .\target\release\bastion-shell.exe `
#       -PrimaryUrl https://linux-box.tailnet.ts.net:8443 `
#       -OwnerToken <paired-device-token> [-Autostart]

param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$PrimaryUrl,
    [string]$OwnerToken = "",
    [switch]$Autostart
)

$ErrorActionPreference = "Stop"

$installDir = Join-Path $env:LOCALAPPDATA "Bastion"
$configDir  = Join-Path $env:APPDATA "bastion\bastion"   # matches directories crate
New-Item -ItemType Directory -Force -Path $installDir, $configDir | Out-Null

$target = Join-Path $installDir "bastion-shell.exe"
Copy-Item -Path $Exe -Destination $target -Force
Write-Host "Installed $target"

$config = [ordered]@{ primary_url = $PrimaryUrl }
if ($OwnerToken -ne "") { $config.owner_token = $OwnerToken }
$configPath = Join-Path $configDir "shell.json"
$config | ConvertTo-Json | Set-Content -Path $configPath -Encoding UTF8
Write-Host "Wrote $configPath"

# Start-Menu shortcut (per user).
$startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
$shortcut  = Join-Path $startMenu "Bastion.lnk"
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut($shortcut)
$link.TargetPath = $target
$link.Save()
Write-Host "Created Start-Menu shortcut"

if ($Autostart) {
    # HKCU Run key — per-user autostart, no admin.
    $runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
    Set-ItemProperty -Path $runKey -Name "Bastion" -Value "`"$target`""
    Write-Host "Enabled autostart at login"
}

Write-Host "Done. Launch Bastion from the Start Menu; it lives in the tray."
