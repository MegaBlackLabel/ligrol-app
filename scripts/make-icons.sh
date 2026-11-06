#!/usr/bin/env bash
# アプリアイコン（SVG）から配布用アセットを生成する。
#
#   bash scripts/make-icons.sh [svg]
#
# ImageMagick（librsvg 入り）と Python 3 が必要。Rust / mise は使わない。
# Windows は WSL の bash から Windows の ImageMagick を呼べないので
# scripts/make-icons.ps1 を使うこと。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SVG="${1:-$REPO_ROOT/crates/app/assets/app-icon/app-icon-ligrol-w6b-original-tab.svg}"
OUT="$REPO_ROOT/crates/app/assets/app-icon"

# magick: PATH -> Windows の既定インストール先 -> エラー
if ! command -v magick >/dev/null 2>&1; then
  for d in "/c/Program Files"/ImageMagick*/; do
    if [ -x "${d}magick.exe" ]; then
      sep=":"
      case "$PATH" in *";"*) sep=";" ;; esac
      PATH="$PATH$sep${d%/}"
    fi
  done
fi

MAGICK="${MAGICK:-}"
if [ -z "$MAGICK" ]; then
  MAGICK="$(command -v magick || true)"
fi
if [ -z "$MAGICK" ]; then
  MAGICK="$(ls -d "/c/Program Files"/ImageMagick*/magick.exe 2>/dev/null | head -1 || true)"
fi
if [ -z "$MAGICK" ]; then
  echo "ImageMagick (magick) が見つかりません。MAGICK=/path/to/magick を指定してください" >&2
  exit 1
fi

# python: PATH -> mise
PYTHON="${PYTHON:-}"
if [ -z "$PYTHON" ]; then
  PYTHON="$(command -v python3 || command -v python || true)"
fi

if [ ! -f "$SVG" ]; then
  echo "SVG not found: $SVG" >&2
  exit 1
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

for size in 16 32 48 64 128 256 512 1024; do
  "$MAGICK" -background none "$SVG" -resize "${size}x${size}" -depth 8 "$TMP/$size.png"
done

cp "$TMP/1024.png" "$OUT/icon_1024.png"
cp "$TMP/512.png" "$OUT/icon_512.png"
cp "$TMP/256.png" "$OUT/icon_256.png"

# Windows: 複数サイズを 1 ファイルに
"$MAGICK" "$TMP/1024.png" -define icon:auto-resize=256,128,64,48,32,16 "$OUT/icon.ico"

# macOS: icns を自前で組み立てる（iconutil 非依存）
if [ -z "$PYTHON" ]; then
  echo "python3 / python が見つかりません（icns の生成に必要）" >&2
  exit 1
fi
"$PYTHON" "$REPO_ROOT/scripts/make-icns.py" "$OUT/icon.icns" "$TMP"

echo "generated: icon.icns / icon.ico / icon_1024.png / icon_512.png / icon_256.png"
