# Generates the self-made demo cover (assets/demo/cover.png) used by --demo.
# Abstract shapes only; no third-party artwork.
Add-Type -AssemblyName System.Drawing

$size = 600
$out = Join-Path $PSScriptRoot '..\assets\demo\cover.png'
New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null

$bmp = New-Object System.Drawing.Bitmap $size, $size
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias

$bg = New-Object System.Drawing.Drawing2D.LinearGradientBrush (
    (New-Object System.Drawing.Point 0, 0),
    (New-Object System.Drawing.Point $size, $size),
    [System.Drawing.Color]::FromArgb(255, 18, 10, 48),
    [System.Drawing.Color]::FromArgb(255, 70, 12, 70))
$g.FillRectangle($bg, 0, 0, $size, $size)

$sun = New-Object System.Drawing.Drawing2D.LinearGradientBrush (
    (New-Object System.Drawing.Point 0, 120),
    (New-Object System.Drawing.Point 0, 420),
    [System.Drawing.Color]::FromArgb(255, 255, 170, 40),
    [System.Drawing.Color]::FromArgb(255, 255, 40, 150))
$g.FillEllipse($sun, 150, 120, 300, 300)

$teal = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 0, 210, 200))
$pts = @(
    (New-Object System.Drawing.Point 0, 600),
    (New-Object System.Drawing.Point 0, 430),
    (New-Object System.Drawing.Point 170, 330),
    (New-Object System.Drawing.Point 320, 450),
    (New-Object System.Drawing.Point 470, 360),
    (New-Object System.Drawing.Point 600, 440),
    (New-Object System.Drawing.Point 600, 600))
$g.FillPolygon($teal, [System.Drawing.Point[]]$pts)

$stripe = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 18, 10, 48))
foreach ($y in 300, 330, 358, 384) { $g.FillRectangle($stripe, 150, $y, 300, 9) }

$g.Dispose()
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Output "written $out"
