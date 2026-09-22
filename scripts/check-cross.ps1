<#
.SYNOPSIS
  在 Windows 上验证 macOS / Linux 目标的 Rust 编译（`cargo check`），不需要非 Windows 机器。

.DESCRIPTION
  本仓的开发全在 Windows 上，而「`#[cfg(windows)]` 下定义的东西被**未门禁**代码引用」
  这类缺口只有 mac/linux 目标会暴露 —— 本机构建永远编不到那段代码，于是 CI 成了唯一
  的检查手段。这个脚本把它提前到本地。

  原理（三步，缺一不可）：

    1. `cargo check --target <triple>` **不做链接** → 不需要目标平台的链接器，Windows 上能跑。
    2. 但 build script 照常执行：`libsqlite3-sys`(bundled) 要编 `sqlite3.c`、
       `ring` 要编 C/汇编，它们都会去找**目标平台**的 C 编译器。
       Windows 上没有 → 用 `scripts/cross-cc-stub.rs` 编出的桩顶替（造出空目标文件并返回 0）。
       因为不链接，空目标文件不影响类型检查结论。
    3. 同时设 `CARGO_BUILD_WARNINGS=deny`，与 CI 的策略一致（CI 的
       `actions-rust-lang/setup-rust-toolchain@v1` 默认 `build-warnings: "deny"`）。
       这样「只在非 Windows 上出现」的 `unused_mut` 之类警告也会在本地被拦下 ——
       否则它会变成 CI 上的硬错误（退出码 101）。

  覆盖范围：
    * buddy-switch-core / buddy-switch-gateway / buddy-switch-server —— 三个目标全覆盖
      （等价于 CI 的 `Build server binary` 那一步）。
    * src-tauri（`buddy-switch-rust`）—— 加 `-IncludeDesktop` 时对 **macOS** 目标一并检查。
      **Linux 桌面端无法在本机检查**：gtk / glib / webkit2gtk / soup3 的 build script 依赖
      真实的 pkg-config 与 `.pc` 元数据，Windows 上没有，桩也顶替不了。
      这一块只能靠 CI 的 ubuntu job 暴露。

.PARAMETER Targets
  要检查的目标三元组，默认三个（linux-x64 / mac-arm64 / mac-x64）。

.PARAMETER IncludeDesktop
  额外检查 src-tauri（仅对 macOS 目标生效）。

.EXAMPLE
  检查三个目标（默认）：
    powershell -ExecutionPolicy Bypass -File scripts/check-cross.ps1

.EXAMPLE
  只查 Linux：
    powershell -ExecutionPolicy Bypass -File scripts/check-cross.ps1 -Targets x86_64-unknown-linux-gnu

.EXAMPLE
  连桌面端一起查（仅 macOS 目标）：
    powershell -ExecutionPolicy Bypass -File scripts/check-cross.ps1 -IncludeDesktop
