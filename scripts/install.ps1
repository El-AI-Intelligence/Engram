# Engram by El AI Intelligence Windows installer - no admin rights, installs to ~/.local/bin.
#
# Usage:
#   iex (irm https://engram.ellmstack.dev/install.ps1)
# or:
#   powershell -ExecutionPolicy Bypass -File install.ps1
#
# Downloads engramd-windows-x86_64.zip from GitHub Releases (by default -
# override with the ENGRAM_RELEASE_BASE env var), verifies its sha256 sidecar,
# and installs engram.exe / engramd.exe / engramd-mcp.exe. Never touches
# secrets; the vault passphrase is entered interactively by `engram onboarding` later.

$ErrorActionPreference = 'Stop'

# PowerShell 5.1 defaults to TLS 1.0 for Invoke-WebRequest - modern servers refuse it.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12

$ReleaseBase = if ($env:ENGRAM_RELEASE_BASE) { $env:ENGRAM_RELEASE_BASE } else { "https://github.com/El-AI-Intelligence/engram/releases/latest/download" }
$InstallDir  = if ($env:ENGRAM_INSTALL_DIR)  { $env:ENGRAM_INSTALL_DIR }  else { Join-Path $env:USERPROFILE ".local\bin" }
$ZipName     = "engramd-windows-x86_64.zip"

Write-Host ""
Write-Host "  Engram by El AI Intelligence - Windows installer"
Write-Host "  ----------------------------------------"
Write-Host ""

# -- Download --------------------------------------------------------------
$tmpZip = Join-Path $env:TEMP $ZipName
Write-Host "  Downloading $ReleaseBase/$ZipName ..."
Invoke-WebRequest -Uri "$ReleaseBase/$ZipName" -OutFile $tmpZip

Write-Host "  Downloading checksum ..."
$tmpSha = Join-Path $env:TEMP "$ZipName.sha256"
Invoke-WebRequest -Uri "$ReleaseBase/$ZipName.sha256" -OutFile $tmpSha

# -- Verify ----------------------------------------------------------------
$expected = (Get-Content $tmpSha | Select-Object -First 1).Split(' ')[0].Trim().ToLower()
$actual   = (Get-FileHash -Algorithm SHA256 $tmpZip).Hash.ToLower()
if ($actual -ne $expected) {
    Write-Host "  [FAIL] Checksum mismatch - refusing to install."
    Write-Host "     expected: $expected"
    Write-Host "     actual:   $actual"
    exit 1
}
Write-Host "  [OK] Checksum verified."

# -- Install ---------------------------------------------------------------
New-Item -ItemType Directory -Force $InstallDir | Out-Null
$extract = Join-Path $env:TEMP "engram-install-extract"
if (Test-Path $extract) { Remove-Item -Recurse -Force $extract }
Expand-Archive -Path $tmpZip -DestinationPath $extract -Force

Get-ChildItem -Path $extract -Filter "*.exe" | ForEach-Object {
    Copy-Item $_.FullName -Destination $InstallDir -Force
}

# -- PATH (User scope - no admin needed) -----------------------------------
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable('Path', "$InstallDir;$userPath", 'User')
    Write-Host "  [OK] Added $InstallDir to your user PATH (new terminals pick it up)."
}
$env:Path = "$InstallDir;$env:Path"

Write-Host ""
Write-Host "  [OK] Engram by El AI Intelligence installed to $InstallDir"
Write-Host ""
Write-Host "  Note: the first time you run engram, SmartScreen may show"
Write-Host "  'Windows protected your PC' - click More info > Run anyway."
Write-Host "  (Our Windows builds are unsigned for now.)"
Write-Host ""
Write-Host "  Next steps:"
Write-Host "    engram onboarding    # vault + first memory + running daemon (~5 min)"
Write-Host "    engram mcp install   # connect Claude Code, Claude Desktop, Cursor, Windsurf"
Write-Host "    engram pair ENG-XXXX # link this machine to your Engram account"
Write-Host "    http://localhost:8787        Vault UI once the daemon is running"
Write-Host ""
