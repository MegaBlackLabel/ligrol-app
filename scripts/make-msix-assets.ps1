<#
  MSIX（Microsoft Store）用のアイコンアセットを SVG から生成する。

  使い方:
    powershell -ExecutionPolicy Bypass -File scripts/make-msix-assets.ps1 [-Magick <exe>]

  `packaging/windows/Assets` に、マニフェストが参照する基本ファイルと、
  シェルがサイズ別に選ぶ修飾付きファイル（targetsize / scale）を書き出す。

  なぜ targetsize が要るか:
    アプリ一覧（スタートの「すべてのアプリ」）は 16px 前後で描画される。
    44x44 のワードマークを縮小すると潰れて、暗い背景ではほぼ見えなくなる。
    16/24px は `app-icon-ligrol-small16-L.svg`（モノグラム）から直接描く。
#>
param(
  [string]$Magick = ""
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$iconDir = Join-Path $repo "crates/app/assets/app-icon"
$outDir = Join-Path $repo "packaging/windows/Assets"

$magick = $Magick
if ($magick) {
  if (-not (Test-Path $magick)) { throw "ImageMagick が見つかりません: $magick" }
}
if (-not $magick) {
  $magick = (Get-Command magick -ErrorAction SilentlyContinue).Source
}
if (-not $magick) {
  $candidate = Get-ChildItem "C:\Program Files\ImageMagick*\magick.exe" -ErrorAction SilentlyContinue |
    Select-Object -First 1
  if ($candidate) { $magick = $candidate.FullName }
}
if (-not $magick) { throw "ImageMagick (magick) が見つかりません。PATH に追加するか -Magick で指定してください" }

$small16 = Join-Path $iconDir "app-icon-ligrol-small16-L.svg"
$small32 = Join-Path $iconDir "app-icon-ligrol-small32.svg"
foreach ($svg in @($small16, $small32)) {
  if (-not (Test-Path $svg)) { throw "SVG が見つかりません: $svg" }
}

New-Item -ItemType Directory -Path $outDir -Force | Out-Null

function Render([string]$svg, [int]$size, [string]$name) {
  $path = Join-Path $outDir $name
  & $magick -background none $svg -resize "${size}x${size}" -depth 8 $path
  if ($LASTEXITCODE -ne 0) { throw "描画に失敗: $name ($size)" }
  return $path
}

# 基本ファイル（マニフェストが参照する名前）。
# StoreLogo / Square150 / Square310 / Wide310x150 は別デザインなのでここでは触らない。
Render $small32 44 "Square44x44Logo.png" | Out-Null

# スタートのアプリ一覧は targetsize を優先して使う。
# 16/24 はモノグラム、32/48 はワードマーク。
$targets = @(
  @{ Size = 16; Svg = $small16 },
  @{ Size = 24; Svg = $small16 },
  @{ Size = 32; Svg = $small32 },
  @{ Size = 48; Svg = $small32 }
)
foreach ($t in $targets) {
  $name = "Square44x44Logo.targetsize-$($t.Size).png"
  Render $t.Svg $t.Size $name | Out-Null
  Copy-Item (Join-Path $outDir $name) (Join-Path $outDir "Square44x44Logo.targetsize-$($t.Size)_altform-unplated.png") -Force
}

# DPI スケール別（targetsize に一致しない倍率でも解像度を落とさない）
$scales = @(
  @{ Scale = 100; Size = 44 },
  @{ Scale = 125; Size = 55 },
  @{ Scale = 150; Size = 66 },
  @{ Scale = 200; Size = 88 },
  @{ Scale = 400; Size = 176 }
)
foreach ($s in $scales) {
  Render $small32 $s.Size "Square44x44Logo.scale-$($s.Scale).png" | Out-Null
}

Write-Host "generated MSIX assets in ${outDir}:"
Get-ChildItem $outDir -Filter "Square44x44Logo*" | Sort-Object Name | ForEach-Object {
  Write-Host ("  {0,-52} {1}" -f $_.Name, $_.Length)
}
