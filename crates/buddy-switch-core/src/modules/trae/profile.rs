//! Trae 登录态快照与切换。
//!
//! ## 与参考实现的关键差异：不再依赖 PowerShell
//!
//! 参考实现用 `trae-switch-bridge.ps1`（30KB）做这件事，原因是它顺手把「6 层设备标识
//! 重置」「注册表 MachineGuid」也塞进了同一个脚本。但快照/恢复本身**只是文件复制**：
//!
//! ```text
//! 备份：<客户端 userData>/<核心文件>  →  ~/.buddy-switch/trae/profiles/<uid>/
//! 恢复：~/.buddy-switch/trae/profiles/<uid>/  →  <客户端 userData>/<核心文件>
//! ```
//!
//! 因此这里用原生 Rust 实现，三平台通用；仅「设备标识重置」里真正平台相关的部分
//! （注册表）留在 [`crate::modules::trae::platform`] 并明示不支持。
//!
//! ## 数据安全约定（不可绕过）
//!
//! 恢复目标账号快照会**覆盖用户当前登录态**。若当前登录态尚未保存，覆盖即永久丢失。
//! 因此切换流程固定为：
//!
//! 1. 保存当前登录态到 `last` 槽位（可回滚的兜底）；
//! 2. 若已知当前账号 uid，再保存一份到该账号自己的槽位；
//! 3. 才执行恢复到目标账号。
//!
//! 第 1 步是**强制的**，不提供开关：它只多占一份快照的空间，却能让任何一次误切换
//! 都可回滚。第 2 步依赖 `current_account.txt` 是否记录过当前账号。
//!
//! ## 快照只复制「核心文件」而非整目录
//!
//! Trae 的 userData 目录包含缓存、日志、扩展、崩溃转储等大量与登录态无关的内容，
//! 全量镜像会让每个快照膨胀到数百 MB 且显著拖慢切换。核心文件清单见 [`CORE_ENTRIES`]。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::modules::trae::paths;
use crate::modules::trae::platform;
use crate::modules::trae::store;
use crate::modules::trae::variant::TraeVariant;

/// 核心文件/目录条目的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// 单个文件。
    File,
    /// 目录（递归复制）。
    Dir,
}

/// 一个核心条目：相对客户端 userData 目录的路径。
#[derive(Debug, Clone, Copy)]
pub struct CoreEntry {
    /// 相对路径（用 `/` 分隔，由 [`CoreEntry::resolve`] 转成本地分隔符）。
    pub relative: &'static str,
    /// 类型。
    pub kind: EntryKind,
    /// 说明（用于 UI 与诊断）。
    pub label: &'static str,
}

/// 登录态核心文件清单。
///
/// 9 类，与参考实现 `Backup-CurrentProfile` 逐条对应。顺序无关紧要，
/// 但**每一条都要保留**：漏掉 `state.vscdb` 会丢令牌、漏掉 `Network/` 会丢 Cookie、
/// 漏掉 `machineid` 会让上游把恢复后的账号识别成新设备。
pub const CORE_ENTRIES: &[CoreEntry] = &[
    CoreEntry {
        relative: "User/globalStorage/storage.json",
        kind: EntryKind::File,
        label: "存储（设备标识/遥测/认证）",
    },
    CoreEntry {
        relative: "User/globalStorage/state.vscdb",
        kind: EntryKind::File,
        label: "令牌数据库",
    },
    CoreEntry {
        relative: "User/globalStorage/state.vscdb.backup",
        kind: EntryKind::File,
        label: "令牌数据库备份",
    },
    CoreEntry {
        relative: "machineid",
        kind: EntryKind::File,
        label: "机器标识",
    },
    CoreEntry {
        relative: "aha",
        kind: EntryKind::Dir,
        label: "设备认证数据",
    },
    CoreEntry {
        relative: "Preferences",
        kind: EntryKind::File,
        label: "客户端偏好",
    },
    CoreEntry {
        relative: "Local State",
        kind: EntryKind::File,
        label: "本地状态",
    },
    CoreEntry {
        relative: "Local Storage/config.db",
        kind: EntryKind::File,
        label: "本地存储",
    },
    CoreEntry {
        relative: "Network",
        kind: EntryKind::Dir,
        label: "网络凭据（Cookie）",
    },
    CoreEntry {
        relative: "Partitions/trae-webview",
        kind: EntryKind::Dir,
        label: "WebView 分区站点数据",
    },
    CoreEntry {
        relative: "Partitions/icube-web-crawler",
        kind: EntryKind::Dir,
        label: "抓取器分区站点数据",
    },
];

impl CoreEntry {
    /// 把相对路径解析到给定根目录下（同时支持 `/` 与平台分隔符）。
    pub fn resolve(&self, root: &Path) -> PathBuf {
        self.relative
            .split('/')
            .fold(root.to_path_buf(), |acc, part| acc.join(part))
    }
}

/// 从客户端登录态目录里提取当前登录账号的 JWT。
///
/// ## 语义已收敛为「TraeWork 限定」——**不再是全局选目录**
///
/// 本函数现在只是 [`extract_local_jwt_for`]`(TraeVariant::default())` 的兼容壳，
/// 而 `TraeVariant::default()` = [`TraeVariant::TraeWork`]。也就是说它**只读
/// TraeWork（`TRAE SOLO CN`）这一条产品线的目录**，绝不会横跨变体去挑最近活跃的目录。
///
/// 保留它只为兼容既有签名：**全仓库已无生产调用点**，仅定义行与测试引用它。
/// 新代码请直接调用 [`extract_local_jwt_for`] 并显式传入变体，不要再依赖此壳——
/// 否则会重新落入「用户在 A 分区操作、代码却读 B 产品线」的老坑（见下一函数的说明）。
///
/// ## 扫哪些文件（两处，不能只扫第一处）
///
/// Trae 是 VSCode 系客户端，旧版登录凭据落在 `state.vscdb`（SQLite，键值表
/// `ItemTable`）与 `storage.json`（JSON）里，形态是 `Cloud-IDE-JWT <token>`。
/// 不同版本把 key 放在不同位置，因此这里**不按固定 key 取**，
/// 而是把文件当**文本**扫一遍、捞出 JWT 字面量 —— 这是对未知版布局的容错。
///
/// **但只扫这两个文件在 1.107.x 上必然失败**：实测该版本已把凭据改为加密存储
/// （`Local State` 里是 `os_crypt.encrypted_key`，即 Electron `safeStorage`），
/// 两个文件里已无 `Cloud-IDE-JWT` 明文。用户因此看到「明明登录了却识别不到」。
///
/// 真正的明文来源是客户端自己的**扩展日志**（见 [`collect_log_candidates`]）——
/// 实测 `exthost/trae.ai-code-completion/completion.log` 里有
/// `"Authorization":"Cloud-IDE-JWT eyJ…"` 的完整可用 token。
/// 因此本函数同时扫日志，并按 `exp` 取**最新那个**。
///
/// 只读、不改写客户端任何文件：导入失败时客户端登录态不受影响。
pub fn extract_local_jwt() -> Result<(String, String), String> {
    extract_local_jwt_for(TraeVariant::default())
}

