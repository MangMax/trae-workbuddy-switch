//! Trae 数据目录与文件路径。
//!
//! 全部路径由 [`crate::modules::config::store_dir`] 派生，因此自动继承
//! `BUDDY_SWITCH_HOME` 覆盖与 `.wb-switch` 兼容回落，测试里可用隔离目录重定向。
//!
//! ## ★ 按产品线变体分家（2026-09-18 起）
//!
//! Trae 有两条**可同机并存、账号互不相通**的产品线（见 [`super::variant`]）。
//! 它们各有独立的客户端、独立的 userData、**独立的登录凭据**，因此账号库也必须分家 ——
//! 否则会出现「Trae CN 的账号被写进 Trae Work 的账号库」，两个产品线互相污染。
//! 这与 WorkBuddy 侧 [`crate::modules::region::accounts_file_for`] 的分家动机完全同源。
//!
//! **命名规则（沿用 `region.rs` 的既有惯例）**：
//!
//! - **`TraeWork`（默认变体）沿用无后缀的旧文件名** —— 老用户既有数据零失效；
//! - **`TraeCn` 加 `.trae_cn` 中缀**，如 `checkin_accounts.trae_cn.json`。
//!
//! ⇒ 插入位置在**扩展名之前**，不是简单追加。这样文件名仍是 `.json` 结尾，
//! 与「同名不同目录」的旧约定不冲突，也让用户一眼能看出归属。
//!
//! 布局（`~/.buddy-switch/trae/`，括号内为 `TraeCn` 的额外后缀）：
//!
//! ```text
//! trae/
//! ├── settings.json                    # Trae 模块设置（端口、客户端路径、域名白名单…）※不分家
//! ├── checkin_accounts(.trae_cn).json  # 账号库（参考实现格式，UserID + jwt + refresh_token）
//! ├── groups(.trae_cn).json            # 分组与成员关系
//! ├── device_map(.trae_cn).json        # user_id -> 伪设备标识（与代理脚本共用）
//! ├── credits_history(.trae_cn).json   # 签到明细（按日期追加）
//! ├── credits_daily(.trae_cn).json     # 每日积分快照（趋势图数据源）
//! ├── remaining_credits(.trae_cn).json # 剩余积分与到期时间缓存
//! ├── account_cooldowns(.trae_cn).json # 签到错误冷却状态
//! ├── checkin_summary(.trae_cn).json   # 最近一次签到摘要
//! ├── api_pool.json                    # API 网关账号池配置（按变体分目录，见下）
//! ├── profiles/<user_id>/              # 登录态快照（精准复制的 9 类核心文件）
//! └── logs/                            # app / checkin / switcher / proxy 日志
//! ```
//!
//! ## 哪些**刻意不分家**
//!
//! - **`settings.json`**：端口、域名白名单、`traePath` 这类**应用级**配置，
//!   描述的是「本工具怎么工作」，不是「哪条产品线的数据」。两条产品线共用一份，
//!   否则用户在设置页改一次要改两遍。
//! - **`api_gateway.json` / `api_gateway_logs.json` / `api_pool.json`**：
//!   网关是**单一进程、单一监听端口**（7864），一次只能服务一个账号池。
//!   ⇒ 网关配置保持全局单份（与「网关是平行第二套实现」的既有设计一致）。
//!   `api_pool.json` 里若含按变体区分的账号引用，由**内容**区分，不由文件区分。
//! - **`logs/`**：日志按**文件名前缀**区分而非目录，便于在同一个 `logs/` 里对照排查。

use std::path::PathBuf;

use crate::modules::config::store_dir;

use super::variant::TraeVariant;

/// 为文件名插入变体中缀。
///
/// `TraeWork` 是默认变体，**原样返回**（沿用旧文件名，老数据不失效）；
/// 其余变体在**扩展名之前**插入 `.<variant>`，如
/// `checkin_accounts.json` + `TraeCn` → `checkin_accounts.trae_cn.json`。
///
/// 无扩展名时直接追加后缀（`foo` → `foo.trae_cn`），不会产生 `foo.trae_cn.` 这种悬空点。
fn variant_scoped_file_name(stem_with_ext: &str, variant: TraeVariant) -> String {
    if variant == TraeVariant::default() {
        return stem_with_ext.to_string();
    }
    match stem_with_ext.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}.{}.{ext}", variant.as_str()),
        None => format!("{stem_with_ext}.{}", variant.as_str()),
    }
}

