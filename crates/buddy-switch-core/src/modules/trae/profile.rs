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

use crate::modules::trae::icube;
use crate::modules::trae::jwt;
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
///
/// ## ★ 切换不变式（**加新条目/新来源前必读**）
///
/// > **切换完成后，任何「未参与本清单快照」的凭据来源，都不得残留上一账号的内容。**
///
/// 本清单是**白名单**，所以每漏一个凭据来源，切换就会把它留在原地、带进下一个账号。
/// 两类已知来源按不同方式满足这条不变式，**两条都不能省**：
///
/// 1. **本清单内的条目**：`restore_from_slot_for` 用 `copy_entry` 覆盖，
///    目录类条目还会先 `remove_dir_all`（见 [`copy_entry`]）—— 靠**覆盖**满足；
/// 2. **清单外的凭据来源**：`restore_from_slot_for` 逐个**主动清除** ——
///    见 [`RESTORE_PURGE_RELATIVES`]。目前有两项：
///    - `logs/`：`extract_local_jwt_for` 会扫
///      `logs/**/trae.ai-code-completion/completion.log` 里的明文 JWT。
///      不清 ⇒ 切到 A 之后导入仍读到 B 的 token，症状就是「切换后账号不变」。
///    - SQLite 边车文件（`-wal` / `-shm` / `-journal`）：不删会让 SQLite
///      下次打开时**回放旧事务**，把上一账号的页写回刚恢复的库里，症状同上。
///
/// **因此：新增任何凭据来源时，要么把它加进本清单，要么加进清除清单。**
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

