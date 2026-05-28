param(
  [string]$OutDir = "design",
  [string]$IconsDir = "src-tauri/icons"
)

Add-Type -AssemblyName System.Drawing

$ErrorActionPreference = "Stop"

function New-RoundedRectPath {
  param(
    [float]$X,
    [float]$Y,
    [float]$W,
    [float]$H,
    [float]$R
  )
  $path = [System.Drawing.Drawing2D.GraphicsPath]::new()
  $d = $R * 2
  $path.AddArc($X, $Y, $d, $d, 180, 90)
  $path.AddArc($X + $W - $d, $Y, $d, $d, 270, 90)
  $path.AddArc($X + $W - $d, $Y + $H - $d, $d, $d, 0, 90)
  $path.AddArc($X, $Y + $H - $d, $d, $d, 90, 90)
  $path.CloseFigure()
  return $path
}

function Add-RoundedRect {
  param(
    [System.Drawing.Drawing2D.GraphicsPath]$Path,
    [float]$X,
    [float]$Y,
    [float]$W,
    [float]$H,
    [float]$R
  )
  $d = $R * 2
  $Path.StartFigure()
  $Path.AddArc($X, $Y, $d, $d, 180, 90)
  $Path.AddArc($X + $W - $d, $Y, $d, $d, 270, 90)
  $Path.AddArc($X + $W - $d, $Y + $H - $d, $d, $d, 0, 90)
  $Path.AddArc($X, $Y + $H - $d, $d, $d, 90, 90)
  $Path.CloseFigure()
}

function New-ColorBlend {
  param(
    [System.Drawing.Color[]]$Colors,
    [single[]]$Positions
  )
  $blend = [System.Drawing.Drawing2D.ColorBlend]::new()
  $blend.Colors = $Colors
  $blend.Positions = $Positions
  return $blend
}

function Export-ResizedPng {
  param(
    [System.Drawing.Bitmap]$Source,
    [string]$Path,
    [int]$Width,
    [int]$Height
  )
  $dir = Split-Path -Parent $Path
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  $scaled = [System.Drawing.Bitmap]::new($Width, $Height, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $g = [System.Drawing.Graphics]::FromImage($scaled)
  try {
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.Clear([System.Drawing.Color]::Transparent)
    $g.DrawImage($Source, [System.Drawing.Rectangle]::new(0, 0, $Width, $Height))
  }
  finally {
    $g.Dispose()
  }
  $scaled.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
  $scaled.Dispose()
}

$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$targetDir = Join-Path $root $OutDir
New-Item -ItemType Directory -Force -Path $targetDir | Out-Null
$outPath = Join-Path $targetDir "app-icon.png"

$size = 1024
$bitmap = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
$graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality

try {
  $graphics.Clear([System.Drawing.Color]::Transparent)

  $body = New-RoundedRectPath 72 72 880 880 228
  $shadow = $body.Clone()
  $shadowMatrix = [System.Drawing.Drawing2D.Matrix]::new()
  $shadowMatrix.Translate(0, 28)
  $shadow.Transform($shadowMatrix)
  $graphics.FillPath([System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(70, 24, 12, 62)), $shadow)

  $rect = [System.Drawing.RectangleF]::new(72, 72, 880, 880)
  $bgBrush = [System.Drawing.Drawing2D.LinearGradientBrush]::new(
    $rect,
    [System.Drawing.Color]::FromArgb(255, 155, 124, 255),
    [System.Drawing.Color]::FromArgb(255, 9, 182, 242),
    128
  )
  $bgBrush.InterpolationColors = New-ColorBlend `
    ([System.Drawing.Color[]]@(
      [System.Drawing.Color]::FromArgb(255, 155, 124, 255),
      [System.Drawing.Color]::FromArgb(255, 108, 59, 255),
      [System.Drawing.Color]::FromArgb(255, 9, 182, 242)
    )) `
    ([single[]]@(0.0, 0.50, 1.0))

  $graphics.FillPath($bgBrush, $body)
  $graphics.SetClip($body)

  $graphics.FillEllipse(
    [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(95, 255, 255, 255)),
    -150,
    -190,
    760,
    540
  )
  $graphics.FillEllipse(
    [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(82, 126, 235, 255)),
    540,
    520,
    560,
    510
  )
  $graphics.FillEllipse(
    [System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(56, 67, 18, 197)),
    -120,
    650,
    610,
    360
  )

  $arcPen1 = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(54, 255, 255, 255), 28)
  $arcPen1.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
  $arcPen1.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
  $graphics.DrawArc($arcPen1, 542, 314, 260, 420, -52, 104)

  $arcPen2 = [System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(42, 255, 255, 255), 24)
  $arcPen2.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
  $arcPen2.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
  $graphics.DrawArc($arcPen2, 520, 388, 176, 260, -50, 100)

  $graphics.ResetClip()

  $innerLight = New-RoundedRectPath 82 82 860 860 218
  $graphics.DrawPath([System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(92, 255, 255, 255), 10), $innerLight)
  $graphics.DrawPath([System.Drawing.Pen]::new([System.Drawing.Color]::FromArgb(40, 20, 12, 62), 8), $body)

  $mark = [System.Drawing.Drawing2D.GraphicsPath]::new([System.Drawing.Drawing2D.FillMode]::Winding)
  Add-RoundedRect $mark 312 286 116 452 58
  Add-RoundedRect $mark 312 286 430 118 59
  Add-RoundedRect $mark 312 453 348 112 56
  Add-RoundedRect $mark 312 620 430 118 59

  $markShadow = $mark.Clone()
  $markShadowMatrix = [System.Drawing.Drawing2D.Matrix]::new()
  $markShadowMatrix.Translate(0, 12)
  $markShadow.Transform($markShadowMatrix)
  $graphics.FillPath([System.Drawing.SolidBrush]::new([System.Drawing.Color]::FromArgb(72, 25, 10, 80)), $markShadow)

  $markBrush = [System.Drawing.Drawing2D.LinearGradientBrush]::new(
    [System.Drawing.RectangleF]::new(300, 270, 460, 500),
    [System.Drawing.Color]::White,
    [System.Drawing.Color]::FromArgb(255, 233, 226, 255),
    90
  )
  $graphics.FillPath($markBrush, $mark)
}
finally {
  $graphics.Dispose()
}