/// 拼出某一变体的数据文件路径。
fn scoped_file(name_with_ext: &str, variant: TraeVariant) -> PathBuf {
    trae_dir().join(variant_scoped_file_name(name_with_ext, variant))
}

/// Trae 模块根目录：`~/.buddy-switch/trae`。
///
/// 与 WorkBuddy 数据同库不同名：复用 `store_dir()` 的 home 覆盖与兼容回落，
/// 但放在独立子目录，避免 `accounts.json` / `checkin_logs.json` 这类既有文件撞名。
pub fn trae_dir() -> PathBuf {
    store_dir().join("trae")
}

/// 确保目录存在后返回；`create_dir_all` 失败时仍返回路径（与仓库既有 `path()` 一致，
/// 由实际写入暴露错误，而不是在取路径阶段就失败）。
fn ensured(dir: PathBuf) -> PathBuf {
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Trae 模块设置文件。
///
/// **刻意不分变体**：描述的是「本工具怎么工作」（端口、白名单、客户端路径），
/// 不是某条产品线的数据。两条产品线共用一份，用户只需配置一次。
pub fn settings_file() -> PathBuf {
    trae_dir().join("settings.json")
}

/// 账号库文件（默认变体，兼容壳）。
pub fn accounts_file() -> PathBuf {
    accounts_file_for(TraeVariant::default())
}

/// 账号库文件（按变体分家）。
///
/// 这是「两条产品线账号互不污染」的**第一道闸门**：写错了这里，
/// 一个产品线的签到会把账号灌进另一个产品线的库。
pub fn accounts_file_for(variant: TraeVariant) -> PathBuf {
    scoped_file("checkin_accounts.json", variant)
}

/// 分组文件（默认变体，兼容壳）。
pub fn groups_file() -> PathBuf {
    groups_file_for(TraeVariant::default())
}

/// 分组文件（按变体分家）。
pub fn groups_file_for(variant: TraeVariant) -> PathBuf {
    scoped_file("groups.json", variant)
}

/// 设备标识映射文件（默认变体，兼容壳）。
pub fn device_map_file() -> PathBuf {
    device_map_file_for(TraeVariant::default())
}

/// 设备标识映射文件（按变体分家；签到与代理共用，结构必须一致）。
///
/// 分家的另一个理由：伪设备标识与账号绑定，两条产品线的 user_id 空间不同。
pub fn device_map_file_for(variant: TraeVariant) -> PathBuf {
    scoped_file("device_map.json", variant)
}

/// OAuth 登录设备身份文件（默认变体，兼容壳）。
///
/// 与 [`device_map_file`] 同构：落在 `trae/` 根、**不按变体分目录**，
/// 只按变体改文件名（`oauth_device(.trae_cn).json`）。
pub fn oauth_device_file() -> PathBuf {
    oauth_device_file_for(TraeVariant::default())
}

/// OAuth 登录设备身份文件（按变体分家）。
///
/// 存的是**授权 URL 用的身份 A2**：只有 `machine_id` 一个键（本机自造、变体级稳定）。
///
/// **`device_id` 不在这里**（曾经在，已移出）：授权 URL 的 `device_id` 是身份 A1，
/// 必须与 icube 设备凭证**同源**（= 签名私钥所属的那个 `icube-dc` deviceId），
/// 由 [`crate::modules::trae::icube::device_identity_for`] 提供 —— 否则服务端 20403/20405。
/// 自造 `device_id` 的能力在**类型层面**就不存在（见 `OAuthLoginMachine`）。
///
/// 与 `device_map.json`（身份 C，账号级）仍然不同源。
/// 分家的硬理由同 [`device_map_file_for`]：两条产品线的身份空间不同，
/// 混用不会报错、只会让上游把它们当成两个设备。
pub fn oauth_device_file_for(variant: TraeVariant) -> PathBuf {
    scoped_file("oauth_device.json", variant)
}

/// OAuth 客户端凭证外置配置文件：`trae/conf/oauth_client.json`。
///
/// **刻意不分变体**：`client_id` / `client_secret` / 交换路径是「客户端身份」，
/// 两条 CN 产品线实测共用同一套（见 `oauth_client` 模块文档）。
pub fn oauth_client_config_file() -> PathBuf {
    trae_dir().join("conf").join("oauth_client.json")
}

/// 某个快照槽位的**单代回滚目录**：`profiles[_<variant>]/<slot>.bak`。
///
/// 备份时先把旧槽位整体 rename 到这里，再拷新内容 —— 拷贝中断时上一份快照仍在，
/// 用户不会两头落空。返回 `None` 表示 `slot` 不是安全的目录名（含路径分隔符等）。
pub fn profile_bak_dir_for(variant: TraeVariant, slot: &str) -> Option<PathBuf> {
    if !safe_slot_name(slot) {
        return None;
    }
    Some(profiles_dir_for(variant).join(format!("{slot}.bak")))
}

/// 签到积分明细文件（默认变体，兼容壳）。
pub fn credits_history_file() -> PathBuf {
    credits_history_file_for(TraeVariant::default())
}

/// 签到积分明细文件（按变体分家）。
pub fn credits_history_file_for(variant: TraeVariant) -> PathBuf {
    scoped_file("credits_history.json", variant)
}

/// 每日积分快照文件（默认变体，兼容壳）。
pub fn credits_daily_file() -> PathBuf {
    credits_daily_file_for(TraeVariant::default())
}

/// 每日积分快照文件（按变体分家）。
pub fn credits_daily_file_for(variant: TraeVariant) -> PathBuf {
    scoped_file("credits_daily.json", variant)
}

/// 剩余积分缓存文件（默认变体，兼容壳）。
pub fn remaining_credits_file() -> PathBuf {
    remaining_credits_file_for(TraeVariant::default())
}

/// 剩余积分缓存文件（按变体分家）。
pub fn remaining_credits_file_for(variant: TraeVariant) -> PathBuf {
    scoped_file("remaining_credits.json", variant)
}

/// 账号冷却状态文件（默认变体，兼容壳）。
pub fn cooldowns_file() -> PathBuf {
    cooldowns_file_for(TraeVariant::default())
}

/// 账号冷却状态文件（按变体分家）。
///
/// 分家的硬理由：冷却表以 `user_id` 为键，两条产品线的 user_id 空间不重叠但**格式相同**，
/// 混在一起不会报错、只会让冷却状态错乱 —— 典型的静默缺陷。
pub fn cooldowns_file_for(variant: TraeVariant) -> PathBuf {
    scoped_file("account_cooldowns.json", variant)
}

/// 最近一次签到摘要文件（默认变体，兼容壳）。
pub fn checkin_summary_file() -> PathBuf {
    checkin_summary_file_for(TraeVariant::default())
}

/// 最近一次签到摘要文件（按变体分家）。
pub fn checkin_summary_file_for(variant: TraeVariant) -> PathBuf {
    scoped_file("checkin_summary.json", variant)
}

/// API 网关账号池配置文件。
///
/// **刻意不分变体**：网关是单一进程、单一监听端口（7864），一次只能服务一个账号池。
/// 池内若同时含两条产品线的账号，由**条目内容**区分归属，不由文件区分。
pub fn api_pool_file() -> PathBuf {
    trae_dir().join("api_pool.json")
}

/// API 网关运行配置（开关、监听地址/端口、日志保留、默认模型）。
pub fn api_gateway_file() -> PathBuf {
    trae_dir().join("api_gateway.json")
}

/// API 网关请求日志（JSON 数组，与 `logs/api.log` 的纯文本诊断日志分开）。
pub fn api_gateway_log_file() -> PathBuf {
    trae_dir().join("api_gateway_logs.json")
}

/// 登录态快照根目录（默认变体，兼容壳）。
pub fn profiles_dir() -> PathBuf {
    profiles_dir_for(TraeVariant::default())
}

/// 登录态快照根目录（按变体分家）。
///
/// **必须分家**：快照目录名就是 `user_id`，而两条产品线的 `user_id` 是两套空间。
/// 更关键的是——快照恢复会把文件写回**该变体客户端**的 userData，
/// 若把 Trae CN 账号的快照误用于 Trae Work，等于把一个未知格式的登录态灌进另一个客户端。
pub fn profiles_dir_for(variant: TraeVariant) -> PathBuf {
    // 默认变体沿用旧目录名 `profiles`（老用户的既有快照仍可用）；
    // 其余变体用 `profiles_trae_cn` 这样的独立目录。
    let name = if variant == TraeVariant::default() {
        "profiles".to_string()
    } else {
        format!("profiles_{}", variant.as_str())
    };
    ensured(trae_dir().join(name))
}

/// 单个账号的登录态快照目录（默认变体，兼容壳）。
pub fn profile_dir(user_id: &str) -> Option<PathBuf> {
    profile_dir_for(TraeVariant::default(), user_id)
}

/// 单个账号的登录态快照目录（按变体分家）。
///
/// `user_id` 直接拼进路径，必须先过滤掉路径分隔符与上跳片段，否则一个形如
/// `../../` 的 userId 会把快照写到数据目录之外。
pub fn profile_dir_for(variant: TraeVariant, user_id: &str) -> Option<PathBuf> {
    if !safe_slot_name(user_id) {
        return None;
    }
    Some(profiles_dir_for(variant).join(user_id))
}

/// 校验快照槽位名（userId）是否可作为单个目录名使用。
///
/// 拒绝：空、`.`、`..`、含 `/` 或 `\`、含 Windows 保留字符、含 NUL。
pub fn safe_slot_name(name: &str) -> bool {
    if name.is_empty() || name == "." || name == ".." {
        return false;
    }
    !name
        .chars()
        .any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c == '\0')
}

