//! 跨平台抽象层：Trae 客户端探测、进程控制、以及平台能力声明。
//!
//! ## 设计原则
//!
//! 参考实现是纯 Windows：注册表查安装路径、`tasklist`/`taskkill` 管进程、
//! `certutil` 装证书、`schtasks` 注册计划任务、注册表改系统代理与 `MachineGuid`。
//! 本模块把这些能力收敛成**一组带明确成败语义的函数**，并让每个平台给出自己的实现：
//!
//! - 三平台都能做的（安装探测、进程检测/终止、客户端启动、userData 目录定位）
//!   → 各写一份等价实现；
//! - 只有特定平台能做的（注册表 `MachineGuid`、`certutil`）→ 返回
//!   [`Unsupported`]，**带上「在哪支持、为什么这里不行」的说明**。
//!
//! 绝不用一个跨平台的「假实现」来冒充成功：例如把「重置 MachineGuid」在 macOS 上
//! 实现成空操作，会让用户以为设备已隔离，而实际上游仍能关联到原设备。
//!
//! ## 为什么 Trae 客户端目录名要枚举多个候选
//!
//! Trae 有**多条可以同机并存的产品线**：`TRAE SOLO CN`、`TRAE SOLO`、`Trae CN`、
//! `Trae`。它们各有独立的安装目录、userData 目录与进程名 —— 实测同一台机器上
//! `D:\Programs\TRAE SOLO CN\` 与 `D:\Programs\Trae CN\` 就各装了一份。
//! 写死一个名字会导致两类错误：
//!
//! - 「明明装了却提示未安装」（候选表没覆盖到实际的名字或安装位置）；
//! - 「在跑却判定为未运行」（进程名只认一个），进而让切换流程在客户端运行时
//!   照常去改登录态，而客户端退出时又会把改动写回覆盖掉。
//!
//! 因此统一走候选列表 + 存在性判定；多个候选同时存在时按**最近活跃**选
//! （见 [`data_dir_names_by_activity`]），并在返回的结构里带上实际命中的路径，
//! 便于用户核对。

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{json, Value};

use crate::modules::trae::paths;
use crate::modules::trae::store;

/// 平台受限能力：说明「哪个能力、在哪些平台可用、当前平台为什么不行」。
///
/// 字段刻意带上 `supported_on` 与 `reason`：前端可直接渲染成一条可操作的提示，
/// 而不是让用户面对「不支持」三个字去猜。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    /// 能力标识，例 `machine_guid_reset`。
    pub capability: &'static str,
    /// 能力的人类可读名称。
    pub label: &'static str,
    /// 该能力可用的平台。
    pub supported_on: &'static str,
    /// 当前平台不可用的具体原因。
    pub reason: String,
}

impl Unsupported {
    /// 构造一条受限能力说明。
    pub fn new(
        capability: &'static str,
        label: &'static str,
        supported_on: &'static str,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            capability,
            label,
            supported_on,
            reason: reason.into(),
        }
    }

    /// 线上形态（camelCase）。
    pub fn to_json(&self) -> Value {
        json!({
            "capability": self.capability,
            "label": self.label,
            "supportedOn": self.supported_on,
            "reason": self.reason,
        })
    }
}

/// 当前平台标识。
pub fn platform_tag() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unknown"
    }
}

/// 带 `CREATE_NO_WINDOW` 的 `Command`（非 Windows 上原样返回）。
///
/// Windows 上不设该标志会让每次子进程调用都闪一个控制台窗口，
/// 对一个常驻托盘的应用而言是不可接受的干扰。
pub(crate) fn hidden_command(program: &str) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
}

/// Trae 客户端可执行文件名候选（按优先级，**跨全部变体**）。
///
/// `Trae CN` 是与 `TRAE SOLO CN` **并列的另一条产品线**，不是它的旧名 ——
/// 两者可以同机并存（实测：`D:\Programs\TRAE SOLO CN\` 与 `D:\Programs\Trae CN\`
/// 各一份，`%APPDATA%` 下也各有一份 userData）。漏掉它就会出现
/// 「明明装了却提示未安装」，以及「装的是 Trae CN、切换的却是 SOLO CN 的登录态」。
///
/// ## 值来自变体表，顺序仍按「Trae Work 优先」
///
/// 具体名字由 [`super::variant`] 的 [`VariantSpec::exe_names`] 提供，
/// 本函数只是把它们**按变体顺序摊平**成一个静态切片 —— 顺序与改造前的
/// 硬编码列表逐字相同（`TRAE SOLO CN` → `TRAE SOLO` → `Trae CN` → `Trae`），
/// 保证「一个候选都不存在」时的兜底选择不变。
///
/// [`VariantSpec::exe_names`]: super::variant::VariantSpec::exe_names
#[cfg(windows)]
const EXE_NAMES: &[&str] = &[
    "TRAE SOLO CN.exe",
    "TRAE SOLO.exe",
    "Trae CN.exe",
    "Trae.exe",
];

/// 非 Windows：可执行文件名不带 `.exe`（macOS 是 bundle 内的裸二进制名）。
#[cfg(not(windows))]
const EXE_NAMES: &[&str] = &["TRAE SOLO CN", "TRAE SOLO", "Trae CN", "Trae"];

fn exe_names() -> &'static [&'static str] {
    EXE_NAMES
}

/// userData 目录名候选（与 [`exe_names`] 一一对应，首项为兜底主候选）。
///
/// 同 [`exe_names`]：值来自变体表，这里只做摊平。
fn data_dir_names() -> &'static [&'static str] {
    &["TRAE SOLO CN", "TRAE SOLO", "Trae CN", "Trae"]
}

/// 取某个变体的可执行文件名候选（Windows 带 `.exe`）。
///
/// 与 [`exe_names`] 的区别：这是**单变体**视角，用于「已知目标产品线」的场景
/// （例如用户显式指定了要走 Trae Work）。[`exe_names`] 是跨变体视角，
/// 用于自动探测（不知道目标是谁，只能都试）。
pub fn exe_names_for(variant: super::variant::TraeVariant) -> &'static [&'static str] {
    let spec = super::variant::variant_spec(variant);
    if cfg!(target_os = "windows") {
        spec.exe_names
    } else {
        // 非 Windows 去掉 `.exe` 后缀：`&'static str` 无法在运行期裁掉后缀，
        // 因此这里用一个与表一一对应的静态切片。新增变体时要同步。
        match variant {
            super::variant::TraeVariant::TraeWork => &["TRAE SOLO CN", "TRAE SOLO"],
            super::variant::TraeVariant::TraeCn => &["Trae CN", "Trae"],
        }
    }
}

/// 求**[单项变体]**的 userData 目录名候选。
pub fn data_dir_names_for(variant: super::variant::TraeVariant) -> &'static [&'static str] {
    super::variant::variant_spec(variant).data_dir_names
}

/// 在**单个变体**的候选里探测该产品线的安装（不跨变体）。
///
/// 与 [`detect_install`] 的区别：后者接受一个自定义路径并横跨全部变体按最近活跃挑，
/// 只适合回答"本机装的是哪个 Trae"。本函数回答的是"**这一条**产品线装了吗"，
/// 因为界面上要**并排**显示两条产品线的独立状态（对齐 WorkBuddy 的三个图标）。
///
/// `custom` 仅在它确实属于该变体时才生效 —— 用户在设置里指定的路径
/// 不应该让"另一条产品线"也显示成已安装。
pub fn detect_install_for(variant: super::variant::TraeVariant) -> InstallProbe {
    let settings = crate::modules::trae::settings::load();
    if let Some(custom) = settings
        .trae_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        let path = PathBuf::from(custom);
        // 只有自定义路径确实指向该变体时才采纳，否则它会让两条产品线都显示"已安装"。
        let belongs = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(super::variant::variant_of_name)
            == Some(variant);
        if belongs && path.is_file() {
            return InstallProbe {
                installed: true,
                version: version_from_exe(&path),
                exe: Some(path),
            };
        }
    }

    for root in windows_install_roots() {
        for name in data_dir_names_for(variant) {
            for exe in exe_names_for(variant) {
                let candidate = root.join(name).join(exe);
                if candidate.is_file() {
                    return InstallProbe {
                        installed: true,
                        version: version_from_exe(&candidate),
                        exe: Some(candidate),
                    };
                }
            }
        }
    }

    InstallProbe {
        installed: false,
        exe: None,
        version: None,
    }
}