#>
[CmdletBinding()]
param(
  [string[]]$Targets = @(
    "x86_64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-apple-darwin"
  ),
  [switch]$IncludeDesktop
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot

# ---------------------------------------------------------------- 定位工具链
# 注意：本机 `~/.cargo/bin/` 里**只有 rustup**，cargo / rustc 实际位于
# `~/.rustup/toolchains/<toolchain>/bin/`，而那个目录通常不在 PATH 上
# （直接 `& "…\bin\cargo.exe"` 能跑，但 build script 再去找 rustc 就找不到了）。
# 因此一律先定位 rustup，再用 `rustup which` 解析出真实的 cargo / rustc。
function Resolve-Rustup {
  $candidates = @(
    (Join-Path $env:USERPROFILE ".cargo\bin\rustup.exe"),
    (Join-Path $env:USERPROFILE ".cargo\bin\rustup")
  )
  foreach ($c in $candidates) {
    if ([System.IO.File]::Exists($c)) { return $c }
  }
  $cmd = Get-Command rustup -ErrorAction SilentlyContinue
  if ($cmd) { return $cmd.Source }
  throw "找不到 rustup。请先安装 rustup（https://rustup.rs）。"
}

function Resolve-RustTool([string]$name) {
  $resolved = (& $rustup which $name 2>$null)
  if ($resolved) { $resolved = "$resolved".Trim() }
  if ($resolved -and [System.IO.File]::Exists($resolved)) { return $resolved }
  $cmd = Get-Command $name -ErrorAction SilentlyContinue
  if ($cmd) { return $cmd.Source }
  throw "找不到 $name（rustup which $name 也没解析出来）。"
}

$rustup = Resolve-Rustup
$cargo = Resolve-RustTool "cargo"
$rustc = Resolve-RustTool "rustc"

# 把工具链目录加进 PATH：build script 会自己去调 rustc，PATH 里没有就会 panic
# （实测 `libc` 的 build.rs：「Failed to get rustc version: program not found」）。
$env:PATH = "$(Split-Path -Parent $cargo);$(Split-Path -Parent $rustc);$env:PATH"

Write-Host "cargo : $cargo"
Write-Host "rustc : $rustc"
Write-Host ""

# ---------------------------------------------------------------- 补齐目标
$installed = & $rustup target list --installed
$missing = @($Targets | Where-Object { $installed -notcontains $_ })
if ($missing.Count -gt 0) {
  Write-Host "安装缺失的目标标准库: $($missing -join ', ')"
  & $rustup target add @missing
  if ($LASTEXITCODE -ne 0) { throw "rustup target add 失败" }
  Write-Host ""
}

# ----------------------------------------------------- 编译 cc/ar 桩（本脚本自持）
$stubDir = Join-Path ([System.IO.Path]::GetTempPath()) "buddy-switch-cross-cc"
[System.IO.Directory]::CreateDirectory($stubDir) | Out-Null
$stubExe = Join-Path $stubDir "ccstub.exe"
$stubSrc = Join-Path $PSScriptRoot "cross-cc-stub.rs"
Write-Host "编译 C 编译器桩 -> $stubExe"
& $rustc -O -o $stubExe $stubSrc
if ($LASTEXITCODE -ne 0) { throw "cc/ar 桩编译失败" }

$stubEnvNames = @{}
foreach ($t in $Targets) {
  $key = $t -replace '[-.]', '_'
  $stubEnvNames["CC_$key"] = $stubExe
  $stubEnvNames["AR_$key"] = $stubExe
}

# 与 CI 一致：警告即错误（CI 的 setup-rust-toolchain 默认 build-warnings=deny）
$env:CARGO_BUILD_WARNINGS = "deny"
$env:CARGO_INCREMENTAL = "0"

# ---------------------------------------------------------------- 逐个目标检查
$results = @()

foreach ($t in $Targets) {
  $key = $t -replace '[-.]', '_'
  Set-Item -Path "Env:CC_$key" -Value $stubExe
  Set-Item -Path "Env:AR_$key" -Value $stubExe

  $pkgs = @("buddy-switch-server")
  $isMac = $t -like "*apple-darwin*"
  if ($IncludeDesktop -and $isMac) {
    # Linux 桌面端依赖 pkg-config（gtk/webkit2gtk），本机无法检查 —— 故只加 macOS。
    $pkgs += "buddy-switch-rust"
  }
  if ($IncludeDesktop -and -not $isMac) {
    Write-Host "[跳过] $t 的桌面端：Linux 桌面端需要 pkg-config(gtk/webkit2gtk)，本机无法检查"
  }

  foreach ($pkg in $pkgs) {
    Write-Host ""
    Write-Host "=== cargo check -p $pkg --target $t ==="
    Write-Host "(CARGO_BUILD_WARNINGS=deny，与 CI 一致)"
    Push-Location $repoRoot
    try {
      & $cargo check -p $pkg --target $t
      $code = $LASTEXITCODE
    } finally {
      Pop-Location
    }
    $results += [pscustomobject]@{ Target = $t; Package = $pkg; ExitCode = $code }
    if ($code -eq 0) {
      Write-Host "[通过] $pkg / $t" -ForegroundColor Green
    } else {
      Write-Host "[失败] $pkg / $t（退出码 $code）" -ForegroundColor Red
    }
  }
}

# ---------------------------------------------------------------- 汇总
Write-Host ""
Write-Host "================ 汇总 ================"
$failed = 0
foreach ($r in $results) {
  $mark = if ($r.ExitCode -eq 0) { "通过" } else { "失败" }
  if ($r.ExitCode -ne 0) { $failed++ }
  "{0,-7} {1,-26} {2}" -f $mark, $r.Target, $r.Package | Write-Host
}
Write-Host ""
if ($failed -gt 0) {
  Write-Host "$failed 项失败。注意区分两类原因：" -ForegroundColor Red
  Write-Host "  · rustc 诊断（E0425 / unused_mut 等）→ 代码在非 Windows 上真编不过，必须修；"
  Write-Host "  · 『拒绝访问 (os error 5)』→ 本机权限/沙箱拦截，不是代码问题，换普通终端重跑。"
  exit 1
}
Write-Host "全部通过：这些 crate 在对应的非 Windows 目标上零警告编过。" -ForegroundColor Green
exit 0
