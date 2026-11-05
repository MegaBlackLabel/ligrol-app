#!/usr/bin/env python3
"""PNG から macOS の .icns を組み立てる。

`iconutil` は macOS 専用なので、開発機・CI を選ばないように自前で書く。
ICNS は `icns` マジック + 全長 + (タイプ + 長さ + PNG) の並び。

使い方: make-icns.py <out.icns> <size-png-dir>
  ディレクトリには 16.png / 32.png / 64.png / 128.png / 256.png / 512.png / 1024.png
  を置く（無いサイズはスキップする）。
"""

import struct
import sys
from pathlib import Path

# ICNS のタイプ -> 使う PNG の辺。Retina 用のエイリアスも入れる。
LAYOUT = [
    ("icp4", 16),
    ("icp5", 32),
    ("icp6", 64),
    ("ic07", 128),
    ("ic08", 256),
    ("ic09", 512),
    ("ic10", 1024),
    ("ic11", 32),   # 16@2x
    ("ic12", 64),   # 32@2x
    ("ic13", 256),  # 128@2x
    ("ic14", 512),  # 256@2x
]


def chunk(kind: str, data: bytes) -> bytes:
    return kind.encode("ascii") + struct.pack(">I", 8 + len(data)) + data


def main() -> int:
    if len(sys.argv) != 3:
        print(
            "usage: make-icns.py <out.icns> <size-png-dir>", file=sys.stderr
        )
        return 2

    out = Path(sys.argv[1])
    src_dir = Path(sys.argv[2])

    parts = []
    for kind, size in LAYOUT:
        png = src_dir / f"{size}.png"
        if not png.exists():
            continue
        parts.append(chunk(kind, png.read_bytes()))

    if not parts:
        print(f"no PNGs found in {src_dir}", file=sys.stderr)
        return 1

    body = b"".join(parts)
    out.write_bytes(b"icns" + struct.pack(">I", 8 + len(body)) + body)
    print(f"wrote {out} ({out.stat().st_size} bytes, {len(parts)} slots)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