/// 按**产品线变体**提取当前登录账号的 JWT。
///
/// ## 为什么必须带 `variant`（这是「导入报错说错产品线」的修复点）
///
/// 旧签名走 [`platform::detect_data_dir`]，它**横跨全部变体**挑最近活跃的目录。
/// 于是用户在 Trae Work 分区点「导入本机账号」时，若本机 `Trae CN` 更活跃，
/// 代码会去读 `Trae CN` 的目录 —— 报错文案也会跟着说成 Trae CN，把用户往错误
/// 的排障方向带。本函数把候选**限定在传入变体之内**（[`platform::select_data_dir_for`]），
/// 使「用户在哪个分区操作」真正进入代码。
///
/// 该变体没有任何存在的候选目录时，给出**指向该变体**的明确错误，
/// 而不是含糊的「未检测到 Trae 客户端数据目录」。
pub fn extract_local_jwt_for(variant: TraeVariant) -> Result<(String, String), String> {
    let data_dir = platform::select_data_dir_for(variant).ok_or_else(|| {
        format!(
            "未找到【{}】的数据目录，请先启动一次该客户端并登录",
            variant.display_name()
        )
    })?;

    // 主来源（旧版明文所在）+ 次来源（新版唯一明文来源）。
    let mut candidates: Vec<(PathBuf, &'static str)> = vec![
        (
            data_dir.join("User").join("globalStorage").join("storage.json"),
            "登录态文件",
        ),
        (
            data_dir.join("User").join("globalStorage").join("state.vscdb"),
            "令牌数据库",
        ),
    ];
    for path in collect_log_candidates(&data_dir) {
        candidates.push((path, "扩展日志"));
    }

    let mut best: Option<(String, i64, &'static str)> = None;
    for (path, source) in candidates {
        if !path.is_file() {
            continue;
        }
        let Ok(raw) = read_capped(&path, LOG_SCAN_MAX_BYTES) else {
            continue;
        };
        // 二进制 SQLite 里 JWT 仍是可读 ASCII 串，按 lossy 解码即可扫描。
        let text = String::from_utf8_lossy(&raw);
        for token in scan_jwt_tokens(&text) {
            let exp = crate::modules::trae::jwt::parse(&token)
                .exp_timestamp
                .unwrap_or(0);
            if best
                .as_ref()
                .map(|(_, best_exp, _)| exp > *best_exp)
                .unwrap_or(true)
            {
                best = Some((token, exp, source));
            }
        }
    }

    let (token, exp, source) =
        best.ok_or_else(|| diagnose_missing_credential(&data_dir, variant))?;
    if exp > 0 && exp < chrono::Utc::now().timestamp() {
        return Err(format!(
            "在{source}找到的登录凭据已过期，请先在 Trae 中重新登录后再导入；\
             或改用「OAuth 网页登录」自动获取可续期的凭据。"
        ));
    }
    let uid = crate::modules::trae::jwt::user_id_of(&token)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "解析到登录凭据但无法确定账号归属".to_string())?;
    Ok((uid, token))
}

/// 找不到凭据时，产出**可操作**的诊断信息（而不是一句笼统的"没找到"）。
///
/// ## 为什么需要它（2026-09-18 实测成因）
///
/// 本机装有两条产品线，实测差异极大：
///
/// | 产品线 | `storage.json` 明文 | 扩展日志明文 | 结论 |
/// |:--|:--|:--|:--|
/// | `Trae CN` | 无（`iCubeAuthInfo://*` 是 iCube 自有加密，非 DPAPI） | 有（`trae.ai-code-completion` 扩展写） | 可提取 |
/// | `TRAE SOLO CN` | 无 | **无** —— 该产品线**没装** `trae.ai-code-completion` 扩展 | 无论如何都提不出 |
///
/// 用户看到的现象是「明明装了、也登录了，却识别不到账号」。真正的原因是
/// **该产品线的客户端不写明文凭据**，与我们的代码无关、也无法通过改代码绕过。
/// 因此这里必须说清「是哪条产品线、缺的是什么来源」，并给出唯一可行的替代路径
/// （OAuth 网页登录）——否则用户会反复重试导入。
///
/// ## 标签取自**传入的变体**，不再全局探测
///
/// 旧实现调 [`platform::detected_variant`]，而它是无入参的全局探测（挑最近活跃的目录）。
/// 于是用户在 Trae Work 分区导入失败时，只要本机 `Trae CN` 更活跃，标签就会显示成
/// Trae CN —— 报错说错产品线，把用户往错误的排障方向带。现在标签严格等于
/// 调用方指定的变体，与实际读取的 `data_dir` 同源。
fn diagnose_missing_credential(data_dir: &Path, variant: TraeVariant) -> String {
    let label = variant.display_name();

    // 日志是可选的明文来源：有日志说明客户端写过请求、只是没写凭据；
    // 完全没有 logs/ 目录说明客户端可能从未在此 userData 下启动过。
    let has_logs = data_dir.join("logs").is_dir();
    let log_note = if has_logs {
        "客户端日志存在，但其中没有明文凭据 —— 该产品线未安装会记录 \
         Authorization 头的扩展（实测 `trae.ai-code-completion` 缺失时必然如此）。"
    } else {
        "客户端日志目录不存在 —— 请先启动一次该客户端并确认已登录。"
    };

    // `storage.json` 里若已有 iCube 认证条目，说明**登录确实发生过**，
    // 只是内容被 iCube 自己加密（`tC\\x05\\x10` 魔数，非 Windows DPAPI，无法离线解密）。
    let auth_rows = storage_auth_entry_count(data_dir);
    let auth_note = if auth_rows > 0 {
        format!(
            "检测到 {auth_rows} 条 iCube 认证记录，说明**该客户端确实已登录**；\
             但内容由 iCube 自行加密（非 Windows DPAPI），不可离线解密。"
        )
    } else {
        "未在存储中发现 iCube 认证记录，可能未登录或登录态尚未落盘。".to_string()
    };

    format!(
        "未在【{label}】的数据目录（{}）中找到可用的登录凭据。\n\
         · {log_note}\n\
         · {auth_note}\n\
         请改用「OAuth 网页登录」——它不依赖本地文件，且能获得可自动续期的凭据。",
        data_dir.display()
    )
}

/// 统计 `storage.json` 里 `iCubeAuthInfo://*` 条目数（仅用于诊断，读失败返回 0）。
///
/// 不解析内容：这些值由 iCube 自有格式加密，我们只关心"登录是否发生过"。
fn storage_auth_entry_count(data_dir: &Path) -> usize {
    let path = data_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return 0;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return 0;
    };
    value
        .as_object()
        .map(|map| {
            map.keys()
                .filter(|key| key.starts_with("iCubeAuthInfo://"))
                .count()
        })
        .unwrap_or(0)
}

/// 单次日志扫描允许读取的**单个文件**上限。
///
/// 客户端日志可能长到几十 MB（`renderer.log` 实测 400KB～数 MB），
/// 全读会白白吃内存。JWT 只会出现在请求头行里，读前 8MB 足够覆盖；
/// 真超出这一段的极旧日志，其 token 也早已过期。
const LOG_SCAN_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// 单次日志扫描允许检查的**文件数**上限（按修改时间取最新的若干个）。
///
/// Trae 每启动一次就新建一个 `logs/<timestamp>/` 目录，长期使用会累积成百上千个。
/// 只取最近的在语义上也更对：明文 token 越新越可能仍然有效。
const LOG_SCAN_MAX_FILES: usize = 40;

/// 读取文件的前 `max_bytes` 字节（超出部分丢弃，不报错）。
fn read_capped(path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let mut buf = Vec::new();
    file.take(max_bytes).read_to_end(&mut buf)?;
    Ok(buf)
}

/// 收集客户端日志里「可能含明文凭据」的候选文件，按修改时间倒序、限量。
///
/// ## 为什么必须有这一条来源（实测依据，勿删）
///
/// Trae 1.107.x 起凭据为加密存储，`storage.json` / `state.vscdb` 里已无明文。
/// 而客户端扩展会把出站请求头原样打进自己的日志：
///
/// ```text
/// request: headers: {…,"Authorization":"Cloud-IDE-JWT eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.…"}
/// ```
///
/// 本机实测 `logs/<ts>/window1/exthost/trae.ai-code-completion/completion.log`
/// 含完整 JWT（同一文件内 6 处）。这是当前唯一稳定的本地明文来源。
///
/// ## 为什么不必排除 `Cache/` 与 `CachedData/`
///
/// 那两个目录里也含 `Cloud-IDE-JWT` 字样，但内容是打包后的 JS 源码
/// （模板串 `` Authorization:`Cloud-IDE-JWT ${e}` ``），后面跟的不是 token。
/// [`scan_jwt_tokens`] 要求 base64url 且至少两个点，这类命中会得到空串被丢弃。
/// 本函数更是只走 `logs/`，根本不进那两个目录。
fn collect_log_candidates(data_dir: &Path) -> Vec<PathBuf> {
    let logs_root = data_dir.join("logs");
    if !logs_root.is_dir() {
        return Vec::new();
    }
    let mut files: Vec<(PathBuf, std::time::SystemTime)> = Vec::new();
    // 手写深度优先：只用 std，不引 walkdir；日志目录层级固定且不深。
    let mut stack = vec![logs_root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if !name.ends_with(".log") {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            files.push((path, modified));
        }
    }
    // 新的排前面：同时命中的多个 token 由调用方按 exp 取最新，但先扫新文件
    // 能让「确定性截断」发生在最不可能相关的那一批上。
    files.sort_by(|a, b| b.1.cmp(&a.1));
    files.truncate(LOG_SCAN_MAX_FILES);
    files.into_iter().map(|(path, _)| path).collect()
}

