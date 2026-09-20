#!/bin/bash
# 打包时动态计算版本号并写回所有版本载体。
#
# usage:  sh scripts/stamp-version.sh              # 用当前时间生成
#         sh scripts/stamp-version.sh --print      # 只打印不写入
#         sh scripts/stamp-version.sh --tag v1.2.3 # 显式指定（CI tag 构建用）
#
# ## 为什么需要它
#
# 手工 `bump-version.sh <版本>` 的问题：每次打包都要人记着改，改漏一个载体就会出现
# 「安装包叫 2026.9.17，API 服务页显示 2026.9.16」这类不一致 —— 而且 `env!("CARGO_PKG_VERSION")`
# 透出的值还会被网关 `/status` 与自动更新的版本比较消费。
#
# 因此版本号改为**打包时从系统时间推导**，不再手工 bump。
#
# ## 版本形态：`<YYYY>.<M>.<DHHMM>`
#
# 例：2026-09-18 12:14 → `2026.9.181214`
#
# **必须是 3 段数字**，不能用 4 段。Cargo 强制严格 semver：
#   `version = "2026.9.18.1214"` → error: unexpected character '.' after patch version number
# 所以「日期」占前两段（沿用本仓库既有的 CalVer 习惯 `2026.9.18`），
# 「当日时分」折进第三段：18 日 12:14 → `181214`。
#
# 这样做的收益：
#   - 与历史产物（`BuddySwitch_2026.9.17_x64-setup.exe`）形态连续，不突兀；
#   - **同日多次打包版本号不同**（3 段方案 `<YYYY>.<M>.<D>` 做不到），
#     否则自动更新会误判「已是最新」，当天的修复推不出去，NSIS 产物还会同名覆盖；
#   - 与 `update::version_tuple()`（按 `.` 切分取整数）完全兼容，且**严格单调递增**：
#     同日内 DHHMM 递增，跨月/跨日因前两段递增而递增。
#
# 反例（不要用）：`2026.9.18.1214`（Cargo 拒绝）、`2026.918.1214`（丢失日期可读性）。
#
# ## 月/日不补零
#
# 与既有 CalVer 习惯一致（`2026.9.18` 而非 `2026.09.18`）。`version_tuple` 不关心
# 补零与否（`09` 与 `9` 都 parse 成 9），但不补零更符合本仓库既有产物的观感。
# 第三段的日与时**必须补零**：`181214` 若写成 `181214`/`91214` 会失去定长，
# 例如 9 日 9:09 的 `90909` 与 10 日 9:09 的 `100909` 虽仍递增，但跨月比较会错乱。
set -e

PRINT_ONLY=0
EXPLICIT=""
while [ $# -gt 0 ]; do
  case "$1" in
    --print) PRINT_ONLY=1; shift ;;
    --tag)   EXPLICIT="$2"; shift 2 ;;
    *)       echo "未知参数: $1" >&2; exit 2 ;;
  esac
done

cd "$(dirname "$0")/.."

if [ -n "$EXPLICIT" ]; then
  # CI tag 构建：tag 是唯一真相，不要用时间覆盖它。
  VER="${EXPLICIT#v}"
else
  # %-m/%-d 去掉月/日的前导零；%d%H%M 需要定长，所以单独取日并补零后拼接。
  VER=$(date +'%Y.%-m.')
  DAY=$(date +'%d')
  VER="${VER}${DAY}$(date +'%H%M')"
fi

if [ "$PRINT_ONLY" = "1" ]; then
  echo "$VER"
  exit 0
fi

node - "$VER" <<'EOF'
const fs = require('fs');
const path = require('path');
const version = process.argv[2];

/** 只替换 JSON 顶层 version 字段，避免误伤 dependencies 里的同名键。 */
function stampJson(file) {
  if (!fs.existsSync(file)) return;
  const json = JSON.parse(fs.readFileSync(file, 'utf8'));
  if (json.version === undefined) return;
  if (json.version === version) return;
  json.version = version;
  fs.writeFileSync(file, JSON.stringify(json, null, 2) + '\n');
  console.log(`  版本已更新: ${file}`);
}

/**
 * `package-lock.json` 有**两个** version 字段：顶层 `version` 与 `packages[""]`.
 * 漏掉后者时 `npm ci` 会因「lock 里的根包版本 ≠ package.json」而报错；
 * 漏掉前者会让 `npm version` / 部分 CI 工具读到旧值。两处都要写，且不得
 * 重排整个 lock 文件（`npm ci` 对字段顺序不敏感，但保持原格式便于 review diff）。
 */
function stampLock(file) {
  if (!fs.existsSync(file)) return;
  const raw = fs.readFileSync(file, 'utf8');
  const lock = JSON.parse(raw);
  let changed = false;
  if (lock.version !== undefined && lock.version !== version) {
    lock.version = version;
    changed = true;
  }
  if (lock.packages && lock.packages[''] && lock.packages[''].version !== version) {
    lock.packages[''].version = version;
    changed = true;
  }
  if (!changed) return;
  fs.writeFileSync(file, JSON.stringify(lock, null, 2) + '\n');
  console.log(`  版本已更新: ${file}`);
}

/** 只替换 Cargo.toml 中 [package] 段下的 version（首个 ^version = "..." 行）。 */
function stampCargo(file) {
  if (!fs.existsSync(file)) return;
  const raw = fs.readFileSync(file, 'utf8');
  const next = raw.replace(/^version = "[^"]+"/m, `version = "${version}"`);
  if (next === raw) return;
  fs.writeFileSync(file, next);
  console.log(`  版本已更新: ${file}`);
}

stampJson('package.json');
stampJson('src-tauri/tauri.conf.json');
stampJson('npm/package.json');
stampLock('package-lock.json');

// 平台包的 version 与主包 optionalDependencies 的引用必须同步，
// 否则 npm 安装时会去拉一个不存在的版本。
for (const dir of fs.readdirSync('npm/platform')) {
  stampJson(path.join('npm/platform', dir, 'package.json'));
}
const mainPkgPath = 'npm/package.json';
if (fs.existsSync(mainPkgPath)) {
  const mainPkg = JSON.parse(fs.readFileSync(mainPkgPath, 'utf8'));
  let changed = false;
  for (const key of Object.keys(mainPkg.optionalDependencies || {})) {
    if (mainPkg.optionalDependencies[key] !== version) {
      mainPkg.optionalDependencies[key] = version;
      changed = true;
    }
  }
  if (changed) {
    fs.writeFileSync(mainPkgPath, JSON.stringify(mainPkg, null, 2) + '\n');
    console.log(`  optionalDependencies 已更新: ${mainPkgPath}`);
  }
}

// 四个 crate 都要同步：漏掉任何一个，`env!("CARGO_PKG_VERSION")` 都会在
// 「API 服务」页显示成旧版本号（该值会被透出到网关 /status）。
for (const file of [
  'src-tauri/Cargo.toml',
  'crates/buddy-switch-core/Cargo.toml',
  'crates/buddy-switch-server/Cargo.toml',
  'crates/buddy-switch-gateway/Cargo.toml',
]) {
  stampCargo(file);
}
EOF

echo "版本已统一为 $VER"