/// 某一个变体的客户端是否正在运行（只读探测）。
///
/// 与 [`is_running`] 的区别：后者只回答"有没有 Trae 在跑"（任一产品线），
/// 适合"切换前必须关掉客户端"这类**阻断性**判断；本函数回答的是
/// "**这一条**产品线在跑吗"，因为界面上要给两条产品线各自的运行指示灯。
///
/// 判定只比对**该变体**的进程名，因此两条产品线可以各自显示运行状态。
pub fn is_running_for(variant: super::variant::TraeVariant) -> bool {
    #[cfg(windows)]
    {
        match hidden_command("tasklist").args(["/NH"]).output() {
            Ok(output) => {
                let listing = String::from_utf8_lossy(&output.stdout).to_lowercase();
                exe_names_for(variant)
                    .iter()
                    .any(|exe| listing.contains(&exe.to_lowercase()))
            }
            Err(_) => false,
        }
    }
    #[cfg(not(windows))]
    {
        for name in exe_names_for(variant) {
            let ok = hidden_command("pgrep")
                .args(["-x", name])
                .output()
                .map(|output| output.status.success() && !output.stdout.is_empty())
                .unwrap_or(false);
            if ok {
                return true;
            }
        }
        false
    }
}

/// **全部变体**的环境状态（线上形态，数组）。
///
/// ## 为什么需要它（与 [`env_status`] 的分工）
///
/// [`env_status`] 是**单一视角**：它返回「自动探测挑中的那一条产品线」，
/// 适合"当前在操作哪条线"的页面。但界面上要**并排**显示两条产品线的独立状态
/// （对齐 WorkBuddy 右上角的三个图标 —— 每个图标是一个独立实体、各自有状态），
/// 这时单一视角就不够了：它只会告诉你挑中的那一条，另一条压根不出现。
///
/// 本函数一次返回**全部变体**，每条都带自己的安装/运行/数据目录/版本，
/// 前端直接遍历渲染即可。**不做「最近活跃」筛选** —— 那正是要避免的语义。
///
/// `installed` 与 `dataDirExists` 分开返回：装了但从未登录过的客户端
/// 有其安装目录、却没有 userData 目录，这两种状态在界面上要区别对待。
pub fn variants_status() -> Value {
    let items: Vec<Value> = super::variant::all_specs()
        .iter()
        .map(|spec| {
            let variant = spec.variant;
            let probe = detect_install_for(variant);
            let data_dir = select_data_dir_for(variant);
            json!({
                "variant": variant.as_str(),
                "variantLabel": variant.display_name(),
                // 官方别名（product.json 的 nameAlias），用于诊断文案与核对。
                "nameAlias": spec.name_alias,
                "installed": probe.installed,
                "running": is_running_for(variant),
                "version": probe.version,
                "path": probe.exe.as_ref().map(|p| p.to_string_lossy().to_string()),
                "dataDir": data_dir.as_ref().map(|p| p.to_string_lossy().to_string()),
                "dataDirExists": data_dir.map(|p| p.is_dir()).unwrap_or(false),
            })
        })
        .collect();

    json!({
        "platform": platform_tag(),
        "variants": items,
    })
}

/// 变体表与 [`exe_names`] / [`data_dir_names`] 的摊平结果是否一致。
///
/// 这是防「表改了但摊平常量忘了同步」的护栏：两处写着同一份名字，
/// 一旦漂移就会出现「某个变体的 exe 永远探测不到」这类静默故障。
/// 编译期无法校验（`cfg` 分支 + 静态切片），故放一条测试。
#[cfg(test)]
#[test]
fn flattened_candidates_match_variant_table() {
    let mut from_table_exe: Vec<&str> = Vec::new();
    let mut from_table_dir: Vec<&str> = Vec::new();
    for variant in super::variant::TraeVariant::all() {
        from_table_dir.extend(super::variant::variant_spec(variant).data_dir_names.iter().copied());
    }
    assert_eq!(data_dir_names(), from_table_dir.as_slice(), "userData 目录名候选与变体表漂移");
    // exe 名只在 Windows 上与表逐字相同（非 Windows 去过 `.exe`）。
    if cfg!(target_os = "windows") {
        for variant in super::variant::TraeVariant::all() {
            from_table_exe.extend(super::variant::variant_spec(variant).exe_names.iter().copied());
        }
        assert_eq!(exe_names(), from_table_exe.as_slice(), "exe 名候选与变体表漂移");
    }
}

/// Windows 安装目录候选。
#[cfg(windows)]
fn windows_install_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        roots.push(PathBuf::from(&local).join("Programs"));
    }
    for key in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Ok(dir) = std::env::var(key) {
            roots.push(PathBuf::from(dir));
        }
    }
    roots.extend(secondary_drive_roots());
    roots
}

/// 非系统盘上的 `<盘符>:\Programs`。
///
/// 上面那几个环境变量**只覆盖系统盘**，而 Trae 安装器允许自选目录，装到
/// `D:\Programs\...` 是很常见的选择（实测本机就是 `D:\Programs\TRAE SOLO CN\`）。
/// 此时自动探测会一无所获、界面显示「未安装」，而用户几乎不可能想到
/// 「原因是它装在 D 盘」。
///
/// 先对 `<盘符>:\Programs` 做一次 `is_dir()` 再展开：不存在的路径判定很快，
/// 而展开后每个候选都要 `is_file()`，先过滤能把探测次数从
/// 「盘符数 × 名字数 × exe 数」压到只剩真实存在的那些。`C:` 不重复探测
/// （`%LOCALAPPDATA%\Programs` 与 `ProgramFiles` 已覆盖）。
///
/// 已知代价：盘符里若挂着**已断开的网络驱动器**，`is_dir()` 可能要等超时。
/// 这里接受该代价 —— 探测只发生在「环境状态」查询上，不是热路径。
#[cfg(windows)]
fn secondary_drive_roots() -> Vec<PathBuf> {
    (b'D'..=b'Z')
        .map(|letter| PathBuf::from(format!("{}:\\Programs", letter as char)))
        .filter(|dir| dir.is_dir())
        .collect()
}

/// 从「可执行文件所在目录」推导版本号。
///
/// Trae 是 VS Code 系 Electron 应用，`resources/app/package.json` 里带版本号。
/// 读文件比 spawn 一个 `powershell` 取 `VersionInfo` 更快、无窗口闪烁、且三平台通用。
fn version_from_install_dir(dir: &Path) -> Option<String> {
    // 候选清单按「元素个数」成对给出（Windows 布局 3 段，macOS bundle 4 段），
    // 统一转成 `&[&str]` 切片遍历，避免数组字面量长度必须一致的约束。
    let relative_candidates: &[&[&str]] = &[
        &["resources", "app", "package.json"],
        &["Contents", "Resources", "app", "package.json"],
        &["resources", "app", "product.json"],
    ];
    for relative in relative_candidates {
        let path = relative.iter().fold(dir.to_path_buf(), |acc, part| acc.join(part));
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if let Some(version) = value.get("version").and_then(|v| v.as_str()) {
                    let version = version.trim();
                    if !version.is_empty() && version != "0.0.0" {
                        return Some(version.to_string());
                    }
                }
            }
        }
    }
    // macOS bundle：回落到 Info.plist
    let plist = dir.join("Contents").join("Info.plist");
    if plist.is_file() {
        return crate::modules::identity::read_bundle_version(&plist);
    }
    None
}

/// 从**可执行文件路径**推导版本号。
///
/// 单独包一层，是因为 [`version_from_install_dir`] 收的是**目录**，而
/// [`detect_install`] 手上只有 exe 的完整路径。曾经直接把 exe 路径当目录传进去，
/// 于是拼出 `...\TRAE SOLO CN.exe\resources\app\package.json` 这种不可能存在的路径
/// （Windows 报 `Not a directory`），**版本号因此永远是 `None`** ——
/// 而 `None` 在界面上只表现为「不显示版本」，几乎不会有人当成 bug 报上来。
fn version_from_exe(exe: &Path) -> Option<String> {
    exe.parent().and_then(version_from_install_dir)
}

/// 探测结果。
#[derive(Debug, Clone)]
pub struct InstallProbe {
    /// 是否已安装。
    pub installed: bool,
    /// 可执行文件路径。
    pub exe: Option<PathBuf>,
    /// 版本号。
    pub version: Option<String>,
}

