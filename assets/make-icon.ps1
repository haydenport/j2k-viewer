# Generates the application icon from code so it can be regenerated and tweaked reproducibly.
#
#   powershell -ExecutionPolicy Bypass -File assets/make-icon.ps1
#
# Writes assets/icon.ico (16..256 px, used for viewer.exe and the installer), assets/icon-256.png
# (used for the window/taskbar icon on every platform) and assets/icon-512.png (README / social).
#
# The picture: a dark rounded tile holding a grid of grayscale "image tiles" (the viewer decodes
# huge images tile by tile) with an amber magnifying glass over it showing fine detail (zooming into
# a huge image). It is drawn separately at each size so small sizes stay crisp.

Add-Type -AssemblyName System.Drawing
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path

function New-RoundedRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = [Math]::Min($r * 2, [Math]::Min($w, $h))
    if ($d -lt 0.5) {                     # square corners (arcs need a positive size)
        $p.AddRectangle([System.Drawing.RectangleF]::new($x, $y, $w, $h))
        return $p
    }
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

function Gray([int]$v, [int]$a = 255) { [System.Drawing.Color]::FromArgb($a, $v, $v, $v) }

function New-IconBitmap([int]$S) {
    $bmp = New-Object System.Drawing.Bitmap $S, $S, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.CompositingQuality = 'HighQuality'
    $g.Clear([System.Drawing.Color]::Transparent)
    $u = $S / 100.0            # one design unit
    $small = $S -lt 32
    $mid = $S -lt 64

    # --- dark rounded background with a subtle vertical gradient
    $bg = New-RoundedRect (3 * $u) (3 * $u) (94 * $u) (94 * $u) (19 * $u)
    $grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush `
        ([System.Drawing.PointF]::new(0, 0)), ([System.Drawing.PointF]::new(0, $S)), `
        ([System.Drawing.Color]::FromArgb(255, 36, 44, 62)), ([System.Drawing.Color]::FromArgb(255, 14, 17, 26))
    $g.FillPath($grad, $bg)

    # --- the "image": a grid of grayscale tiles
    $n = if ($small) { 3 } else { 4 }
    $shades4 = @(@(205, 150, 112, 92), @(168, 232, 142, 104), @(122, 162, 250, 132), @(84, 112, 152, 196))
    $shades3 = @(@(205, 140, 100), @(150, 240, 128), @(100, 140, 190))
    $shades = if ($small) { $shades3 } else { $shades4 }
    $x0 = 13 * $u; $span = 74 * $u
    $gap = if ($small) { 0 } else { 1.8 * $u }
    $cell = ($span - $gap * ($n - 1)) / $n
    for ($j = 0; $j -lt $n; $j++) {
        for ($i = 0; $i -lt $n; $i++) {
            $px = $x0 + $i * ($cell + $gap); $py = $x0 + $j * ($cell + $gap)
            $rr = if ($small) { 0 } else { 2.2 * $u }
            $path = New-RoundedRect $px $py $cell $cell $rr
            $g.FillPath((New-Object System.Drawing.SolidBrush (Gray $shades[$j][$i])), $path)
        }
    }

    # --- magnifying glass: lens (showing finer detail), amber ring, handle
    $cx = 53 * $u; $cy = 53 * $u; $r = 24 * $u
    $ringW = if ($small) { 7 * $u } elseif ($mid) { 6 * $u } else { 5.2 * $u }
    $amber = [System.Drawing.Color]::FromArgb(255, 255, 183, 0)
    $amberDark = [System.Drawing.Color]::FromArgb(255, 224, 140, 0)

    # handle first so the ring sits on top of it
    $ang = [Math]::PI / 4
    $hx0 = $cx + ($r + 1.5 * $u) * [Math]::Cos($ang); $hy0 = $cy + ($r + 1.5 * $u) * [Math]::Sin($ang)
    $hx1 = 86 * $u; $hy1 = 86 * $u
    $hp = New-Object System.Drawing.Pen $amberDark, (10 * $u)
    $hp.StartCap = 'Round'; $hp.EndCap = 'Round'
    $g.DrawLine($hp, [float]$hx0, [float]$hy0, [float]$hx1, [float]$hy1)

    # lens interior, clipped to the circle
    $lens = New-Object System.Drawing.Drawing2D.GraphicsPath
    $lens.AddEllipse([float]($cx - $r), [float]($cy - $r), [float](2 * $r), [float](2 * $r))
    $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 20, 24, 34))), $lens)
    if (-not $small) {
        $state = $g.Save()
        $g.SetClip($lens)
        $m = 5                                   # finer grid inside the lens
        $lc = (2 * $r) / $m
        $detail = @(@(250, 90, 215, 70, 235), @(60, 225, 120, 245, 100), @(230, 110, 255, 85, 200),
                    @(80, 240, 95, 220, 150), @(210, 130, 245, 65, 230))
        for ($j = 0; $j -lt $m; $j++) {
            for ($i = 0; $i -lt $m; $i++) {
                $px = $cx - $r + $i * $lc; $py = $cy - $r + $j * $lc
                $g.FillRectangle((New-Object System.Drawing.SolidBrush (Gray $detail[$j][$i])), [float]$px, [float]$py, [float]($lc - 0.8 * $u), [float]($lc - 0.8 * $u))
            }
        }
        $g.Restore($state)
    } else {
        $g.FillPath((New-Object System.Drawing.SolidBrush (Gray 238)), $lens)
        $g.FillEllipse((New-Object System.Drawing.SolidBrush (Gray 120)), [float]($cx - 7 * $u), [float]($cy - 7 * $u), [float](14 * $u), [float](14 * $u))
    }
    $rp = New-Object System.Drawing.Pen $amber, $ringW
    $g.DrawEllipse($rp, [float]($cx - $r), [float]($cy - $r), [float](2 * $r), [float](2 * $r))

    $g.Dispose()
    return $bmp
}