/// 日志目录。
///
/// **刻意不分变体**：日志按**文件名前缀**区分（见 [`log_file_for`]），
/// 让两条产品线的日志并排放在同一个 `logs/` 里，排查跨产品线问题时不必切目录。
pub fn logs_dir() -> PathBuf {
    ensured(trae_dir().join("logs"))
}

/// 单一日志文件路径。
pub fn log_file(name: &str) -> PathBuf {
    logs_dir().join(name)
}

/// 按变体取日志文件路径：在文件名前加变体前缀（默认变体不加）。
///
/// 例：`TraeCn` + `checkin.log` → `checkin.trae_cn.log`。
/// **未在文件名里出现 `.log` 时退化为直接追加**，不会丢扩展名。
pub fn log_file_for(variant: TraeVariant, name: &str) -> PathBuf {
    if variant == TraeVariant::default() {
        return log_file(name);
    }
    let scoped = match name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem}.{}.{ext}", variant.as_str()),
        None => format!("{name}.{}", variant.as_str()),
    };
    log_file(&scoped)
}

/// 应用级日志（切换、托盘、启动等关键路径）。
///
/// **不分变体**：这是「本应用自己」的日志，不归属任何产品线。
pub fn app_log_file() -> PathBuf {
    log_file("app.log")
}

