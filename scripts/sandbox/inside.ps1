# Runs inside Windows Sandbox (clean Windows, no extra runtimes) and checks
# the installer and the portable ZIP (SPEC 10, criterion 11).
# Results go to C:\glowframe\results (mapped writable folder on the host).

$ErrorActionPreference = 'Continue'
$dist = 'C:\glowframe\dist'
$res = 'C:\glowframe\results'
$report = Join-Path $res 'report.txt'
function Log($msg) { $line = "[{0:HH:mm:ss}] {1}" -f (Get-Date), $msg; Add-Content -Path $report -Value $line -Encoding UTF8 }
function Check($name, $ok) { Log ("{0} {1}" -f $(if ($ok) { 'PASS' } else { 'FAIL' }), $name) }

Add-Type -AssemblyName System.Drawing, System.Windows.Forms
function Shot($file) {
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
    $bmp.Save((Join-Path $res $file), [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose(); $bmp.Dispose()
}

Set-Content -Path $report -Value "Glowframe clean system test" -Encoding UTF8
Log ("OS: " + (Get-CimInstance Win32_OperatingSystem).Caption + " " + [Environment]::OSVersion.Version)
Log ("VC runtime present: " + (Test-Path "$env:WINDIR\System32\vcruntime140.dll"))

# ----- installer -----
$setup = Get-ChildItem $dist -Filter 'Glowframe-*-setup.exe' | Select-Object -First 1
Log "installer: $($setup.Name)"
Start-Process $setup.FullName -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait
$exe = "$env:LOCALAPPDATA\Programs\Glowframe\Glowframe.exe"
Check 'installed exe exists (per user)' (Test-Path $exe)
Check 'start menu entry' (Test-Path "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Glowframe.lnk")

# Silent install starts the app ([Run] postinstall is skipped when silent).
$p = Start-Process $exe -ArgumentList '--demo', '--start', '--exit-after', '10', '--log-level', 'glowframe=debug' -PassThru
Start-Sleep -Seconds 5
Check 'installed exe running after 5 s' (-not $p.HasExited)
Shot 'installed-demo.png'
# Second start opens the settings window of the running instance.
Start-Process $exe | Out-Null
Start-Sleep -Seconds 3
Shot 'installed-settings.png'
$p.WaitForExit(15000) | Out-Null
Check "installed exe exited cleanly (code $($p.ExitCode))" ($p.ExitCode -eq 0)
Check 'settings.json created in %APPDATA%' (Test-Path "$env:APPDATA\Glowframe\settings.json")
$logs = Get-ChildItem "$env:LOCALAPPDATA\Glowframe\logs" -ErrorAction SilentlyContinue
Check 'log written in %LOCALAPPDATA%' ($logs.Count -gt 0)
foreach ($l in $logs) { Copy-Item $l.FullName (Join-Path $res ("installed-" + $l.Name)) }

# ----- uninstall -----
$unins = Get-ChildItem "$env:LOCALAPPDATA\Programs\Glowframe" -Filter 'unins*.exe' | Select-Object -First 1
Start-Process $unins.FullName -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait
Start-Sleep -Seconds 2
Check 'uninstall removed exe' (-not (Test-Path $exe))
Check 'uninstall removed start menu entry' (-not (Test-Path "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Glowframe.lnk"))

# ----- portable -----
$zip = Get-ChildItem $dist -Filter 'Glowframe-*-portable.zip' | Select-Object -First 1
$pdir = 'C:\glowframe-portable'
Expand-Archive $zip.FullName -DestinationPath $pdir -Force
$pexe = Join-Path $pdir 'Glowframe.exe'
$pp = Start-Process $pexe -ArgumentList '--demo', '--start', '--exit-after', '6' -PassThru
Start-Sleep -Seconds 4
Shot 'portable-demo.png'
$pp.WaitForExit(12000) | Out-Null
Check "portable exe exited cleanly (code $($pp.ExitCode))" ($pp.ExitCode -eq 0)
Check 'portable settings next to exe' (Test-Path (Join-Path $pdir 'settings.json'))
Check 'portable logs next to exe' (Test-Path (Join-Path $pdir 'logs'))
Get-ChildItem (Join-Path $pdir 'logs') -ErrorAction SilentlyContinue | ForEach-Object { Copy-Item $_.FullName (Join-Path $res ("portable-" + $_.Name)) }

Log 'done'
Set-Content -Path (Join-Path $res 'done.txt') -Value 'done'