/// 从任意文本里扫出所有 `Cloud-IDE-JWT <token>` 形态的字面量（去重，保序）。
///
/// 纯函数，便于单测。token 的字符集按 JWT 规范限定为 base64url 与 `.`，
/// 这样在二进制 SQLite 内容里也不会把相邻的二进制字节吞进来。
pub fn scan_jwt_tokens(text: &str) -> Vec<String> {
    const PREFIX: &str = "Cloud-IDE-JWT ";
    let mut found: Vec<String> = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = text[cursor..].find(PREFIX) {
        let start = cursor + offset + PREFIX.len();
        let token: String = text[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
            .collect();
        // JWT 至少是 `header.payload.signature` 三段；过滤掉半截匹配的噪声。
        if token.matches('.').count() >= 2 && !found.contains(&token) {
            found.push(token);
        }
        cursor = start;
    }
    found
}

/// 切换过程中的一步（线上形态 camelCase）。
#[derive(Debug, Clone)]
pub struct SwitchStep {
    /// 阶段标识，例 `precheck` / `stop` / `backup` / `restore` / `launch` / `done` / `fatal`。
    pub stage: &'static str,
    /// 状态：`ok` / `skip` / `fail`。
    pub status: &'static str,
    /// 人可读说明。
    pub message: String,
}

impl SwitchStep {
    /// 构造一步。
    pub fn new(stage: &'static str, status: &'static str, message: impl Into<String>) -> Self {
        Self {
            stage,
            status,
            message: message.into(),
        }
    }

    /// 线上形态。
    pub fn to_json(&self) -> Value {
        json!({
            "stage": self.stage,
            "status": self.status,
            "message": self.message,
            "time": store::now_iso(),
        })
    }
}

/// 切换选项。
#[derive(Debug, Clone, Default)]
pub struct SwitchOptions {
    /// 目标账号。
    pub user_id: String,
    /// 恢复后是否启动客户端。
    pub launch: bool,
    /// 若代理正在运行，启动时注入的端口。
    pub proxy_port: Option<u16>,
    /// 是否在恢复前重置设备标识。
    pub reset_device: bool,
    /// 产品线变体：决定读/写哪条产品线的快照目录与设备标识。
    ///
    /// 用 `#[derive(Default)]` 的零值即可得 `TraeWork`（枚举的 `Default` 实现），
    /// 因此既有的 `SwitchOptions { ..Default::default() }` 调用点不需要显式写这一项。
    pub variant: TraeVariant,
}

/// 切换结果。
#[derive(Debug, Clone, Default)]
pub struct SwitchOutcome {
    /// 是否成功。
    pub success: bool,
    /// 逐步骤记录。
    pub steps: Vec<SwitchStep>,
    /// 失败原因（成功时为 `None`）。
    pub error: Option<String>,
}

impl SwitchOutcome {
    /// 线上形态。
    pub fn to_json(&self) -> Value {
        json!({
            "success": self.success,
            "steps": self.steps.iter().map(SwitchStep::to_json).collect::<Vec<_>>(),
            "error": self.error,
        })
    }
}

/// 快照信息（线上形态，见 [`list_profiles`]）。
#[derive(Debug, Clone)]
pub struct ProfileInfo {
    /// 槽位名（账号 uid，或 `last`）。
    pub slot: String,
    /// 总字节数。
    pub size_bytes: u64,
    /// 文件数。
    pub file_count: u64,
    /// 最近修改时间（`YYYY-MM-DD HH:MM:SS`）。
    pub last_modified: String,
}

impl ProfileInfo {
    /// 线上形态。
    pub fn to_json(&self) -> Value {
        json!({
            "slot": self.slot,
            "sizeBytes": self.size_bytes,
            "fileCount": self.file_count,
            "lastModified": self.last_modified,
            "sizeText": format_size(self.size_bytes),
        })
    }
}

/// 「最近一次切换前保存」的兜底槽位名。
pub const LAST_SLOT: &str = "last";

/// 记录「当前活跃账号 uid」的文件（位于该变体的 `profiles` 目录下）。
fn current_account_file_for(variant: TraeVariant) -> PathBuf {
    paths::profiles_dir_for(variant).join("current_account.txt")
}

/// 读取当前活跃账号 uid（默认变体，兼容壳）。
pub fn current_account() -> Option<String> {
    current_account_for(TraeVariant::default())
}

/// 读取当前活跃账号 uid（按变体分家）。
///
/// 「当前账号」是**每条产品线各自的事实**：Trae Work 与 Trae CN 有各自的客户端数据目录，
/// 可以同时登录不同账号。共用一份会让两条线在 UI 上互相冒充。
pub fn current_account_for(variant: TraeVariant) -> Option<String> {
    std::fs::read_to_string(current_account_file_for(variant))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

/// 写入当前活跃账号 uid（默认变体，兼容壳）。
pub fn set_current_account(user_id: &str) -> Result<(), String> {
    set_current_account_for(TraeVariant::default(), user_id)
}

/// 写入当前活跃账号 uid（按变体分家）。
pub fn set_current_account_for(variant: TraeVariant, user_id: &str) -> Result<(), String> {
    if !paths::safe_slot_name(user_id) {
        return Err("非法的账号 ID".into());
    }
    store::atomic_write_text(&current_account_file_for(variant), user_id)
}

/// 人类可读的文件大小。
pub fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes_f = bytes as f64;
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes_f < MB {
        format!("{:.1} KB", bytes_f / KB)
    } else if bytes_f < GB {
        format!("{:.1} MB", bytes_f / MB)
    } else {
        format!("{:.2} GB", bytes_f / GB)
    }
}

/// 递归统计目录的字节数与文件数。
fn dir_stats(path: &Path) -> (u64, u64) {
    let mut size = 0u64;
    let mut count = 0u64;
    let Ok(entries) = std::fs::read_dir(path) else {
        return (0, 0);
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            let (child_size, child_count) = dir_stats(&child);
            size += child_size;
            count += child_count;
        } else {
            size += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            count += 1;
        }
    }
    (size, count)
}

/// 列出所有已保存的登录态快照，按最后修改时间倒序（默认变体，兼容壳）。
pub fn list_profiles() -> Vec<ProfileInfo> {
    list_profiles_for(TraeVariant::default())
}

/// 列出所有已保存的登录态快照，按最后修改时间倒序（按变体分家）。
pub fn list_profiles_for(variant: TraeVariant) -> Vec<ProfileInfo> {
    let dir = paths::profiles_dir_for(variant);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<ProfileInfo> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .map(|entry| {
            let (size_bytes, file_count) = dir_stats(&entry.path());
            let last_modified = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .map(|time| {
                    let datetime: chrono::DateTime<chrono::Local> = time.into();
                    datetime.format("%Y-%m-%d %H:%M:%S").to_string()
                })
                .unwrap_or_else(|| "-".into());
            ProfileInfo {
                slot: entry.file_name().to_string_lossy().to_string(),
                size_bytes,
                file_count,
                last_modified,
            }
        })
        .collect();
    out.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
    out
}

/// 递归复制文件或目录。
///
/// `overwrite_dir` 为 `true` 时，若目标目录已存在会先整体删除再复制
/// （用于 `aha/` / `Network/` 这类「目录即身份」的数据：合并会留下目标账号的
/// 残留凭据，导致切换后仍是旧账号）。
fn copy_entry(source: &Path, target: &Path, kind: EntryKind) -> Result<u64, String> {
    match kind {
        EntryKind::File => {
            if !source.is_file() {
                return Ok(0);
            }
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
            }
            std::fs::copy(source, target).map_err(|e| {
                format!(
                    "复制 {} -> {} 失败: {e}",
                    source.display(),
                    target.display()
                )
            })?;
            Ok(1)
        }
        EntryKind::Dir => {
            if !source.is_dir() {
                return Ok(0);
            }
            if target.exists() {
                std::fs::remove_dir_all(target)
                    .map_err(|e| format!("清理目标目录 {} 失败: {e}", target.display()))?;
            }
            copy_dir_recursive(source, target)
        }
    }
}

/// 递归复制目录，返回复制的文件数。
fn copy_dir_recursive(source: &Path, target: &Path) -> Result<u64, String> {
    std::fs::create_dir_all(target).map_err(|e| format!("创建目录失败: {e}"))?;
    let mut copied = 0u64;
    for entry in std::fs::read_dir(source)
        .map_err(|e| format!("读取目录 {} 失败: {e}", source.display()))?
        .flatten()
    {
        let child_source = entry.path();
        let child_target = target.join(entry.file_name());
        if child_source.is_dir() {
            copied += copy_dir_recursive(&child_source, &child_target)?;
        } else {
            // 单个文件复制失败不中断整体：Trae 运行中会锁住某些 db 文件，
            // 为一个大体无关紧要的附属文件放弃整次快照得不偿失。
            if std::fs::copy(&child_source, &child_target).is_ok() {
                copied += 1;
            }
        }
    }
    Ok(copied)
}

