<#
.SYNOPSIS
  Clean-system test in Windows Sandbox (SPEC 10, criterion 11).
  Requires dist\ from scripts\package.ps1. Starts a sandbox that installs,
  runs and uninstalls Glowframe and tests the portable ZIP; results end up
  in dist\sandbox-results\ (report.txt, logs, screenshots).
#>
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..')
$dist = Join-Path $root 'dist'
$results = Join-Path $dist 'sandbox-results'
if (-not (Get-ChildItem $dist -Filter 'Glowframe-*-setup.exe' -ErrorAction SilentlyContinue)) {
    throw 'dist\ is empty, run scripts\package.ps1 first'
}
# Only one sandbox can run at a time; a second start fails silently.
if (Get-Process -Name 'WindowsSandbox*' -ErrorAction SilentlyContinue) {
    throw 'Windows Sandbox is already running. Close it first, then run this script again.'
}
Remove-Item $results -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $results | Out-Null

$wsb = @"
<Configuration>
  <Networking>Enable</Networking>
  <vGPU>Enable</vGPU>
  <MappedFolders>
    <MappedFolder><HostFolder>$dist</HostFolder><SandboxFolder>C:\glowframe\dist</SandboxFolder><ReadOnly>true</ReadOnly></MappedFolder>
    <MappedFolder><HostFolder>$(Join-Path $root 'scripts\sandbox')</HostFolder><SandboxFolder>C:\glowframe\scripts</SandboxFolder><ReadOnly>true</ReadOnly></MappedFolder>
    <MappedFolder><HostFolder>$results</HostFolder><SandboxFolder>C:\glowframe\results</SandboxFolder><ReadOnly>false</ReadOnly></MappedFolder>
  </MappedFolders>
  <LogonCommand>
    <Command>powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\glowframe\scripts\inside.ps1</Command>
  </LogonCommand>
</Configuration>
"@
$wsbFile = Join-Path $dist 'clean-test.wsb'
[IO.File]::WriteAllText($wsbFile, $wsb)
Start-Process $wsbFile
Write-Host "Sandbox started; waiting for $results\done.txt ..."
$deadline = (Get-Date).AddMinutes(8)
while (-not (Test-Path (Join-Path $results 'done.txt'))) {
    if ((Get-Date) -gt $deadline) { throw 'timeout waiting for the sandbox test' }
    Start-Sleep -Seconds 5
}
Get-Content (Join-Path $results 'report.txt')
