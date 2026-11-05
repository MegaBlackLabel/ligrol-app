# アプリアイコン（SVG）から配布用アセットを生成する（Windows 用）。
#
#   powershell -ExecutionPolicy Bypass -File scripts/make-icons.ps1 [-Svg <path>]
#
# macOS / Linux は scripts/make-icons.sh を使う。WSL の bash では Windows の
# ImageMagick を呼べないので、このスクリプトを使うこと。
param(
  [string]$Svg = "crates/app/assets/app-icon/app-icon-ligrol-w6b-original-tab.svg"
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$out = Join-Path $repo "crates/app/assets/app-icon"

$magick = (Get-Command magick -ErrorAction SilentlyContinue).Source
if (-not $magick) {
  $candidate = Get-ChildItem "C:\Program Files\ImageMagick*\magick.exe" -ErrorAction SilentlyContinue |
    Select-Object -First 1
  if ($candidate) { $magick = $candidate.FullName }
}
if (-not $magick) { throw "ImageMagick (magick) が見つかりません。PATH に追加するか -Magick で指定してください" }

$svgPath = if ([System.IO.Path]::IsPathRooted($Svg)) { $Svg } else { Join-Path $repo $Svg }
if (-not (Test-Path $svgPath)) { throw "SVG not found: $svgPath" }

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("ligrol-icons-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
  foreach ($size in 16, 32, 48, 64, 128, 256, 512, 1024) {
    & $magick -background none $svgPath -resize "${size}x${size}" -depth 8 (Join-Path $tmp "$size.png")
  }

  Copy-Item (Join-Path $tmp "1024.png") (Join-Path $out "icon_1024.png") -Force
  Copy-Item (Join-Path $tmp "512.png")  (Join-Path $out "icon_512.png")  -Force
  Copy-Item (Join-Path $tmp "256.png")  (Join-Path $out "icon_256.png")  -Force

  & $magick (Join-Path $tmp "1024.png") -define icon:auto-resize=256,128,64,48,32,16 (Join-Path $out "icon.ico")

  python (Join-Path $repo "scripts/make-icns.py") (Join-Path $out "icon.icns") $tmp
  if ($LASTEXITCODE -ne 0) { throw "make-icns.py failed" }

  Write-Host "generated: icon.icns / icon.ico / icon_1024.png / icon_512.png / icon_256.png"
}
finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
