#!/bin/bash
# 统一 bump 版本：usage: sh scripts/bump-version.sh 0.1.5
#
# 用 node 而不是 perl 做替换：本项目本就强依赖 node（Tauri + vite），
# 而 perl 在 Windows / Git Bash 上默认不存在——原实现会在这里直接失败。
set -e
V=$1
[ -z "$V" ] && echo "用法: sh scripts/bump-version.sh <新版本>" && exit 1
cd "$(dirname "$0")/.."

node - "$V" <<'EOF'
const fs = require('fs');
const path = require('path');
const version = process.argv[2];

/** 只替换 JSON 顶层 version 字段，避免误伤 dependencies 里的同名键。 */
function bumpJsonVersion(file) {
  if (!fs.existsSync(file)) {
    console.warn(`跳过（不存在）: ${file}`);
    return;
  }
  const raw = fs.readFileSync(file, 'utf8');
  const json = JSON.parse(raw);
  if (json.version === undefined) {
    console.warn(`跳过（无 version 字段）: ${file}`);
    return;
  }
  json.version = version;
  fs.writeFileSync(file, JSON.stringify(json, null, 2) + '\n');
  console.log(`已更新: ${file}`);
}

/** 只替换 Cargo.toml 中 [package] 段下的 version（首个 ^version = "..." 行）。 */
function bumpCargoVersion(file) {
  if (!fs.existsSync(file)) {
    console.warn(`跳过（不存在）: ${file}`);
    return;
  }
  const raw = fs.readFileSync(file, 'utf8');
  const next = raw.replace(/^version = "[^"]+"/m, `version = "${version}"`);
  if (next === raw) {
    console.warn(`跳过（未匹配 version 行）: ${file}`);
    return;
  }
  fs.writeFileSync(file, next);
  console.log(`已更新: ${file}`);
}

const jsonTargets = [
  'package.json',
  'src-tauri/tauri.conf.json',
  'npm/package.json',
];
for (const file of jsonTargets) bumpJsonVersion(file);

// 平台包的 version 与主包 optionalDependencies 的引用必须同步，
// 否则 npm 安装时会去拉一个不存在的版本。
for (const dir of fs.readdirSync('npm/platform')) {
  const file = path.join('npm/platform', dir, 'package.json');
  bumpJsonVersion(file);
}

const mainPkgPath = 'npm/package.json';
const mainPkg = JSON.parse(fs.readFileSync(mainPkgPath, 'utf8'));
for (const key of Object.keys(mainPkg.optionalDependencies || {})) {
  mainPkg.optionalDependencies[key] = version;
}
fs.writeFileSync(mainPkgPath, JSON.stringify(mainPkg, null, 2) + '\n');

// 四个 crate 都要同步：漏掉任何一个，`env!("CARGO_PKG_VERSION")` 就会
// 在「API 服务」页显示成旧版本号（该值会被透出到网关 /status）。
const cargoTargets = [
  'src-tauri/Cargo.toml',
  'crates/buddy-switch-core/Cargo.toml',
  'crates/buddy-switch-server/Cargo.toml',
  'crates/buddy-switch-gateway/Cargo.toml',
];
for (const file of cargoTargets) bumpCargoVersion(file);

console.log(`所有版本已同步为 ${version}（含平台包、optionalDependencies 与 4 个 crate）`);
EOF