/// 在给定候选路径中探测 Trae 客户端。
///
/// `custom` 为用户在设置里显式指定的路径，**优先级最高**。
///
/// 自动探测（[`candidate_exe_paths`]）覆盖 `%LOCALAPPDATA%\Programs`、三个
/// `ProgramFiles` 变体，以及非系统盘上的 `<盘符>:\Programs` —— 最后一项是必需的：
/// Trae 安装器允许自选目录，装到 `D:\Programs\...` 很常见，而那几个环境变量
/// 只覆盖系统盘，漏掉就会表现为「明明装了却提示未安装」。
///
/// 仍然覆盖不到的只剩「装在任意自定义目录」（例如 `D:\Apps\Trae\`），
/// 该场景由 `custom` 兜底，且报错文案会主动提示这一点。
pub fn detect_install(custom: Option<&str>) -> InstallProbe {
    if let Some(custom) = custom.map(str::trim).filter(|path| !path.is_empty()) {
        let path = PathBuf::from(custom);
        if path.is_file() {
            return InstallProbe {
                installed: true,
                version: version_from_exe(&path),
                exe: Some(path),
            };
        }
    }

    for candidate in candidate_exe_paths() {
        if candidate.is_file() {
            return InstallProbe {
                installed: true,
                version: version_from_exe(&candidate),
                exe: Some(candidate),
            };
        }
    }

    // 刻意**不查询注册表卸载项**：
    //
    // 1. 参考实现用 `reg query /s /f TRAE` 兜底，代价是每次探测都起一个子进程
    //    （Windows 上还会有控制台窗口闪烁），且在受限环境（沙箱、企业策略）里会被拦截；
    // 2. [`candidate_exe_paths`] 已覆盖全部**常见**安装位置（`%LOCALAPPDATA%\Programs`、
    //    `ProgramFiles` / `ProgramFiles(x86)` / `ProgramW6432`，以及非系统盘的
    //    `<盘符>:\Programs`）；
    // 3. 真正需要注册表兜底的场景只剩「装在任意自定义目录」（例如 `D:\Apps\Trae\`），
    //    而该场景已有**更好的**手段：用户在设置里显式指定 `trae_path`
    //    （见本函数开头的 `custom` 分支），且报错文案会主动提示这一点。
    //
    // 换句话说，注册表兜底换来的是「极少数用户少点一次设置」，代价是
    // 「所有用户每次探测都付一次子进程」。这里选择不付。
    InstallProbe {
        installed: false,
        exe: None,
        version: None,
    }
}

