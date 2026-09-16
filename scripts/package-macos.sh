#!/bin/bash
# package-macos.sh
# BuddySwitch 本地一键打包 —— macOS .app（含 fix-app 补 dist + 签名）
#
# 用法：
#   sh scripts/package-macos.sh                 # release .app
#   sh scripts/package-macos.sh debug          # debug .app
#   BUDDY_SWITCH_SIGN_PASSWORD=xxx sh scripts/package-macos.sh   # 覆盖默认签名密码
#
# 说明：
#   - 自动加载 ~/.cargo/env（rustup 不在 PATH 时）
#   - 检测 ~/.buddy-switch/buddy-switch-updater.key：有则生成 updater 签名产物；
#     无则临时关掉 createUpdaterArtifacts，仅打本地 .app
#   - 构建后执行 scripts/fix-app.sh（把前端 dist 补进 .app 并用证书/adhoc 签名，供 TCC 识别）
#   - 产物复制到 deliverables/BuddySwitch.app

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
  echo "[2/4] 检测到更新签名密钥，生成 updater 产物。"
else
  echo "[2/4] 未检测到 $KEY，仅打包本地 .app（跳过 updater 签名）。"
  TMP="$(mktemp /tmp/buddy-switch-no-updater.XXXXXX.json)"
  printf '{ "bundle": { "createUpdaterArtifacts": false } }' > "$TMP"
  EXTRA=(--config "$TMP")
fi
[ "$MODE" = "debug" ] && EXTRA+=(--debug)
EXTRA+=(--bundles app)

# ---------- 3. 构建 ----------
echo "[3/4] 开始 tauri build ($MODE, app) ..."
npm run tauri -- build "${EXTRA[@]}"

# 把前端 dist 补进 .app 并用证书/adhoc 签名（TCC 识别）
if [ -f scripts/fix-app.sh ]; then
  sh scripts/fix-app.sh
fi

# ---------- 4. 收集产物 ----------
echo "[4/4] 收集产物 ..."
VER="$(grep '"version"' package.json | head -1 | sed -E 's/.*"([0-9.]+)".*/\1/')"
SRC="src-tauri/target/$MODE/bundle/macos/BuddySwitch.app"
[ -d "$SRC" ] || SRC="target/$MODE/bundle/macos/BuddySwitch.app"
if [ -d "$SRC" ]; then
  mkdir -p deliverables
  rm -rf "deliverables/BuddySwitch.app"
  cp -R "$SRC" "deliverables/BuddySwitch.app"
  echo "[ok] 已复制 -> deliverables/BuddySwitch.app (v$VER, $MODE)"
else
  echo "[warn] 未找到 $SRC，请检查构建输出。"
fi

# 可选：生成 DMG（脚本已提供 scripts/make-dmg.sh）
if [ -f scripts/make-dmg.sh ]; then
  echo "[info] 如需 DMG 安装盘，运行: sh scripts/make-dmg.sh"
fi

echo "[done] 打包完成 (v$VER, $MODE)"
