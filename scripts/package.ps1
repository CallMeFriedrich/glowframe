<#
.SYNOPSIS
  Builds the release and creates both deliverables in dist\ (SPEC 9):
  Glowframe-<version>-portable.zip and Glowframe-<version>-setup.exe.
.PARAMETER SkipBuild
  Reuse the existing target\release\glowframe.exe.
#>
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $root

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "Glowframe $version"

if (-not $SkipBuild) {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
}

$dist = Join-Path $root 'dist'
$stage = Join-Path $dist 'stage'
New-Item -ItemType Directory -Force $stage | Out-Null
Copy-Item target\release\glowframe.exe (Join-Path $stage 'Glowframe.exe') -Force

# Portable ZIP: exe + portable.txt (settings, cache and logs next to the exe).
$portableDir = Join-Path $dist 'portable'
Remove-Item $portableDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $portableDir | Out-Null
Copy-Item (Join-Path $stage 'Glowframe.exe') $portableDir
$note = @(
    'Glowframe portable mode.',
    'While this file is next to Glowframe.exe, settings, cache and logs are stored in this folder.',
    'Portabler Modus: Solange diese Datei neben Glowframe.exe liegt, landen Einstellungen, Cache und Logs in diesem Ordner.'
)
[IO.File]::WriteAllLines((Join-Path $portableDir 'portable.txt'), $note, [Text.UTF8Encoding]::new($false))
$zip = Join-Path $dist "Glowframe-$version-portable.zip"
Remove-Item $zip -Force -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $portableDir '*') -DestinationPath $zip
Remove-Item $portableDir -Recurse -Force

# Installer (Inno Setup 6, per-user or machine-wide installation of ISCC).
$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw 'Inno Setup 6 (ISCC.exe) not found' }
& $iscc /Q "/DAppVersion=$version" installer\glowframe.iss
if ($LASTEXITCODE -ne 0) { throw 'ISCC failed' }

Get-ChildItem $dist -File | ForEach-Object {
    $hash = (Get-FileHash $_.FullName -Algorithm SHA256).Hash.Substring(0, 16)
    '{0,-40} {1,8:N0} KB  sha256 {2}...' -f $_.Name, ($_.Length / 1KB), $hash
}
