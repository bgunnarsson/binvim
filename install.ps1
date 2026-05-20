# binvim Windows installer.
#
#   iwr https://binvim.dev/install.ps1 -UseBasicParsing | iex
#
# Optional environment overrides:
#   $env:BINVIM_VERSION = 'v0.4.6'  # pin to a specific tag (default: latest)
#   $env:BINVIM_INSTALL_DIR = 'C:\bin'  # override install dir
#       (default: $env:LOCALAPPDATA\binvim\bin)

$ErrorActionPreference = 'Stop'

$repo = 'bgunnarsson/binvim'
$installDir = if ($env:BINVIM_INSTALL_DIR) { $env:BINVIM_INSTALL_DIR } else {
    Join-Path $env:LOCALAPPDATA 'binvim\bin'
}

function Info($msg) { Write-Host "==> $msg" -ForegroundColor Cyan }
function Fail($msg) { Write-Error $msg; exit 1 }

# Resolve the release tag — pinned via $env:BINVIM_VERSION or the
# latest GitHub release.
$version = $env:BINVIM_VERSION
if (-not $version) {
    Info 'resolving latest release'
    $api = Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest" -UseBasicParsing
    $version = $api.tag_name
    if (-not $version) { Fail 'could not resolve latest release; set $env:BINVIM_VERSION explicitly' }
}

# Only x86_64-pc-windows-msvc ships today; ARM64 Windows will need its own
# matrix entry in release.yml when that becomes a real target.
$target = 'x86_64-pc-windows-msvc'
$archive = "binvim-$version-$target.zip"
$url = "https://github.com/$repo/releases/download/$version/$archive"

$tmp = Join-Path $env:TEMP "binvim-install-$([guid]::NewGuid())"
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    Info "downloading $archive"
    Invoke-WebRequest -Uri $url -OutFile (Join-Path $tmp $archive) -UseBasicParsing

    Info 'extracting'
    Expand-Archive -Path (Join-Path $tmp $archive) -DestinationPath $tmp -Force

    New-Item -ItemType Directory -Path $installDir -Force | Out-Null
    Move-Item -Force (Join-Path $tmp 'binvim.exe') (Join-Path $installDir 'binvim.exe')
    if (Test-Path (Join-Path $tmp 'binvim-install.exe')) {
        Move-Item -Force (Join-Path $tmp 'binvim-install.exe') (Join-Path $installDir 'binvim-install.exe')
    }
    Info "installed binvim $version → $installDir\binvim.exe"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

# PATH hint — we don't mutate the registry on the user's behalf;
# print the one-liner instead so it's reversible.
$path = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not ($path -split ';' | Where-Object { $_ -eq $installDir })) {
    Write-Host ''
    Write-Host "note: $installDir is not on your user PATH."
    Write-Host '      add it for this session:'
    Write-Host "          `$env:Path = `"$installDir;`$env:Path`""
    Write-Host '      add it permanently:'
    Write-Host "          [Environment]::SetEnvironmentVariable('Path', `"$installDir;`" + [Environment]::GetEnvironmentVariable('Path','User'), 'User')"
}
