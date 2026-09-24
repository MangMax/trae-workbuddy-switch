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

主包与平台分包统一在 **`@mangmax` scope** 下：主包 `@mangmax/buddy-switch`，
平台包 `@mangmax/buddy-switch-<platform>-<arch>`（**目录名不带 scope**，
仍是 `npm/platform/buddy-switch-<tag>`，`build.yml` 的 `PKG_DIR` 与生成脚本都按目录名拼路径）。
安装后的**命令名仍是 `buddy-switch`**（`bin` 字段决定，与包名无关）。

1. 编译 server 二进制并上传 GitHub Release（`.github/workflows/build.yml` 自动执行）
2. 先 `sh scripts/gen-platform-packages.sh <版本>` 生成 5 个平台包，把对应二进制放进各包 `bin/` 后逐个 `npm publish --access public`
3. `cd npm && npm publish --access public`（主包，`postinstall` 从已安装的平台包复制二进制）

> ⚠️ **发布前提**：npm 账号必须拥有 **`mangmax` 这个 scope**（用户名即为 `mangmax`，
> 或在该账号下创建同名 organization）。scope 不属于自己时 `npm publish` 会 403，
> 且 `@mangmax/*` 是别人无法代持的命名空间。
>
> 另外：`buddy-switch`（不带 scope）这个包名**在 npm 上已被他人占用**，所以不能再退回无 scope 命名；
> 本项目此前的发布用的是 `workbuddy-switch`。
> scoped 包必须带 `--access public`，否则会以私有包发布（私有包需要付费账号）。
>
> `npm/package.json` 的 `optionalDependencies` 目前列了 4 个平台（darwin-arm64 / darwin-x64 /
> win32-x64 / linux-x64），与 CI 矩阵一致；`npm/platform/buddy-switch-linux-arm64/` 这个目录
> **尚未接入**（既不在依赖里，CI 也不构建它），别误以为 linux-arm64 已可用。

### 只读演示（本地构建）

本仓库为**私人维护版**，不做在线演示部署：原先每次 push 到 `main` 都会触发
`.github/workflows/pages.yml` 部署 GitHub Pages，该工作流已**移除**（私人仓库的 Pages
在免费计划下不可用，且每次 push 都留一条失败的 run）。

需要只读演示时在本地生成：

```bash
npm run build:demo   # 输出到 dist-demo/，base 为 /trae-workbuddy-switch/
```

> ⚠️ 演示构建与 WebUI/桌面构建**必须分流到不同目录**：`dist/` 被 `rust-embed`、
> Tauri `frontendDist` 与 `scripts/fix-app.sh` 三处消费，把演示产物写进 `dist/` 会让
> `index.html` 请求 embed 中不存在的资源路径，webui 与桌面端双双白屏。
> `build:demo` 已用 `--outDir dist-demo` 固定输出目录，回归测试
> `embedded_index_html_references_only_embedded_assets` 守住这条边界。

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