/// 恢复快照前**主动清除**的「清单外的凭据来源」（相对 `<客户端 userData>`）。
///
/// 存在的唯一理由是 [`CORE_ENTRIES`] 的**切换不变式**：
/// 任何未参与快照的凭据来源都不得残留上一账号的内容。这些来源刻意**不进快照**
/// （体积与副作用都不划算），所以必须靠清除来满足不变式。
///
/// | 相对路径 | 为什么必须清 | 不清的后果 |
/// |:--|:--|:--|
/// | `logs` | [`extract_local_jwt_for`] 会扫其中的 `completion.log` 明文 JWT | 切到 A 后导入仍读到 B 的 token（症状：**切换后账号不变**） |
/// | `User/globalStorage/state.vscdb-wal` 等 | SQLite 会在下次打开时**回放**这些文件里的事务 | 旧事务把上一账号的页写回刚恢复的库（症状同上） |
///
/// 目录条目按「整目录删除」处理，文件条目按「单文件删除、不存在即跳过」处理。
const RESTORE_PURGE_RELATIVES: &[&str] = &[
    // ── 明文凭据来源：客户端扩展日志（跨账号累积，且不在快照内） ──
    "logs",
    // ── SQLite 边车文件：三件套都要清，缺一个就会回放 ──
    // 主库 `state.vscdb` 本身由 `CORE_ENTRIES` 覆盖，这里只处理它的附属文件。
    "User/globalStorage/state.vscdb-wal",
    "User/globalStorage/state.vscdb-shm",
    "User/globalStorage/state.vscdb-journal",
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
/// ## 来源顺序（主来源 + 兜底）
///
/// **主来源**是 `storage.json` 里 `iCubeAuthInfo://icube.cloudide` 的 tc 信封
/// （见 [`icube_login_candidate`]）—— 它是**两条产品线都有**、且可解密的来源。
/// 下面描述的只是**兜底**那一半，只在主来源不可用时才会走到。
///
/// ## 兜底扫哪些文件（两处，不能只扫第一处）
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
///
/// ## 来源优先级（**顺序不可调换**）
///
/// | 序 | 来源 | 覆盖范围 | 实现 |
/// |:--|:--|:--|:--|
/// | 1 | `storage.json` 的 `iCubeAuthInfo://icube.cloudide` **tc 信封** | **两条产品线都有** | [`icube_login_candidate`] |
/// | 2 | `storage.json` / `state.vscdb` 明文 + 扩展日志 | 只有装了 `trae.ai-code-completion` 的产品线 | [`collect_log_candidates`] |
///
/// **为什么主来源必须是 tc 信封**：`TRAE SOLO CN`（Trae Work）实测 355 个日志文件、
/// **0** 个 `completion.log`、0 处 `Cloud-IDE-JWT` —— 明文来源在它身上**根本不存在**，
/// 只扫明文等于「Trae Work 永远导入不了」，用户看到的是「明明登录了却识别不到」。
/// 而它的凭据一直都在，只是躺在加密信封里：**不是提不出，是找错了地方**。
///
/// **为什么 tc 优先于明文，而不是「两边取 `exp` 最大者」**：明文来源是**历史累积**的
/// （`logs/` 会跨账号留存，见 [`restore_from_slot_for`] 的不变式），tc 信封才是客户端
/// **当前**的登录态。若按 `exp` 取最大，切换账号后残留的上一账号日志可能胜出
/// ⇒ 导入到错账号，症状正是「切换后账号不变」。
///
/// ## 返回值的形态约定
///
/// `Ok((uid, header_value))` 的第二个值**恒为完整请求头值**（含 `Cloud-IDE-JWT ` 前缀），
/// 与 OAuth 路径落库的形态一致（`account.rs` 里的 `jwt::authorization_header`）。
/// 调用方**不得**再自行拼前缀，也**不得**把裸 token 当完整头值落库。
pub fn extract_local_jwt_for(variant: TraeVariant) -> Result<(String, String), String> {
    let data_dir = platform::select_data_dir_for(variant).ok_or_else(|| {
        format!(
            "未找到【{}】的数据目录，请先启动一次该客户端并登录",
            variant.display_name()
        )
    })?;
    extract_local_jwt_from_dir(&data_dir, variant)
}

/// 从**指定数据目录**读凭据（显式入参）。
///
/// ## 为什么要有这个「显式目录」的形态
///
/// 本模块有**两个**目录选择器，语义不同，**在同一台机器上可能给出不同目录**：
///
/// | 选择器 | 语义 | 谁在用 |
/// |:--|:--|:--|
/// | [`platform::select_data_dir_for`] | **最近活跃**的候选 | 导入（`extract_local_jwt_for`） |
/// | [`platform::detect_data_dir_for`] | **首位**候选（`names[0]`） | 备份 / 恢复 / 守卫要守护的那个操作 |
///
/// 实测（Trae Work，本机）：`select` 给 `TRAE SOLO`（客户端启动过、**从未登录**），
/// `detect` 给 `TRAE SOLO CN`（**登录态在这里**）。于是「用 `select` 校验、
/// 用 `detect` 操作」会出现两种坏法：
///
/// 1. **假阴性**：`select` 那个目录没有凭据 ⇒ 校验拿不到 uid ⇒ 守卫 fail-open
///    **静默放行**，等于没有守卫；
/// 2. **假阳性**（更坏）：两个目录各有登录态且**属于不同账号** ⇒ 校验读到 A 判定「就是 A」
///    ⇒ 放行，而操作从 `detect` 目录拷的是 B 的状态存进 A 的槽位 ——
///    守卫不但没拦住，还**为一次错误的保存盖了章**。
///
/// 所以：**校验的对象与操作的对象必须用同一个目录**。本函数把「读哪个目录」
/// 变成调用方的显式入参，让两件事能锁定同一个目录。
///
/// ⚠️ 两个入口**各取所需，不要合并**：导入必须读**活跃**目录（用户在活跃客户端里
/// 刚登录完就点导入，读首位候选会读到另一份旧登录态）；备份/恢复与守卫必须读
/// **首位候选**（那是既有语义，改它会动到切换行为）。共用的是这个原语，不是目录选择器。
fn extract_local_jwt_from_dir(
    data_dir: &Path,
    variant: TraeVariant,
) -> Result<(String, String), String> {
    // ── 主来源：iCube 登录态副本（tc 信封；Trae Work 唯一可用的来源） ──────────
    if let Some(found) = icube_login_candidate_from_dir(data_dir, variant)? {
        return Ok(found);
    }

    // ── 兜底：明文来源（旧版 storage.json / state.vscdb，新版只剩扩展日志） ──
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
    for path in collect_log_candidates(data_dir) {
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
    // 统一形态：明文来源捞出的是**裸** token（`scan_jwt_tokens` 已剥前缀），
    // 落库前补成完整请求头值 —— 与主来源、与 OAuth 路径三者一致。
    Ok((uid, crate::modules::trae::jwt::authorization_header(&token)))
}

/// 主来源：iCube 登录态副本（`storage.json` 的 `iCubeAuthInfo://icube.cloudide` tc 信封）。
///
/// 客户端把当前登录态加密写进**与设备凭证同一个 `storage.json`** 的另一个键，
/// 信封格式与设备凭证完全相同（见 [`crate::modules::trae::icube::tc_decrypt`]），
/// 解出来是 `{token, refreshToken, host, userId, expiredAt, …}`。
///
/// 目录是**显式入参**，由调用方决定（理由见 [`extract_local_jwt_from_dir`]）。
///
/// ## 三态返回值
///
/// - `Ok(Some((uid, header_value)))` —— 拿到可用凭据，`header_value` **已含**
///   `Cloud-IDE-JWT ` 前缀（信封里存的是裸 token，补前缀是本函数的职责）；
/// - `Ok(None)` —— 这条来源不可用（目录/键缺失、信封解不开、或凭据已过期），
///   调用方继续走明文兜底。**不在这里报错**：主来源缺失不代表导入该失败；
/// - `Err` —— 信封**可用**但归属解析不出。宁可报错，也不要往账号库落一条无主凭据。
///
/// ## 到期判定
///
/// 优先用信封的 `expiredAt`（客户端自己算好的 epoch 秒，比解 JWT 直接）；
/// 取不到时回落 JWT 的 `exp`；两者都没有时按「未知 = 可用」处理 ——
/// 与明文兜底路径的既有口径一致（只拦 `exp > 0 && exp < now`）。
fn icube_login_candidate_from_dir(
    data_dir: &Path,
    variant: TraeVariant,
) -> Result<Option<(String, String)>, String> {
    let Ok(info) = icube::cloudide_auth_info_from_dir(data_dir, variant) else {
        return Ok(None);
    };
    // 🔴 信封里是**裸** token（实测 1004 字符、三段、无前缀）。直接落库会让
    // 「账号库里的值」与 OAuth 路径落库的值形态不同，故此处统一补前缀。
    let header_value = jwt::authorization_header(&info.token);
    if jwt::normalize(&header_value).is_empty() {
        return Ok(None);
    }
    let exp = info
        .expired_at
        .or_else(|| jwt::parse(&header_value).exp_timestamp)
        .unwrap_or(0);
    if exp > 0 && exp < chrono::Utc::now().timestamp() {
        return Ok(None);
    }
    // `userId` 直接取（比从 JWT payload 猜更直接、且不依赖 payload 结构）；缺失时才解 token。
    let uid = info
        .user_id
        .filter(|value| !value.is_empty())
        .or_else(|| jwt::user_id_of(&header_value))
        .ok_or_else(|| "在 iCube 登录态副本中找到凭据，但无法确定账号归属".to_string())?;
    Ok(Some((uid, header_value)))
}

/// 找不到凭据时，产出**可操作**的诊断信息（而不是一句笼统的"没找到"）。
///
/// ## 为什么需要它（2026-09-18 实测成因）
///
/// 本机装有两条产品线，**明文**来源的覆盖差异极大：
///
/// | 产品线 | `storage.json` 明文 | 扩展日志明文（`completion.log`） |
/// |:--|:--|:--|
/// | `Trae CN` | 无（`iCubeAuthInfo://*` 是 iCube 自有加密，非 DPAPI） | 有（`trae.ai-code-completion` 扩展写） |
/// | `TRAE SOLO CN` | 无 | **无** —— 该产品线**没装** `trae.ai-code-completion` 扩展 |
///
/// 但**主来源不是明文**：两条产品线的 `storage.json` 都有
/// `iCubeAuthInfo://icube.cloudide` 的 tc 信封，那是唯一对 Trae Work 也成立、
/// 且可解密的来源（见 [`icube_login_candidate`]）。
/// 因此本函数只负责**兜底那一半**：走到这里说明主来源也没给出可用凭据
/// （信封缺失 / 解不开 / 已过期），于是必须说清「哪条产品线、主来源什么状态、
/// 明文来源缺什么」，并给出可行替代路径（OAuth 网页登录）——
/// 否则用户会反复重试导入，甚至去重装客户端。
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

    // 主来源（tc 信封）的状态。它才是 Trae Work 唯一可用的来源，所以「在不在、
    // 为什么用不上」必须出现在诊断里，否则用户会把「凭据过期」误判成「没登录」。
    // 只查键名、不解密，读的正是上面那个 `data_dir`，与 `device_note` 口径自洽。
    let envelope_note = if storage_has_key(data_dir, icube::CLOUDIDE_KEY) {
        "iCube 登录态副本键**存在**，但其中的凭据已过期或无法解密 —— \
         请在 Trae 中重新登录后重试。"
    } else {
        "storage.json 中没有 iCube 登录态副本键 —— 该客户端在此数据目录下可能从未登录过。"
    };

    // 设备身份（`icube-dc`）是**另一件事**：客户端首次启动就会写入，从未登录也存在。
    // 早先这里数的是全部 `iCubeAuthInfo://*` 键，于是「只有一个 `icube-dc`」被说成
    // 「该客户端确实已登录」—— 与上面那条 bullet 直接矛盾（真机 `TRAE SOLO` 实测如此）。
    // 现在它只陈述「客户端是否在此目录启动过」，不与登录态混为一谈。
    let device_note = if storage_device_entry_count(data_dir) > 0 {
        "检测到设备身份（`icube-dc`），说明客户端在此数据目录下启动过 —— \
         但**设备身份不代表登录过**，登录态请看上一条。"
    } else {
        "未检测到设备身份（`icube-dc`）—— 该客户端可能从未在此数据目录下启动过。"
    };

    format!(
        "未在【{label}】的数据目录（{}）中找到可用的登录凭据。\n\
         · {envelope_note}\n\
         · {log_note}\n\
         · {device_note}\n\
         请改用「OAuth 网页登录」——它不依赖本地文件，且能获得可自动续期的凭据。",
        data_dir.display()
    )
}

/// `storage.json` 里是否存在指定键（**只看键名，不解密**；读失败返回 `false`）。
///
/// 与 [`storage_device_entry_count`] 同源同风格：诊断专用，
/// 三种退化输入（文件缺失 / JSON 损坏 / 顶层非对象）一律返回 `false`，绝不 panic。
fn storage_has_key(data_dir: &Path, key: &str) -> bool {
    let path = data_dir
        .join("User")
        .join("globalStorage")
        .join("storage.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    value
        .as_object()
        .map(|map| map.contains_key(key))
        .unwrap_or(false)
}

/// 统计 `storage.json` 里 `iCubeAuthInfo://icube-dc:*` 条目数（仅用于诊断，读失败返回 0）。
///
/// 不解析内容：这些值由 iCube 自有格式加密，我们只关心「客户端是否在此目录启动过」。
///
/// **刻意不数** `iCubeAuthInfo://icube.cloudide`（登录态副本）—— 那是另一件事，
/// 由 [`storage_has_key`] 单独回答。两者混在一个计数里，就会出现
/// 「只有一个设备身份」被解读成「已经登录」的错误结论。
fn storage_device_entry_count(data_dir: &Path) -> usize {
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
                .filter(|key| key.starts_with(icube::ICUBE_DC_PREFIX))
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

/// 快照类操作（备份 / 恢复 / 保存守卫 / 恢复后复核）读取的**唯一取值点**。
///
/// ## 为什么必须唯一取值点（I-4 同源不变式）
///
/// 不变式：**校验的输入必须取自被校验操作将要作用的那个对象。**
/// 快照类操作的作用对象是「客户端 userData 目录」——`backup_to_slot_for` 从它复制、
/// `restore_from_slot_for` 向它写入、`ensure_save_target_matches_client` 从它取证。
/// 它们必须指向**同一个目录**。
///
/// 本模块有**两个**语义不同的目录选择器，同一台机器上可能给出不同目录：
///
/// | 选择器 | 语义 |
/// |:--|:--|
/// | [`platform::select_data_dir_for`] | 该变体候选里**最近活跃**的那个（导入用） |
/// | [`platform::detect_data_dir_for`] | 该变体**首位候选**（`names[0]`） |
///
/// 若调用点各自去调选择器，任一处被换成另一个（例如 `select_data_dir_for`）
/// 都会产生「**校验读了 A、操作改了 B**」，且表现是**静默**的：守卫在真机上等于不存在
/// （假阴性），或复核把正常切换误报成失败（读错目录）。改动是局部的，评审时看不出来。
///
/// 收敛到这一个函数后，调用点只表达「我要快照目录」，
/// **选择器策略的变更只需改这一处**。调用点**不得**再各自调用 `detect_data_dir_for`。
///
/// ## 本函数只负责「取路径」，不判断存在性
///
/// 是否要求目录存在由**调用方显式声明**，因为两个操作的既有语义不同：
///
/// - [`backup_to_slot_for`] 要求源目录存在（不存在 ⇒ 报「未找到客户端数据目录」）；
/// - [`restore_from_slot_for`] **不要求**目标目录存在（会 `create_dir_all` 新建）。
///
/// 把这一差异留在调用点（`.filter(|dir| dir.is_dir())`），是为了让「谁要求存在」
/// 在代码里一眼可见，而不是被取值点悄悄统一掉（那会改变 restore 的既有行为）。
///
/// ## 取值点唯一 ≠ 构造同源：切换链还要**显式传值**
///
/// 把取值点收敛到一处，只消除了「同一份逻辑里两处各自取目录」；**同一份值**仍可能
/// 被取两次而只是**今天恰好相等**（「**约定同源**」）。[`switch_account`] 的
/// 「恢复 → 复核」链要求更强：目录算**一次**，由调用方把**同一个值**分别交给
/// 写入（[`restore_from_slot_in_dir`]）与复核（[`verify_restored_login_in`]）
/// —— 即「**构造同源**」，两者**在类型层面**是同一个值、无法分叉。
/// 复核**不得**自行再调本函数。
fn snapshot_data_dir_for(variant: TraeVariant) -> Option<PathBuf> {
    platform::detect_data_dir_for(variant)
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
    // 快照源目录：唯一取值点。**要求存在**（不存在时下面的 `ok_or` 给出可操作提示）。
    let source_root = snapshot_data_dir_for(variant)
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
///
/// ## 两步，顺序不可颠倒
///
/// 1. **先清「清单外的凭据来源」**（[`RESTORE_PURGE_RELATIVES`]）；
/// 2. **再用快照覆盖「清单内条目」**（[`CORE_ENTRIES`]）。
///
/// 先清后覆盖有两个理由：一是覆盖期间旧日志/旧 WAL 不应与新库并存；
/// 二是将来若有条目同时出现在两份清单里，**快照内容应当胜出**。
/// 不变式的完整说明见 [`CORE_ENTRIES`]。
pub fn restore_from_slot_for(variant: TraeVariant, slot: &str) -> Result<u64, String> {
    // 目标目录取「快照类操作的唯一取值点」；**不要求存在** —— 委托方会 `create_dir_all` 新建。
    let target_root = snapshot_data_dir_for(variant).ok_or("无法定位 Trae 客户端数据目录")?;
    restore_from_slot_in_dir(&target_root, variant, slot)
}

/// 把指定槽位的快照恢复到**指定目标目录**（显式目录；构造同源）。
///
/// ## 与 [`restore_from_slot_for`] 的关系
///
/// 后者是「取目录 + 委托本函数」的薄封装（公开签名与行为均不变）。本函数把目标目录
/// 变成**显式入参**，使调用方能把它**同一个值**同时交给「写入」与「复核」，
/// 而不是让两边各自再推导一次。
///
/// ## 为什么必须是显式入参（I-4 同源不变式）
///
/// 不变式：**校验的输入必须取自被校验操作将要作用的那个对象。**
/// 恢复后的复核要读「刚被写入的那个目录」；若复核自己去调一次取值点，
/// 就只是「**约定同源**」——两次调用**今天恰好同值**，一旦取值点被改成依赖
/// 运行时状态（活跃度、环境变量等），写入与复核就会**静默分叉**：
/// 复核读到另一个目录，把正常切换误报成失败（或反过来盖章）。
/// 显式传参是「**构造同源**」：两者**在类型层面**就是同一个值，无法分叉。
///
/// ## 行为
///
/// 与原先的 [`restore_from_slot_for`] 函数体逐条一致，只把目标目录的来源换成入参：
/// 先校验槽位名 → 确认快照存在 → `create_dir_all(target_root)` → 清「清单外的凭据来源」
/// → 覆盖清单内条目 → 清单实例锁 → 失败项写日志。
fn restore_from_slot_in_dir(
    target_root: &Path,
    variant: TraeVariant,
    slot: &str,
) -> Result<u64, String> {
    if !paths::safe_slot_name(slot) {
        return Err(format!("非法的槽位名: {slot}"));
    }
    let source_root = paths::profiles_dir_for(variant).join(slot);
    if !source_root.is_dir() {
        return Err(format!("槽位 {slot} 的登录态快照不存在"));
    }
    std::fs::create_dir_all(target_root)
        .map_err(|e| format!("创建客户端数据目录失败: {e}"))?;

    let unpurged = purge_restore_relatives(target_root);

    let mut restored = 0u64;
    for entry in CORE_ENTRIES {
        restored += copy_entry(
            &entry.resolve(&source_root),
            &entry.resolve(target_root),
            entry.kind,
        )?;
    }

    // 客户端单实例锁残留会让下次启动直接退出（Electron 常见困局），
    // 恢复后一并清掉，代价极小。
    let _ = std::fs::remove_file(target_root.join("code.lock"));

    // 不变式被破坏时必须留痕（不阻断切换，理由见 `purge_restore_relatives`）。
    if !unpurged.is_empty() {
        store::append_log(
            &paths::switcher_log_file_for(variant),
            &format!(
                "恢复快照时未能清除 {} 项「清单外的凭据来源」：{}。\
                 这些来源可能仍残留上一账号的内容，下次切换前请先关闭客户端再试",
                unpurged.len(),
                unpurged.join("、")
            ),
        );
    }

    Ok(restored)
}

/// 清除 [`RESTORE_PURGE_RELATIVES`] 列出的「清单外的凭据来源」。
///
/// 返回**未能清除**的项（`"<相对路径>（<原因>）"`），全部清干净时返回空表。
///
/// ## 为什么是「尽力而为」而不是硬失败
///
/// Windows 上 `taskkill /F` 之后文件句柄不一定立刻释放（`platform::kill_client_for`
/// 的 Windows 分支不会等待），此时删 `logs/` 下的个别文件会失败。
/// 让整次切换因此失败**比残留一份日志更糟**：用户会卡在「切不了」，
/// 而且没有任何替代路径（恢复已经被拒绝，客户端也已经被关掉了）。
///
/// 但**不静默**：调用方把失败项写进 `switcher.log`，
/// 使「不变式被破坏」这件事始终有人知道。
/// 这与本模块既有的取舍一致（见 [`copy_dir_recursive`] 对单文件失败的容忍）。
fn purge_restore_relatives(target_root: &Path) -> Vec<String> {
    let mut failed = Vec::new();
    for relative in RESTORE_PURGE_RELATIVES {
        let path = relative
            .split('/')
            .fold(target_root.to_path_buf(), |acc, part| acc.join(part));
        // 不存在 ⇒ 不变式已满足，静默跳过（不是错误）。
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else if path.is_file() {
            std::fs::remove_file(&path)
        } else {
            continue;
        };
        if let Err(error) = result {
            failed.push(format!("{relative}（{error}）"));
        }
    }
    failed
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
    //
    // ★ 不变式：**凡把「客户端当前状态」写入「账号槽位」的路径，都必须过守卫；
    //   `LAST_SLOT`（回滚槽）是唯一豁免。**
    //
    // 本步写的是**账号槽位**（`previous` 是 userId），且写入是**覆盖**式的：
    // `LAST_SLOT` 里已经存了「切换前的现场」，`previous` 原本可能正确的旧快照一旦被
    // 未登录态覆盖就**不可逆**了（回滚槽救不回来）。所以这一步必须先过
    // `ensure_save_target_matches_client`，失败时**降级为 skip**、不影响后续切换。
    //
    // 注意 `LAST_SLOT` 的豁免理由：它的语义就是「切换前的现场」，必须允许在客户端
    // 未登录时也照旧写入，否则回滚能力就没了（见第 2 步与守卫的文档）。
    if let Some(previous) = previous_account.as_deref().filter(|uid| *uid != target) {
        match ensure_save_target_matches_client(variant, previous) {
            Err(error) => emit(
                &mut outcome,
                SwitchStep::new(
                    "backup-current",
                    "skip",
                    format!("跳过更新 {previous} 的快照：{error}"),
                ),
            ),
            Ok(()) => match backup_to_slot_for(variant, previous) {
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
                    SwitchStep::new(
                        "backup-current",
                        "skip",
                        format!("更新 {previous} 失败: {error}"),
                    ),
                ),
            },
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
    //
    // ★ 目标目录在这里**只算一次**，并把它**显式传给**第 6 步（写入）与第 6.5 步（复核）。
    //   这是 I-4 同源不变式要求的**构造同源**：写入与复核共用同一个值，
    //   而不是各自再去取一次目录（「约定同源」——两次取值今天恰好同值，明天可能分叉）。
    //
    //   位置刻意留在第 5 步（关客户端）**之后**：accessor 取不到目录时，用户看到的
    //   失败步骤与文案必须与「由 `restore_from_slot_for` 内部报错」时**逐字一致**
    //   （stage=fatal / status=fail / 「恢复失败: 无法定位 Trae 客户端数据目录」）。
    let restore_dir = match snapshot_data_dir_for(variant) {
        Some(dir) => dir,
        None => {
            let message = "无法定位 Trae 客户端数据目录".to_string();
            emit(
                &mut outcome,
                SwitchStep::new("fatal", "fail", format!("恢复失败: {message}")),
            );
            outcome.error = Some(message);
            return outcome;
        }
    };
    match restore_from_slot_in_dir(&restore_dir, variant, &target) {
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

    // 6.5 恢复后**复核**：客户端实际登录的账号必须等于目标账号。
    //
    // 为什么必须有：`restore_from_slot_in_dir` 只保证「文件被覆盖了」，不保证
    // 「覆盖进去的就是 target 的登录态」—— `profiles/<target>/` 可能是被历史上
    // 那条「把当前登录态存进别人槽位」的缺陷**污染过的快照**。没有这一步，
    // 用户看到的是「切换成功」，然后发现还是同一个人（正是报障的那个症状）。
    //
    // ★ 复核**复用 `restore_dir` 这个值**（构造同源），不再自取目录。
    match verify_restored_login_in(&restore_dir, variant, &target) {
        RestoreCheck::Confirmed => emit(
            &mut outcome,
            SwitchStep::new("verify", "ok", format!("已确认客户端当前登录为 {target}")),
        ),
        RestoreCheck::Mismatch { actual } => {
            let message = format!(
                "切换未生效：恢复后客户端实际登录的是 {actual}，而不是目标账号 {target}。\
                 该槽位的快照可能是在「保存守卫」上线前被写坏的（内容属于另一个账号）。\
                 请在 Trae 客户端里登录 {target} 后重新保存该账号的登录态，再切换。"
            );
            emit(&mut outcome, SwitchStep::new("verify", "fail", message.clone()));
            outcome.error = Some(message);
            return outcome;
        }
        RestoreCheck::Unverifiable => emit(
            &mut outcome,
            SwitchStep::new(
                "verify",
                "skip",
                "无法从客户端读取当前账号（快照内没有可解凭据），跳过复核",
            ),
        ),
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

/// 恢复后复核的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
enum RestoreCheck {
    /// 客户端**实际**登录的账号 == 目标账号。
    Confirmed,
    /// 实际登录的是另一个账号 —— 切换没有生效。
    Mismatch { actual: String },
    /// 读不到（客户端没装 / 快照内没有可解凭据）⇒ fail-open。
    Unverifiable,
}

/// 复核逻辑本体：读**显式传入的目录**，判断客户端此刻登录着谁。
///
/// ## ★ 目录必须与「写入」是**同一个值**（构造同源，I-4 同源不变式）
///
/// 本函数是复核的**唯一实现**，目录是**显式入参**——由调用方
/// （[`switch_account`]）把 [`restore_from_slot_in_dir`] 刚写入的那个目录
/// **原样传进来**。这与 [`restore_from_slot_for`] 的写入目标必然一致：
/// 二者**在类型层面**是同一个值，不存在「各自再推导一次」的可能。
///
/// 历史上这里曾有一个无参薄封装 `verify_restored_login_for`，它自己再调一次目录取值点
/// —— 那只是「**约定同源**」（两次取值今天恰好同值）。取值点一旦依赖运行时状态
/// （活跃度、环境变量等），写入与复核就会**静默分叉**：复核读到另一个目录，
/// 把正常切换误报成失败。该封装已删除，改由调用方显式传值。
///
/// 因此**不要**在此函数内部再引入任何目录选择器 —— 那会把构造同源退回约定同源。
///
/// ## fail-open 仅在读不到时
///
/// 客户端没装、快照里没有信封、或该产品线本就没有可解凭据，都可能读不到
/// （目录不存在时同样读不到）；把「读不到」当成失败会让正常切换被误挡。
fn verify_restored_login_in(root: &Path, variant: TraeVariant, target: &str) -> RestoreCheck {
    match extract_local_jwt_from_dir(root, variant) {
        Ok((actual, _)) if actual == target => RestoreCheck::Confirmed,
        Ok((actual, _)) => RestoreCheck::Mismatch { actual },
        Err(_) => RestoreCheck::Unverifiable,
    }
}

/// 保存前的守卫：客户端**此刻实际登录**的账号必须与目标槽位一致。
///
/// ## 为什么必须有它
///
/// [`backup_to_slot_for`] 的源是 [`snapshot_data_dir_for`]（首位候选），也就是
/// **客户端此刻真实的登录态**。若调用方指定的槽位是另一个账号，快照就会被贴到错误的账号名下：
/// `profiles/<B>/` 里装的是 A 的内容，`currentAccount` 却记成 B ⇒ 之后切到 B，
/// 恢复出来的还是 A。用户看到的症状是「**切换怎么切都是同一个账号**」。
///
/// 这不是假想：参考实现把它当**实测事故**修过（`switch.rs` 的「F2-5 保存守卫」——
/// CodeBuddy 两个槽位互相污染后内容完全相同，切换怎么切都是同一个账号）。
///
/// ## 校验依据必须是「客户端实际状态」，不能是程序自己写的标签
///
/// `currentAccount` 这类由本程序自己维护的标签**正是被这条缺陷写坏的** ——
/// 拿它做门禁，等于用被污染的值去判断污染。所以这里回到
/// [`extract_local_jwt_from_dir`] 读客户端真实凭据。
///
/// ## ★ 必须读**被守护操作所读的那个目录**（曾经在这里踩过一个 P0）
///
/// 本模块有两个目录选择器：`select_data_dir_for`（最近活跃）与
/// `detect_data_dir_for`（首位候选），**同一台机器上可能给出不同目录**
/// （实测 Trae Work：`select` → `TRAE SOLO`、`detect` → `TRAE SOLO CN`）。
/// [`backup_to_slot_for`] 用的是 **`detect_data_dir_for`**。
///
/// 本函数曾用 `extract_local_jwt_for`（内部走 `select_data_dir_for`）取证，于是：
///
/// - **假阴性**：`select` 那个目录没有凭据 ⇒ 取证失败 ⇒ fail-open **静默放行**，
///   守卫在真机上等于不存在；
/// - **假阳性**（更坏）：两个目录各有登录态且**属于不同账号** ⇒ 取证读到 A 判定
///   「就是 A」⇒ 放行，而 `backup_to_slot_for` 从 `detect` 目录拷的是 B 的状态
///   存进 A 的槽位 —— 守卫**为一次错误的保存盖了章**。
///
/// 故此处走 [`snapshot_data_dir_for`]（唯一取值点，内部即 `detect_data_dir_for`），
/// 与 [`backup_to_slot_for`] 严格同目录。
/// 改这一行前请先读 [`extract_local_jwt_from_dir`] 与 [`snapshot_data_dir_for`] 的文档。
///
/// ## 为什么不能放进 `backup_to_slot_for`（`LAST_SLOT` 是唯一豁免）
///
/// **不变式：凡把「客户端当前状态」写入「账号槽位」的路径，都必须过本守卫；
/// [`LAST_SLOT`]（回滚槽）是唯一豁免。**
///
/// [`switch_account`] 自己会调 `backup_to_slot_for(variant, LAST_SLOT)`
/// 把当前状态存进**回滚槽**。`last` 不是 userId，拿 uid 比必然不等 ⇒
/// 若把守卫下沉到 `backup_to_slot_for`，守卫会**整体废掉切换的回滚兜底**。
///
/// 两条语义决定了这个豁免：
///
/// - [`LAST_SLOT`] 的语义是「**切换前的现场**」——它**必须**允许在客户端未登录时
///   也照旧写入，否则「切坏了再回滚」的能力就没了；
/// - 「账号槽位」（以 userId 命名）的语义是「**该账号的可用登录态**」——
///   绝不能被未登录态污染，且写入是**覆盖**式的，会毁掉原本正确的旧快照。
///
/// 因此守卫加在「把当前状态写进**账号槽位**」的**全部**路径上：
/// [`save_current_login_for`]、[`crate::modules::trae::handlers::backup_profile_for`]，
/// 以及 [`switch_account`] 的第 3 步（`backup-current`）。
///
/// ## 只放行一种情形：**源目录不存在**（R5 修正）
///
/// 本函数有**两个**出口，语义必须区分开：
///
/// - **出口①「源目录不存在」⇒ 放行**。此时「客户端到底登录着谁」这个问题本身
///   不成立（没有客户端数据），把报错留给 [`backup_to_slot_for`] 的
///   「未找到 Trae 客户端数据目录…」，用户看到的是**可操作**的原因；
/// - **出口②「目录在、但读不出登录态」⇒ 拒绝**。曾经的实现也在这里放行，
///   于是产生一条**静默**的坏路径：`backup_to_slot_for` 只要求目录存在，
///   它会照旧复制 `Local Storage/`、`Network/`、`machineid` 等**与登录无关**的文件
///   ⇒ `copied > 0` ⇒ 返回 `Ok`。于是一份「看起来正常、内容却是未登录态」的快照被
///   存进账号槽位并被记成当前账号；之后切到该账号，恢复出来的是**未登录态**。
///
/// 「从未登录」与「凭据读不出来」都属于出口②，**都必须拒绝**：
/// 本程序的职责是把「客户端此刻的登录态」存进「该账号的槽位」，
/// 没有登录态就没有可存的东西，继续存只会污染槽位。
///
/// ## 为什么不做「四态枚举」
///
/// [`extract_local_jwt_from_dir`] 的 `Err` 至少有 5 种成因（没有 `storage.json` /
/// 没有 cloudide 键 / 信封解不开 / **凭据已过期** / uid 解不出）。要把它们映射成
/// 「未登录」与「读不出来」两张语义不同的脸，需要一张**没有依据**的对照表，
/// 且「已过期」两类都不合适。故只用「读得到 / 读不到」二分。
pub(crate) fn ensure_save_target_matches_client(
    variant: TraeVariant,
    user_id: &str,
) -> Result<(), String> {
    // 取证目录 = 被守护操作（`backup_to_slot_for`）所读的那个目录。
    // 两者都必须走 `snapshot_data_dir_for` 这个**唯一取值点**，否则「校验读了 A、操作改了 B」。
    let Some(dir) = snapshot_data_dir_for(variant).filter(|dir| dir.is_dir()) else {
        // 出口①：源目录不存在 ⇒ 放行，报错交给 `backup_to_slot_for`。
        return Ok(());
    };
    let Ok((client_uid, _)) = extract_local_jwt_from_dir(&dir, variant) else {
        // 出口②：目录在、但读不出登录态 ⇒ **拒绝**（文案必须与下面的出口③可区分，
        // 这里没有 `client_uid` 这个值，绝不能复用「另一个账号」的措辞）。
        return Err(format!(
            "无法读取【{}】客户端当前的登录态，因此不能把一份**没有登录态**的快照存进【{user_id}】名下 —— \
             那样之后切到该账号会变成未登录。请先在 Trae 客户端里登录后再保存，或改用「OAuth 网页登录」。",
            variant.display_name()
        ));
    };
    if client_uid == user_id {
        return Ok(());
    }
    // 出口③：读到了登录态，但属于另一个账号。
    Err(format!(
        "客户端当前登录的是另一个账号（{client_uid}），不能把它的登录态保存到【{user_id}】名下 —— \
         否则之后切到该账号，恢复出来的还是现在这个人（症状：切换怎么切都是同一个账号）。\
         请先在 Trae 客户端里登录【{user_id}】再保存，或改用「OAuth 网页登录」。"
    ))
}

/// 保存当前登录态到指定账号槽位（不切换、不重启客户端；默认变体，兼容壳）。
pub fn save_current_login(user_id: &str) -> Result<u64, String> {
    save_current_login_for(TraeVariant::default(), user_id)
}

/// 保存当前登录态到指定账号槽位（不切换、不重启客户端；按变体分家）。
///
/// 先过 [`ensure_save_target_matches_client`]：客户端登录着谁，就只能存进谁的槽位。
pub fn save_current_login_for(variant: TraeVariant, user_id: &str) -> Result<u64, String> {
    ensure_save_target_matches_client(variant, user_id)?;
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
        // 返回值是**完整请求头值**：明文兜底捞出的是裸 token，落库前统一补前缀
        // （与 tc 信封主来源、与 OAuth 路径三者同形）。
        assert_eq!(found, format!("Cloud-IDE-JWT {token}"));
    }

    /// `storage_device_entry_count` 必须**只**数 `iCubeAuthInfo://icube-dc:*` 前缀的键。
    ///
    /// 它不是业务逻辑，而是诊断文案的事实依据（「客户端是否在此目录启动过」）。
    /// 早先它数的是全部 `iCubeAuthInfo://*` 键，于是真机 `TRAE SOLO`（只有一个
    /// `icube-dc`、没有登录态副本）被诊断成「该客户端确实已登录」，与同一段文案里
    /// 「没有登录态副本键」**自相矛盾**。数多了会把「没登录」说成「登录了」，
    /// 两种错法都会把用户引向错误的排障方向。
    #[test]
    fn storage_device_entry_count_excludes_the_login_state_copy() {
        let root = std::env::temp_dir().join(format!("trae-device-count-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let global = root.join("User").join("globalStorage");
        std::fs::create_dir_all(&global).unwrap();
        let path = global.join("storage.json");
        std::fs::write(
            &path,
            r#"{
                "iCubeAuthInfo://icube-dc:2292929806738024": "tC\u0005\u0010AAAA",
                "iCubeAuthInfo://icube.cloudide": "tC\u0005\u0010BBBB",
                "iCubeAuthInfo://usertag": "tC\u0005\u0010CCCC",
                "telemetry.machineId": "abc",
                "icubeAuthInfo://icube-dc:lowercase": "should-not-count"
            }"#,
        )
        .unwrap();

        // 三个 iCube 键里只有一个是设备身份 —— 登录态副本与 usertag 都不算。
        assert_eq!(
            storage_device_entry_count(&root),
            1,
            "只有 icube-dc: 前缀算设备身份；登录态副本与 usertag 不得计入"
        );

        // 只有登录态副本、没有设备身份时必须是 0（反向：两者不能互相顶替）。
        std::fs::write(
            &path,
            r#"{"iCubeAuthInfo://icube.cloudide": "tC\u0005\u0010BBBB"}"#,
        )
        .unwrap();
        assert_eq!(
            storage_device_entry_count(&root),
            0,
            "登录态副本不得被当成设备身份"
        );

        // 三种退化输入都必须是 0，而不是 panic —— 诊断函数在读失败时也要能出文案。
        std::fs::write(&path, r#"{"iCubeAuthInfo://x":"y""#).unwrap();
        assert_eq!(storage_device_entry_count(&root), 0, "JSON 损坏时返回 0");
        std::fs::write(&path, r#"["not","an","object"]"#).unwrap();
        assert_eq!(storage_device_entry_count(&root), 0, "顶层不是对象时返回 0");
        let _ = std::fs::remove_file(&path);
        assert_eq!(storage_device_entry_count(&root), 0, "文件缺失时返回 0");

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
        // 该 fixture 只有登录态副本、**没有** `icube-dc` 设备身份 ⇒ 第三条 bullet
        // 必须说「没检测到设备身份」。设备身份与登录态是两件事，不得互相顶替。
        assert!(
            text.contains("未检测到设备身份"),
            "必须如实报告设备身份缺失：{text}"
        );
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

        // storage.json 整个不存在时必须换另一套说法：主来源缺失 + 从未在此目录启动过。
        std::fs::remove_file(global.join("storage.json")).unwrap();
        let no_storage = diagnose_missing_credential(&root, TraeVariant::TraeWork);
        assert!(
            no_storage.contains("没有 iCube 登录态副本键"),
            "缺 storage.json 时应报主来源缺失：{no_storage}"
        );
        assert!(
            no_storage.contains("未检测到设备身份"),
            "缺 storage.json 时应报设备身份缺失：{no_storage}"
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
        // 返回值是**完整请求头值**（主来源与明文兜底同形），故这里带前缀比对。
        let (work_uid, work_found) = work_result.expect("应取到 Trae Work 目录里的凭据");
        assert_eq!(work_uid, "1111111111111111", "不得回落到 Trae CN 的凭据");
        assert_eq!(work_found, format!("Cloud-IDE-JWT {work_token}"));

        // 反向也成立：TraeCn 请求只取自己目录里的凭据。
        let (cn_uid, cn_found) = cn_result.expect("应取到 Trae CN 目录里的凭据");
        assert_eq!(cn_uid, "2222222222222222");
        assert_eq!(cn_found, format!("Cloud-IDE-JWT {cn_token}"));
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

    // -----------------------------------------------------------------------
    // 导入来源：tc 信封（主来源）
    // -----------------------------------------------------------------------

    /// 从 fixture 列表里挑出某个变体的候选目录对应的 `userId`。
    fn fixture_uids(fixtures: &[(String, String)], variant: TraeVariant) -> Vec<String> {
        let names = platform::data_dir_names_for(variant);
        fixtures
            .iter()
            .filter(|(_, name)| names.contains(&name.as_str()))
            .map(|(uid, _)| uid.clone())
            .collect()
    }

    /// ★ Trae Work 的「导入本机账号」必须成功：凭据**只在 tc 信封里**时也要能导入。
    ///
    /// fixture 刻意做成「只有 tc 信封」：没有 `icube-dc` 设备凭证、没有任何明文
    /// `Cloud-IDE-JWT`、**连 `logs/` 目录都没有** —— 这正是 `TRAE SOLO CN`
    /// （Trae Work）的真机形态（实测 355 个日志文件、0 个 `completion.log`、0 处明文）。
    ///
    /// 修之前该产品线**恒导入失败**：旧实现只扫明文，而明文在它身上不存在。
    /// 反向验证：删掉 `extract_local_jwt_for` 里取主来源那一步，本用例必红。
    #[cfg(windows)]
    #[test]
    fn extract_local_jwt_reads_the_icube_envelope_when_no_plaintext_exists() {
        let exp = chrono::Utc::now().timestamp() + 3600;
        let env = crate::modules::trae::test_support::TempEnv::empty();
        let fixtures = icube::test_support::write_cloudide_only_user_data(&env.appdata(), exp);

        let work_uids = fixture_uids(&fixtures, TraeVariant::TraeWork);
        let cn_uids = fixture_uids(&fixtures, TraeVariant::TraeCn);
        assert_eq!(work_uids.len(), 2, "Trae Work 应有两个候选目录");
        assert_eq!(cn_uids.len(), 2, "Trae CN 应有两个候选目录");

        let (work_uid, work_header) =
            extract_local_jwt_for(TraeVariant::TraeWork).expect("Trae Work 必须能从 tc 信封导入");
        let (cn_uid, cn_header) =
            extract_local_jwt_for(TraeVariant::TraeCn).expect("Trae CN 必须能从 tc 信封导入");

        assert!(work_uids.contains(&work_uid), "读到了别的目录的账号：{work_uid}");
        assert!(cn_uids.contains(&cn_uid), "读到了别的目录的账号：{cn_uid}");
        assert_ne!(work_uid, cn_uid, "两条产品线必须各自读到自己的目录");

        // 落库形态：**完整请求头值**（含 `Cloud-IDE-JWT ` 前缀），与 OAuth 路径一致。
        for (label, header) in [("Trae Work", &work_header), ("Trae CN", &cn_header)] {
            assert!(
                header.starts_with("Cloud-IDE-JWT "),
                "{label} 的落库值必须含前缀，实际：{header}"
            );
            assert_eq!(
                header.matches('.').count(),
                2,
                "{label} 补前缀后仍应是三段 JWT：{header}"
            );
        }
        // 前缀之后必须**逐字**等于信封里那个裸 token（证明是「补前缀」而不是另造）。
        assert_eq!(
            jwt::normalize(&work_header),
            icube::test_support::bare_jwt(&work_uid, exp),
            "前缀后必须逐字等于信封里的裸 token"
        );
    }

    /// ★ tc 信封**优先于**明文兜底 —— 「切换后账号不变」的直接护栏。
    ///
    /// 场景：客户端当前登录的是 A（tc 信封里是 A），但 `logs/` 里残留着上一账号 B
    /// 的明文 token，且 **B 的 `exp` 更晚**。旧实现按 `exp` 取最大 ⇒ 导入到 B。
    /// tc 信封才是客户端**当前**登录态，因此必须返回 A。
    #[cfg(windows)]
    #[test]
    fn extract_local_jwt_prefers_icube_envelope_over_stale_plaintext_log() {
        let now = chrono::Utc::now().timestamp();
        let env = crate::modules::trae::test_support::TempEnv::empty();
        // A：信封里的当前登录态（exp 较近）。
        let fixtures =
            icube::test_support::write_cloudide_only_user_data(&env.appdata(), now + 600);
        let work_uids = fixture_uids(&fixtures, TraeVariant::TraeWork);

        // B：上一账号，明文残留在**每一个** Trae Work 候选目录的日志里，且 exp 更晚
        //    （这样用例不依赖 `select_data_dir_for` 选中哪个候选目录）。
        let stale_uid = "1111222233334444";
        let stale = icube::test_support::bare_jwt(stale_uid, now + 86_400);
        for name in platform::data_dir_names_for(TraeVariant::TraeWork) {
            let log = env
                .appdata()
                .join(name)
                .join("logs")
                .join("20260918T000000")
                .join("window1")
                .join("exthost")
                .join("trae.ai-code-completion")
                .join("completion.log");
            std::fs::create_dir_all(log.parent().unwrap()).unwrap();
            std::fs::write(
                &log,
                format!(r#"{{"Authorization":"Cloud-IDE-JWT {stale}"}}"#),
            )
            .unwrap();
        }

        let (uid, _) =
            extract_local_jwt_for(TraeVariant::TraeWork).expect("有 tc 信封时导入必须成功");
        assert_ne!(uid, stale_uid, "不得读回上一账号（尽管它的 exp 更晚）");
        assert!(work_uids.contains(&uid), "应返回信封里的账号，实际：{uid}");
    }

    /// 诊断必须说清**主来源**（tc 信封）的状态。
    ///
    /// 它现在是 Trae Work 唯一可用的来源，所以「在不在、为什么用不上」不能缺席：
    /// 否则用户会把「凭据已过期 / 解不开」误判成「没登录」，去重装客户端。
    #[test]
    fn diagnose_reports_the_icube_envelope_state() {
        let root = std::env::temp_dir().join(format!("trae-diag-env-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let global = root.join("User").join("globalStorage");
        std::fs::create_dir_all(&global).unwrap();

        // ① 信封键存在（哪怕内容解不开）⇒ 必须说「存在但用不上」并给出下一步。
        std::fs::write(
            global.join("storage.json"),
            r#"{"iCubeAuthInfo://icube.cloudide":"tC\u0005\u0010AAAA"}"#,
        )
        .unwrap();
        let with_key = diagnose_missing_credential(&root, TraeVariant::TraeWork);
        assert!(with_key.contains("登录态副本键"), "必须报告主来源：{with_key}");
        assert!(with_key.contains("重新登录"), "必须给出下一步：{with_key}");

        // ② 信封键不存在 ⇒ 必须明说主来源缺失，而不是笼统的「没找到凭据」。
        std::fs::write(global.join("storage.json"), r#"{"telemetry.machineId":"x"}"#).unwrap();
        let without_key = diagnose_missing_credential(&root, TraeVariant::TraeWork);
        assert!(
            without_key.contains("没有 iCube 登录态副本键"),
            "必须报告主来源缺失：{without_key}"
        );

        // ③ **只有设备身份、没有登录态副本** ⇒ 文案不得自相矛盾。
        //
        // 真机 `TRAE SOLO` 就是这个形态（客户端首次启动过、用户从未登录），
        // 而早先的实现会因为「有一条 iCubeAuthInfo 记录」直接断言「该客户端确实已登录」，
        // 与同一段里的「没有登录态副本键」正面冲突。设备身份不是登录证据。
        std::fs::write(
            global.join("storage.json"),
            r#"{"iCubeAuthInfo://icube-dc:2292929806738024":"tC\u0005\u0010AAAA"}"#,
        )
        .unwrap();
        let device_only = diagnose_missing_credential(&root, TraeVariant::TraeWork);
        assert!(
            device_only.contains("不代表登录过"),
            "设备身份必须与登录态分开陈述：{device_only}"
        );
        assert!(
            !device_only.contains("确实已登录"),
            "只有设备身份时不得断言已登录：{device_only}"
        );
        assert!(
            device_only.contains("没有 iCube 登录态副本键"),
            "设备身份存在也不影响「主来源缺失」这一事实：{device_only}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    // -----------------------------------------------------------------------
    // 快照 / 恢复：切换不变式与主干往返
    // -----------------------------------------------------------------------

    /// ★ 恢复快照必须清掉「清单外的凭据来源」—— 这是 [`CORE_ENTRIES`] 的切换不变式。
    ///
    /// fixture：目标客户端里有上一账号的两类残留 ——
    /// `logs/`（明文 JWT，`extract_local_jwt_for` 会扫）与 SQLite 边车文件
    /// （`-wal`/`-shm`/`-journal`，SQLite 下次打开会**回放**）。
    /// 恢复之后这两类都必须不存在；不清就会「切换后账号不变」。
    #[cfg(windows)]
    #[test]
    fn restore_purges_sources_outside_the_snapshot() {
        let _env = crate::modules::trae::test_support::TempEnv::empty();
        let variant = TraeVariant::TraeWork;
        let target = platform::detect_data_dir_for(variant).expect("临时 APPDATA 下应能定位目录");

        // 「上一账号」的残留。
        let stale_log = target
            .join("logs")
            .join("20260918T000000")
            .join("window1")
            .join("exthost")
            .join("trae.ai-code-completion")
            .join("completion.log");
        std::fs::create_dir_all(stale_log.parent().unwrap()).unwrap();
        std::fs::write(&stale_log, "Authorization: Cloud-IDE-JWT old.old.old").unwrap();
        let global = target.join("User").join("globalStorage");
        std::fs::create_dir_all(&global).unwrap();
        for sidecar in ["state.vscdb-wal", "state.vscdb-shm", "state.vscdb-journal"] {
            std::fs::write(global.join(sidecar), b"stale").unwrap();
        }

        // 快照：只放 `CORE_ENTRIES` 里的一件文件 —— 正好证明「清单外的东西不进快照」。
        let slot = "acctA";
        let snapshot = paths::profiles_dir_for(variant).join(slot);
        std::fs::create_dir_all(snapshot.join("User").join("globalStorage")).unwrap();
        std::fs::write(
            snapshot.join("User").join("globalStorage").join("storage.json"),
            br#"{"aha":{"account":"A"}}"#,
        )
        .unwrap();

        let restored = restore_from_slot_for(variant, slot).expect("恢复应成功");
        assert_eq!(restored, 1, "快照里只有一件文件");

        assert!(
            !target.join("logs").exists(),
            "logs/ 必须被清除，否则导入会读到上一账号的明文 token"
        );
        for sidecar in ["state.vscdb-wal", "state.vscdb-shm", "state.vscdb-journal"] {
            assert!(
                !global.join(sidecar).exists(),
                "{sidecar} 必须被清除，否则 SQLite 会回放旧事务"
            );
        }
        // 正向对照：快照内容确实到位了（别把「清干净」做成「什么都没恢复」）。
        assert!(global.join("storage.json").is_file(), "快照内容必须被恢复");
    }

    /// ★ 切换的主干：A → B → A 之后，**A 的登录态真的回来了**。
    ///
    /// 现有用例只覆盖 precheck / 形状（缺快照、非法 userId），主干（备份 → 恢复）
    /// 一直没有正向护栏。本用例在**文件级**做完整的 A→B→A 往返，并再切一次到 B
    /// 证明恢复是**双向**的、不是「只认第一份」。
    ///
    /// **刻意不调 `switch_account`**：那条路径会走 `platform::kill_client_for`，
    /// 在开发机上会**真的杀掉用户正在用的 Trae**。这里只测快照/恢复的实体部分，
    /// 也就是「登录态有没有真的换回来」。
    #[cfg(windows)]
    #[test]
    fn snapshot_round_trip_restores_account_a_login_state() {
        let _env = crate::modules::trae::test_support::TempEnv::empty();
        let variant = TraeVariant::TraeWork;
        let target = platform::detect_data_dir_for(variant).expect("临时 APPDATA 下应能定位目录");
        let storage = target
            .join("User")
            .join("globalStorage")
            .join("storage.json");
        std::fs::create_dir_all(storage.parent().unwrap()).unwrap();

        // A 登录 → 存快照。
        std::fs::write(&storage, br#"{"aha":{"account":"A"}}"#).unwrap();
        backup_to_slot_for(variant, "acctA").expect("A 的登录态应能快照");

        // 切到 B（客户端文件被 B 覆盖）→ 存快照。
        std::fs::write(&storage, br#"{"aha":{"account":"B"}}"#).unwrap();
        backup_to_slot_for(variant, "acctB").expect("B 的登录态应能快照");

        // 切回 A。
        restore_from_slot_for(variant, "acctA").expect("恢复 A 应成功");
        assert_eq!(
            std::fs::read_to_string(&storage).unwrap(),
            r#"{"aha":{"account":"A"}}"#,
            "A 的登录态必须真的回来"
        );

        // 再切到 B：证明双向。
        restore_from_slot_for(variant, "acctB").expect("恢复 B 应成功");
        assert_eq!(
            std::fs::read_to_string(&storage).unwrap(),
            r#"{"aha":{"account":"B"}}"#,
            "恢复必须是双向的，不能只认第一份快照"
        );

        // `restore_from_slot_for` 是**纯文件操作**：记录「当前账号」是 `switch_account`
        // 的职责（且只在整个流程都成功后写）。这条把两者的边界钉住。
        assert_eq!(
            current_account_for(variant),
            None,
            "只做快照恢复不应凭空产生「当前账号」记录"
        );
    }

    // -----------------------------------------------------------------------
    // 保存守卫：客户端登录着谁，就只能存进谁的槽位
    // -----------------------------------------------------------------------

    /// ★ 客户端登录着 A 时，不得把 A 的登录态存进 B 的槽位。
    ///
    /// 这是用户报障「**切换怎么切都是同一个账号**」的直接成因：快照的源是客户端
    /// 此刻的真实登录态，槽位却是调用方指定的 userId ⇒ 存错槽位后
    /// `profiles/<B>/` 装的是 A 的内容，切到 B 恢复出来还是 A。
    /// 参考实现把它当**实测事故**修过（两个槽位被污染成完全相同）。
    ///
    /// ## 为什么必须断言「内容归属」，不能只断言 `currentAccount` / `count`
    ///
    /// 那两个值都是**本程序自己写的**，正是被这条缺陷污染的东西 —— 只断言它们，
    /// 测试会在「守卫放行了一次错误保存」时照样变绿。所以这里把快照**读回来、
    /// 解出信封里的 uid**，直接比对归属。
    ///
    /// （这不是假设：本用例最初只断言 `currentAccount` 与 `count > 0`，在守卫
    /// **读错目录**的那个 P0 下**一直是绿的**，同时演示着它要防的那个 bug。）
    #[cfg(windows)]
    #[test]
    fn save_refuses_to_store_the_client_state_under_another_account() {
        let exp = chrono::Utc::now().timestamp() + 3600;
        let env = crate::modules::trae::test_support::TempEnv::empty();
        let fixtures = icube::test_support::write_cloudide_only_user_data(&env.appdata(), exp);
        let variant = TraeVariant::TraeWork;
        let work_uids = fixture_uids(&fixtures, variant);
        assert!(work_uids.len() >= 2, "需要至少两个候选目录才能构造「存错槽位」");

        // 前置：fixture 必须让两个选择器分叉 —— 否则本用例证明不了「同目录」这件事。
        let op_dir = platform::detect_data_dir_for(variant).expect("临时 APPDATA 下应能定位目录");
        let active_dir = platform::select_data_dir_for(variant).expect("应能定位活跃目录");
        assert_ne!(
            op_dir, active_dir,
            "前置：fixture 必须让「首位候选」与「最近活跃」指向不同目录"
        );

        // 客户端「实际登录的账号」= **被守护操作所读目录**里的那个 uid。
        let (client_uid, _) =
            extract_local_jwt_from_dir(&op_dir, variant).expect("fixture 必须可读");
        let other_uid = work_uids
            .iter()
            .find(|uid| uid.as_str() != client_uid)
            .expect("必须存在一个与客户端不同的账号")
            .clone();
        let other_slot = paths::profiles_dir_for(TraeVariant::TraeWork).join(&other_uid);

        // ① 存到**别人**的槽位 ⇒ 必须拒绝，且不得留下半个槽位目录。
        let error = save_current_login_for(variant, &other_uid)
            .expect_err("登录着 A 却往 B 的槽位存，必须被拒绝");
        assert!(
            error.contains(&client_uid),
            "错误必须说清客户端当前是谁：{error}"
        );
        assert!(
            error.contains(&other_uid),
            "错误必须说清目标槽位是谁：{error}"
        );
        assert!(
            !other_slot.exists(),
            "被拒绝的保存不得留下槽位目录：{}",
            other_slot.display()
        );

        // ② `backup_profile_for` 是同一操作的另一条入口，必须同样被拦 ——
        //    只在 `save_login` 上加守卫，等于留下一扇可绕过的大门。
        let backup_error = crate::modules::trae::handlers::backup_profile_for(variant, &other_uid)
            .expect_err("另一条入口也必须被拦");
        assert!(
            backup_error.contains(&other_uid),
            "另一条入口的报错同样要说清目标槽位：{backup_error}"
        );
        assert!(!other_slot.exists(), "另一条入口也不得留下槽位目录");

        // ③ 存到**自己**的槽位 ⇒ 必须成功（守卫不得把正常流程一起挡掉）。
        let count = save_current_login_for(variant, &client_uid)
            .expect("客户端登录着 A、存进 A 的槽位必须成功");
        assert!(count > 0, "必须真的复制到文件");
        assert_eq!(
            current_account_for(variant).as_deref(),
            Some(client_uid.as_str())
        );

        // ④ ★ **核心断言**：把快照读回来、解出信封里的 uid，归属必须 == 槽位 uid。
        //    这一条才是「切换不会切到同一个人」的证明；上面那两个自写标签证明不了。
        let own_slot = paths::profiles_dir_for(variant).join(&client_uid);
        let (saved_uid, _) = extract_local_jwt_from_dir(&own_slot, variant)
            .expect("快照里必须能解出凭据（否则这条断言证明不了归属）");
        assert_eq!(
            saved_uid, client_uid,
            "★ 快照内容的归属必须等于槽位 uid —— 否则切过去还是现在这个人"
        );
    }

    /// ★ 守卫的证据来源必须与被守护的操作**同一个目录**（曾经读错，守卫在真机上静默失效）。
    ///
    /// fixture 构造成**首位候选有凭据、最近活跃的那个没有** —— 真机 Trae Work 就是这个形态
    /// （`TRAE SOLO CN` 有登录态、`TRAE SOLO` 更活跃但没有）。
    /// 旧实现用 `extract_local_jwt_for`（走 `select_data_dir_for`）取证 ⇒ 读不到 ⇒
    /// fail-open ⇒ **静默放行**，守卫等于不存在。
    #[cfg(windows)]
    #[test]
    fn save_guard_reads_the_same_dir_as_the_guarded_operation() {
        let exp = chrono::Utc::now().timestamp() + 3600;
        let env = crate::modules::trae::test_support::TempEnv::empty();
        let fixtures = icube::test_support::write_cloudide_only_user_data(&env.appdata(), exp);
        let variant = TraeVariant::TraeWork;

        let first_name = platform::data_dir_names_for(variant)[0];
        let first_dir = env.appdata().join(first_name);
        let first_uid = fixtures
            .iter()
            .find(|(_, name)| name == first_name)
            .map(|(uid, _)| uid.clone())
            .expect("首位候选必须在 fixture 里");

        // 把**非首位**候选清空（写成空对象）并弄成最新 ⇒ `select` 指向它，而它没有登录态。
        for (_, name) in &fixtures {
            if name == first_name {
                continue;
            }
            let storage = env
                .appdata()
                .join(name)
                .join("User")
                .join("globalStorage")
                .join("storage.json");
            std::fs::write(&storage, b"{}").unwrap();
        }

        // 前置：两个选择器确实分叉，且「活跃目录读不到凭据」而「首位目录读得到」。
        assert_ne!(
            platform::detect_data_dir_for(variant).unwrap(),
            platform::select_data_dir_for(variant).unwrap(),
            "前置：fixture 必须让两个选择器分叉"
        );
        assert!(
            extract_local_jwt_for(variant).is_err(),
            "前置：活跃目录没有凭据 —— 旧实现正是靠这一点静默 fail-open"
        );
        assert_eq!(
            extract_local_jwt_from_dir(&first_dir, variant)
                .expect("前置：首位目录必须有凭据")
                .0,
            first_uid
        );

        // 核心：守卫的证据来自**首位目录**，所以它必须拦得住存错槽位。
        let error = save_current_login_for(variant, "9999999999999999")
            .expect_err("守卫必须读首位目录；读活跃目录会在这里静默放行");
        assert!(
            error.contains(&first_uid),
            "报错必须说清客户端当前是谁：{error}"
        );
    }

    /// 守卫**不得**下沉到 `backup_to_slot_for`：`switch_account` 的回滚槽
    /// （[`LAST_SLOT`]，不是 userId）必须照旧可用。
    ///
    /// 天真实现会把守卫放进 `backup_to_slot_for`，于是「切换前先把当前状态存进
    /// `last`」这一步拿 `"last"` 去和客户端 uid 比、必然不等 ⇒
    /// **整个回滚兜底失效**（切换失败时救不回来）。
    #[cfg(windows)]
    #[test]
    fn backup_primitive_still_accepts_non_account_slots() {
        let _env = crate::modules::trae::test_support::TempEnv::empty();
        let variant = TraeVariant::TraeWork;
        let target = platform::detect_data_dir_for(variant).expect("临时 APPDATA 下应能定位目录");
        let storage = target
            .join("User")
            .join("globalStorage")
            .join("storage.json");
        std::fs::create_dir_all(storage.parent().unwrap()).unwrap();
        std::fs::write(&storage, br#"{"aha":{"account":"A"}}"#).unwrap();

        backup_to_slot_for(variant, LAST_SLOT).expect("回滚槽不是 userId，必须照旧可用");
        assert!(paths::profiles_dir_for(variant).join(LAST_SLOT).is_dir());
    }

    /// 守卫必须**拒绝**「客户端读不到登录态」时的保存（R5）。
    ///
    /// ## ⚠️ 本用例由旧用例**改名 + 语义反转**而来
    ///
    /// 旧名 `save_guard_fails_open_when_the_client_state_is_unreadable`，旧断言是
    /// 「读不到客户端状态时**必须放行**」—— 它把一条有害行为**钉成了正确行为**。
    /// 该行为允许把一份**未登录态**的快照静默存进账号槽位：
    /// [`backup_to_slot_for`] 只要求目录存在，会照旧复制 `Local Storage/`、`Network/`、
    /// `machineid` 等与登录无关的文件 ⇒ `copied > 0` ⇒ 返回 `Ok`。之后切到该账号，
    /// 恢复出来的是未登录态。
    ///
    /// 现语义：**只有「源目录不存在」放行**；「从未登录」「凭据读不出来」都属于出口② ⇒ 拒绝。
    #[cfg(windows)]
    #[test]
    fn save_refuses_when_the_client_has_no_login_state() {
        // 只有设备凭证、没有登录态副本、没有明文日志 ⇒ 取证必然失败。
        let _env = crate::modules::trae::test_support::TempEnv::with_device_fixture();
        let variant = TraeVariant::TraeWork;
        // 前置断言打在**守卫真正读的那个目录**上（`detect_data_dir_for`）——
        // 打在活跃目录上会掩盖「两个选择器分叉」这类问题。
        let op_dir = platform::detect_data_dir_for(variant).expect("临时 APPDATA 下应能定位目录");
        assert!(
            extract_local_jwt_from_dir(&op_dir, variant).is_err(),
            "前置条件：本 fixture 必须读不到客户端登录态"
        );

        let error = save_current_login_for(variant, "1234567890123456")
            .expect_err("客户端没有登录态时必须拒绝，而不是把未登录态存进账号槽位");
        assert!(
            error.contains("无法读取"),
            "报错必须说清「读不到登录态」：{error}"
        );
        assert!(
            !error.contains("另一个账号"),
            "出口② 不得复用出口③（uid 不匹配）的措辞：{error}"
        );
        let slot = paths::profile_dir_for(variant, "1234567890123456")
            .expect("合法 userId 应能定位槽位目录");
        assert!(
            !slot.exists(),
            "被拒绝的保存不得留下槽位目录：{}",
            slot.display()
        );
    }

    /// 守卫的 fail-open **唯一出口**：源目录不存在时放行，把报错留给 `backup_to_slot_for`。
    ///
    /// 与 [`save_refuses_when_the_client_has_no_login_state`] 成对：那条证明「目录在、读不到
    /// ⇒ 拒绝」，本条证明「目录不存在 ⇒ 放行」。若实现把「目录不存在」也改成拒绝，
    /// 本用例会红（报错会变成守卫的「无法读取…」而不是 `backup_to_slot_for` 的
    /// 「未找到 Trae 客户端数据目录…」）。
    #[cfg(windows)]
    #[test]
    fn save_guard_fails_open_only_when_the_source_dir_is_absent() {
        let _env = crate::modules::trae::test_support::TempEnv::empty();
        let variant = TraeVariant::TraeWork;
        let op_dir = platform::detect_data_dir_for(variant).expect("临时 APPDATA 下应能定位目录");
        assert!(
            !op_dir.is_dir(),
            "前置条件：本 fixture 必须没有客户端数据目录（{}）",
            op_dir.display()
        );

        let error = save_current_login_for(variant, "1234567890123456")
            .expect_err("源目录不存在时，报错应由 backup_to_slot_for 给出");
        assert!(
            error.contains("未找到 Trae 客户端数据目录"),
            "报错必须来自 backup_to_slot_for（而不是守卫）：{error}"
        );
        assert!(
            !error.contains("无法读取"),
            "守卫不得在「源目录不存在」时拦截：{error}"
        );
    }

    // -----------------------------------------------------------------------
    // 切换后复核：恢复完了要确认客户端**真的**换了人
    // -----------------------------------------------------------------------

    /// ★ 复核逻辑本体读的是**它收到的那个目录**，而不是活跃目录。
    ///
    /// fixture 让两个选择器分叉、且各自装着**不同**账号：首位目录 = `first_uid`、
    /// 活跃目录 = 另一个 uid。
    ///
    /// 改造成**构造同源**后，复核不再自取目录（见
    /// [`switch_passes_the_same_dir_to_restore_and_verification`] 的结构断言），
    /// 故本用例改为把目录**显式喂进去**：
    ///
    /// - 喂**写入目标**（首位候选）⇒ `Confirmed`；
    /// - 喂**活跃目录** ⇒ `Mismatch`（读到的是另一个人）。
    ///
    /// 第二条证明「复核确实在读它收到的目录并做比较」，不是恒真。
    #[cfg(windows)]
    #[test]
    fn restore_verification_reads_the_write_target_not_the_active_dir() {
        let exp = chrono::Utc::now().timestamp() + 3600;
        let env = crate::modules::trae::test_support::TempEnv::empty();
        let fixtures = icube::test_support::write_cloudide_only_user_data(&env.appdata(), exp);
        let variant = TraeVariant::TraeWork;

        let first_name = platform::data_dir_names_for(variant)[0];
        let first_uid = fixtures
            .iter()
            .find(|(_, name)| name == first_name)
            .map(|(uid, _)| uid.clone())
            .expect("首位候选必须在 fixture 里");
        let active_uid = fixtures
            .iter()
            .find(|(_, name)| name != first_name)
            .map(|(uid, _)| uid.clone())
            .expect("非首位候选必须在 fixture 里");
        assert_ne!(first_uid, active_uid, "fixture 必须给两个目录不同 uid");

        // `snapshot_data_dir_for` 就是 `restore_from_slot_in_dir` 的写入目标来源。
        let write_target = snapshot_data_dir_for(variant).expect("临时 APPDATA 下应能定位目录");
        let active_dir = platform::select_data_dir_for(variant).expect("活跃候选应存在");
        assert_ne!(
            write_target, active_dir,
            "前置：fixture 必须让两个选择器分叉"
        );

        assert_eq!(
            verify_restored_login_in(&write_target, variant, &first_uid),
            RestoreCheck::Confirmed,
            "写入目标里的账号 == 目标 ⇒ 必须 Confirmed"
        );
        // 反向：喂活跃目录 ⇒ 读到的是另一个人 ⇒ Mismatch（证明复核真的在用收到的目录）。
        assert_eq!(
            verify_restored_login_in(&active_dir, variant, &first_uid),
            RestoreCheck::Mismatch {
                actual: active_uid.clone()
            },
            "喂活跃目录时必须读到活跃账号并报 Mismatch（否则本用例证明不了什么）"
        );
    }

    /// 从源码里取某个函数的函数体（花括号配平），供结构断言用。
    fn fn_body_in_source(source: &str, name: &str) -> String {
        let needle = format!("fn {name}");
        let start = source
            .find(&needle)
            .unwrap_or_else(|| panic!("源码里必须存在 {needle}"));
        let open = start
            + source[start..]
                .find('{')
                .unwrap_or_else(|| panic!("{needle} 必须有函数体"));
        let mut depth = 0i32;
        for (offset, ch) in source[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return source[open + 1..open + offset].to_string();
                    }
                }
                _ => {}
            }
        }
        panic!("{needle} 的花括号不配平");
    }

    /// ★ **构造同源**的结构断言：`switch_account` 必须把**同一个**目录值
    /// 同时交给「写入」与「复核」。
    ///
    /// ## 为什么是结构断言而不是运行时断言
    ///
    /// 「复核是否复用写入目录」**只体现在调用形态上**；而运行 `switch_account` 会走到
    /// 第 5 步 `kill_client_for`，在开发机上会真的杀掉用户正在用的 Trae
    /// （本模块既有约定：切换流程只测各步骤的本体，见
    /// [`restore_verification_reports_mismatch_for_a_polluted_snapshot`]）。
    ///
    /// 因此直接读模块自身源码（`include_str!`）断言：两处都必须收到**同一个标识符**
    /// `restore_dir`，且不得再出现「自取目录」的旧封装 `verify_restored_login_for`。
    ///
    /// ## 反向验证
    ///
    /// 把 `verify_restored_login_in(&restore_dir, …)` 换回
    /// `verify_restored_login_for(variant, …)`，本用例必须报红。
    #[test]
    fn switch_passes_the_same_dir_to_restore_and_verification() {
        let body = fn_body_in_source(include_str!("profile.rs"), "switch_account");
        assert!(
            body.contains("restore_from_slot_in_dir(&restore_dir,"),
            "写入（step 6）必须收到显式目录 `restore_dir`（构造同源）"
        );
        assert!(
            body.contains("verify_restored_login_in(&restore_dir,"),
            "复核（step 6.5）必须复用同一个 `restore_dir`，而不是自取目录（否则退回约定同源）"
        );
        assert!(
            !body.contains("verify_restored_login_for("),
            "复核不得再走「自取目录」的旧封装（该封装已删除）"
        );
    }

    /// ★ 恢复后**复核**：快照里装的是别人 ⇒ 必须响亮失败，而不是记成「已切换」。
    ///
    /// 这条覆盖「历史污染快照」：`profiles/<B>/` 里其实是 A 的登录态
    /// （守卫上线前被写坏的）。切换流程若不复核，用户会看到「切换成功」然后发现还是同一个人。
    ///
    /// 只测**复核逻辑本体**（显式目录），不跑完整切换流程 —— 仓库既有约定：
    /// `switch_account` 会 `kill_client_for`，在开发机上会真的杀掉用户正在用的 Trae。
    #[cfg(windows)]
    #[test]
    fn restore_verification_reports_mismatch_for_a_polluted_snapshot() {
        let exp = chrono::Utc::now().timestamp() + 3600;
        let env = crate::modules::trae::test_support::TempEnv::empty();
        let fixtures = icube::test_support::write_cloudide_only_user_data(&env.appdata(), exp);
        let variant = TraeVariant::TraeWork;
        let uids = fixture_uids(&fixtures, variant);
        let (owner, target) = (uids[0].clone(), uids[1].clone());
        assert_ne!(owner, target);

        // 模拟一个被污染的槽位目录：槽位名是 `target`，内容却是 `owner` 的。
        let polluted = paths::profiles_dir_for(variant).join(&target);
        let src = env
            .appdata()
            .join(platform::data_dir_names_for(variant)[0])
            .join("User")
            .join("globalStorage");
        std::fs::create_dir_all(polluted.join("User").join("globalStorage")).unwrap();
        std::fs::copy(
            src.join("storage.json"),
            polluted.join("User").join("globalStorage").join("storage.json"),
        )
        .unwrap();

        assert_eq!(
            verify_restored_login_in(&polluted, variant, &target),
            RestoreCheck::Mismatch { actual: owner },
            "内容属于 owner 却切到 target ⇒ 必须响亮失败"
        );

        // 同一份内容，目标改成它真正的归属 ⇒ 必须 Confirmed（证明上一条不是恒真）。
        assert_eq!(
            verify_restored_login_in(&polluted, variant, &uids[0]),
            RestoreCheck::Confirmed
        );

        // 读不到（目录里没有 storage.json）⇒ Unverifiable（fail-open，不得误判为失败）。
        let empty = paths::profiles_dir_for(variant).join("empty-slot");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(
            verify_restored_login_in(&empty, variant, &target),
            RestoreCheck::Unverifiable,
            "读不到时必须 fail-open，不能把正常切换判成失败"
        );
    }
}
