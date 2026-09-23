# gwae installer for Windows (PowerShell).
#   irm https://hongnoul.github.io/gwae/install.ps1 | iex
# Fallback: https://raw.githubusercontent.com/hongnoul/gwae/main/scripts/install.ps1
#
# Installs gwae.exe into %LOCALAPPDATA%\gwae\bin and adds that directory to
# the user PATH (registry, no admin rights needed). Override the directory
# with $env:GWAE_INSTALL_DIR. Set $env:GWAE_NO_MODIFY_PATH = "1" to skip the
# PATH edit (CI, scripted setups); instructions are printed instead.
$ErrorActionPreference = 'Stop'

$Repo = 'hongnoul/gwae'

# Transcript-as-logo: every output line carries a row of the pixel mark as
# its left gutter, cycling every 5 rows, so the left logo stays visible no
# matter how many lines print and logs only ever stack on the right.
$script:LogoRow = 0
function Get-Gutter {
    $script:LogoRow++
    switch (($script:LogoRow - 1) % 5 + 1) {
        1 { '  ▄▄▄▄ ▄ ▄' }
        2 { '   ▄ █ █▄█' }
        3 { '   █ █ █ █' }
        4 { '   █ ▀ █ █' }
        5 { '  ▀▀▀▀ ▀ ▀' }
    }
}

function Say([string]$Message) {
    $g = Get-Gutter
    Write-Host "$g   > " -NoNewline
    Write-Host $Message
}

function Ok([string]$Message) {
    $g = Get-Gutter
    Write-Host "$g   " -NoNewline
    Write-Host '>' -ForegroundColor Green -NoNewline
    Write-Host " $Message"
}

function Fail([string]$Message) {
    $g = Get-Gutter
    Write-Host "$g   > $Message" -ForegroundColor Red
    # Close the current mark so even a failed run leaves a whole logo.
    while ($script:LogoRow % 5 -ne 0) {
        Write-Host (Get-Gutter)
    }
    # `throw`, not `exit`: under `irm | iex` an `exit` would close the
    # user's PowerShell session, not just this installer.
    throw "gwae install failed: $Message"
}

Write-Host ''

# --- platform ------------------------------------------------------------------
$arch = if ([System.Environment]::Is64BitOperatingSystem) {
    $procArch = $env:PROCESSOR_ARCHITECTURE
    if ($procArch -eq 'ARM64') { 'aarch64' } else { 'x86_64' }
} else {
    Fail 'gwae requires 64-bit Windows'
}
if ($arch -eq 'aarch64') {
    # No native ARM64 build yet; the x64 binary runs under emulation.
    $arch = 'x86_64'
}
Ok "detected windows/$arch"
$target = "$arch-pc-windows-msvc"
$artifact = "gwae-$target"

# --- install dir ---------------------------------------------------------------
if ($env:GWAE_INSTALL_DIR) {
    $installDir = $env:GWAE_INSTALL_DIR
} else {
    $installDir = Join-Path $env:LOCALAPPDATA 'gwae\bin'
}

# --- download ------------------------------------------------------------------
# The /releases/latest/download/ redirect avoids api.github.com rate limits.
$url = "https://github.com/$Repo/releases/latest/download/$artifact.zip"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) "gwae-install-$PID"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null
try {
    Say 'downloading latest release...'
    $zipPath = Join-Path $tmp 'pkg.zip'
    try {
        Invoke-WebRequest -Uri $url -OutFile $zipPath -UseBasicParsing
    } catch {
        Fail "download failed: $url"
    }

    # --- checksum (required: every release ships a .sha256) ---------------------
    $shaPath = Join-Path $tmp 'pkg.sha256'
    try {
        Invoke-WebRequest -Uri "$url.sha256" -OutFile $shaPath -UseBasicParsing
    } catch {
        Fail "could not fetch $artifact.zip.sha256; refusing to install without verification"
    }
    $expected = ((Get-Content $shaPath -Raw).Trim() -split '\s+')[0].ToLowerInvariant()
    $actual = (Get-FileHash -Algorithm SHA256 -Path $zipPath).Hash.ToLowerInvariant()
    if ($expected -ne $actual) { Fail 'checksum verification failed' }
    Ok 'checksum verified'

    # --- install (atomic: extract -> staging name -> rename) ---------------------
    Expand-Archive -Path $zipPath -DestinationPath $tmp -Force
    $exeSrc = Join-Path $tmp 'gwae.exe'
    if (-not (Test-Path $exeSrc)) { Fail 'archive did not contain gwae.exe' }
    New-Item -ItemType Directory -Force -Path $installDir | Out-Null
    $staged = Join-Path $installDir "gwae.new.$PID.exe"
    Copy-Item $exeSrc $staged -Force
    $dest = Join-Path $installDir 'gwae.exe'
    Move-Item $staged $dest -Force

    # Report the version by asking the installed binary, which also proves it
    # executes on this machine before the user ever runs it.
    $version = $null
    try { $version = (& $dest --version 2>&1 | Select-Object -First 1) } catch {}
    if (-not $version) { Fail "installed binary at $dest does not run on this machine" }
    $versionNum = ($version -split '\s+')[-1]
    Ok "installed $versionNum to $dest"

    # --- receipt -----------------------------------------------------------------
    # Record *how* gwae got here, so `gwae upgrade` knows the route instead of
    # guessing it from the install path. State, not config: machine-written
    # bookkeeping; a missing or stale receipt just means "detect from the path".
    $stateDir = Join-Path $env:LOCALAPPDATA 'gwae\state'
    New-Item -ItemType Directory -Force -Path $stateDir | Out-Null
    @"
# Written by gwae's install.ps1; read by ``gwae upgrade``. Safe to delete.
source = "install.ps1"
dir = "$($installDir -replace '\\', '\\')"
version = "$versionNum"
"@ | Set-Content -Path (Join-Path $stateDir 'install.toml') -Encoding UTF8

    # --- PATH --------------------------------------------------------------------
    # User-scope PATH via the registry: no admin rights, survives restarts.
    # Fresh terminals pick it up; the current session gets $env:Path updated
    # directly so `gwae` works immediately.
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $onPath = ($userPath -split ';' | Where-Object { $_ -eq $installDir }).Count -gt 0
    if (-not $onPath) {
        if ($env:GWAE_NO_MODIFY_PATH) {
            Say "$installDir is not on your PATH. Add it in Settings > System > About > Advanced system settings > Environment Variables."
        } else {
            $newPath = if ($userPath) { "$installDir;$userPath" } else { $installDir }
            [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
            Ok "added $installDir to your user PATH (fresh terminals pick it up automatically)"
        }
    }
    if (($env:Path -split ';' | Where-Object { $_ -eq $installDir }).Count -eq 0) {
        $env:Path = "$installDir;$env:Path"
    }

    Ok 'ready. run gwae to get started.'
    Write-Host ''
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
