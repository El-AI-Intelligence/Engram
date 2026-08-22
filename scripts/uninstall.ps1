# Engram by El AI Intelligence Windows uninstaller - removes binaries, the
# background task, the user PATH entry, and (only when you say yes) the
# vault data directory.
#
# Usage:
#   iex (irm https://engram.ellmstack.dev/uninstall.ps1)
#
# Never deletes vault data without an explicit "y". Editor MCP configs
# written by `engram mcp install` are left alone. ENGRAM_DRY_RUN=1 prints
# every action without doing it. No admin rights needed.

$ErrorActionPreference = 'Continue'

$InstallDir = if ($env:ENGRAM_INSTALL_DIR) { $env:ENGRAM_INSTALL_DIR } else { Join-Path $env:USERPROFILE ".local\bin" }
$DryRun = $env:ENGRAM_DRY_RUN -eq '1'

Write-Host ""
Write-Host "  Engram by El AI Intelligence - uninstaller"
Write-Host "  ------------------------------------------"
Write-Host ""

# -- Binaries ---------------------------------------------------------------
foreach ($bin in @('engram.exe', 'engramd.exe', 'engramd-mcp.exe')) {
  $p = Join-Path $InstallDir $bin
  if (Test-Path $p) {
    if (-not $DryRun) { Remove-Item -Force $p }
    Write-Host "  [OK] Removed $p"
  } else {
    Write-Host "  [--] Not found: $p"
  }
}

# -- Running daemon -----------------------------------------------------------
$proc = Get-Process engramd -ErrorAction SilentlyContinue
if ($proc) {
  if (-not $DryRun) { Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue }
  Write-Host "  [OK] Stopped running engramd"
} else {
  Write-Host "  [--] No running engramd process"
}

# -- Background task (installed by `engram onboarding`) ---------------------
$task = Get-ScheduledTask -TaskName 'Engramd' -ErrorAction SilentlyContinue
if ($task) {
  if (-not $DryRun) {
    Stop-ScheduledTask -TaskName 'Engramd' -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName 'Engramd' -Confirm:$false -ErrorAction SilentlyContinue
  }
  Write-Host "  [OK] Removed scheduled task Engramd"
} else {
  Write-Host "  [--] No scheduled task named Engramd"
}

# -- PATH (user scope) -------------------------------------------------------
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($userPath) {
  $parts = $userPath -split ';' | Where-Object { $_ -and ($_.TrimEnd('\') -ne $InstallDir.TrimEnd('\')) }
  if (($parts -join ';') -ne $userPath) {
    if (-not $DryRun) { [Environment]::SetEnvironmentVariable('Path', ($parts -join ';'), 'User') }
    Write-Host "  [OK] Removed $InstallDir from your user PATH (new terminals pick it up)"
  } else {
    Write-Host "  [--] $InstallDir was not in your user PATH"
  }
}

# -- Vault data (optional - never without an explicit "y") -------------------
$DataDir = Join-Path $env:USERPROFILE ".engram"
Write-Host ""
if (Test-Path $DataDir) {
  $answer = 'n'
  if ($DryRun) {
    Write-Host "  [dry-run] Would ask: remove vault data at $DataDir? [y/N]"
  } else {
    $answer = Read-Host "  Remove vault data at $DataDir (memories, passphrase, config)? [y/N]"
  }
  if ($answer -eq 'y' -or $answer -eq 'Y') {
    if (-not $DryRun) { Remove-Item -Recurse -Force $DataDir }
    Write-Host "  [OK] Removed $DataDir"
  } else {
    Write-Host "  [--] Kept $DataDir (run this again and answer y to remove it)"
  }
} else {
  Write-Host "  [--] No vault data at $DataDir"
}

Write-Host ""
Write-Host "  Done. Editor integrations (engram mcp install) were left untouched."
Write-Host ""
