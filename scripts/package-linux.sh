#!/bin/bash
# package-linux.sh
# BuddySwitch 本地一键打包 —— Linux .deb + AppImage
#
# 用法：
#   sh scripts/package-linux.sh                 # release .deb + AppImage
#   sh scripts/package-linux.sh debug          # debug
#   BUDDY_SWITCH_SIGN_PASSWORD=xxx sh scripts/package-linux.sh   # 覆盖默认签名密码
#
# 说明：
#   - 自动加载 ~/.cargo/env（rustup 不在 PATH 时）
#   - 检测 ~/.buddy-switch/buddy-switch-updater.key：有则设置签名环境变量；
#     无则临时关掉 createUpdaterArtifacts（Linux 未在 updater 端点配置，通常无需签名）
#   - 产物复制到 deliverables/

set -euo pipefail
cd "$(dirname "$0")/.."

MODE="${1:-release}"   # release | debug
SIGN_PASSWORD="${BUDDY_SWITCH_SIGN_PASSWORD:-buddy-switch-dev}"

# ---------- 1. 工具链 ----------
[ -s "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
command -v cargo >/dev/null 2>&1 || { echo "[err] 缺少 cargo，请安装 Rust。"; exit 1; }
command -v npm  >/dev/null 2>&1 || { echo "[err] 缺少 npm。"; exit 1; }
echo "[1/4] 工具链就绪: cargo=$(command -v cargo)  npm=$(command -v npm)"

# ---------- 2. 更新签名密钥检测 ----------
KEY="$HOME/.buddy-switch/buddy-switch-updater.key"
EXTRA=()
if [ -f "$KEY" ]; then
  export TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY")"
  export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$SIGN_PASSWORD"
  echo "[2/4] 检测到更新签名密钥。"
else
  echo "[2/4] 未检测到 $KEY，跳过 updater 签名（Linux 未配置 updater 端点）。"
  TMP="$(mktemp /tmp/buddy-switch-no-updater.XXXXXX.json)"
  printf '{ "bundle": { "createUpdaterArtifacts": false } }' > "$TMP"
  EXTRA=(--config "$TMP")
fi
[ "$MODE" = "debug" ] && EXTRA+=(--debug)
EXTRA+=(--bundles deb,appimage)

# ---------- 3. 构建 ----------
echo "[3/4] 开始 tauri build ($MODE, deb+appimage) ..."
npm run tauri -- build "${EXTRA[@]}"

# ---------- 4. 收集产物 ----------
echo "[4/4] 收集产物 ..."
VER="$(grep '"version"' package.json | head -1 | sed -E 's/.*"([0-9.]+)".*/\1/')"
BD="src-tauri/target/$MODE/bundle"
[ -d "$BD" ] || BD="target/$MODE/bundle"
mkdir -p deliverables
FOUND=0
for f in "$BD"/deb/*.deb "$BD"/appimage/*.AppImage; do
  if [ -e "$f" ]; then
    cp -f "$f" deliverables/
    echo "[ok] 复制 -> deliverables/$(basename "$f")"
    FOUND=1
  fi
done
[ "$FOUND" -eq 0 ] && echo "[warn] 未找到 $BD 下的 .deb/.AppImage，请检查构建输出。"

echo "[done] 打包完成 (v$VER, $MODE)"