/// 签到日志（默认变体，兼容壳）。
pub fn checkin_log_file() -> PathBuf {
    checkin_log_file_for(TraeVariant::default())
}

/// 签到日志（按变体分家）。
pub fn checkin_log_file_for(variant: TraeVariant) -> PathBuf {
    log_file_for(variant, "checkin.log")
}

/// 登录态切换日志（默认变体，兼容壳）。
pub fn switcher_log_file() -> PathBuf {
    switcher_log_file_for(TraeVariant::default())
}

/// 登录态切换日志（按变体分家）。
pub fn switcher_log_file_for(variant: TraeVariant) -> PathBuf {
    log_file_for(variant, "switcher.log")
}

/// 代理日志（默认变体，兼容壳）。
pub fn proxy_log_file() -> PathBuf {
    proxy_log_file_for(TraeVariant::default())
}

/// 代理日志（按变体分家）。
pub fn proxy_log_file_for(variant: TraeVariant) -> PathBuf {
    log_file_for(variant, "proxy.log")
}

/// 网关请求日志。
///
/// **不分变体**：网关只有一套（单一进程、单一端口）。
pub fn api_log_file() -> PathBuf {
    log_file("api.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trae_paths_live_under_store_dir_trae() {
        // 相对断言：不依赖真实 HOME，只验证层级关系与命名空间隔离。
        let base = store_dir();
        let dir = trae_dir();
        assert!(dir.starts_with(&base), "{dir:?} 应位于 {base:?} 之下");
        assert_eq!(dir.file_name().and_then(|s| s.to_str()), Some("trae"));
    }

    #[test]
    fn trae_files_do_not_collide_with_workbuddy_files() {
        // 本用例要断言「同一函数族返回的路径彼此一致」，而它们各自**独立**读取
        // 进程级的 `BUDDY_SWITCH_HOME`。若有并发测试在中途改掉该变量，
        // 就会出现「左边临时目录、右边真实目录」的假失败 —— 所以先取 env 锁，
        // 让本用例与所有改 home 的测试互斥（见 `config::env_lock`）。
        let _lock = crate::modules::config::env_lock();

        // 关键护栏：Trae 与 WorkBuddy 的账号/签到文件同名会互相覆盖。
        let wb_accounts = crate::modules::config::accounts_file();
        assert_ne!(accounts_file(), wb_accounts);
        assert_ne!(trae_dir(), store_dir());
        // 都在 trae/ 子目录内，而非 store_dir 根。
        for path in [
            accounts_file(),
            groups_file(),
            device_map_file(),
            credits_history_file(),
            credits_daily_file(),
            remaining_credits_file(),
            cooldowns_file(),
            checkin_summary_file(),
            api_pool_file(),
            api_gateway_file(),
            api_gateway_log_file(),
            settings_file(),
        ] {
            assert_eq!(path.parent(), Some(trae_dir().as_path()), "{path:?}");
        }
    }

    #[test]
    fn safe_slot_name_rejects_traversal_and_separators() {
        assert!(safe_slot_name("1234567890123456"));
        assert!(safe_slot_name("user_abc-1"));
        assert!(!safe_slot_name(""));
        assert!(!safe_slot_name("."));
        assert!(!safe_slot_name(".."));
        assert!(!safe_slot_name("../../etc/passwd"));
        assert!(!safe_slot_name("a/b"));
        assert!(!safe_slot_name("a\\b"));
        assert!(!safe_slot_name("C:evil"));
        assert!(!safe_slot_name("a*b"));
    }

    #[test]
    fn profile_dir_rejects_unsafe_user_id() {
        assert!(profile_dir("1234567890").is_some());
        assert!(profile_dir("../escape").is_none());
        assert!(profile_dir("").is_none());
        assert!(profile_dir_for(TraeVariant::TraeCn, "1234567890").is_some());
        assert!(profile_dir_for(TraeVariant::TraeCn, "../escape").is_none());
    }

    /// 默认变体**必须沿用旧文件名**，否则老用户数据白失效。
    ///
    /// 这条断言是本次分家改造的「零回归」承诺：TraeWork 是 `Default`，
    /// 它的路径必须与改造前**逐字相同**。改这里等于改兼容性契约。
    ///
    /// ## 为什么这里与 `trae_files_do_not_collide_with_workbuddy_files` 一样要取 env 锁
    ///
    /// 这些函数是**无参全局路径**，每次调用都重新读进程级 `BUDDY_SWITCH_HOME`。
    /// lib 单测在**同一进程内并行**跑，只要有别的用例（如 `handlers.rs` 里那两条
    /// 各自设不同临时目录的签到选项用例）中途改掉该变量并短暂恢复，
    /// 同一条 `assert_eq!` 的左右两侧就会取到不同的值 —— 症状是
    /// 「左右目录名差一个后缀」这种看起来像竞态、实则是**跨用例状态泄漏**的失败。
    ///
    /// 修法不是"给断言加容错"，而是让本用例与所有改 home 的用例互斥（取 env 锁）。
    #[test]
    fn 默认变体沿用旧文件名() {
        let _lock = crate::modules::config::env_lock();

        assert_eq!(accounts_file(), trae_dir().join("checkin_accounts.json"));
        assert_eq!(groups_file(), trae_dir().join("groups.json"));
        assert_eq!(device_map_file(), trae_dir().join("device_map.json"));
        assert_eq!(
            credits_history_file(),
            trae_dir().join("credits_history.json")
        );
        assert_eq!(profiles_dir(), trae_dir().join("profiles"));

        // 兼容壳 == 显式传默认变体。
        assert_eq!(accounts_file(), accounts_file_for(TraeVariant::default()));
        assert_eq!(settings_file(), trae_dir().join("settings.json"));
    }

    /// ★ 核心护栏：两条产品线的**每一个**分家文件都不能相同。
    ///
    /// 如果这个测试红了，说明有一个数据文件被两条产品线共用 ——
    /// 后果是「Trae CN 的签到把账号灌进 Trae Work 的库」这类静默污染。
    #[test]
    fn 两条产品线的数据文件互不相同() {
        let work = TraeVariant::TraeWork;
        let cn = TraeVariant::TraeCn;

        // 逐个函数族对拍：左边是 TraeWork，右边是 TraeCn。
        let pairs: [(&str, PathBuf, PathBuf); 7] = [
            ("账号库", accounts_file_for(work), accounts_file_for(cn)),
            ("分组", groups_file_for(work), groups_file_for(cn)),
            ("设备映射", device_map_file_for(work), device_map_file_for(cn)),
            (
                "签到明细",
                credits_history_file_for(work),
                credits_history_file_for(cn),
            ),
            (
                "每日积分",
                credits_daily_file_for(work),
                credits_daily_file_for(cn),
            ),
            (
                "剩余积分",
                remaining_credits_file_for(work),
                remaining_credits_file_for(cn),
            ),
            ("冷却状态", cooldowns_file_for(work), cooldowns_file_for(cn)),
        ];
        for (label, work_path, cn_path) in pairs {
            assert_ne!(work_path, cn_path, "{label} 被两条产品线共用: {work_path:?}");
        }

        // 签到摘要与快照目录单独对拍（它们不在上面的数组里）。
        assert_ne!(
            checkin_summary_file_for(work),
            checkin_summary_file_for(cn),
            "签到摘要被两条产品线共用"
        );
        assert_ne!(profiles_dir_for(work), profiles_dir_for(cn), "快照目录共用");
        assert_ne!(
            checkin_log_file_for(work),
            checkin_log_file_for(cn),
            "签到日志共用"
        );
    }

    /// 变体后缀必须插在**扩展名之前**，而不是简单追加。
    ///
    /// 若写反成 `checkin_accounts.json.trae_cn`，文件不再是 `.json` 结尾，
    /// 任何按扩展名过滤的逻辑（备份、清理、用户肉眼辨认）都会失效。
    ///
    /// 本用例只断言 **basename**（`file_name()`），不碰绝对路径 ——
    /// 这样它天然不受「别的用例改了 HOME」影响，**不需要** env 锁。
    /// 这是更可取的写法：能只看文件名就别看全路径。
    #[test]
    fn 变体后缀插在扩展名之前() {
        let cn = accounts_file_for(TraeVariant::TraeCn);
        let name = cn.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        assert_eq!(name, "checkin_accounts.trae_cn.json");

        let log = checkin_log_file_for(TraeVariant::TraeCn);
        let log_name = log.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        assert_eq!(log_name, "checkin.trae_cn.log");

        // 分家文件与默认变体同目录（只改文件名，不改目录层级）。
        // 用 basename 比较父级层级数，而不是比较绝对路径。
        assert_eq!(
            cn.parent().map(|p| p.file_name()),
            accounts_file_for(TraeVariant::TraeWork)
                .parent()
                .map(|p| p.file_name()),
            "两条产品线的数据文件必须落在同一个 trae/ 目录下"
        );
    }

    /// 刻意**不分家**的文件：应用级设置与网关配置。
    ///
    /// 这些是「本工具怎么工作」的配置，不是某条产品线的数据。
    /// 若有人"顺手"给它们也加了变体后缀，这条会红 —— 那是提醒他先想清楚。
    ///
    /// 同样要取 env 锁：断言里同时出现"无参全局路径"与"基于 `trae_dir()` 的期望值"，
    /// 二者若在两次读之间被别的用例改走 HOME 就会假失败。
    #[test]
    fn 应用级配置与网关配置刻意不分家() {
        let _lock = crate::modules::config::env_lock();

        // 只有无参版本，没有 `_for` 变体 —— 这是刻意的。
        assert_eq!(settings_file(), trae_dir().join("settings.json"));
        assert_eq!(api_pool_file(), trae_dir().join("api_pool.json"));
        assert_eq!(api_gateway_file(), trae_dir().join("api_gateway.json"));
        assert_eq!(
            api_gateway_log_file(),
            trae_dir().join("api_gateway_logs.json")
        );
        assert_eq!(app_log_file(), logs_dir().join("app.log"));
        assert_eq!(api_log_file(), logs_dir().join("api.log"));
    }

    /// 无扩展名的输入不能产生悬空点（`foo.trae_cn.`）。
    #[test]
    fn 无扩展名输入不产生悬空点() {
        assert_eq!(variant_scoped_file_name("bare", TraeVariant::TraeCn), "bare.trae_cn");
        assert_eq!(
            variant_scoped_file_name("a.b.json", TraeVariant::TraeCn),
            "a.b.trae_cn.json"
        );
        // 默认变体原样返回。
        assert_eq!(
            variant_scoped_file_name("checkin_accounts.json", TraeVariant::TraeWork),
            "checkin_accounts.json"
        );
    }

    /// ★ OAuth 设备身份文件：默认变体沿用无后缀名，另一变体加中缀；两者必须不同。
    ///
    /// 只比 basename，不碰绝对路径 —— 因此**不需要** env 锁（见
    /// [`Self::变体后缀插在扩展名之前`] 的说明）。
    #[test]
    fn oauth设备身份文件按变体分家且默认沿用旧名() {
        let work = oauth_device_file_for(TraeVariant::TraeWork);
        let cn = oauth_device_file_for(TraeVariant::TraeCn);
        assert_eq!(
            work.file_name().and_then(|s| s.to_str()),
            Some("oauth_device.json")
        );
        assert_eq!(
            cn.file_name().and_then(|s| s.to_str()),
            Some("oauth_device.trae_cn.json")
        );
        assert_ne!(work, cn, "两条产品线的 OAuth 设备身份必须分家");
        // 与 device_map 同目录（同构语义：只改文件名，不改目录层级）。
        assert_eq!(
            work.parent().map(|p| p.file_name()),
            device_map_file().parent().map(|p| p.file_name())
        );
        // 兼容壳 == 显式默认变体。
        assert_eq!(oauth_device_file(), work);
    }

    /// OAuth 客户端配置**刻意不分变体**（只有无参版本）。
    #[test]
    fn oauth客户端配置刻意不分变体() {
        let path = oauth_client_config_file();
        assert_eq!(
            path.file_name().and_then(|s| s.to_str()),
            Some("oauth_client.json")
        );
        // 落在 trae/conf/ 下（不是 trae/ 根，避免与数据文件混在一起）。
        assert_eq!(
            path.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()),
            Some("conf")
        );
    }

    /// 槽位回滚目录：`<slot>.bak`，且拒绝不安全的槽位名。
    ///
    /// 用 [`crate::modules::config::HomeOverrideGuard`] 隔离 home：
    /// `profiles_dir_for` 会 `create_dir_all`，不隔离就会在用户真实
    /// `~/.buddy-switch` 下建目录（测试纪律：不得写用户真实数据目录）。
    /// 该 guard 内部已取 `env_lock()`，因此本用例与其它改 home 的用例互斥。
    #[test]
    fn 槽位回滚目录为同目录单代备份() {
        let dir = std::env::temp_dir().join(format!(
            "buddy-switch-profile-bak-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("临时 home 应能创建");
        let guard = crate::modules::config::HomeOverrideGuard::set(&dir);

        let slot = profile_dir("1234567890123456").expect("安全槽位名应有快照目录");
        let bak = profile_bak_dir_for(TraeVariant::TraeWork, "1234567890123456")
            .expect("安全槽位名应有 .bak 路径");
        assert_eq!(
            bak.file_name().and_then(|s| s.to_str()),
            Some("1234567890123456.bak")
        );
        // 与正式槽位同目录（单代轮转靠 rename，必须同目录同卷）。
        assert_eq!(bak.parent(), slot.parent(), ".bak 必须与槽位同目录");
        assert_eq!(
            bak.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()),
            Some("profiles"),
            "默认变体的槽位目录名必须沿用旧名 profiles"
        );
        // 另一变体落在自己的 profiles_<variant>/ 下，互不干扰。
        let cn_bak = profile_bak_dir_for(TraeVariant::TraeCn, "1234567890123456").unwrap();
        assert_eq!(
            cn_bak.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()),
            Some("profiles_trae_cn")
        );
        assert_ne!(cn_bak.parent(), bak.parent());

        // 不安全的名字一律拒绝，绝不拼出越界路径。
        assert!(profile_bak_dir_for(TraeVariant::TraeWork, "../escape").is_none());
        assert!(profile_bak_dir_for(TraeVariant::TraeWork, "").is_none());
        assert!(profile_bak_dir_for(TraeVariant::TraeCn, "a/b").is_none());

        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
