# Installs the agent-switch CLI from the latest GitHub release (Windows).
#
# Usage:
#   irm https://raw.githubusercontent.com/AntyRia/agent-switch/main/scripts/install.ps1 | iex
#
# Downloads the latest release asset named
#   agent-switch-<version>-x86_64-pc-windows-msvc.zip
# installs it to %LOCALAPPDATA%\agent-switch\bin and adds that directory to
# your user PATH. The command is usable in the CURRENT terminal right away —
# no new terminal needed.

$ErrorActionPreference = "Stop"

$Repo = "AntyRia/agent-switch"

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "Only x86_64 Windows is published so far."
}

# Fetch the latest release and find our asset in it.
$api = "https://api.github.com/repos/$Repo/releases/latest"
Write-Host "Querying latest release of $Repo ..."
$rel = Invoke-RestMethod -Uri $api -Headers @{ "User-Agent" = "agent-switch-installer" }
$version = $rel.tag_name -replace '^v', ''
$AssetName = "agent-switch-$version-x86_64-pc-windows-msvc.zip"
$asset = $rel.assets | Where-Object { $_.name -eq $AssetName }
if (-not $asset) {
    throw "Asset '$AssetName' not found in release $($rel.tag_name). Published assets: $($rel.assets.name -join ', ')"
}

$installDir = Join-Path $env:LOCALAPPDATA "agent-switch\bin"
New-Item -ItemType Directory -Force -Path $installDir | Out-Null

$zip = Join-Path $env:TEMP "agent-switch-install.zip"
Write-Host "Downloading $AssetName ..."
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zip
Expand-Archive -Path $zip -DestinationPath $installDir -Force
Remove-Item $zip -Force
$exe = Join-Path $installDir "agent-switch.exe"
if (-not (Test-Path $exe)) {
    throw "agent-switch.exe not found after extraction. The release zip must contain the executable at its root."
}

# Add to the user PATH if it is not there yet (persists for all future
# terminals).
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPath -notlike "*$installDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$installDir", "User")
    Write-Host "Added $installDir to your user PATH."
}

# Prepend to the PATH of THIS process so `agent-switch` works in the current
# terminal session immediately — no new terminal needed.
$procPath = [Environment]::GetEnvironmentVariable("Path", "Process")
if ($procPath -notlike "*$installDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$installDir;$procPath", "Process")
}

# Prove it: call it the way the user will (unqualified).
agent-switch --version
Write-Host "agent-switch installed: $exe"
Write-Host "Ready — try: agent-switch add   (create your first profile)"