$bitmap.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)
$iconsRoot = Join-Path $root $IconsDir
if (Test-Path $iconsRoot) {
  $exports = @(
    @("32x32.png", 32),
    @("64x64.png", 64),
    @("128x128.png", 128),
    @("128x128@2x.png", 256),
    @("icon.png", 512),
    @("StoreLogo.png", 50),
    @("Square30x30Logo.png", 30),
    @("Square44x44Logo.png", 44),
    @("Square71x71Logo.png", 71),
    @("Square89x89Logo.png", 89),
    @("Square107x107Logo.png", 107),
    @("Square142x142Logo.png", 142),
    @("Square150x150Logo.png", 150),
    @("Square284x284Logo.png", 284),
    @("Square310x310Logo.png", 310),
    @("ios/AppIcon-20x20@1x.png", 20),
    @("ios/AppIcon-20x20@2x.png", 40),
    @("ios/AppIcon-20x20@2x-1.png", 40),
    @("ios/AppIcon-20x20@3x.png", 60),
    @("ios/AppIcon-29x29@1x.png", 29),
    @("ios/AppIcon-29x29@2x.png", 58),
    @("ios/AppIcon-29x29@2x-1.png", 58),
    @("ios/AppIcon-29x29@3x.png", 87),
    @("ios/AppIcon-40x40@1x.png", 40),
    @("ios/AppIcon-40x40@2x.png", 80),
    @("ios/AppIcon-40x40@2x-1.png", 80),
    @("ios/AppIcon-40x40@3x.png", 120),
    @("ios/AppIcon-60x60@2x.png", 120),
    @("ios/AppIcon-60x60@3x.png", 180),
    @("ios/AppIcon-76x76@1x.png", 76),
    @("ios/AppIcon-76x76@2x.png", 152),
    @("ios/AppIcon-83.5x83.5@2x.png", 167),
    @("ios/AppIcon-512@2x.png", 1024),
    @("android/mipmap-mdpi/ic_launcher.png", 48),
    @("android/mipmap-mdpi/ic_launcher_round.png", 48),
    @("android/mipmap-mdpi/ic_launcher_foreground.png", 108),
    @("android/mipmap-hdpi/ic_launcher.png", 49),
    @("android/mipmap-hdpi/ic_launcher_round.png", 49),
    @("android/mipmap-hdpi/ic_launcher_foreground.png", 162),
    @("android/mipmap-xhdpi/ic_launcher.png", 96),
    @("android/mipmap-xhdpi/ic_launcher_round.png", 96),
    @("android/mipmap-xhdpi/ic_launcher_foreground.png", 216),
    @("android/mipmap-xxhdpi/ic_launcher.png", 144),
    @("android/mipmap-xxhdpi/ic_launcher_round.png", 144),
    @("android/mipmap-xxhdpi/ic_launcher_foreground.png", 324),
    @("android/mipmap-xxxhdpi/ic_launcher.png", 192),
    @("android/mipmap-xxxhdpi/ic_launcher_round.png", 192),
    @("android/mipmap-xxxhdpi/ic_launcher_foreground.png", 432)
  )

  foreach ($export in $exports) {
    $path = Join-Path $iconsRoot $export[0]
    Export-ResizedPng $bitmap $path $export[1] $export[1]
  }
}
$bitmap.Dispose()
Write-Host "Generated $outPath"
