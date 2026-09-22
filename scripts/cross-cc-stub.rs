// cross-cc-stub.rs —— `scripts/check-cross.ps1` 专用的一次性 `cc` / `ar` 桩程序。
//
// ## 它解决什么问题
//
// 本仓全部开发都在 Windows 上进行，而「`#[cfg(windows)]` 下定义的东西被**未门禁**代码
// 引用」这类缺口**只有** mac/linux 目标能暴露（本机 Windows 构建永远编不到那段代码）。
// 于是 CI 成了唯一的检查手段 —— 一次 red 要等十几分钟，还得靠猜。
//
// 想在本机自己查，唯一可行的办法是 `cargo check --target <triple>`：
//   * `check` 不做链接 → 不需要目标平台的链接器，Windows 上就能跑；
//   * 但 **build script 照常执行** → `libsqlite3-sys`（bundled）要编译 `sqlite3.c`、
//     `ring` 要编译一批 C/汇编，二者都会去调用**目标平台**的 C 编译器
//     （`x86_64-linux-gnu-gcc` / `aarch64-apple-darwin-clang`）——
//     Windows 上没有，build script 直接失败，检查根本走不到 Rust 代码。
//
// 本桩就是那个「目标平台 C 编译器」：把请求的输出文件造出来（空文件）并返回 0。
// 因为 `check` 不链接，空目标文件不影响**类型检查**结论，而类型检查正是我们要的。
//
// ## ⚠️ 使用边界（务必遵守）
//
// 只可用于 `cargo check`。**绝不可用于 `build` / `link`** ——
// 真链接会因为空对象文件而失败，或更糟：产出看起来正常但实际缺失 native 代码的二进制。
//
// 由 `scripts/check-cross.ps1` 自动编译调用，不需要手工使用。
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut out: Option<PathBuf> = None;

    // 1) 编译器形态：`-o PATH`（gcc/clang）或 `-FoPATH`（cl.exe）
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "-o" || a == "-out" {
            if i + 1 < args.len() {
                out = Some(PathBuf::from(&args[i + 1]));
            }
        } else if let Some(rest) = a.strip_prefix("-Fo") {
            if !rest.is_empty() {
                out = Some(PathBuf::from(rest));
            }
        }
        i += 1;
    }

    // 2) 归档器形态：`ar crs libfoo.a obj.o` —— 输出是首个非选项的归档名
    if out.is_none() {
        for a in &args {
            if a.starts_with('-') {
                continue;
            }
            if a.ends_with(".a") || a.ends_with(".lib") || a.ends_with(".rlib") {
                out = Some(PathBuf::from(a));
                break;
            }
        }
    }

    if let Some(p) = out {
        if let Some(dir) = p.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let _ = fs::write(&p, b"");
    }

    // 永远成功 —— 这是本工具存在的唯一目的。
    std::process::exit(0);
}
