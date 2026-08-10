#!/usr/bin/env python3
"""產生佔位用的應用程式圖示。

Tauri 的 `generate_context!` 在編譯時就要讀到 `icons/icon.png`，
所以 repo 裡必須有一張，不能等打包時才生。

這支腳本只用標準函式庫寫 PNG，不需要 Pillow——為了一張佔位圖
而讓所有貢獻者裝一個影像函式庫並不划算。

有正式 logo 之後，改用官方工具產生各平台尺寸：

    cd apps/desktop && npx tauri icon path/to/logo.png

用法：
    python3 make_placeholder_icon.py [輸出路徑]
"""

import math
import struct
import sys
import zlib

SIZE = 512
RADIUS = 96  # 圓角半徑

BG = (194, 65, 12)  # 橘：在淺色與深色工作列上都看得見
FG = (255, 247, 237)  # 近白

# 「W」的骨架，座標是相對於邊長的比例
STROKE = [
    (0.20, 0.30),
    (0.35, 0.72),
    (0.50, 0.45),
    (0.65, 0.72),
    (0.80, 0.30),
]
STROKE_WIDTH = 0.075


def dist_to_segment(px, py, ax, ay, bx, by):
    """點到線段的距離。用來把折線畫成有寬度的筆畫。"""
    dx, dy = bx - ax, by - ay
    if dx == 0 and dy == 0:
        return math.hypot(px - ax, py - ay)
    t = max(0.0, min(1.0, ((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)))
    return math.hypot(px - (ax + t * dx), py - (ay + t * dy))


def rounded_rect_alpha(x, y):
    """圓角矩形的覆蓋率，邊界做一像素的漸變當作抗鋸齒。"""
    cx = min(max(x, RADIUS), SIZE - RADIUS)
    cy = min(max(y, RADIUS), SIZE - RADIUS)
    d = math.hypot(x - cx, y - cy)
    return max(0.0, min(1.0, RADIUS - d + 0.5))


def build_pixels():
    half = STROKE_WIDTH * SIZE / 2
    rows = []
    for y in range(SIZE):
        row = bytearray()
        py = y + 0.5
        for x in range(SIZE):
            px = x + 0.5
            bg_alpha = rounded_rect_alpha(px, py)

            # 到 W 折線的最短距離，決定這個像素有多「在筆畫上」
            d = min(
                dist_to_segment(px, py, a[0] * SIZE, a[1] * SIZE, b[0] * SIZE, b[1] * SIZE)
                for a, b in zip(STROKE, STROKE[1:])
            )
            stroke_alpha = max(0.0, min(1.0, half - d + 0.5))

            r = BG[0] + (FG[0] - BG[0]) * stroke_alpha
            g = BG[1] + (FG[1] - BG[1]) * stroke_alpha
            b_ = BG[2] + (FG[2] - BG[2]) * stroke_alpha
            row += bytes((int(r), int(g), int(b_), int(bg_alpha * 255)))
        rows.append(row)
    return rows


def write_png(path, rows):
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    # 每條掃描線前面要加一個 filter type byte，這裡一律用 0（None）
    raw = b"".join(b"\x00" + bytes(r) for r in rows)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    with open(path, "wb") as f:
        f.write(png)


if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "icon.png"
    write_png(out, build_pixels())
    print(f"已寫入 {out}")
