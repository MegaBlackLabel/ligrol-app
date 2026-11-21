# アプリアイコン（SVG）から配布用アセットを生成する（Windows 用）。
#
#   powershell -ExecutionPolicy Bypass -File scripts/make-icons.ps1 [-Svg <path>] [-SmallSvg <path>] [-Magick <exe>]
#
# 16 / 24px はワードマークが潰れるため L モノグラム、32px 以上はワードマーク。
# MSIX の Assets を生成する scripts/make-msix-assets.ps1 と同じ規則に揃える。
#
# macOS / Linux は scripts/make-icons.sh を使う。WSL の bash では Windows の
# ImageMagick を呼べないので、このスクリプトを使うこと。
param(
  [string]$Svg = "crates/app/assets/app-icon/app-icon-ligrol-w6b-original-tab.svg",
  [string]$SmallSvg = "crates/app/assets/app-icon/app-icon-ligrol-small16-L.svg",
  [string]$Magick = ""
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$out = Join-Path $repo "crates/app/assets/app-icon"

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

$svgPath = if ([System.IO.Path]::IsPathRooted($Svg)) { $Svg } else { Join-Path $repo $Svg }
if (-not (Test-Path $svgPath)) { throw "SVG not found: $svgPath" }
$smallSvgPath = if ([System.IO.Path]::IsPathRooted($SmallSvg)) { $SmallSvg } else { Join-Path $repo $SmallSvg }
if (-not (Test-Path $smallSvgPath)) { throw "Small SVG not found: $smallSvgPath" }

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("ligrol-icons-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
  # macOS 用（icns）と大きい ICO はワードマーク。24px は ICO 専用。
  foreach ($size in 16, 32, 48, 64, 128, 256, 512, 1024) {
    & $magick -background none $svgPath -resize "${size}x${size}" -depth 8 (Join-Path $tmp "$size.png")
  }
  # 小さい Windows アイコンは L モノグラム（MSIX の targetsize-16/24 と同じ）。
  foreach ($size in 16, 24) {
    & $magick -background none $smallSvgPath -resize "${size}x${size}" -depth 8 (Join-Path $tmp "$size-L.png")
  }

  Copy-Item (Join-Path $tmp "1024.png") (Join-Path $out "icon_1024.png") -Force
  Copy-Item (Join-Path $tmp "512.png")  (Join-Path $out "icon_512.png")  -Force
  Copy-Item (Join-Path $tmp "256.png")  (Join-Path $out "icon_256.png")  -Force

  # フレームごとに別 SVG を使うため auto-resize は使わない。
  & $magick `
    (Join-Path $tmp "16-L.png") `
    (Join-Path $tmp "24-L.png") `
    (Join-Path $tmp "32.png") `
    (Join-Path $tmp "48.png") `
    (Join-Path $tmp "64.png") `
    (Join-Path $tmp "128.png") `
    (Join-Path $tmp "256.png") `
    (Join-Path $out "icon.ico")
  if ($LASTEXITCODE -ne 0) { throw "icon.ico の生成に失敗しました" }

  python (Join-Path $repo "scripts/make-icns.py") (Join-Path $out "icon.icns") $tmp
  if ($LASTEXITCODE -ne 0) { throw "make-icns.py failed" }

  Write-Host "generated: icon.icns / icon.ico (16/24=L, 32+=wordmark) / icon_1024.png / icon_512.png / icon_256.png"
}
finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
