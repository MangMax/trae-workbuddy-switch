#!/usr/bin/env python3
"""把源 logo 同步到 `public/` 下的前端图标资源。

## 为什么需要这个脚本

`gen-icons.mjs` 只负责 `src-tauri/icons/`（打包进安装包的图标），
**从不触碰 `public/`** —— 而前端另有两处图标走的是 `public/`：

1. `public/icon-transparent.png` —— 侧栏品牌标记（`AppIconMark` → `product-marks.tsx`），
   以 `object-contain` 直接渲染，不带圆角包装；
2. `public/icon.png` —— Web/演示版的 favicon 与分享缩略图（与上者内容一致，
   分成两个文件是因为引用语义不同：标记 vs 站点图标）。

这两处曾长期停留在 09-10 的旧设计（绿色扁平猫），与 09-16 启用的新 logo
（青绿圆角方块 + 白猫 + 黑面罩）完全不同，且因为不在任何脚本的产出清单里，
换 logo 时**永远不会**被更新 —— 表现为「侧栏图标不是最新的」。

## 为什么不在这里再烘焙圆角

源 logo 本身已经是**烘焙好圆角的圆角方块**（四角透明、`RADIUS_RATIO≈0.2237`
与 Windows 侧一致）。再套一层圆角只会把边缘磨两次，出现毛边；
因此这里只做「等比缩放 + 落盘」，不引入任何遮罩。

用法：python scripts/gen-public-icons.py [源图路径]（默认 pic/logo.png）
"""

import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
PUBLIC = ROOT / "public"
SIZE = 1024


def main() -> None:
    src_path = (
        Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / "pic" / "logo.png"
    )
    if not src_path.is_file():
        raise SystemExit(f"[gen-public-icons] 源图不存在：{src_path}")

    src = Image.open(src_path).convert("RGBA")
    if src.width != src.height:
        raise SystemExit(f"[gen-public-icons] 源图必须是正方形，当前 {src.size}")

    src = src.resize((SIZE, SIZE), Image.Resampling.LANCZOS)

    PUBLIC.mkdir(parents=True, exist_ok=True)
    transparent = PUBLIC / "icon-transparent.png"
    src.save(transparent, "PNG")
    src.save(PUBLIC / "icon.png", "PNG")

    for p in (transparent, PUBLIC / "icon.png"):
        kb = p.stat().st_size / 1024
        print(f"[gen-public-icons] {p.relative_to(ROOT)}  {SIZE}x{SIZE}  {kb:.1f} KB")


if __name__ == "__main__":
    main()
