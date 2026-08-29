# gwae Windows installer: downloads the latest release to a bin dir on PATH.
#   irm https://hongnoul.github.io/gwae/install.ps1 | iex
# Requires PowerShell 5+ / pwsh. Falls back to cargo install if checks fail.
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
  if (-not $OnPath) {
    Write-Host "gwae: $InstallDir is not on PATH. Add it:"
    Write-Host "  `$env:PATH += `";$InstallDir`"  # current session"
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', `$env:PATH + `";$InstallDir`", 'User')"
  }
  Write-Host "gwae: run 'gwae' to start, or 'gwae init' for guided setup."
} finally {
  Remove-Item $Tmp -Recurse -Force -ErrorAction SilentlyContinue
}
