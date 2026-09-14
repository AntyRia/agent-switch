# Installs the newest published CLI package for Windows x64.
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

# Use a GitHub token when one is available: unauthenticated API calls from
# shared IPs (CI runners) can hit the 60/hour rate limit and get 403.
$headers = @{ "User-Agent" = "agent-switch-installer" }
$token = $env:GITHUB_TOKEN
if (-not $token) { $token = $env:GH_TOKEN }
if ($token) { $headers["Authorization"] = "Bearer $token" }

# Releases can contain packages for different platforms. Skip releases that
# do not have a Windows package, including drafts and prereleases.
Write-Host "Finding the latest Windows x64 package from $Repo ..."
$asset = $null
$page = 1
do {
    # Invoke-RestMethod returns the JSON array as one pipeline object. Assign
    # it directly so foreach enumerates releases instead of a nested array.
    $releases = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases?per_page=100&page=$page" -Headers $headers
    foreach ($rel in $releases) {
        if ($rel.draft -or $rel.prerelease) { continue }
        $version = $rel.tag_name -replace '^v', ''
        $AssetName = "agent-switch-$version-x86_64-pc-windows-msvc.zip"
        $asset = $rel.assets | Where-Object { $_.name -eq $AssetName } | Select-Object -First 1
        if ($asset) { break }
    }
    $page++
} while (-not $asset -and $releases.Count -gt 0)
if (-not $asset) {
    throw "No published package for Windows x64. See https://github.com/$Repo/releases or build from source."
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
