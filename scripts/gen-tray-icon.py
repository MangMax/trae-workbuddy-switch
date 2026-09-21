#!/usr/bin/env python3
"""生成托盘图标（macOS 菜单栏 template image / Windows 通知区）。

背景：两个平台对「颜色」的处理完全不同，这决定了本脚本填什么 RGB。
  - macOS 菜单栏是 **template image**：系统只取 **alpha 通道**当形状，
    颜色由菜单栏前景色着色（浅色/深色模式自动适配）⇒ **RGB 被完全忽略**。
  - Windows 通知区**没有** template 语义（`icon_as_template` 是 no-op），
    RGB **原样生效** ⇒ 深色任务栏下黑色剪影几乎看不见。

结论：RGB 一律填白色（`INK`）。对 macOS 无副作用（RGB 被忽略），对 Windows 是必需的。
alpha 通道始终才是形状的唯一来源。

做法：
  1. 去掉青绿底色（判据：R 明显低于 G 与 B），得到猫头整体轮廓；
  2. 把「被暗部完全包围的白色区域」挖成镂空——它们是猫的眼睛（白脸直接贴着青色
     背景，故不会被误判）；
  3. 缩放到 36x36 并输出两种形态：
       - `tray-icon-template.png`  PNG（便于预览/替换）
       - `tray-icon-template.rgba` 原始 RGBA 字节（`tray.rs` 用 include_bytes! 直接引用）

⚠️ `tray.rs` 里有 `const ICON: &[u8; 36 * 36 * 4]` 的**编译期长度断言**：
尺寸一旦不是 36x36，Rust 侧会直接编译失败，两者必须同步修改。

⚠️ **不要手工改 `tray-icon-template.png`**。运行时真正生效的是 `.rgba`
（`tray.rs` 以 `include_bytes!` 引用），PNG 只是预览。手改 PNG 既不影响运行，
也会在下次跑本脚本时被静默覆盖，使两个产物不一致——要改就改这里的 `INK`。

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
# 剪影的填充色。白色：Windows 通知区按 RGB 原样显示（深色任务栏需要白），
# macOS 走 template image 只取 alpha、忽略 RGB，故同一份产物两端通用。
INK = (255, 255, 255)

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

    # 找出「被暗部完全包围的白色连通块」= 猫眼。
    # 判据：该白色块的所有边界邻居都在图形内部（不接触任何青色/透明背景）。
    # 猫的白脸直接贴着青色底色，故不会被误判——实测白脸 bg 接触率 0.59、两眼均为 0.00。
    seen = [[False] * WORK for _ in range(WORK)]
    holes = [[False] * WORK for _ in range(WORK)]
    for sy in range(WORK):
        for sx in range(WORK):
            if not white[sy][sx] or seen[sy][sx]:
                continue
            comp: list[tuple[int, int]] = []
            touches_bg = False
            queue = deque([(sy, sx)])
            seen[sy][sx] = True
            while queue:
                y, x = queue.popleft()
                comp.append((y, x))
                for dy, dx in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                    ny, nx = y + dy, x + dx
                    if not (0 <= ny < WORK and 0 <= nx < WORK):
                        touches_bg = True
                        continue
                    if glyph[ny][nx]:
                        if white[ny][nx] and not seen[ny][nx]:
                            seen[ny][nx] = True
                            queue.append((ny, nx))
                    else:
                        touches_bg = True
            # 面积上限防止把「整片不接触背景的白脸」误当镂空
            if not touches_bg and len(comp) < WORK * WORK // 8:
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


def verify(png_path: Path, raw: bytes) -> None:
    """回读 PNG，确认它与 .rgba 同源：alpha 逐像素一致，且不透明像素的 RGB 都是 INK。

    挡的是 **PNG 往返编码失真**——Rust 侧吃的是 `.rgba`，PNG 只是给人看的预览；
    一旦 Pillow 在存盘时改动了 alpha 或把填充色压回黑色，两边就会悄悄分叉，
    而肉眼只看 PNG 看不出来。注意它**拦不住**「手改产物」：两个文件都在本函数
    之前被重写了，手改只会被静默覆盖（见文件头说明）。
    """
    img = Image.open(png_path).convert("RGBA")
    if img.size != (SIZE, SIZE):
        raise SystemExit(f"[gen-tray-icon] PNG 尺寸异常：{img.size} != {(SIZE, SIZE)}")
    got = img.tobytes()
    if len(got) != len(raw):
        raise SystemExit(f"[gen-tray-icon] PNG 与 .rgba 长度不一致：{len(got)} != {len(raw)}")
    for i in range(0, len(raw), 4):
        if got[i + 3] != raw[i + 3]:
            raise SystemExit(
                f"[gen-tray-icon] PNG 与 .rgba 的 alpha 在像素 {i // 4} 处不一致 "
                f"（{got[i + 3]} != {raw[i + 3]}）"
            )
        if raw[i + 3] > 0 and (got[i], got[i + 1], got[i + 2]) != INK:
            raise SystemExit(
                f"[gen-tray-icon] 像素 {i // 4} 的填充色不是 {INK}："
                f"{(got[i], got[i + 1], got[i + 2])}"
            )


def main() -> None:
    src = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / "pic" / "logo.png"
    if not src.is_file():
        raise SystemExit(f"[gen-tray-icon] 源图不存在：{src}")

    mask = build_mask(src)

    glyph = Image.new("RGBA", (SIZE, SIZE), (*INK, 0))
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
    verify(png_path, raw)

    opaque = sum(1 for i in range(3, len(raw), 4) if raw[i] > 0)
    print(
        f"[gen-tray-icon] 已生成 {SIZE}x{SIZE} 单色托盘图标 "
        f"（填充色 RGB{INK}，不透明像素 {opaque}/{SIZE * SIZE}）"
    )
    print(f"[gen-tray-icon]   {png_path.name}  {len(raw)} bytes -> tray-icon-template.rgba")


if __name__ == "__main__":
    main()