/// 把客户端当前的登录态复制到指定槽位（默认变体，兼容壳）。
pub fn backup_to_slot(slot: &str) -> Result<u64, String> {
    backup_to_slot_for(TraeVariant::default(), slot)
}

/// 把客户端当前的登录态复制到指定槽位（按变体分家）。
///
/// 返回复制的文件数。客户端 userData 目录不存在时返回 `Err`——
/// 这通常意味着 Trae 从未启动过，继续「备份」只会产出一个空快照，
/// 让用户以为已经存过。
pub fn backup_to_slot_for(variant: TraeVariant, slot: &str) -> Result<u64, String> {
    if !paths::safe_slot_name(slot) {
        return Err(format!("非法的槽位名: {slot}"));
    }
    let source_root = platform::detect_data_dir_for(variant)
        .filter(|dir| dir.is_dir())
        .ok_or("未找到 Trae 客户端数据目录，请先启动一次 Trae 并登录")?;
    let target_root = paths::profiles_dir_for(variant).join(slot);

    let mut copied = 0u64;
    for entry in CORE_ENTRIES {
        copied += copy_entry(
            &entry.resolve(&source_root),
            &entry.resolve(&target_root),
            entry.kind,
        )?;
    }
    if copied == 0 {
        return Err("未找到任何登录态文件，请确认已在 Trae 中登录".into());
    }
    store::atomic_write_text(&target_root.join(".source"), &source_root.to_string_lossy())?;
    Ok(copied)
}

/// 把指定槽位的快照恢复到客户端（默认变体，兼容壳）。
pub fn restore_from_slot(slot: &str) -> Result<u64, String> {
    restore_from_slot_for(TraeVariant::default(), slot)
}

/// 把指定槽位的快照恢复到客户端（按变体分家）。
///
/// 返回恢复的文件数。槽位不存在时报错，而不是「静默成功」——
/// 切换流程据此判定失败并停下，避免留下「关掉了客户端但没恢复」的中间态。
pub fn restore_from_slot_for(variant: TraeVariant, slot: &str) -> Result<u64, String> {
    if !paths::safe_slot_name(slot) {
        return Err(format!("非法的槽位名: {slot}"));
    }
    let source_root = paths::profiles_dir_for(variant).join(slot);
    if !source_root.is_dir() {
        return Err(format!("槽位 {slot} 的登录态快照不存在"));
    }
    let target_root = platform::detect_data_dir_for(variant)
        .ok_or("无法定位 Trae 客户端数据目录")?;
    std::fs::create_dir_all(&target_root)
        .map_err(|e| format!("创建客户端数据目录失败: {e}"))?;

    let mut restored = 0u64;
    for entry in CORE_ENTRIES {
        restored += copy_entry(
            &entry.resolve(&source_root),
            &entry.resolve(&target_root),
            entry.kind,
        )?;
    }

    // 客户端单实例锁残留会让下次启动直接退出（Electron 常见困局），
    // 恢复后一并清掉，代价极小。
    let _ = std::fs::remove_file(target_root.join("code.lock"));

    Ok(restored)
}

/// 删除指定槽位的快照（默认变体，兼容壳）。
pub fn delete_slot(slot: &str) -> Result<(), String> {
    delete_slot_for(TraeVariant::default(), slot)
}

/// 删除指定槽位的快照（按变体分家）。
///
/// 槽位不存在视为成功（幂等）：删除是「让状态变成不存在」，重复执行语义相同。
pub fn delete_slot_for(variant: TraeVariant, slot: &str) -> Result<(), String> {
    if !paths::safe_slot_name(slot) {
        return Err(format!("非法的槽位名: {slot}"));
    }
    let dir = paths::profiles_dir_for(variant).join(slot);
    if !dir.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("删除快照失败: {e}"))
}

/// 执行一次完整的账号切换。
///
/// 流程（顺序不可调整，见模块头注释）：
/// 预检查目标快照 → 保存当前到 `last` → （已知 uid 时）保存当前到其槽位 →
/// （可选）重置设备标识 → 关闭客户端 → 恢复目标快照 → （可选）启动客户端。
pub fn switch_account<F>(options: &SwitchOptions, mut on_step: F) -> SwitchOutcome
where
    F: FnMut(&SwitchStep),
{
    let variant = options.variant;
    let mut outcome = SwitchOutcome::default();
    let mut emit = |outcome: &mut SwitchOutcome, step: SwitchStep| {
        on_step(&step);
        outcome.steps.push(step);
    };

    let target = options.user_id.trim().to_string();
    if !paths::safe_slot_name(&target) {
        let step = SwitchStep::new("fatal", "fail", format!("非法的账号 ID: {target}"));
        emit(&mut outcome, step);
        outcome.error = Some("非法的账号 ID".into());
        return outcome;
    }

    // 1. 预检查：目标快照必须存在，否则后面的「关闭客户端」会造成一个无法恢复的中间态。
    let target_slot = match paths::profile_dir_for(variant, &target) {
        Some(dir) if dir.is_dir() => dir,
        _ => {
            emit(
                &mut outcome,
                SwitchStep::new(
                    "fatal",
                    "fail",
                    format!("账号 {target} 的登录态快照不存在，请先保存该账号的登录态"),
                ),
            );
            outcome.error = Some("目标快照不存在".into());
            return outcome;
        }
    };
    let _ = target_slot;
    emit(
        &mut outcome,
        SwitchStep::new("precheck", "ok", "目标账号快照已就绪"),
    );

    // 2. 保存当前登录态到 last 槽位（强制，可回滚兜底）。
    let previous_account = current_account_for(variant);
    match backup_to_slot_for(variant, LAST_SLOT) {
        Ok(count) => emit(
            &mut outcome,
            SwitchStep::new("backup", "ok", format!("当前登录态已保存到 last 槽位（{count} 个文件）")),
        ),
        Err(error) => emit(
            &mut outcome,
            SwitchStep::new(
                "backup",
                "skip",
                format!("当前登录态未能保存（{error}），继续切换"),
            ),
        ),
    }

    // 3. 若已知当前账号，额外保存到它自己的槽位，使该账号可被再次切回。
    if let Some(previous) = previous_account.as_deref().filter(|uid| *uid != target) {
        match backup_to_slot_for(variant, previous) {
            Ok(count) => emit(
                &mut outcome,
                SwitchStep::new(
                    "backup-current",
                    "ok",
                    format!("当前账号 {previous} 的登录态已更新（{count} 个文件）"),
                ),
            ),
            Err(error) => emit(
                &mut outcome,
                SwitchStep::new("backup-current", "skip", format!("更新 {previous} 失败: {error}")),
            ),
        }
    }

    // 4. 设备标识重置（可选）。
    if options.reset_device {
        match crate::modules::trae::platform::reset_device_identity_for(variant) {
            Ok(report) => {
                let ok = report
                    .get("resetCount")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                emit(
                    &mut outcome,
                    SwitchStep::new("device", "ok", format!("设备标识已重置（{ok} 项）")),
                );
            }
            Err(error) => emit(
                &mut outcome,
                SwitchStep::new("device", "skip", format!("设备标识重置被跳过: {error}")),
            ),
        }
    }

    // 5. 关闭客户端（恢复文件时若客户端在运行，会被其内存缓存覆盖回去）。
    match platform::kill_client_for(variant) {
        Ok(true) => emit(
            &mut outcome,
            SwitchStep::new("stop", "ok", "已关闭 Trae 客户端"),
        ),
        Ok(false) => emit(
            &mut outcome,
            SwitchStep::new("stop", "skip", "Trae 客户端未在运行"),
        ),
        Err(error) => {
            // 关不掉就不能恢复：此时恢复必然被客户端覆盖，会造成「看起来切了实际没切」。
            emit(
                &mut outcome,
                SwitchStep::new("fatal", "fail", format!("无法关闭 Trae 客户端: {error}")),
            );
            outcome.error = Some(error);
            return outcome;
        }
    }

    // 6. 恢复目标快照。
    match restore_from_slot_for(variant, &target) {
        Ok(count) => emit(
            &mut outcome,
            SwitchStep::new("restore", "ok", format!("已恢复 {target} 的登录态（{count} 个文件）")),
        ),
        Err(error) => {
            emit(
                &mut outcome,
                SwitchStep::new("fatal", "fail", format!("恢复失败: {error}")),
            );
            outcome.error = Some(error);
            return outcome;
        }
    }

    // 记录当前账号（仅在前几步都成功后写，避免把失败态记成「已切换」）。
    if let Err(error) = set_current_account_for(variant, &target) {
        emit(
            &mut outcome,
            SwitchStep::new("record", "skip", format!("记录当前账号失败: {error}")),
        );
    }

    // 7. 启动客户端。
    if options.launch {
        match crate::modules::trae::platform::detect_install_for(variant) {
            probe if probe.installed => match probe.exe {
                Some(exe) => match platform::launch_client_for(variant, &exe, options.proxy_port) {
                    Ok(()) => emit(
                        &mut outcome,
                        SwitchStep::new("launch", "ok", "已启动 Trae 客户端"),
                    ),
                    Err(error) => emit(
                        &mut outcome,
                        SwitchStep::new("launch", "fail", format!("启动失败: {error}")),
                    ),
                },
                None => emit(
                    &mut outcome,
                    SwitchStep::new("launch", "skip", "未找到客户端可执行文件，请手动启动"),
                ),
            },
            _ => emit(
                &mut outcome,
                SwitchStep::new(
                    "launch",
                    "skip",
                    "未检测到本地 Trae 安装，请在设置中指定客户端路径",
                ),
            ),
        }
    }

    outcome.success = outcome.error.is_none();
    emit(&mut outcome, SwitchStep::new("done", "ok", "账号切换完成"));
    outcome
}