function Get-PngBytes($bmp) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    return $ms.ToArray()
}

# BMP (DIB) payload for an ICO entry: BITMAPINFOHEADER (height doubled) + bottom-up BGRA + 1bpp AND mask.
function Get-DibBytes($bmp) {
    $w = $bmp.Width; $h = $bmp.Height
    $ms = New-Object System.IO.MemoryStream
    $bw = New-Object System.IO.BinaryWriter $ms
    $bw.Write([int]40); $bw.Write([int]$w); $bw.Write([int]($h * 2)); $bw.Write([int16]1); $bw.Write([int16]32)
    $bw.Write([int]0); $bw.Write([int]($w * $h * 4)); $bw.Write([int]0); $bw.Write([int]0); $bw.Write([int]0); $bw.Write([int]0)
    for ($y = $h - 1; $y -ge 0; $y--) {
        for ($x = 0; $x -lt $w; $x++) {
            $c = $bmp.GetPixel($x, $y)
            $bw.Write([byte]$c.B); $bw.Write([byte]$c.G); $bw.Write([byte]$c.R); $bw.Write([byte]$c.A)
        }
    }
    $maskRow = [int]([Math]::Ceiling($w / 32.0) * 4)
    $bw.Write((New-Object byte[] ($maskRow * $h)))   # all zero: alpha channel carries transparency
    $bw.Flush()
    return $ms.ToArray()
}

$sizes = 16, 24, 32, 48, 64, 128, 256
$images = foreach ($s in $sizes) {
    $b = New-IconBitmap $s
    $data = if ($s -ge 256) { Get-PngBytes $b } else { Get-DibBytes $b }
    if ($s -eq 256) { $b.Save((Join-Path $here 'icon-256.png'), [System.Drawing.Imaging.ImageFormat]::Png) }
    [pscustomobject]@{ Size = $s; Data = $data }
    $b.Dispose()
}
$big = New-IconBitmap 512
$big.Save((Join-Path $here 'icon-512.png'), [System.Drawing.Imaging.ImageFormat]::Png); $big.Dispose()

# Assemble the .ico container.
$ms = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $ms
$w.Write([int16]0); $w.Write([int16]1); $w.Write([int16]$images.Count)
$offset = 6 + 16 * $images.Count
foreach ($i in $images) {
    $dim = if ($i.Size -ge 256) { 0 } else { $i.Size }
    $w.Write([byte]$dim); $w.Write([byte]$dim); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([int16]1); $w.Write([int16]32); $w.Write([int]$i.Data.Length); $w.Write([int]$offset)
    $offset += $i.Data.Length
}
foreach ($i in $images) { $w.Write([byte[]]$i.Data) }   # cast: PowerShell hands over object[] otherwise
$w.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $here 'icon.ico'), $ms.ToArray())
Write-Host ("wrote icon.ico ({0:N0} bytes, sizes {1}), icon-256.png, icon-512.png" -f $ms.Length, ($sizes -join ','))
