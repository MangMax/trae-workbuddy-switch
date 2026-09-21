# 开发指南

## 环境要求

Node.js ≥ 20、Rust stable、macOS（或 Windows/Linux）。

## 开发命令

```bash
npm install
npm run tauri dev        # 开发模式
npm run build:app        # 构建 debug .app（含前端资源补丁）
npm run build:app:release  # 构建 release .app + 签名更新包
```

## 发布新版本

签名密钥（自动更新用）存放于 `~/.buddy-switch/buddy-switch-updater.key`，构建脚本通过
`TAURI_SIGNING_PRIVATE_KEY` 注入。发布新版本时：

1. `npm run build:app:release` 生成 `.app.tar.gz` + `.sig`（Windows NSIS 构建会额外生成当前版本的 `*_x64-setup.exe` + `.exe.sig`）。CI 会先清掉 `target/**/release/bundle`，避免 cargo cache 把旧安装包带进 Release。
2. macOS：`UPDATE_OS=macos UPDATE_ARCH=aarch64 sh scripts/gen-update-json.sh` 生成 `latest-macos-aarch64.json`；Intel 用 `UPDATE_ARCH=x86_64`
3. Windows：`UPDATE_OS=windows UPDATE_ARCH=x86_64 sh scripts/gen-update-json.sh` 生成 `latest-windows-x86_64.json`
4. `python3 scripts/merge-update-manifests.py <产物目录>` 合并为 `latest.json`，并把 Windows 平台项写入 `latest-macos-x86_64.json`（兼容已安装的 Windows 客户端）
5. 将安装包、签名更新包、`latest*.json` 一并上传到 GitHub Release

### npm 版（webui）发布

1. 编译 server 二进制并上传 GitHub Release（`.github/workflows/build.yml` 自动执行）
2. 先 `sh scripts/gen-platform-packages.sh <版本>` 生成 5 个平台包（`buddy-switch-<platform>-<arch>`），把对应二进制放进各包 `bin/` 后逐个 `npm publish`
3. `cd npm && npm publish`（主包名 `buddy-switch`，`postinstall` 从已安装的平台包复制二进制）

> ⚠️ **发布前先确认包名可用**：截至 2026-09-21，`buddy-switch` 在 npm 上**已被他人占用**
> （`0.1.2`，描述 "Customize your Claude Code buddy pet"，维护者 `zjding`），直接 `npm publish` 会 403，
> 而 `npm i -g buddy-switch` 会装上别人的包。需要用别的名字（例如 scope 包）或保留旧名
> `workbuddy-switch`（该名字下的 `0.1.x` 是本项目此前的发布）。

### 在线演示（GitHub Pages）部署前提

`.github/workflows/pages.yml` 在每次 push 到 `main` 时构建只读演示并部署。
**首次部署前必须先手动启用 Pages**：仓库 Settings → Pages → Source 选 **GitHub Actions**。

未启用时该工作流会**恰好失败在 `Configure Pages` 这一步**（前面的 `npm ci` 与
`npm run build:demo` 都是通过的，容易误判成构建坏了），并且此后每次 push 都会留一条红的 run。

> **不要试图用 `enablement: true` 绕过这一步**：`actions/configure-pages` 的文档明确要求该选项
> 使用 `GITHUB_TOKEN` **以外**的 token（PAT 的 `repo` scope，或 GitHub App 的
> `administration:write` + `pages:write`），加了不但仍然失败，还会平白引入一个密钥依赖。

## 目录结构

```
src-tauri/
  src/
    commands.rs      # Tauri command 薄包装（对应 Python 版 HTTP API）
    modules/         # 已抽离到 crates/buddy-switch-core（三宿主复用）
crates/
  buddy-switch-core/    # 核心逻辑：account/auth_file/oauth/process/switch/session/checkin/refresh/update/config
  buddy-switch-server/  # HTTP server + CLI：axum API + rust-embed 前端
src/                 # 前端：components/pages/lib（api.ts 双通道：Tauri invoke / HTTP fetch）
npm/                 # npm 包：package.json + bin + scripts/install.js
```

## 隐私注意事项

- 仓库不提交本地数据（accounts.json、认证文件、密钥、token 由 `.gitignore` 排除）
- 发布前用 `git grep` 扫描 token 模式（`ghp_`/`npm_`/`gho_` 等）