/// 生成候选可执行文件路径（跨平台）。
///
/// **候选名放外层循环**：名字已按「最近活跃」排序（见 [`data_dir_names_by_activity`]），
/// 放外层才能让「活跃产品」的安装路径整体优先于「非活跃产品」的。
/// 若把安装根目录放外层，一个装在 C 盘的非活跃产品会盖过装在 D 盘的活跃产品，
/// 于是出现「切换器管的是我没在用的那个 Trae」。
fn candidate_exe_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();

    #[cfg(windows)]
    {
        let names = data_dir_names_by_activity();
        // 提到循环外：`windows_install_roots` 会做一轮盘符探测，不必每个名字都重算。
        let roots = windows_install_roots();
        for name in &names {
            for root in &roots {
                for exe in exe_names() {
                    out.push(root.join(name).join(exe));
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let names = data_dir_names_by_activity();
        for root in ["/Applications", "/System/Applications"] {
            for name in &names {
                for exe in exe_names() {
                    out.push(
                        PathBuf::from(root)
                            .join(format!("{name}.app"))
                            .join("Contents")
                            .join("MacOS")
                            .join(exe),
                    );
                }
            }
        }
        if let Some(home) = dirs::home_dir() {
            for name in &names {
                for exe in exe_names() {
                    out.push(
                        home.join("Applications")
                            .join(format!("{name}.app"))
                            .join("Contents")
                            .join("MacOS")
                            .join(exe),
                    );
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let names = data_dir_names_by_activity();
        for root in ["/opt", "/usr/local", "/usr/share"] {
            for name in &names {
                for exe in exe_names() {
                    out.push(PathBuf::from(root).join(name).join(exe));
                }
            }
        }
        if let Some(home) = dirs::home_dir() {
            for name in &names {
                for exe in exe_names() {
                    out.push(home.join(".local").join("share").join(name).join(exe));
                }
            }
        }
    }

    out
}

/// 定位 Trae 客户端的 userData 目录（登录态所在处；默认变体，兼容壳）。
///
/// 多个渠道的 userData **可以同时存在**（实测同一台机器上 `TRAE SOLO CN` 与
/// `Trae CN` 各有一份）。此时按**最近活跃**选，而不是按候选表顺序取第一个 ——
/// 否则用户在用的是 Trae CN，切换器却去改 SOLO CN 的登录态，且界面上看不出任何异常。
///
/// 全部不存在时返回主候选（用于 UI 展示「预期位置」），而不是 `None`——
/// 否则用户看不到「应该把客户端数据放在哪」。
///
/// **注意**：本函数的候选列表横跨全部变体，只适合"环境自检"。
/// 要按产品线取目录必须用 [`select_data_dir_for`] 或 [`detect_data_dir_for`]。
pub fn detect_data_dir() -> Option<PathBuf> {
    let base = data_dir_base()?;
    Some(base.join(data_dir_names_by_activity()[0]))
}

/// 定位**指定变体**的 userData 目录（登录态所在处）。
///
/// 与 [`detect_data_dir`] 的区别不是"多一个参数"，而是**语义**：
/// 后者横跨全部候选按最近活跃挑，回答「本机最常用的那条产品线」；
/// 本函数只在该变体的候选里挑，回答「这条产品线的客户端数据在哪」。
///
/// 候选**一个都不存在**时**不返回 `None`**，而是回落到该变体的主候选名 ——
/// 保持 [`detect_data_dir`] 的既有契约（UI 要能展示"应该放在哪"）。
/// 需要「该变体到底有没有装」时用 [`select_data_dir_for`]（那个会返回 `None`）。
pub fn detect_data_dir_for(variant: super::variant::TraeVariant) -> Option<PathBuf> {
    let base = data_dir_base()?;
    let names = data_dir_names_for(variant);
    Some(base.join(names[0]))
}

/// 在**单个变体**的候选目录里定位 userData 目录（登录态所在处）。
///
/// 与 [`detect_data_dir`] 的关键区别：后者的候选列表**横跨全部变体**，会按最近活跃
/// 挑出「本机最常用的那一条产品线」——当用户明确要在 Trae Work 分区操作时，这个
/// 全局视角并不成立（实测本机 `Trae CN` 更活跃，于是「在 Trae Work 分区导入」会
/// 去读 `Trae CN` 的目录、并把报错也说成 Trae CN）。
///
/// 本函数把候选**限定在传入变体之内**：
///
/// - 该变体有任何存在的候选目录时，返回其中**最近活跃**的那个（复用
///   [`data_dir_activity`]，与全局视角同一套判定）；
/// - 该变体一个候选目录都不存在时返回 `None` —— 调用方据此产出「未找到【Trae Work】的
///   数据目录，请先启动一次该客户端」这类**指向该变体**的错误，
///   而不是含糊的「未检测到」。
///
/// 返回 `None` 不区分「base 取不到」与「候选都不存在」：两者对调用方的处置相同
/// （都提示该变体没有数据目录），且后者是唯一可操作的情形。
pub fn select_data_dir_for(variant: super::variant::TraeVariant) -> Option<PathBuf> {
    let base = data_dir_base()?;
    let mut scored: Vec<(PathBuf, Option<SystemTime>)> = data_dir_names_for(variant)
        .iter()
        .map(|name| {
            let dir = base.join(name);
            let activity = dir.is_dir().then(|| data_dir_activity(&dir)).flatten();
            (dir, activity)
        })
        .collect();

    // 降序：`Option` 的 `Ord` 里 `None < Some(_)`，反转比较即「有活跃时间的在前、
    // 新的在前、不存在的垫底」。稳定排序保证同分时保持候选表原顺序。
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored
        .into_iter()
        .find_map(|(dir, activity)| activity.map(|_| dir))
}

/// 按「最近活跃」排序的 userData 候选名。
///
/// 排序规则：**存在且活跃时间新**的排前面；目录不存在的排在后面，且彼此保持候选表
/// 原顺序（`sort_by` 是稳定排序）。因此当没有任何候选存在时，结果与
/// [`data_dir_names`] 完全一致 —— 兜底返回值仍然是主候选 `TRAE SOLO CN`。
fn data_dir_names_by_activity() -> Vec<&'static str> {
    let base = data_dir_base();
    let mut scored: Vec<(&'static str, Option<SystemTime>)> = data_dir_names()
        .iter()
        .map(|name| {
            let activity = base
                .as_ref()
                .map(|base| base.join(name))
                .filter(|dir| dir.is_dir())
                .and_then(|dir| data_dir_activity(&dir));
            (*name, activity)
        })
        .collect();

    // 降序：`Option` 的 `Ord` 里 `None < Some(_)`，所以反过来比就是
    // 「有活跃时间的在前、新的在前、没有的垫底」。
    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored.into_iter().map(|(name, _)| name).collect()
}

/// userData 目录的「最近活跃时间」。
///
/// **刻意不直接用目录自身的 mtime**：Electron 的 userData 目录只在增删顶层条目时
/// 更新 mtime，而登录态文件是「内容一变就写」，后者才真正反映「这个客户端最近被用过」。
/// 取一组启动/登录必写文件里最新的那个 mtime；一个都没有时回落到目录 mtime；
/// 都拿不到则返回 `None`（该候选按「从未使用」处理）。
fn data_dir_activity(dir: &Path) -> Option<SystemTime> {
    // 每次启动或登录都会被写到的文件，跨渠道/版本都比较稳定。
    const MARKERS: &[&str] = &[
        "User/globalStorage/storage.json",
        "aha/TinyStorage",
        "Local Storage",
        "Network/Cookies",
        "machineid",
    ];

    MARKERS
        .iter()
        .filter_map(|marker| std::fs::metadata(dir.join(marker)).ok())
        .filter_map(|meta| meta.modified().ok())
        .max()
        .or_else(|| {
            std::fs::metadata(dir)
                .ok()
                .and_then(|meta| meta.modified().ok())
        })
}

/// userData 的父目录（平台约定）。
fn data_dir_base() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        // 优先环境变量；缺失时用 `dirs::config_dir()`（Windows 上就是 `%APPDATA%`，
        // 即 `C:\Users\<用户>\AppData\Roaming`）。
        //
        // 这层兜底不多余：`APPDATA` 属于「通常有、但不保证有」的变量，
        // 某些终端、CI、被裁剪过的启动环境里就是没有。缺了它 `detect_data_dir()`
        // 会直接返回 `None`，表现成「登录态快照与账号切换整体不可用」，
        // 而根因仅仅是取不到一个目录 —— 排查成本极高。
        std::env::var("APPDATA")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .map(PathBuf::from)
            .or_else(dirs::config_dir)
    }
    #[cfg(target_os = "macos")]
    {
        dirs::home_dir().map(|home| home.join("Library").join("Application Support"))
    }
    #[cfg(target_os = "linux")]
    {
        // XDG 规范优先，其次 ~/.config
        std::env::var("XDG_CONFIG_HOME")
            .ok()
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

// 这里刻意**没有**「Trae 客户端进程名」这样的单值函数。
// `TRAE SOLO CN` 与 `Trae CN` 是可以并存的两条产品线，进程名、安装目录、
// userData 目录各有一套，因此一律以 [`exe_names`] 的候选列表为准。

/// 当前正在运行的 Trae 候选进程（Windows）。
///
/// 只调一次 `tasklist` 拿到全量列表再逐个名字比对，而不是「每个候选名各调一次
/// `tasklist /FI IMAGENAME eq ...`」—— 后者在装了多个渠道时要起 4 个子进程，
/// 这里只起 1 个。
#[cfg(windows)]
fn running_processes() -> Vec<&'static str> {
    match hidden_command("tasklist").args(["/NH"]).output() {
        Ok(output) => {
            let listing = String::from_utf8_lossy(&output.stdout).to_lowercase();
            exe_names()
                .iter()
                .copied()
                .filter(|exe| listing.contains(&exe.to_lowercase()))
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

/// 当前正在运行的**指定变体**候选进程（Windows）。
///
/// 与 [`running_processes`] 的区别：只比对传入变体的进程名。用于按变体结束客户端
/// ——若用全局版本，`kill_client` 会把另一条产品线的客户端一起杀掉。
#[cfg(windows)]
fn running_processes_for(variant: super::variant::TraeVariant) -> Vec<&'static str> {
    match hidden_command("tasklist").args(["/NH"]).output() {
        Ok(output) => {
            let listing = String::from_utf8_lossy(&output.stdout).to_lowercase();
            exe_names_for(variant)
                .iter()
                .copied()
                .filter(|exe| listing.contains(&exe.to_lowercase()))
                .collect()
        }
        Err(_) => Vec::new(),
    }
}

/// 客户端是否正在运行。
///
/// Windows 走 `tasklist`，其余平台走 `pgrep -f`。两者都只做**只读**探测，
/// 失败一律按「未运行」处理——代价是可能让切换流程在客户端仍在运行时继续，
/// 而该情形已由切换前置检查兜底（见 [`crate::modules::trae::profile`]）。
///
/// **必须遍历全部候选名**：只认 `TRAE SOLO CN` 会把「Trae CN 正在跑」判成未运行，
/// 于是切换流程照常去改登录态，而客户端退出时又会把改动写回覆盖掉。
pub fn is_running() -> bool {
    #[cfg(windows)]
    {
        !running_processes().is_empty()
    }
    #[cfg(not(windows))]
    {
        // 用可执行文件基名匹配，避免 pid 被别的进程复用；`-x` 要求精确进程名。
        for name in exe_names() {
            let ok = hidden_command("pgrep")
                .args(["-x", name])
                .output()
                .map(|output| output.status.success() && !output.stdout.is_empty())
                .unwrap_or(false);
            if ok {
                return true;
            }
        }
        false
    }
}

/// 结束 Trae 客户端进程。
///
/// 返回 `Ok(true)` 表示确实终止了进程，`Ok(false)` 表示本来就没运行。
///
/// 为什么必须「先杀再启」：Trae 是 Electron 单实例应用，已运行的实例会忽略新的
/// 启动参数（包括 `--proxy-server`）。不先结束旧进程，注入代理后用户看到的是
/// 「窗口被聚焦但完全不走代理」，而日志显示启动成功——极难排查。
pub fn kill_client() -> Result<bool, String> {
    kill_client_for(super::variant::TraeVariant::default())
}

/// 结束**指定变体**的 Trae 客户端进程。
///
/// 为什么需要按变体：两条产品线可以同机并存，`taskkill /IM <exe>` 只能针对具体进程名。
/// 若用 [`kill_client`]（横跨全部候选名），切换 Trae Work 会把正在用的 Trae CN 一起杀掉。
///
/// 返回 `Ok(true)` 表示确实终止了进程，`Ok(false)` 表示本来就没运行。
pub fn kill_client_for(variant: super::variant::TraeVariant) -> Result<bool, String> {
    if !is_running_for(variant) {
        return Ok(false);
    }
    #[cfg(windows)]
    {
        // 只对「确实在跑」的候选名下手，不为每个候选名都白起一次 taskkill。
        let targets = running_processes_for(variant);
        if targets.is_empty() {
            return Ok(false);
        }
        let mut killed = false;
        let mut last_error = String::new();
        for exe in targets {
            let output = hidden_command("taskkill")
                .args(["/F", "/IM", exe])
                .output()
                .map_err(|e| format!("结束客户端进程失败: {e}"))?;
            if output.status.success() {
                killed = true;
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                if !stderr.is_empty() {
                    last_error = stderr;
                }
            }
        }
        // taskkill 在「进程刚退出」时会返回非 0，此时按成功处理更贴近事实。
        if killed || !is_running_for(variant) {
            return Ok(true);
        }
        return Err(format!(
            "结束客户端进程失败: {}",
            if last_error.is_empty() {
                "需要手动关闭 Trae".to_string()
            } else {
                last_error
            }
        ));
    }
    #[cfg(not(windows))]
    {
        let mut killed = false;
        for name in exe_names_for(variant) {
            let output = hidden_command("pkill")
                .args(["-x", name])
                .output()
                .map_err(|e| format!("结束客户端进程失败: {e}"))?;
            if output.status.success() {
                killed = true;
            }
        }
        // 给进程一点退出时间，避免紧接着的启动被单实例锁拒绝。
        std::thread::sleep(std::time::Duration::from_millis(600));
        if killed || !is_running_for(variant) {
            Ok(true)
        } else {
            Err("结束客户端进程失败: 需要手动关闭 Trae".into())
        }
    }
}

/// 启动 Trae 客户端。
///
/// `proxy_port` 为 `Some` 时注入 `--proxy-server`，让客户端流量走本地代理
/// ——这是让用户「不必去 Trae 设置里手填代理」的关键，也是唯一可靠的方式：
/// Electron 不会读取系统代理来走 MITM 的自签 CA。
pub fn launch_client(exe: &Path, proxy_port: Option<u16>) -> Result<(), String> {
    launch_client_for(super::variant::TraeVariant::default(), exe, proxy_port)
}

/// 启动**指定变体**的 Trae 客户端。
///
/// `proxy_port` 为 `Some` 时注入 `--proxy-server`；注入前先按变体结束已运行实例
/// （Electron 单实例语义会让新参数被忽略，且用全局 kill 会误杀另一条产品线）。
pub fn launch_client_for(
    variant: super::variant::TraeVariant,
    exe: &Path,
    proxy_port: Option<u16>,
) -> Result<(), String> {
    if !exe.is_file() {
        return Err(format!("找不到可执行文件: {}", exe.display()));
    }
    // 注入代理前必须先结束已运行实例，否则参数不生效（单实例语义）。
    if proxy_port.is_some() {
        kill_client_for(variant)?;
    }
    let mut command = std::process::Command::new(exe);
    if let Some(port) = proxy_port {
        command.arg(format!("--proxy-server=http://127.0.0.1:{port}"));
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("启动 Trae 失败: {e}"))?;
    Ok(())
}

/// 当前平台的能力清单（用于前端的「平台支持」说明面板）。
pub fn capabilities() -> Value {
    let mut unsupported: Vec<Value> = Vec::new();

    if !cfg!(windows) {
        unsupported.push(
            Unsupported::new(
                "machine_guid_reset",
                "注册表 MachineGuid 重置",
                "Windows",
                "MachineGuid 是 Windows 专有的系统级设备标识，其他平台没有等价机制。",
            )
            .to_json(),
        );
        unsupported.push(
            Unsupported::new(
                "system_ca_install",
                "系统根证书安装",
                "Windows / macOS / Linux",
                // 这条在 mac/linux 上是「实现了但机制不同」，此处只在真正缺失时下发。
                "当前平台使用系统自带证书工具安装，若失败需要管理员权限。",
            )
            .to_json(),
        );
    }

    json!({
        "platform": platform_tag(),
        "processControl": true,
        "clientDetection": true,
        "userDataDir": detect_data_dir().map(|dir| dir.to_string_lossy().to_string()),
        "machineGuidReset": cfg!(windows),
        "scheduledTask": cfg!(any(windows, target_os = "macos", target_os = "linux")),
        "unsupported": unsupported,
    })
}

/// Trae 客户端环境状态（线上形态）。
///
/// ## `variant` / `variantLabel`：探测到的是哪条产品线
///
/// 自动探测会**横跨全部变体**挑最近活跃的那一个（见 [`data_dir_names_by_activity`]），
/// 所以界面必须能说出「挑中的是谁」。这两个字段就是那个答案：
///
/// - `variant`：稳定标识（`"trae_work"` / `"trae_cn"`），供程序判定；
/// - `variantLabel`：展示名（`"Trae Work"` / `"Trae CN"`），直接上界面。
///
/// 判定依据是 [`detect_data_dir`] 的目录名（唯一的权威来源 —— 它是 `%APPDATA%\<产品名>`
/// 逐字拼出来的）；目录不存在时退到安装路径的 exe 名。
/// **两者都推不出来时为 `null`**，调用方应省略该标签而不是显示一个猜测值。
pub fn env_status() -> Value {
    let settings = crate::modules::trae::settings::load();
    let probe = detect_install(settings.trae_path.as_deref());
    let data_dir = detect_data_dir();

    let variant = detected_variant();

    json!({
        "installed": probe.installed,
        "running": is_running(),
        "version": probe.version,
        "path": probe.exe.as_ref().map(|p| p.to_string_lossy().to_string()),
        "dataDir": data_dir.as_ref().map(|p| p.to_string_lossy().to_string()),
        "dataDirExists": data_dir.map(|p| p.is_dir()).unwrap_or(false),
        "platform": platform_tag(),
        "configuredPath": settings.trae_path,
        "variant": variant.map(|v| v.as_str()),
        "variantLabel": variant.map(|v| v.display_name()),
    })
}

/// 探测「当前选定的是哪条 Trae 产品线」。
///
/// 判据顺序：**userData 目录名优先**（它由 Rust 按 `<产品名>` 逐字拼出，等于产品名），
/// 再退到 exe 文件名。两边都用 [`super::variant::variant_of_name`] 反查，
/// 因此新增产品线只需改变体表。
///
/// 与 [`env_status`] 共用同一份逻辑：`env_status` 把它透出给前端，
/// 凭据提取失败时的诊断信息也用它 —— **不要在两处各写一份**，否则会出现
/// 「界面显示 Trae CN，报错却说 Trae Work」这类错位。
pub fn detected_variant() -> Option<super::variant::TraeVariant> {
    let settings = crate::modules::trae::settings::load();
    detect_data_dir()
        .as_ref()
        .and_then(|dir| dir.file_name().and_then(|n| n.to_str()))
        .and_then(super::variant::variant_of_name)
        .or_else(|| {
            detect_install(settings.trae_path.as_deref())
                .exe
                .as_ref()
                .and_then(|exe| exe.file_name().and_then(|n| n.to_str()))
                .and_then(super::variant::variant_of_name)
        })
}

/// Trae 模块数据目录（供 UI 展示与「打开目录」）。
pub fn trae_data_dir() -> String {
    paths::trae_dir().to_string_lossy().to_string()
}

// ---------------------------------------------------------------------------
// 6 层设备标识重置
// ---------------------------------------------------------------------------

/// 生成 32 位十六进制机器码（替代 PowerShell 的 `New-Guid` 去连字符形态）。
fn new_machine_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// 生成标准 GUID（带连字符），用于 `telemetry.sqmId` 与注册表 `MachineGuid`。
fn new_guid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 重置 Trae 客户端的设备标识（6 层）。
///
/// **调用前置条件**：当前登录态必须已备份。本操作会改写客户端数据文件，
/// 与 [`crate::modules::trae::profile::switch_account`] 的备份步骤配合使用；
/// 单独调用前请自行确认已保存快照。
///
/// 逐层行为：
///
/// | # | 位置 | 操作 | 平台 |
/// |:--|:---|:---|:---|
/// | 1 | `<userData>/machineid` | 覆写为新的 32 位 hex | 全平台 |
/// | 2 | `storage.json` 的 `telemetry.machineId` / `telemetry.sqmId` | 替换 | 全平台 |
/// | 3 | `storage.json` 的 `aha.device.device_id` | 替换 | 全平台 |
/// | 4 | `<userData>/aha/TinyStorage` 内含 `device_id` 的文件 | 删除 | 全平台 |
/// | 5 | 注册表 `HKLM\SOFTWARE\Microsoft\Cryptography\MachineGuid` | 替换 | **仅 Windows** |
/// | 6 | `Partitions/trae-webview` 的 Network / Local Storage / Session Storage | 删除 | 全平台 |
///
/// 另外删除 `storage.json` 的 `has_device_id_updated_to_aha` 标记位——它会让客户端
/// 认为「设备 ID 已上报过」，从而跳过重新生成的流程，使前几步白做。
///
/// 返回线上形态的重置报告。**任一层失败都不返回 `Err`**，而是记入 `steps` 的
/// `skip` 状态：设备隔离是「尽力而为」的加固措施，个别文件被占用不应该让整次
/// 账号切换失败。
pub fn reset_device_identity() -> Result<Value, String> {
    reset_device_identity_for(super::variant::TraeVariant::default())
}

/// 重置**指定变体**的设备标识（默认变体见 [`reset_device_identity`]）。
///
/// 设备标识存在**该变体客户端的 userData** 里，故必须按变体重置 ——
/// 否则在 Trae CN 分区点"重置设备"，改的是 Trae Work 的目录（或反之），
/// 而界面会显示"成功"，实际什么都没生效。
pub fn reset_device_identity_for(variant: super::variant::TraeVariant) -> Result<Value, String> {
    let data_dir = detect_data_dir_for(variant)
        .filter(|dir| dir.is_dir())
        .ok_or("未找到 Trae 客户端数据目录，无法重置设备标识")?;

    let machine_id = new_machine_id();
    let guid = new_guid();
    let mut steps: Vec<Value> = Vec::new();
    let mut reset_count = 0u32;

    // 1. machineid 文件
    let machineid_file = data_dir.join("machineid");
    if machineid_file.is_file() {
        match std::fs::write(&machineid_file, &machine_id) {
            Ok(()) => {
                reset_count += 1;
                steps.push(json!({"layer": 1, "label": "machineid 文件", "status": "ok"}));
            }
            Err(e) => steps.push(
                json!({"layer": 1, "label": "machineid 文件", "status": "skip", "reason": e.to_string()}),
            ),
        }
    } else {
        steps.push(json!({"layer": 1, "label": "machineid 文件", "status": "skip", "reason": "文件不存在"}));
    }

    // 2 + 3. storage.json（点号扁平键，不是嵌套对象）
    let storage_file = data_dir.join("User").join("globalStorage").join("storage.json");
    if storage_file.is_file() {
        match rewrite_storage_json(&storage_file, &machine_id, &guid) {
            Ok(changed) => {
                if changed {
                    reset_count += 1;
                    steps.push(json!({"layer": 2, "label": "storage.json 遥测与设备标识", "status": "ok"}));
                } else {
                    steps.push(json!({
                        "layer": 2,
                        "label": "storage.json 遥测与设备标识",
                        "status": "skip",
                        "reason": "未找到需要替换的键"
                    }));
                }
            }
            Err(e) => steps.push(
                json!({"layer": 2, "label": "storage.json 遥测与设备标识", "status": "skip", "reason": e}),
            ),
        }
    } else {
        steps.push(json!({
            "layer": 2,
            "label": "storage.json 遥测与设备标识",
            "status": "skip",
            "reason": "storage.json 不存在"
        }));
    }

    // 4. aha/TinyStorage 内含 device_id 的文件
    let tiny_storage = data_dir.join("aha").join("TinyStorage");
    if tiny_storage.is_dir() {
        let removed = purge_files_containing(&tiny_storage, "device_id");
        if removed > 0 {
            reset_count += 1;
        }
        steps.push(json!({
            "layer": 4,
            "label": "aha/TinyStorage device_id",
            "status": if removed > 0 { "ok" } else { "skip" },
            "removed": removed,
        }));
    } else {
        steps.push(json!({
            "layer": 4,
            "label": "aha/TinyStorage device_id",
            "status": "skip",
            "reason": "目录不存在"
        }));
    }

    // 5. 注册表 MachineGuid —— Windows 专有
    #[cfg(windows)]
    {
        match reset_machine_guid(&guid) {
            Ok(()) => {
                reset_count += 1;
                steps.push(json!({"layer": 5, "label": "注册表 MachineGuid", "status": "ok"}));
            }
            Err(e) => steps.push(json!({
                "layer": 5,
                "label": "注册表 MachineGuid",
                "status": "skip",
                "reason": format!("{e}（重置注册表需要管理员权限，不影响账号切换）"),
            })),
        }
    }
    #[cfg(not(windows))]
    {
        // 不加 `supported_on` 之外的任何伪装：明确标注该层在非 Windows 上不存在。
        steps.push(json!({
            "layer": 5,
            "label": "注册表 MachineGuid",
            "status": "unsupported",
            "reason": "MachineGuid 是 Windows 专有的系统级设备标识，当前平台不存在该机制",
        }));
    }

    // 6. trae-webview 追踪数据
    let webview = data_dir.join("Partitions").join("trae-webview");
    if webview.is_dir() {
        let mut cleared = 0u32;
        for name in ["Network", "Local Storage", "Session Storage"] {
            let target = webview.join(name);
            if target.exists() && std::fs::remove_dir_all(&target).is_ok() {
                cleared += 1;
            }
        }
        if cleared > 0 {
            reset_count += 1;
        }
        steps.push(json!({
            "layer": 6,
            "label": "WebView 追踪数据",
            "status": if cleared > 0 { "ok" } else { "skip" },
            "cleared": cleared,
        }));
    } else {
        steps.push(json!({
            "layer": 6,
            "label": "WebView 追踪数据",
            "status": "skip",
            "reason": "目录不存在"
        }));
    }

    store::append_log(
        &paths::app_log_file(),
        &format!("设备标识重置完成: {reset_count} 项生效"),
    );

    Ok(json!({
        "resetCount": reset_count,
        "machineId": machine_id,
        "guid": guid,
        "totalLayers": 6,
        "steps": steps,
    }))
}

/// 改写 `storage.json` 中的遥测与设备标识键。
///
/// `storage.json` 的键是**扁平的点号字符串**（`"telemetry.machineId"`），
/// 而不是嵌套对象。这是 Trae/VS Code 系的存储约定，按嵌套结构去改会静默失败。
///
/// 返回是否真的发生了替换。
fn rewrite_storage_json(
    path: &Path,
    machine_id: &str,
    guid: &str,
) -> Result<bool, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("读取失败: {e}"))?;
    let Ok(mut value) = serde_json::from_str::<Value>(&text) else {
        return Err("不是合法的 JSON，已跳过".into());
    };
    let Some(map) = value.as_object_mut() else {
        return Err("顶层不是 JSON 对象，已跳过".into());
    };

    let mut changed = false;
    for (key, new_value) in [
        ("telemetry.machineId", machine_id),
        ("telemetry.sqmId", guid),
        ("aha.device.device_id", machine_id),
    ] {
        if map.contains_key(key) {
            map.insert(key.to_string(), Value::String(new_value.to_string()));
            changed = true;
        }
    }
    // 标记位：留着它客户端会认为设备 ID 已上报，跳过重新生成。
    if map.remove("has_device_id_updated_to_aha").is_some() {
        changed = true;
    }

    if changed {
        let pretty =
            serde_json::to_string_pretty(&value).map_err(|e| format!("序列化失败: {e}"))?;
        // 先写临时文件再替换，避免客户端同时读写导致 storage.json 半截损坏。
        // 该文件一旦损坏，客户端会丢失全部本地状态（比丢设备标识严重得多）。
        store::atomic_write_text(path, &pretty)?;
    }
    Ok(changed)
}

/// 递归删除内容中包含指定关键词的文本文件，返回删除数量。
fn purge_files_containing(dir: &Path, needle: &str) -> u32 {
    let mut removed = 0u32;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            removed += purge_files_containing(&path, needle);
            continue;
        }
        // 只处理小体积文本文件：把整个 TinyStorage 逐文件读进内存判断关键词，
        // 遇到大二进制文件既慢又无意义。
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > 4 * 1024 * 1024 {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if content.contains(needle) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 重置注册表 `MachineGuid`（仅 Windows；需要管理员权限）。
#[cfg(windows)]
fn reset_machine_guid(guid: &str) -> Result<(), String> {
    let output = hidden_command("reg")
        .args([
            "add",
            "HKLM\\SOFTWARE\\Microsoft\\Cryptography",
            "/v",
            "MachineGuid",
            "/t",
            "REG_SZ",
            "/d",
            guid,
            "/f",
        ])
        .output()
        .map_err(|e| format!("执行 reg 失败: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_tag_is_known_value() {
        assert!(matches!(
            platform_tag(),
            "windows" | "macos" | "linux" | "unknown"
        ));
    }

    #[test]
    fn unsupported_carries_actionable_context() {
        let unsupported = Unsupported::new(
            "machine_guid_reset",
            "注册表 MachineGuid 重置",
            "Windows",
            "仅 Windows 提供该机制",
        );
        let value = unsupported.to_json();
        assert_eq!(
            value.get("capability").unwrap().as_str(),
            Some("machine_guid_reset")
        );
        assert_eq!(value.get("supportedOn").unwrap().as_str(), Some("Windows"));
        assert!(value.get("reason").unwrap().as_str().unwrap().len() > 5);
        // 线上形态必须 camelCase
        assert!(value.get("supported_on").is_none());
    }

    #[test]
    fn exe_names_are_non_empty_and_distinct() {
        let names = exe_names();
        assert!(!names.is_empty());
        let mut sorted = names.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "候选名不能重复");
    }

    #[test]
    fn data_dir_names_include_primary_candidate() {
        // 主候选必须排第一，否则 detect_data_dir 的兜底返回值会变成次要名字。
        assert_eq!(data_dir_names()[0], "TRAE SOLO CN");
    }

    #[test]
    fn detect_install_prefers_custom_path_when_valid() {
        let temp = std::env::temp_dir().join(format!("fake-trae-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp);
        let exe = temp.join(exe_names()[0]);
        std::fs::write(&exe, b"stub").unwrap();

        let probe = detect_install(Some(&exe.to_string_lossy()));
        assert!(probe.installed);
        assert_eq!(probe.exe.as_deref(), Some(exe.as_path()));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn detect_install_ignores_invalid_custom_path() {
        // 自定义路径不存在时不能直接报「已安装」，应继续走自动探测。
        let probe = detect_install(Some("/definitely/not/here/trae"));
        assert!(!probe.installed || probe.exe.is_some());
        if !probe.installed {
            assert!(probe.exe.is_none());
        }
    }

    #[test]
    fn detect_install_blank_custom_path_does_not_error() {
        let probe = detect_install(Some("   "));
        // 空/空白自定义路径应被忽略而不是当成路径 ""
        if let Some(exe) = probe.exe {
            assert!(!exe.to_string_lossy().trim().is_empty());
        }
    }

    #[test]
    fn version_read_from_electron_package_json() {
        let temp = std::env::temp_dir().join(format!("fake-trae-ver-{}", std::process::id()));
        let app = temp.join("resources").join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            app.join("package.json"),
            br#"{"name":"trae","version":"1.2.3"}"#,
        )
        .unwrap();
        assert_eq!(version_from_install_dir(&temp), Some("1.2.3".into()));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn version_ignores_zero_placeholder() {
        // VS Code 系打包常把 version 写成 0.0.0 占位，必须跳过而不是展示 0.0.0。
        let temp = std::env::temp_dir().join(format!("fake-trae-zero-{}", std::process::id()));
        let app = temp.join("resources").join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("package.json"), br#"{"version":"0.0.0"}"#).unwrap();
        assert_eq!(version_from_install_dir(&temp), None);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn version_tolerates_missing_and_broken_manifests() {
        let temp = std::env::temp_dir().join(format!("fake-trae-broken-{}", std::process::id()));
        let app = temp.join("resources").join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("package.json"), b"{ not json").unwrap();
        assert_eq!(version_from_install_dir(&temp), None);
        assert_eq!(version_from_install_dir(Path::new("/nonexistent")), None);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn capabilities_reports_platform_and_process_support() {
        let value = capabilities();
        assert_eq!(
            value.get("platform").unwrap().as_str(),
            Some(platform_tag())
        );
        assert_eq!(value.get("processControl").unwrap().as_bool(), Some(true));
        // machineGuidReset 必须与编译目标一致，不能在非 Windows 上谎报支持。
        assert_eq!(
            value.get("machineGuidReset").unwrap().as_bool(),
            Some(cfg!(windows))
        );
        assert!(value.get("unsupported").unwrap().is_array());
    }

    #[test]
    fn env_status_has_camel_case_wire_fields() {
        let value = env_status();
        for key in [
            "installed",
            "running",
            "version",
            "path",
            "dataDir",
            "dataDirExists",
            "platform",
            "configuredPath",
        ] {
            assert!(value.get(key).is_some(), "缺少线上字段 {key}");
        }
        // 不能泄漏 snake_case 形式
        assert!(value.get("data_dir").is_none());
        assert!(value.get("running").unwrap().is_boolean());
        assert!(value.get("installed").unwrap().is_boolean());
    }

    #[test]
    fn data_dir_base_and_detect_never_panic() {
        // 探测必须对任意环境安全：只验证不 panic 且返回值形态正确。
        let dir = detect_data_dir();
        if let Some(dir) = dir {
            assert!(!dir.to_string_lossy().is_empty());
        }
    }

    /// ★ 逐变体探测必须**互不干扰**：装了 A 不能让 B 也显示"已安装"。
    ///
    /// 这是「两条产品线并排显示各自状态」的正确性基础。若 `detect_install_for`
    /// 退化成跨变体探测，两个图标会一起亮/一起灭，用户看到的信息量为零。
    #[test]
    fn 逐变体安装探测互不干扰() {
        let work = detect_install_for(super::super::variant::TraeVariant::TraeWork);
        let cn = detect_install_for(super::super::variant::TraeVariant::TraeCn);

        // 探到的 exe 必须落在该变体自己的候选目录名下（不能串到另一条产品线）。
        for (variant, probe) in [
            (super::super::variant::TraeVariant::TraeWork, &work),
            (super::super::variant::TraeVariant::TraeCn, &cn),
        ] {
            if let Some(exe) = probe.exe.as_ref() {
                let parent = exe
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                let belongs = data_dir_names_for(variant)
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(parent));
                assert!(
                    belongs,
                    "变体 {variant:?} 探到的 exe 不在自己的目录名下: {exe:?}（父目录 {parent}）"
                );
            }
        }
    }

    /// `variants_status()` 必须返回**全部**变体（不止挑中的那一条），
    /// 且字段与 `env_status` 同形（camelCase）供前端直接消费。
    #[test]
    fn variants_status_returns_every_variant() {
        let value = variants_status();
        let items = value
            .get("variants")
            .and_then(|v| v.as_array())
            .expect("variants 应为数组");

        // 数量必须等于变体总数 —— 少一条就意味着界面上会少一个图标。
        assert_eq!(
            items.len(),
            super::super::variant::all_specs().len(),
            "variants_status 未返回全部变体"
        );

        let mut seen: Vec<&str> = Vec::new();
        for item in items {
            for key in [
                "variant",
                "variantLabel",
                "nameAlias",
                "installed",
                "running",
                "version",
                "path",
                "dataDir",
                "dataDirExists",
            ] {
                assert!(item.get(key).is_some(), "变体项缺少线上字段 {key}");
            }
            assert!(item.get("installed").unwrap().is_boolean());
            assert!(item.get("running").unwrap().is_boolean());
            // 不能泄漏 snake_case 形式。
            assert!(item.get("variant_label").is_none());
            seen.push(item.get("variant").and_then(|v| v.as_str()).unwrap());
        }

        // 两条产品线的标识必须都在，且不重复。
        for expected in ["trae_work", "trae_cn"] {
            assert!(seen.contains(&expected), "缺少变体 {expected}");
        }
        assert_eq!(seen.len(), 2, "变体标识重复: {seen:?}");
    }

    /// 逐变体运行探测：结果只取决于该变体的进程名。
    ///
    /// 不假设本机装了什么（CI 上两条都没装），只验证「不 panic 且互不耦合」，
    /// 以及「任一产品线在跑 ⇒ 全局 `is_running()` 为真」这条蕴含关系。
    #[test]
    fn 逐变体运行探测与全局一致() {
        let work = is_running_for(super::super::variant::TraeVariant::TraeWork);
        let cn = is_running_for(super::super::variant::TraeVariant::TraeCn);

        // 全局判定是逐变体判定的并集：任一为真则全局必须为真。
        if work || cn {
            assert!(
                is_running(),
                "逐变体探测说有客户端在跑，全局 is_running() 却说没有"
            );
        }
    }

    #[test]
    fn is_running_is_side_effect_free() {
        // 只读探测：连续调用结果一致，且不会误报「运行中」为错误。
        let first = is_running();
        let second = is_running();
        assert_eq!(first, second);
    }

    #[test]
    fn machine_id_and_guid_have_expected_shapes() {
        let machine_id = new_machine_id();
        assert_eq!(machine_id.len(), 32);
        assert!(machine_id.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!machine_id.contains('-'), "machineid 文件不带连字符");

        let guid = new_guid();
        assert_eq!(guid.len(), 36);
        assert!(uuid::Uuid::parse_str(&guid).is_ok());
        // 两次调用必须不同，否则「重置」实际没换标识
        assert_ne!(machine_id, new_machine_id());
        assert_ne!(guid, new_guid());
    }

    #[test]
    fn rewrite_storage_json_uses_flat_dot_keys_not_nested() {
        // 关键回归：Trae 的 storage.json 是**扁平点号键**。
        // 若按嵌套对象去改，"telemetry.machineId" 会被静默漏掉（改了个不存在的嵌套路径）。
        let base = std::env::temp_dir().join(format!("trae-storage-flat-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&base);
        let path = base.join("storage.json");
        std::fs::write(
            &path,
            r#"{"telemetry.machineId":"old-machine","telemetry.sqmId":"old-sqm","aha.device.device_id":"old-device","has_device_id_updated_to_aha":true,"other":"keep"}"#,
        )
        .unwrap();

        let changed = rewrite_storage_json(&path, "NEWMACHINE", "NEW-GUID").unwrap();
        assert!(changed);

        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after.get("telemetry.machineId").unwrap().as_str(), Some("NEWMACHINE"));
        assert_eq!(after.get("telemetry.sqmId").unwrap().as_str(), Some("NEW-GUID"));
        assert_eq!(after.get("aha.device.device_id").unwrap().as_str(), Some("NEWMACHINE"));
        // 标记位必须被删除，否则客户端会跳过设备 ID 重新上报
        assert!(after.get("has_device_id_updated_to_aha").is_none());
        // 无关键必须原样保留
        assert_eq!(after.get("other").unwrap().as_str(), Some("keep"));
        // 不能把点号键拆成嵌套对象
        assert!(after.get("telemetry").is_none());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rewrite_storage_json_reports_no_change_when_keys_absent() {
        let base = std::env::temp_dir().join(format!("trae-storage-absent-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&base);
        let path = base.join("storage.json");
        std::fs::write(&path, r#"{"unrelated":1}"#).unwrap();
        assert_eq!(rewrite_storage_json(&path, "m", "g").unwrap(), false);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn rewrite_storage_json_refuses_to_corrupt_broken_files() {
        // 文件不是 JSON 时必须报错且**保持原样**——重写成半截内容会让客户端丢全部本地状态。
        let base = std::env::temp_dir().join(format!("trae-storage-broken-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&base);
        let path = base.join("storage.json");
        let original = "{ not json at all";
        std::fs::write(&path, original).unwrap();

        assert!(rewrite_storage_json(&path, "m", "g").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

        // 顶层是数组而非对象，同样拒绝改写
        std::fs::write(&path, "[1,2,3]").unwrap();
        assert!(rewrite_storage_json(&path, "m", "g").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "[1,2,3]");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn purge_files_containing_removes_only_matching_text_files() {
        let base = std::env::temp_dir().join(format!("trae-purge-{}", std::process::id()));
        let nested = base.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(base.join("has.txt"), "x device_id y").unwrap();
        std::fs::write(base.join("plain.txt"), "nothing here").unwrap();
        std::fs::write(nested.join("deep.txt"), "device_id").unwrap();
        // 二进制文件不能被当作文本读取而误删
        std::fs::write(base.join("blob.bin"), [0xff, 0xfe, 0x00, 0x01]).unwrap();

        let removed = purge_files_containing(&base, "device_id");
        assert_eq!(removed, 2);
        assert!(!base.join("has.txt").exists());
        assert!(base.join("plain.txt").exists());
        assert!(!nested.join("deep.txt").exists());
        assert!(base.join("blob.bin").exists());

        assert_eq!(purge_files_containing(&base.join("nope"), "x"), 0);
        let _ = std::fs::remove_dir_all(&base);
    }

    // ---------- 多渠道（TRAE SOLO CN / Trae CN 并存）相关 ----------

    #[test]
    fn exe_names_cover_the_trae_cn_product_line() {
        // `Trae CN` 与 `TRAE SOLO CN` 是可以并存的两条产品线（实测同机各装一份）。
        // 漏掉它 → 「明明装了却提示未安装」，以及在跑的那个被判成未运行。
        let names = exe_names();
        let suffix = if cfg!(target_os = "windows") { ".exe" } else { "" };
        for expected in ["TRAE SOLO CN", "Trae CN"] {
            let full = format!("{expected}{suffix}");
            assert!(names.contains(&full.as_str()), "候选名缺少 {full}：{names:?}");
        }
    }

    #[test]
    fn data_dir_names_cover_the_trae_cn_product_line() {
        let names = data_dir_names();
        assert!(
            names.contains(&"Trae CN"),
            "userData 候选缺少 Trae CN：{names:?}"
        );
        // 主候选必须仍是 TRAE SOLO CN：它是「一个候选都不存在」时的兜底返回值。
        assert_eq!(names[0], "TRAE SOLO CN");
    }

    #[test]
    fn data_dir_activity_prefers_the_more_recently_written_marker() {
        let base = std::env::temp_dir().join(format!("trae-activity-{}", std::process::id()));
        let old = base.join("old");
        let fresh = base.join("fresh");
        for dir in [&old, &fresh] {
            let storage = dir.join("User").join("globalStorage");
            std::fs::create_dir_all(&storage).unwrap();
            std::fs::write(storage.join("storage.json"), b"{}").unwrap();
        }
        // 隔开一点再写 fresh，确保两者 mtime 明确不同（NTFS 精度远高于 20ms）
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            fresh.join("User").join("globalStorage").join("storage.json"),
            br#"{"touched":1}"#,
        )
        .unwrap();

        let old_activity = data_dir_activity(&old).expect("old 应有活跃时间");
        let fresh_activity = data_dir_activity(&fresh).expect("fresh 应有活跃时间");
        assert!(
            fresh_activity > old_activity,
            "刚写过的目录活跃时间应更晚：fresh={fresh_activity:?} old={old_activity:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn data_dir_activity_falls_back_to_dir_mtime_then_none() {
        let base = std::env::temp_dir().join(format!("trae-activity-fb-{}", std::process::id()));
        let empty = base.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        // 一个 marker 都没有时回落到目录自身 mtime，而不是直接判成「从未使用」
        assert!(data_dir_activity(&empty).is_some());
        // 目录不存在才是 None
        assert!(data_dir_activity(&base.join("missing")).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn data_dir_names_by_activity_is_a_permutation_of_candidates() {
        // 排序只能改变顺序，不能多出或漏掉候选名。
        let mut ordered = data_dir_names_by_activity();
        assert_eq!(ordered.len(), data_dir_names().len());
        ordered.sort_unstable();
        let mut expected = data_dir_names().to_vec();
        expected.sort_unstable();
        assert_eq!(ordered, expected);
    }

    /// `select_data_dir_for` 必须**只在传入变体的候选里**选，绝不跨变体。
    ///
    /// 这是 Bug 1 的底层护栏：即便另一条产品线的目录更活跃，
    /// TraeWork 的请求也只能命中 `TRAE SOLO CN` / `TRAE SOLO`。
    #[cfg(windows)]
    #[test]
    fn select_data_dir_for_stays_within_the_requested_variant() {
        let base = std::env::temp_dir().join(format!("trae-select-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // 两条产品线的目录都存在，且把 Trae CN 造得更活跃。
        for name in ["TRAE SOLO CN", "Trae CN"] {
            let storage = base.join(name).join("User").join("globalStorage");
            std::fs::create_dir_all(&storage).unwrap();
            std::fs::write(storage.join("storage.json"), b"{}").unwrap();
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
        std::fs::write(
            base.join("Trae CN")
                .join("User")
                .join("globalStorage")
                .join("storage.json"),
            br#"{"touched":1}"#,
        )
        .unwrap();

        let _lock = crate::modules::config::env_lock();
        let original = std::env::var("APPDATA").ok();
        std::env::set_var("APPDATA", &base);

        let work = select_data_dir_for(super::super::variant::TraeVariant::TraeWork);
        let cn = select_data_dir_for(super::super::variant::TraeVariant::TraeCn);

        match original {
            Some(value) => std::env::set_var("APPDATA", value),
            None => std::env::remove_var("APPDATA"),
        }
        let _ = std::fs::remove_dir_all(&base);

        let work_name = work
            .as_ref()
            .and_then(|dir| dir.file_name())
            .map(|name| name.to_string_lossy().to_string());
        assert_eq!(
            work_name.as_deref(),
            Some("TRAE SOLO CN"),
            "TraeWork 只能落在 Trae Work 的候选目录里"
        );
        let cn_name = cn
            .as_ref()
            .and_then(|dir| dir.file_name())
            .map(|name| name.to_string_lossy().to_string());
        assert_eq!(cn_name.as_deref(), Some("Trae CN"));
    }

    /// 变体没有任何存在的候选目录时，`select_data_dir_for` 必须返回 `None`
    /// （而不是含糊地退回一个不存在的路径，那会让调用方以为「目录存在」）。
    #[cfg(windows)]
    #[test]
    fn select_data_dir_for_returns_none_when_variant_has_no_dir() {
        let base = std::env::temp_dir().join(format!("trae-select-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let _lock = crate::modules::config::env_lock();
        let original = std::env::var("APPDATA").ok();
        std::env::set_var("APPDATA", &base);

        let work = select_data_dir_for(super::super::variant::TraeVariant::TraeWork);
        let cn = select_data_dir_for(super::super::variant::TraeVariant::TraeCn);

        match original {
            Some(value) => std::env::set_var("APPDATA", value),
            None => std::env::remove_var("APPDATA"),
        }
        let _ = std::fs::remove_dir_all(&base);

        assert!(work.is_none(), "两条候选目录都不存在时必须是 None");
        assert!(cn.is_none(), "两条候选目录都不存在时必须是 None");
    }

    #[cfg(windows)]
    #[test]
    fn secondary_drive_roots_only_return_existing_directories() {
        for root in secondary_drive_roots() {
            assert!(root.is_dir(), "返回了不存在的根目录：{}", root.display());
            let text = root.to_string_lossy().to_uppercase();
            // C 盘交给 %LOCALAPPDATA%\Programs 与 ProgramFiles，不要重复探测
            assert!(
                !text.starts_with("C:"),
                "C 盘不应出现在非系统盘根目录里：{text}"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn running_processes_lists_only_registered_names() {
        // 只读探测，且不得返回候选表以外的名字（否则 taskkill 会打错目标）。
        let running = running_processes();
        eprintln!("[trae] running_processes -> {running:?}");
        for name in &running {
            assert!(
                exe_names().contains(name),
                "返回了未登记的进程名：{name}"
            );
        }
    }

    #[test]
    fn detect_install_returns_a_registered_exe_name_when_installed() {
        // 命中安装时，返回的必须是候选表里的文件名（防止拼出错误路径）。
        // 同时把结果打出来，便于用 `--nocapture` 核对本机真实探测结果。
        let probe = detect_install(None);
        eprintln!(
            "[trae] detect_install -> installed={} exe={:?} version={:?}",
            probe.installed,
            probe.exe.as_ref().map(|p| p.display().to_string()),
            probe.version
        );
        eprintln!(
            "[trae] detect_data_dir -> {:?}",
            detect_data_dir().map(|p| p.display().to_string())
        );
        if let Some(exe) = probe.exe {
            assert!(exe.is_file(), "返回了不存在的可执行文件：{}", exe.display());
            let file = exe.file_name().unwrap().to_string_lossy().to_string();
            assert!(
                exe_names().contains(&file.as_str()),
                "探测到的可执行文件名不在候选表内：{file}"
            );
        }
    }

    #[test]
    fn detect_install_reads_version_from_the_exe_directory() {
        // 回归：detect_install 曾把**可执行文件路径**当目录传给
        // version_from_install_dir，拼出
        // `...\TRAE SOLO CN.exe\resources\app\package.json` 这种不可能存在的路径，
        // 于是版本号永远是 None —— 界面上只表现为「不显示版本」，很难被发现。
        let temp = std::env::temp_dir().join(format!("fake-trae-ver-exe-{}", std::process::id()));
        let app = temp.join("resources").join("app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("package.json"), br#"{"version":"9.9.9"}"#).unwrap();
        let exe = temp.join(exe_names()[0]);
        std::fs::write(&exe, b"stub").unwrap();

        let probe = detect_install(Some(&exe.to_string_lossy()));
        assert!(probe.installed);
        assert_eq!(probe.version.as_deref(), Some("9.9.9"));
        assert_eq!(probe.exe.as_deref(), Some(exe.as_path()));

        let _ = std::fs::remove_dir_all(&temp);
    }
}
