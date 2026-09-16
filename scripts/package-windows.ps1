# package-windows.ps1
# BuddySwitch 本地一键打包 —— Windows NSIS 安装包 (.exe)
#
# 用法（PowerShell）：
#   .\scripts\package-windows.ps1                 # release 安装包
#   .\scripts\package-windows.ps1 -Mode debug     # debug 安装包
#   .\scripts\package-windows.ps1 -SignPassword xxx   # 覆盖默认签名密码
#
# 特性：
#   - 自动定位受管 Node / cargo（cargo 不在 PATH 也能用）
#   - 自动检测 ~/.buddy-switch/buddy-switch-updater.key：
#       有密钥 -> 生成 updater 签名产物
#       无密钥 -> 临时关闭 createUpdaterArtifacts，仅打本地安装包，不报错
#   - 产物自动复制到 deliverables/

[CmdletBinding()]
param(
  [ValidateSet("release", "debug")]
  [string]$Mode = "release",
  [string]$SignPassword = "buddy-switch-dev"
)

$ErrorActionPreference = "Stop"

# 仓库根目录（脚本所在 scripts/ 的上一级）
$Root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $Root

function Write-Step($msg, $color = "Cyan") { Write-Host $msg -ForegroundColor $color }

# ---------- 1. 解析工具链 ----------
function Find-Node {
  $n = Get-Command node -ErrorAction SilentlyContinue
  if ($n) { return $n.Source }
  # 受管 Node 兜底：~/.workbuddy/binaries/node/versions/<ver>/node.exe
  $cand = Get-ChildItem "$env:USERPROFILE\.workbuddy\binaries\node\versions\*\node.exe" -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
  if ($cand) { return $cand.FullName }
  return $null
}

$nodeExe = Find-Node
if (-not $nodeExe) { Write-Error "未找到 node，请先安装 Node 或将其加入 PATH。"; exit 1 }
$nodeDir = Split-Path $nodeExe
$env:PATH = "$nodeDir;$env:PATH"   # 让 tauri 的 beforeBuildCommand (vite) 能找到 node

# 解析 npm（优先 .cmd，失败回退 .ps1 / PATH）
$npmExe = $null
foreach ($c in @("$nodeDir\npm.cmd", "$nodeDir\npm.ps1", "npm")) {
  if (Test-Path $c) { $npmExe = $c; break }
}
if (-not $npmExe) { $npmExe = "npm" }
Write-Step "[1/4] 工具链: node=$nodeExe`n      npm=$npmExe"

# cargo / rustup
if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
  $cargoCands = @(
    Join-Path $env:USERPROFILE ".cargo\bin"
    (Get-ChildItem "$env:USERPROFILE\.rustup\toolchains\*\bin" -ErrorAction SilentlyContinue |
      Sort-Object LastWriteTime -Descending | Select-Object -First 1 | ForEach-Object { $_.FullName })
  )
  foreach ($d in $cargoCands) {
    if ($d -and (Test-Path (Join-Path $d "cargo.exe"))) { $env:PATH = "$d;$env:PATH" }
  }
}
if (-not (Get-Command cargo.exe -ErrorAction SilentlyContinue)) {
  Write-Error "未找到 cargo，请安装 Rust 工具链 (https://rustup.rs)。"
  exit 1
}

# ---------- 2. 更新签名密钥检测 ----------
$keyPath = Join-Path $env:USERPROFILE ".buddy-switch\buddy-switch-updater.key"
$tempCfg = $null
$tauriArgs = @("build")
if (Test-Path $keyPath) {
  $env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content $keyPath -Raw).Trim()
  $env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = $SignPassword
  Write-Step "[2/4] 检测到更新签名密钥，将生成 updater 产物。" "Green"
} else {
  Write-Step "[2/4] 未检测到 $keyPath，仅打包本地安装包（跳过 updater 签名）。" "Yellow"
  # 缺密钥时若 createUpdaterArtifacts=true 会构建失败，改用临时配置关掉它
  $tempCfg = Join-Path $env:TEMP "buddy-switch-no-updater.json"
  Set-Content -Encoding UTF8 $tempCfg '{ "bundle": { "createUpdaterArtifacts": false } }'
  $tauriArgs += "--config"; $tauriArgs += $tempCfg
}
if ($Mode -eq "debug") { $tauriArgs += "--debug" }
$tauriArgs += "--bundles"; $tauriArgs += "nsis"

# ---------- 3. 构建 ----------
Write-Step "[3/4] 开始 tauri build ($Mode, nsis) ..."
& $npmExe run tauri -- @tauriArgs
if ($LASTEXITCODE -ne 0) {
  if ($tempCfg) { Remove-Item $tempCfg -ErrorAction SilentlyContinue }
  Write-Error "构建失败 (exit $LASTEXITCODE)。"
  exit $LASTEXITCODE
}

# ---------- 4. 收集产物 ----------
Write-Step "[4/4] 收集产物 ..."
$ver = ((Get-Content package.json -Raw | ConvertFrom-Json).version)
$bundleDir = Join-Path $Root "src-tauri\target\$Mode\bundle\nsis"
if (-not (Test-Path $bundleDir)) { $bundleDir = Join-Path $Root "target\$Mode\bundle\nsis" }

$out = Get-ChildItem $bundleDir -Filter "*.exe" -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($out) {
  $destDir = Join-Path $Root "deliverables"
  if (-not (Test-Path $destDir)) { New-Item -ItemType Directory -Path $destDir | Out-Null }
  Copy-Item $out.FullName (Join-Path $destDir $out.Name) -Force
  Write-Step "[ok] 已复制 -> deliverables\$($out.Name)" "Green"
} else {
  Write-Step "[warn] 未找到 $bundleDir\*.exe，请检查构建输出。" "Yellow"
}

if ($tempCfg) { Remove-Item $tempCfg -ErrorAction SilentlyContinue }
Write-Step "[done] 打包完成 (v$ver, $Mode) -> deliverables/" "Green"
