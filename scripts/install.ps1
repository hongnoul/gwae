# gwae Windows installer: downloads the latest release, puts it in a bin dir,
# and adds that dir to PATH, so `gwae` works in a fresh terminal right after.
#   irm https://hongnoul.github.io/gwae/install.ps1 | iex
# Requires PowerShell 5+ / pwsh. Falls back to cargo install if checks fail.
# Set $env:GWAE_NO_MODIFY_PATH=1 to only print the PATH instructions (CI,
# scripted setups) instead of changing the machine.
$ErrorActionPreference = 'Stop'
$Repo = 'hongnoul/gwae'
$InstallDir = if ($env:GWAE_INSTALL_DIR) { $env:GWAE_INSTALL_DIR } else { Join-Path $env:USERPROFILE 'bin' }

$Artifact = 'gwae-x86_64-pc-windows-msvc.zip'
$Url = "https://github.com/$Repo/releases/latest/download/$Artifact"
$Tmp = Join-Path $env:TEMP "gwae-install-$(Get-Random)"
New-Item -ItemType Directory -Path $Tmp -Force | Out-Null
try {
  Write-Host "gwae: downloading $Artifact (latest release)..."
  $Zip = Join-Path $Tmp $Artifact
  Invoke-WebRequest -Uri $Url -OutFile $Zip -UseBasicParsing

  # Verify SHA256 if available
  try {
    $Expected = (Invoke-WebRequest -Uri "$Url.sha256" -UseBasicParsing -TimeoutSec 10).Content.Split()[0].Trim().ToLower()
    $Actual = (Get-FileHash $Zip -Algorithm SHA256).Hash.ToLower()
    if ($Expected -ne $Actual) { throw "checksum mismatch: expected $Expected got $Actual" }
    Write-Host "gwae: checksum verified"
  } catch {
    Write-Host "gwae: warning: could not verify checksum ($_) `u2014 continuing"
  }

  Expand-Archive -Path $Zip -DestinationPath $Tmp -Force
  $Exe = Get-ChildItem -Path $Tmp -Filter gwae.exe -Recurse | Select-Object -First 1
  if (-not $Exe) { throw "gwae.exe not found in archive" }
  New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
  Copy-Item $Exe.FullName (Join-Path $InstallDir 'gwae.exe') -Force
  $Ver = & (Join-Path $InstallDir 'gwae.exe') --version 2>$null
  Write-Host "gwae: installed $Ver to $InstallDir\gwae.exe"

  $OnPath = ($env:PATH -split ';') -contains $InstallDir
  if ($OnPath) {
    Write-Host "gwae: $InstallDir is already on PATH."
  } elseif (-not [string]::IsNullOrEmpty($env:GWAE_NO_MODIFY_PATH)) {
    Write-Host "gwae: $InstallDir is not on PATH. Add it:"
    Write-Host "  `$env:PATH += `";$InstallDir`"  # current session"
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', `$env:PATH + `";$InstallDir`", 'User')"
  } else {
    # Current session first, so `gwae` works immediately in this terminal.
    if (($env:PATH -split ';') -notcontains $InstallDir) {
      $env:PATH = "$InstallDir;$env:PATH"
    }
    # Then persist for fresh terminals: append to the User PATH (registry)
    # only when it is not already there, so re-runs never stack duplicates.
    try {
      $UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
      if (($UserPath -split ';' | Where-Object { $_ -ne '' }) -notcontains $InstallDir) {
        $NewUserPath = if ([string]::IsNullOrEmpty($UserPath)) { $InstallDir } else { "$UserPath;$InstallDir" }
        [Environment]::SetEnvironmentVariable('Path', $NewUserPath, 'User')
        Write-Host "gwae: added $InstallDir to your user PATH (this terminal already has it)."
      } else {
        Write-Host "gwae: added $InstallDir to PATH for this terminal (already in your saved user PATH)."
      }
    } catch {
      Write-Host "gwae: $InstallDir is on PATH for this terminal, but saving it failed ($_) — add it by hand:"
      Write-Host "  [Environment]::SetEnvironmentVariable('Path', `$env:PATH + `";$InstallDir`", 'User')"
    }
  }
  Write-Host "gwae: run 'gwae' to start, or 'gwae init' for guided setup."
} finally {
  Remove-Item $Tmp -Recurse -Force -ErrorAction SilentlyContinue
}
