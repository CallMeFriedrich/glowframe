# Generates assets/icon.ico: a glowing ring in the default palette
# (pink -> violet), in all common Windows icon sizes (PNG-compressed entries).
Add-Type -AssemblyName System.Drawing

$out = Join-Path $PSScriptRoot '..\assets\icon.ico'
$sizes = 16, 20, 24, 32, 40, 48, 64, 128, 256

function New-RingPng([int]$size) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::Transparent)

    $c = $size / 2.0
    $r = $size * 0.33
    $pink = [System.Drawing.Color]::FromArgb(255, 255, 46, 166)
    $violet = [System.Drawing.Color]::FromArgb(255, 91, 60, 255)

    # Soft outer glow (skipped for tiny sizes where it only blurs).
    if ($size -ge 32) {
        for ($i = 8; $i -ge 1; $i--) {
            $w = $size * (0.05 + 0.03 * $i)
            $alpha = [int](26 - 2.6 * $i)
            $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb([Math]::Max($alpha, 4), 200, 50, 220)), $w
            $g.DrawEllipse($pen, [float]($c - $r), [float]($c - $r), [float](2 * $r), [float](2 * $r))
            $pen.Dispose()
        }
    }

    # Ring with a diagonal gradient.
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush (
        (New-Object System.Drawing.PointF 0, 0),
        (New-Object System.Drawing.PointF $size, $size),
        $pink, $violet)
    $ringWidth = [Math]::Max($size * 0.11, 2.0)
    $pen = New-Object System.Drawing.Pen $brush, $ringWidth
    $g.DrawEllipse($pen, [float]($c - $r), [float]($c - $r), [float](2 * $r), [float](2 * $r))

    # Bright highlight arc (the rotating arc of the frame).
    if ($size -ge 24) {
        $hl = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(230, 255, 210, 240)), ($ringWidth * 0.45)
        $hl.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
        $hl.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
        $g.DrawArc($hl, [float]($c - $r), [float]($c - $r), [float](2 * $r), [float](2 * $r), 200, 70)
        $hl.Dispose()
    }

    $pen.Dispose(); $brush.Dispose(); $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
    return , $ms.ToArray()
}

$images = @()
foreach ($s in $sizes) { $images += , (New-RingPng $s) }

$fs = [System.IO.File]::Create($out)
$bw = New-Object System.IO.BinaryWriter $fs
$bw.Write([UInt16]0); $bw.Write([UInt16]1); $bw.Write([UInt16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $s = $sizes[$i]; $data = $images[$i]
    $dim = if ($s -ge 256) { 0 } else { $s }
    $bw.Write([byte]$dim); $bw.Write([byte]$dim); $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([UInt16]1); $bw.Write([UInt16]32)
    $bw.Write([UInt32]$data.Length); $bw.Write([UInt32]$offset)
    $offset += $data.Length
}
foreach ($data in $images) { $bw.Write($data) }
$bw.Close()

# Preview for review (not committed).
$preview = Join-Path $PSScriptRoot '..\target\icon-preview.png'
[System.IO.File]::WriteAllBytes($preview, $images[-1])
Write-Output "written $out ($((Get-Item $out).Length) bytes)"
