#!/bin/bash
# 打包时动态计算版本号并写回所有版本载体。
#
# ⚠️ 这只是一个**薄包装**：真正的逻辑在 `scripts/stamp-version.mjs`（Node 实现）。
#
# 为什么改：本机（Windows）没有 bash/sh（连 Git Bash 都没装），而打包流程还需要
# 在 Windows 上跑（`scripts/package-windows.ps1`）。把逻辑收进 .mjs 后
# **两侧共用同一份实现**，不会出现「bash 侧改了、PowerShell 侧没改」的漂移。
# 本文件保留是为了不破坏 CI（`.github/workflows/*.yml`）与 macOS/Linux 上的既有调用。
#
# usage:  sh scripts/stamp-version.sh              # 用当前时间生成
#         sh scripts/stamp-version.sh --print      # 只打印不写入
#         sh scripts/stamp-version.sh --tag v1.2.3 # 显式指定（CI tag 构建用）
#         sh scripts/stamp-version.sh --check      # 校验所有载体是否一致（不一致退出码 1）
#
# 版本形态与推导规则见 `scripts/stamp-version.mjs` 头部注释（`<YYYY>.<M>.<DHHMM>`，必须 3 段）。
set -e
cd "$(dirname "$0")/.."

if ! command -v node >/dev/null 2>&1; then
  echo "未找到 node（stamp-version.mjs 需要它）。" >&2
  exit 1
fi

exec node scripts/stamp-version.mjs "$@"
