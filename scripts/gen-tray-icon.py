#!/usr/bin/env python3
"""生成 macOS 菜单栏托盘图标（template image）。

背景：macOS 菜单栏使用 **template image**——系统只看 **alpha 通道**决定形状，
颜色由菜单栏前景色着色（浅色/深色模式自动适配）。因此这里输出的是「单色剪影」，
而不是彩色 logo 的等比缩小。

做法：
  1. 去掉青绿底色（判据：R 明显低于 G 与 B），得到猫头整体轮廓；
  2. 把「被暗部完全包围的白色区域」挖成镂空——它们是猫的眼睛（白脸直接贴着青色
     背景，故不会被误判）；
  3. 缩放到 36x36 并输出两种形态：
       - `tray-icon-template.png`  PNG（便于预览/替换）
       - `tray-icon-template.rgba` 原始 RGBA 字节（`tray.rs` 用 include_bytes! 直接引用）

⚠️ `tray.rs` 里有 `const ICON: &[u8; 36 * 36 * 4]` 的**编译期长度断言**：
尺寸一旦不是 36x36，Rust 侧会直接编译失败，两者必须同步修改。

用法：python scripts/gen-tray-icon.py [源图路径]（默认 pic/logo.png）
"""

import sys
from collections import deque
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
ICONS = ROOT / "src-tauri" / "icons"
SIZE = 36          # 必须与 tray.rs 的长度断言一致
WORK = 256         # 识别形状时的工作分辨率（最终仅 36px，无需全分辨率）
TEAL_DELTA = 40    # R 与 min(G,B) 的差超过该值视为青绿背景
WHITE_LUM = 0.78   # 判定「白」的亮度阈值
DILATE = 6         # 非图形区域膨胀像素数，用于判断白色连通块是否贴着外圈

Color = tuple[int, int, int, int]


def luminance(px: Color) -> float:
    r, g, b, _ = px
    return (0.299 * r + 0.587 * g + 0.114 * b) / 255.0


def build_mask(src: Path) -> Image.Image:
    """返回 36x36 的单色 alpha 掩码。"""
    work = Image.open(src).convert("RGBA").resize((WORK, WORK), Image.Resampling.LANCZOS)
    px = work.load()
    assert px is not None

    glyph = [[False] * WORK for _ in range(WORK)]
    white = [[False] * WORK for _ in range(WORK)]
    for y in range(WORK):
        for x in range(WORK):
            r, g, b, a = px[x, y]
            if a < 8:
                continue
            if (min(g, b) - r) > TEAL_DELTA:      # 青色底色，不属于图形
                continue
            glyph[y][x] = True
            white[y][x] = luminance(px[x, y]) > WHITE_LUM

    # 从「非图形区域」向外膨胀，得到一圈外边界；
    # 白色连通块若完全触不到这圈边界，说明被暗部包住 —— 即猫眼。
    outside = [[not glyph[y][x] for x in range(WORK)] for _ in range(WORK)]
    for _ in range(DILATE):
        grown = [row[:] for row in outside]
        for y in range(WORK):
            for x in range(WORK):
                if outside[y][x]:
                    continue
                if any(
                    0 <= y + dy < WORK
                    and 0 <= x + dx < WORK
                    and outside[y + dy][x + dx]
                    for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1))
                ):
                    grown[y][x] = True
        outside = grown

    seen = [[False] * WORK for _ in range(WORK)]
    holes = [[False] * WORK for _ in range(WORK)]
    for sy in range(WORK):
        for sx in range(WORK):
            if not white[sy][sx] or seen[sy][sx]:
                continue
            comp: list[tuple[int, int]] = []
            touches = False
            queue = deque([(sy, sx)])
            seen[sy][sx] = True
            while queue:
                y, x = queue.popleft()
                comp.append((y, x))
                if outside[y][x]:
                    touches = True
                for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    ny, nx = y + dy, x + dx
                    if 0 <= ny < WORK and 0 <= nx < WORK and white[ny][nx] and not seen[ny][nx]:
                        seen[ny][nx] = True
                        queue.append((ny, nx))
            if not touches:
                for y, x in comp:
                    holes[y][x] = True

    mask = Image.new("L", (WORK, WORK), 0)
    mp = mask.load()
    assert mp is not None
    for y in range(WORK):
        for x in range(WORK):
            if glyph[y][x] and not holes[y][x]:
                mp[x, y] = 255
    return mask.resize((SIZE, SIZE), Image.Resampling.LANCZOS)


def main() -> None:
    src = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / "pic" / "logo.png"
    if not src.is_file():
        raise SystemExit(f"[gen-tray-icon] 源图不存在：{src}")

    mask = build_mask(src)

    glyph = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    glyph.putalpha(mask)
    ICONS.mkdir(parents=True, exist_ok=True)
    png_path = ICONS / "tray-icon-template.png"
    glyph.save(png_path, "PNG")

    # 原始 RGBA 字节：tray.rs 以 include_bytes! 直接引用，长度必须恰好 36*36*4
    raw = glyph.tobytes()
    expected = SIZE * SIZE * 4
    if len(raw) != expected:
        raise SystemExit(f"[gen-tray-icon] 字节数异常：{len(raw)} != {expected}")
    (ICONS / "tray-icon-template.rgba").write_bytes(raw)

    opaque = sum(1 for i in range(3, len(raw), 4) if raw[i] > 0)
    print(f"[gen-tray-icon] 已生成 {SIZE}x{SIZE} 单色模板（不透明像素 {opaque}/{SIZE * SIZE}）")
    print(f"[gen-tray-icon]   {png_path.name}  {len(raw)} bytes -> tray-icon-template.rgba")


if __name__ == "__main__":
    main()