/// 保存当前登录态到指定账号槽位（不切换、不重启客户端；默认变体，兼容壳）。
pub fn save_current_login(user_id: &str) -> Result<u64, String> {
    save_current_login_for(TraeVariant::default(), user_id)
}

/// 保存当前登录态到指定账号槽位（不切换、不重启客户端；按变体分家）。
pub fn save_current_login_for(variant: TraeVariant, user_id: &str) -> Result<u64, String> {
    let count = backup_to_slot_for(variant, user_id)?;
    let _ = set_current_account_for(variant, user_id);
    store::append_log(
        &paths::switcher_log_file_for(variant),
        &format!("保存登录态: user={user_id} 文件数={count}"),
    );
    Ok(count)
}

/// 快照总览（线上形态）：槽位列表 + 当前账号 + 客户端数据目录（默认变体，兼容壳）。
pub fn overview() -> Value {
    overview_for(TraeVariant::default())
}

/// 快照总览（线上形态；按变体分家）。
///
/// `dataDir` / `clientRunning` 都取**该变体**的视角 —— 与槽位列表同源，
/// 否则会出现「列出了 Trae CN 的快照，却说 Trae Work 的客户端在运行」。
pub fn overview_for(variant: TraeVariant) -> Value {
    json!({
        "profiles": list_profiles_for(variant).iter().map(ProfileInfo::to_json).collect::<Vec<_>>(),
        "currentAccount": current_account_for(variant),
        "dataDir": platform::detect_data_dir_for(variant).map(|dir| dir.to_string_lossy().to_string()),
        "clientRunning": platform::is_running_for(variant),
        "coreEntryCount": CORE_ENTRIES.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_entries_cover_the_nine_login_state_categories() {
        // 参考实现精准备份 9 类文件；这里允许更多，但每个关键类别都必须在列。
        let relatives: Vec<&str> = CORE_ENTRIES.iter().map(|e| e.relative).collect();
        for required in [
            "User/globalStorage/storage.json",
            "User/globalStorage/state.vscdb",
            "machineid",
            "aha",
            "Preferences",
            "Local State",
            "Local Storage/config.db",
            "Network",
            "Partitions/trae-webview",
        ] {
            assert!(
                relatives.contains(&required),
                "核心文件清单缺少 {required}"
            );
        }
        assert!(CORE_ENTRIES.len() >= 9, "至少覆盖 9 类核心文件");
    }

    #[test]
    fn core_entry_paths_are_relative_and_normalized() {
        for entry in CORE_ENTRIES {
            assert!(!entry.relative.starts_with('/'), "{}", entry.relative);
            assert!(!entry.relative.contains('\\'), "{}", entry.relative);
            assert!(!entry.relative.contains(".."), "{}", entry.relative);
            assert!(!entry.label.is_empty(), "{}", entry.relative);
        }
    }

    #[test]
    fn core_entry_resolve_uses_platform_separator() {
        let root = Path::new("/data");
        let resolved = CORE_ENTRIES[0].resolve(root);
        assert!(resolved.starts_with(root));
        // 必须真的分成多级目录，而不是把 '/' 当成文件名的一部分
        assert!(resolved.ends_with("storage.json"));
        assert!(resolved.to_string_lossy().contains("globalStorage"));
    }

    #[test]
    fn last_slot_name_is_valid_slot() {
        assert!(paths::safe_slot_name(LAST_SLOT));
    }

    // -----------------------------------------------------------------------
    // 日志来源：新版客户端凭据的唯一明文出口
    // -----------------------------------------------------------------------

    /// 造一个临时 userData 目录，可指定日志文件（相对 `logs/` 的路径 → 内容）。
    fn temp_data_dir(tag: &str, logs: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "trae-profile-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("建临时目录失败");
        for (relative, content) in logs {
            let path = root.join("logs").join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).expect("建日志子目录失败");
            std::fs::write(&path, content).expect("写日志失败");
        }
        root
    }

    #[test]
    fn collect_log_candidates_finds_nested_logs_and_skips_non_logs() {
        let root = temp_data_dir(
            "cands",
            &[
                ("20260101T000000/main.log", "a"),
                ("20260102T000000/window1/exthost/trae.ai-code-completion/completion.log", "b"),
                // 非 .log 不能被当成候选
                ("20260102T000000/main.log.bak", "c"),
                ("20260102T000000/notes.txt", "d"),
            ],
        );
        let found = collect_log_candidates(&root);
        let names: Vec<String> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(found.len(), 2, "应只收 .log：{names:?}");
        assert!(names.iter().all(|n| n.ends_with(".log")));
        // 深层目录里的那个也必须被找到——真实凭据正是在 exthost 深层。
        assert!(
            found.iter().any(|p| p
                .to_string_lossy()
                .replace('\\', "/")
                .contains("exthost/trae.ai-code-completion/completion.log")),
            "深层扩展日志未被递归找到: {found:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn collect_log_candidates_returns_empty_without_logs_dir() {
        let root = std::env::temp_dir().join(format!("trae-profile-nologs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(collect_log_candidates(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn collect_log_candidates_is_capped() {
        // 造出超过上限的日志文件，确认截断生效（否则长期使用的机器会扫上千个文件）。
        let root = std::env::temp_dir().join(format!("trae-profile-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for i in 0..(LOG_SCAN_MAX_FILES + 7) {
            let path = root.join("logs").join(format!("run{i:04}")).join("main.log");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "x").unwrap();
        }
        assert_eq!(collect_log_candidates(&root).len(), LOG_SCAN_MAX_FILES);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn read_capped_stops_at_limit_and_ignores_the_rest() {
        let root = std::env::temp_dir().join(format!("trae-profile-cap-read-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("big.log");
        std::fs::write(&path, b"0123456789").unwrap();

        assert_eq!(read_capped(&path, 4).unwrap(), b"0123");
        // 上限大于文件长度时必须完整读出，不能把文件读空。
        assert_eq!(read_capped(&path, 100).unwrap(), b"0123456789");
        assert!(read_capped(&root.join("missing.log"), 10).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 端到端：凭据只存在于扩展日志（`storage.json` / `state.vscdb` 无明文）时，
    /// `extract_local_jwt` 仍必须能取到它。
    ///
    /// 这是「为什么识别不到 Trae CN」的直接回归护栏 —— 修之前只扫两个登录态文件，
    /// 而 1.107.x 那两处已无明文，于是导入恒失败。
    ///
    /// 用 `APPDATA` 把「客户端数据目录」指向临时目录（Windows 上
    /// `platform::data_dir_base()` 优先读它），从而真正走一遍 `extract_local_jwt`。
    #[cfg(windows)]
    #[test]
    fn extract_local_jwt_falls_back_to_extension_log() {
        let exp = chrono::Utc::now().timestamp() + 3600;
        let payload = format!(r#"{{"exp":{exp},"data":{{"id":"9988776655443322"}}}}"#);
        let token = format!(
            "{}.{}.{}",
            base64url(r#"{"alg":"RS256","typ":"JWT"}"#),
            base64url(&payload),
            "c2lnbmF0dXJl"
        );

        let base = std::env::temp_dir().join(format!("trae-profile-appdata-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // 候选目录名必须取自 `platform::data_dir_names()` 的固定列表。
        let client = base.join("TRAE SOLO CN");
        std::fs::create_dir_all(client.join("User").join("globalStorage")).unwrap();
        // 造一个「活跃标志」文件，使该候选按最近活跃排到第一。
        std::fs::write(
            client.join("User").join("globalStorage").join("storage.json"),
            r#"{"telemetry":{}}"#,
        )
        .unwrap();
        let log = client
            .join("logs")
            .join("20260917T174358")
            .join("window1")
            .join("exthost")
            .join("trae.ai-code-completion")
            .join("completion.log");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(
            &log,
            format!(
                r#"2026-09-17T17:44:00.000+08:00 [info] request: headers: {{"X-App-Id":"abc","Authorization":"Cloud-IDE-JWT {token}"}}"#
            ),
        )
        .unwrap();

        // 改进程级 `APPDATA`，必须持 env 锁；这里**不调** `HomeOverrideGuard::set()`
        // （那个是覆盖 BUDDY_SWITCH_HOME 的），故按仓库约定自行加锁。
        let _lock = crate::modules::config::env_lock();
        let original = std::env::var("APPDATA").ok();
        std::env::set_var("APPDATA", &base);

        let result = extract_local_jwt();

        match original {
            Some(value) => std::env::set_var("APPDATA", value),
            None => std::env::remove_var("APPDATA"),
        }
        let _ = std::fs::remove_dir_all(&base);

        let (uid, found) = result.expect("应能从扩展日志取到凭据");
        assert_eq!(uid, "9988776655443322");
        assert_eq!(found, token);
    }

    /// `storage_auth_entry_count` 必须**只**数 `iCubeAuthInfo://` 前缀的键。
    ///
    /// 它不是业务逻辑，而是诊断文案的事实依据（「登录是否发生过」）。
    /// 数多了会把「没登录」说成「登录了」，数少了会把「登录了」说成「没登录」，
    /// 两种错法都会把用户引向错误的排障方向。
    #[test]
    fn storage_auth_entry_count_only_counts_icube_auth_keys() {
        let root = std::env::temp_dir().join(format!("trae-auth-count-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let global = root.join("User").join("globalStorage");
        std::fs::create_dir_all(&global).unwrap();
        let path = global.join("storage.json");
        std::fs::write(
            &path,
            r#"{
                "iCubeAuthInfo://icube.cloudide": "tC\u0005\u0010AAAA",
                "iCubeAuthInfo://other.realm": "tC\u0005\u0010BBBB",
                "telemetry.machineId": "abc",
                "icubeAuthInfo://lowercase.prefix": "should-not-count"
            }"#,
        )
        .unwrap();

        assert_eq!(storage_auth_entry_count(&root), 2, "前缀匹配必须区分大小写");

        // 三种退化输入都必须是 0，而不是 panic —— 诊断函数在读失败时也要能出文案。
        std::fs::write(&path, r#"{"iCubeAuthInfo://x":"y""#).unwrap();
        assert_eq!(storage_auth_entry_count(&root), 0, "JSON 损坏时返回 0");
        std::fs::write(&path, r#"["not","an","object"]"#).unwrap();
        assert_eq!(storage_auth_entry_count(&root), 0, "顶层不是对象时返回 0");
        let _ = std::fs::remove_file(&path);
        assert_eq!(storage_auth_entry_count(&root), 0, "文件缺失时返回 0");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 诊断文案必须**可操作**：说清是哪条产品线、缺的是哪一类来源、下一步该做什么。
    ///
    /// 这是「用户反复重试导入」的根治点 —— 笼统的「没找到凭据」会让人以为是
    /// 代码 bug，而去重装客户端、重启、重登录，全都无效。
    #[test]
    fn diagnose_missing_credential_names_variant_and_next_step() {
        let root = std::env::temp_dir().join(format!("trae-diag-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("logs")).unwrap();
        let global = root.join("User").join("globalStorage");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(
            global.join("storage.json"),
            r#"{"iCubeAuthInfo://icube.cloudide":"tC\u0005\u0010AAAA"}"#,
        )
        .unwrap();

        let text = diagnose_missing_credential(&root, TraeVariant::TraeWork);
        assert!(text.contains("OAuth"), "必须给出可行替代路径：{text}");
        assert!(text.contains("1 条 iCube 认证记录"), "必须报告登录确实发生过：{text}");
        assert!(
            text.contains("trae.ai-code-completion"),
            "必须点明缺失的明文来源：{text}"
        );
        assert!(
            text.contains(&root.display().to_string()),
            "必须回显实际检查的数据目录，便于用户核对：{text}"
        );
        assert!(
            text.lines().count() >= 4,
            "多行诊断才装得下三块信息，实际：{text}"
        );

        // 无 logs/ 时必须换一套说法（提示先启动客户端），不能照搬"有日志但没凭据"。
        std::fs::remove_dir_all(root.join("logs")).unwrap();
        let no_logs = diagnose_missing_credential(&root, TraeVariant::TraeWork);
        assert!(
            no_logs.contains("日志目录不存在"),
            "缺 logs/ 时应提示先启动客户端：{no_logs}"
        );

        // 无 iCube 记录时必须换另一套说法（可能没登录）。
        std::fs::remove_file(global.join("storage.json")).unwrap();
        let no_auth = diagnose_missing_credential(&root, TraeVariant::TraeWork);
        assert!(
            no_auth.contains("未在存储中发现"),
            "无认证记录时应提示未登录：{no_auth}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 诊断标签必须严格等于**传入的变体**，不受全局探测影响。
    ///
    /// 这是 Bug 1 的文案护栏：旧实现调全局 `detected_variant()`，
    /// 于是「在 Trae Work 分区导入失败」会显示成 Trae CN（本机 CN 更活跃）。
    #[test]
    fn diagnose_missing_credential_label_follows_passed_variant_not_global_probe() {
        let root = std::env::temp_dir().join(format!("trae-diag-label-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        // 同一个目录、同一个 data_dir，只有变体不同 → 标签必须跟着变体走。
        let work = diagnose_missing_credential(&root, TraeVariant::TraeWork);
        assert!(
            work.contains(&format!("【{}】", TraeVariant::TraeWork.display_name())),
            "应显示传入变体的展示名 Trae Work：{work}"
        );
        assert!(
            !work.contains(&format!("【{}】", TraeVariant::TraeCn.display_name())),
            "不得出现另一条产品线的标签：{work}"
        );

        let cn = diagnose_missing_credential(&root, TraeVariant::TraeCn);
        assert!(
            cn.contains(&format!("【{}】", TraeVariant::TraeCn.display_name())),
            "应显示传入变体的展示名 Trae CN：{cn}"
        );
        assert!(
            !cn.contains(&format!("【{}】", TraeVariant::TraeWork.display_name())),
            "不得出现另一条产品线的标签：{cn}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **Bug 1 的核心回归护栏**：两条产品线目录都存在、且另一条更「活跃」时，
    /// `extract_local_jwt_for(TraeWork)` 必须**只读 TraeWork 的目录**，绝不回落到 Trae CN。
    ///
    /// 复现原始缺陷的关键在于「让 Trae CN 更活跃」—— 旧实现走跨变体的
    /// `detect_data_dir()`，会按最近活跃挑中 Trae CN，于是 TraeWork 分区的导入
    /// 读到了 Trae CN 的凭据。这里故意把 `Trae CN` 的 storage.json 造得更新。
    #[cfg(windows)]
    #[test]
    fn extract_local_jwt_for_reads_only_the_requested_variant() {
        // Trae Work 目录里的凭据（uid 归属 A）。
        let work_exp = chrono::Utc::now().timestamp() + 3600;
        let work_payload = format!(r#"{{"exp":{work_exp},"data":{{"id":"1111111111111111"}}}}"#);
        let work_token = format!(
            "{}.{}.{}",
            base64url(r#"{"alg":"RS256","typ":"JWT"}"#),
            base64url(&work_payload),
            "d29ya3NpZw"
        );

        // Trae CN 目录里的另一套凭据（uid 归属 B）——它会被造得更活跃。
        let cn_exp = chrono::Utc::now().timestamp() + 7200;
        let cn_payload = format!(r#"{{"exp":{cn_exp},"data":{{"id":"2222222222222222"}}}}"#);
        let cn_token = format!(
            "{}.{}.{}",
            base64url(r#"{"alg":"RS256","typ":"JWT"}"#),
            base64url(&cn_payload),
            "Y25zaWc"
        );

        let base = std::env::temp_dir().join(format!("trae-variant-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);

        let work_dir = base.join("TRAE SOLO CN");
        let work_log = work_dir
            .join("logs")
            .join("20260101T000000")
            .join("window1")
            .join("exthost")
            .join("trae.ai-code-completion")
            .join("completion.log");
        std::fs::create_dir_all(work_log.parent().unwrap()).unwrap();
        std::fs::write(
            &work_log,
            format!(r#"request: headers: {{"Authorization":"Cloud-IDE-JWT {work_token}"}}"#),
        )
        .unwrap();

        let cn_dir = base.join("Trae CN");
        let cn_log = cn_dir
            .join("logs")
            .join("20260101T000000")
            .join("window1")
            .join("exthost")
            .join("trae.ai-code-completion")
            .join("completion.log");
        std::fs::create_dir_all(cn_log.parent().unwrap()).unwrap();
        std::fs::write(
            &cn_log,
            format!(r#"request: headers: {{"Authorization":"Cloud-IDE-JWT {cn_token}"}}"#),
        )
        .unwrap();

        // 故意把 Trae CN 造得「更活跃」：更新的 storage.json（`data_dir_activity` 的判定依据）。
        let _lock = crate::modules::config::env_lock();
        let original = std::env::var("APPDATA").ok();
        std::env::set_var("APPDATA", &base);
        std::thread::sleep(std::time::Duration::from_millis(40));
        let cn_storage = cn_dir.join("User").join("globalStorage").join("storage.json");
        std::fs::create_dir_all(cn_storage.parent().unwrap()).unwrap();
        std::fs::write(&cn_storage, r#"{"telemetry":{}}"#).unwrap();

        let work_result = extract_local_jwt_for(TraeVariant::TraeWork);
        let cn_result = extract_local_jwt_for(TraeVariant::TraeCn);

        match original {
            Some(value) => std::env::set_var("APPDATA", value),
            None => std::env::remove_var("APPDATA"),
        }
        let _ = std::fs::remove_dir_all(&base);

        // 无论另一条产品线多活跃，TraeWork 请求都只能取到 TraeWork 的凭据。
        let (work_uid, work_found) = work_result.expect("应取到 Trae Work 目录里的凭据");
        assert_eq!(work_uid, "1111111111111111", "不得回落到 Trae CN 的凭据");
        assert_eq!(work_found, work_token);

        // 反向也成立：TraeCn 请求只取自己目录里的凭据。
        let (cn_uid, cn_found) = cn_result.expect("应取到 Trae CN 目录里的凭据");
        assert_eq!(cn_uid, "2222222222222222");
        assert_eq!(cn_found, cn_token);
    }

    /// 该变体一个候选目录都不存在时，错误必须**指向该变体**，而不是含糊的「未检测到」。
    #[cfg(windows)]
    #[test]
    fn extract_local_jwt_for_reports_variant_specific_missing_dir() {
        // 空 base：两条产品线的目录都不存在。
        let base = std::env::temp_dir().join(format!("trae-variant-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let _lock = crate::modules::config::env_lock();
        let original = std::env::var("APPDATA").ok();
        std::env::set_var("APPDATA", &base);

        let error = extract_local_jwt_for(TraeVariant::TraeWork)
            .expect_err("两条候选目录都不存在时必须报错");
        let cn_error = extract_local_jwt_for(TraeVariant::TraeCn)
            .expect_err("两条候选目录都不存在时必须报错");

        match original {
            Some(value) => std::env::set_var("APPDATA", value),
            None => std::env::remove_var("APPDATA"),
        }
        let _ = std::fs::remove_dir_all(&base);

        assert!(
            error.contains(TraeVariant::TraeWork.display_name()),
            "报错必须点名 Trae Work：{error}"
        );
        assert!(
            cn_error.contains(TraeVariant::TraeCn.display_name()),
            "报错必须点名 Trae CN：{cn_error}"
        );
    }

    /// 快照目录必须按变体分家，且**默认变体沿用旧目录名**（老用户快照零失效）。
    ///
    /// 反例（改坏会红）：若 `profiles_dir_for` 忽略变体、一律返回 `profiles/`，
    /// 则两条产品线的快照会互相可见 —— 更糟的是「恢复」会把一条产品线的登录态
    /// 灌进另一条产品线的客户端 userData。
    #[test]
    fn profile_paths_split_by_variant_and_default_reuses_legacy_name() {
        // 只断言 basename：这些是无参全局路径函数，每次调用都重读进程级
        // `BUDDY_SWITCH_HOME`。断言绝对路径会被并行跑的其它用例改 home 而"假红"
        // （症状是左右目录名差一个 PID 后缀），只比 basename 天然免疫。
        let work = paths::profiles_dir_for(TraeVariant::TraeWork);
        let cn = paths::profiles_dir_for(TraeVariant::TraeCn);

        assert_eq!(
            work.file_name().and_then(|n| n.to_str()),
            Some("profiles"),
            "默认变体必须沿用旧目录名，否则老用户的既有快照全部失效"
        );
        assert_eq!(
            cn.file_name().and_then(|n| n.to_str()),
            Some("profiles_trae_cn"),
            "非默认变体用独立目录名"
        );
        assert_ne!(work, cn, "两条产品线的快照目录绝不该是同一个");

        // 单个账号的快照目录同样分家。
        assert_ne!(
            paths::profile_dir_for(TraeVariant::TraeWork, "u1"),
            paths::profile_dir_for(TraeVariant::TraeCn, "u1"),
            "同一个 uid 在两条产品线下必须是两个槽位"
        );
    }

    /// 「当前活跃账号」是**每条产品线各自的事实**，不能共用一份。
    ///
    /// 反例（改坏会红）：`current_account_for` 忽略变体 → 两条线互相冒充，
    /// UI 上会把 A 线的当前账号显示成 B 线的。
    #[test]
    fn current_account_is_tracked_per_variant() {
        let dir = std::env::temp_dir().join(format!("trae-cur-acct-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _guard = crate::modules::config::HomeOverrideGuard::set(&dir);

        set_current_account_for(TraeVariant::TraeWork, "u-work").unwrap();
        set_current_account_for(TraeVariant::TraeCn, "u-cn").unwrap();

        assert_eq!(current_account_for(TraeVariant::TraeWork).as_deref(), Some("u-work"));
        assert_eq!(current_account_for(TraeVariant::TraeCn).as_deref(), Some("u-cn"));

        // 覆盖其中一条不得影响另一条。
        set_current_account_for(TraeVariant::TraeCn, "u-cn-2").unwrap();
        assert_eq!(
            current_account_for(TraeVariant::TraeWork).as_deref(),
            Some("u-work"),
            "改 Trae CN 的当前账号把 Trae Work 的也改了 —— 这就是串味"
        );
    }

    /// 快照列表必须按变体分家：A 线写的槽位，B 线列不出来。
    #[test]
    fn list_profiles_is_scoped_to_the_variant() {
        let dir = std::env::temp_dir().join(format!("trae-list-prof-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _guard = crate::modules::config::HomeOverrideGuard::set(&dir);

        // 只往 Trae CN 的目录里放一个槽位。
        let cn_slot = paths::profiles_dir_for(TraeVariant::TraeCn).join("cn-only");
        std::fs::create_dir_all(&cn_slot).unwrap();
        std::fs::write(cn_slot.join("marker.txt"), b"x").unwrap();

        let cn_slots: Vec<String> = list_profiles_for(TraeVariant::TraeCn)
            .into_iter()
            .map(|info| info.slot)
            .collect();
        let work_slots: Vec<String> = list_profiles_for(TraeVariant::TraeWork)
            .into_iter()
            .map(|info| info.slot)
            .collect();

        assert!(cn_slots.contains(&"cn-only".to_string()), "Trae CN 应看到自己的槽位");
        assert!(
            !work_slots.contains(&"cn-only".to_string()),
            "Trae Work 看到了 Trae CN 的槽位 —— 这就是串味"
        );
    }

    /// `overview` 的每个字段都必须取自**传入变体**（不能只有槽位列表分家）。
    #[test]
    fn overview_is_scoped_to_the_variant() {
        let dir = std::env::temp_dir().join(format!("trae-overview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _guard = crate::modules::config::HomeOverrideGuard::set(&dir);

        set_current_account_for(TraeVariant::TraeCn, "u-cn").unwrap();
        let cn_slot = paths::profiles_dir_for(TraeVariant::TraeCn).join("u-cn");
        std::fs::create_dir_all(&cn_slot).unwrap();

        let cn = overview_for(TraeVariant::TraeCn);
        let work = overview_for(TraeVariant::TraeWork);

        assert_eq!(cn.get("currentAccount").and_then(Value::as_str), Some("u-cn"));
        assert_eq!(
            work.get("currentAccount").and_then(Value::as_str),
            None,
            "Trae Work 的当前账号是空的，不该读到 Trae CN 的"
        );
        assert_eq!(
            cn.get("profiles").and_then(Value::as_array).map(Vec::len),
            Some(1)
        );
        assert_eq!(
            work.get("profiles").and_then(Value::as_array).map(Vec::len),
            Some(0)
        );
        // `dataDir` 也必须是该变体的视角。
        assert_ne!(
            cn.get("dataDir").and_then(Value::as_str),
            work.get("dataDir").and_then(Value::as_str),
            "两条产品线的客户端数据目录本就不同，overview 不该共用"
        );
    }

    /// 客户端打包后的 JS（模板串 `Cloud-IDE-JWT ${e}`）不得被误当成凭据。
    #[test]
    fn scan_jwt_tokens_ignores_js_templates() {        let js = r#"if(n.headers={...t.headers,Authorization:`Cloud-IDE-JWT ${e}`},null==a)"#;
        assert!(
            scan_jwt_tokens(js).is_empty(),
            "JS 模板串被误判成 token：{:?}",
            scan_jwt_tokens(js)
        );
    }

    fn base64url(input: &str) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let bytes = input.as_bytes();
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(TABLE[(n >> 18) as usize & 63] as char);
            out.push(TABLE[(n >> 12) as usize & 63] as char);
            if chunk.len() > 1 {
                out.push(TABLE[(n >> 6) as usize & 63] as char);
            }
            if chunk.len() > 2 {
                out.push(TABLE[n as usize & 63] as char);
            }
        }
        out
    }

    #[test]
    fn format_size_is_human_readable() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(2048), "2.0 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024), "3.00 GB");
    }

    #[test]
    fn switch_rejects_unsafe_or_missing_target_without_touching_client() {
        // 关键护栏：目标非法/快照缺失时必须在「关闭客户端」之前就失败，
        // 不能留下「客户端已关但没恢复」的中间态。
        let options = SwitchOptions {
            user_id: "../escape".into(),
            ..Default::default()
        };
        let outcome = switch_account(&options, |_| {});
        assert!(!outcome.success);
        assert!(outcome.error.is_some());
        let stages: Vec<&str> = outcome.steps.iter().map(|s| s.stage).collect();
        assert!(stages.contains(&"fatal"));
        assert!(!stages.contains(&"stop"), "不应关闭客户端: {stages:?}");
        assert!(!stages.contains(&"restore"));
    }

    #[test]
    fn switch_reports_missing_snapshot_as_fatal() {
        let options = SwitchOptions {
            user_id: "definitely_no_such_account_slot".into(),
            launch: true,
            proxy_port: None,
            reset_device: false,
            variant: TraeVariant::default(),
        };
        let outcome = switch_account(&options, |_| {});
        assert!(!outcome.success);
        assert_eq!(outcome.error.as_deref(), Some("目标快照不存在"));
        let stages: Vec<&str> = outcome.steps.iter().map(|s| s.stage).collect();
        // 预检查失败 → 直接 fatal，不进入停止/恢复流程
        assert_eq!(stages.first().copied(), Some("fatal"));
        assert!(!stages.contains(&"stop"));
    }

    #[test]
    fn safe_slot_rejects_traversal_slots() {
        for bad in ["..", "../x", "a/b", "", "a\\b"] {
            assert!(!paths::safe_slot_name(bad), "{bad} 不应通过校验");
        }
        assert_eq!(delete_slot("../x").unwrap_err().contains("非法"), true);
    }

    #[test]
    fn delete_slot_is_idempotent() {
        // 不存在的槽位删除应当成功（幂等），而不是报错。
        let slot = format!("__nonexistent_slot_{}", std::process::id());
        assert!(delete_slot(&slot).is_ok());
        assert!(delete_slot(&slot).is_ok());
    }

    #[test]
    fn copy_dir_recursive_copies_nested_tree() {
        let base = std::env::temp_dir().join(format!("trae-copy-{}", std::process::id()));
        let source = base.join("src");
        let target = base.join("dst");
        std::fs::create_dir_all(source.join("nested")).unwrap();
        std::fs::write(source.join("a.txt"), b"a").unwrap();
        std::fs::write(source.join("nested").join("b.txt"), b"b").unwrap();

        let copied = copy_dir_recursive(&source, &target).unwrap();
        assert_eq!(copied, 2);
        assert!(target.join("a.txt").is_file());
        assert!(target.join("nested").join("b.txt").is_file());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn copy_entry_replaces_existing_directory_instead_of_merging() {
        // 目录类条目必须整体替换：合并会把目标账号的残留凭据留在快照里。
        let base = std::env::temp_dir().join(format!("trae-replace-{}", std::process::id()));
        let source = base.join("src");
        let target = base.join("dst");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(source.join("new.txt"), b"n").unwrap();
        std::fs::write(target.join("stale.txt"), b"s").unwrap();

        copy_entry(&source, &target, EntryKind::Dir).unwrap();
        assert!(target.join("new.txt").is_file());
        assert!(
            !target.join("stale.txt").exists(),
            "旧文件必须被整体替换掉"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn copy_entry_skips_missing_source_without_error() {
        let base = std::env::temp_dir().join(format!("trae-missing-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&base);
        let copied = copy_entry(
            &base.join("nope.txt"),
            &base.join("out.txt"),
            EntryKind::File,
        )
        .unwrap();
        assert_eq!(copied, 0);
        assert!(!base.join("out.txt").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn switch_step_json_is_camel_case_and_shaped_for_ndjson() {
        let step = SwitchStep::new("restore", "ok", "已恢复");
        let value = step.to_json();
        assert_eq!(value.get("stage").unwrap().as_str(), Some("restore"));
        assert_eq!(value.get("status").unwrap().as_str(), Some("ok"));
        assert!(value.get("time").is_some());
        assert!(value.get("message").is_some());
    }

    #[test]
    fn profile_info_json_exposes_size_text() {
        let info = ProfileInfo {
            slot: "123".into(),
            size_bytes: 2048,
            file_count: 3,
            last_modified: "2026-01-01 00:00:00".into(),
        };
        let value = info.to_json();
        assert_eq!(value.get("sizeBytes").unwrap().as_u64(), Some(2048));
        assert_eq!(value.get("sizeText").unwrap().as_str(), Some("2.0 KB"));
        assert_eq!(value.get("fileCount").unwrap().as_u64(), Some(3));
        assert!(value.get("size_bytes").is_none());
    }

    #[test]
    fn overview_never_panics() {
        let value = overview();
        for key in [
            "profiles",
            "currentAccount",
            "dataDir",
            "clientRunning",
            "coreEntryCount",
        ] {
            assert!(value.get(key).is_some(), "缺少字段 {key}");
        }
        assert!(value.get("profiles").unwrap().is_array());
    }

    #[test]
    fn scan_extracts_prefixed_jwt_from_json_text() {
        let text = r#"{"trae.auth":"Cloud-IDE-JWT eyJhbGciOiJIUzI1NiJ9.eyJkYXRhIjp7ImlkIjoiNzUxMjM0NTY3ODkwMTIzNDU2NyJ9fQ.sig"}"#;
        let tokens = scan_jwt_tokens(text);
        assert_eq!(tokens.len(), 1);
        assert!(tokens[0].starts_with("eyJhbGciOiJIUzI1NiJ9."));
    }

    #[test]
    fn scan_tolerates_binary_noise_and_dedupes() {
        // 模拟 SQLite 二进制内容：JWT 前后有非 UTF-8 字节，且同一 token 出现两次。
        let token = "header111.payload222.signature333";
        let text = format!(
            "\u{fffd}\u{0}Cloud-IDE-JWT {token}\u{1}\u{2}again Cloud-IDE-JWT {token}",
        );
        let tokens = scan_jwt_tokens(&text);
        assert_eq!(tokens, vec![token.to_string()], "同 token 必须去重");
    }

    #[test]
    fn scan_ignores_incomplete_tokens() {
        // 只有两段的半截匹配、以及没有点的噪声，都必须被过滤。
        let text = "Cloud-IDE-JWT only.two Cloud-IDE-JWT nodots Cloud-IDE-JWT a.b.c";
        let tokens = scan_jwt_tokens(text);
        assert_eq!(tokens, vec!["a.b.c".to_string()]);
    }

    #[test]
    fn scan_stops_token_at_non_base64_boundary() {
        // token 之后紧跟引号/逗号时，不能把引号吞进 token。
        let text = r#"k":"Cloud-IDE-JWT aaa.bbb.ccc","next":1"#;
        assert_eq!(scan_jwt_tokens(text), vec!["aaa.bbb.ccc".to_string()]);
    }
}
