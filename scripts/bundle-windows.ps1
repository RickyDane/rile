# Builds the Windows release artifacts into dist/:
#   rile-<triple>.zip            bare rile.exe, used by the in-app updater
#   Rile-<version>-x64-setup.exe per-user Inno Setup installer
#
# Usage: scripts/bundle-windows.ps1 [-Target x86_64-pc-windows-msvc] [-SkipBuild]
# Needs the Windows SDK (gpui compiles its shaders with fxc.exe) and Inno
# Setup 6 (iscc.exe on PATH or in its default folder).
param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [switch]$SkipBuild
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
if (-not $SkipBuild) {
    cargo build --release --locked --target $Target -p rile-app
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
$exe = Join-Path $root "target\$Target\release\rile.exe"
if (-not (Test-Path $exe)) { throw "Missing $exe" }

$dist = Join-Path $root "dist"
New-Item -ItemType Directory -Force -Path $dist | Out-Null

# Updater asset: the archive must contain rile.exe (see crates/rile-services/src/updater.rs).
$zip = Join-Path $dist "rile-$Target.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path $exe -DestinationPath $zip

$iscc = (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source
if (-not $iscc) { $iscc = "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" }
if (-not (Test-Path $iscc)) { throw "Inno Setup 6 not found (choco install innosetup)" }
& $iscc "/DAppVersion=$version" "/DSourceExe=$exe" "/DOutputDir=$dist" "packaging\windows\rile.iss"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "Built $zip and $(Join-Path $dist "Rile-$version-x64-setup.exe")"
