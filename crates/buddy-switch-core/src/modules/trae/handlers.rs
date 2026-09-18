//! Trae 操作层：**每条逻辑操作的唯一实现**，供两条通道共用。
//!
//! ## 为什么要有这一层
//!
//! 本仓库同一个功能有两条入口：`crates/buddy-switch-server/src/api.rs`（HTTP/webui）
//! 与 `src-tauri/src/commands.rs`（Tauri invoke）。历史教训是：`copy_sessions` 在
//! Tauri 侧原样透传报告、HTTP 侧却把报告又包了一层 `"copied"`，响应退化成
//! `{copied:{copied:[…]}}` —— **编译与单测都不会报，只有真实调用才暴露**。
//!
//! 根因不是「某一边写错了」，而是「同一个逻辑被写了两遍」。因此这里的纪律是：
//!
//! - 任何**带聚合、过滤、默认值、错误文案**的操作，实现只写在本模块；
//! - `api.rs` 与 `commands.rs` 只做「解析参数 → 调本模块 → 转 HTTP/Tauri 响应」，
//!   不得自行拼装返回对象。
//!
//! 纯转发（例如 `platform::env_status()` 已返回完整线上形态）不必包一层，
//! 直接调用即可——多包一层反而增加漂移点。

use serde_json::{json, Value};

use crate::modules::trae::account::{self, Scope};
use crate::modules::trae::checkin::{self, CheckinOptions};
use crate::modules::trae::credits;
use crate::modules::trae::paths;
use crate::modules::trae::platform;
use crate::modules::trae::profile::{self, SwitchOptions};
use crate::modules::trae::settings;
use crate::modules::trae::store;
use crate::modules::trae::variant::TraeVariant;

/// 账号页需要的全部数据：账号视图 + 分组视图 + 概览计数（默认变体，兼容壳）。
pub fn accounts_overview() -> Value {
    accounts_overview_for(TraeVariant::default())
}

/// 账号页需要的全部数据（按变体分家）：账号视图 + 分组视图 + 概览计数。
///
/// 合并成一次返回而不是两次调用：前端账号页需要「分组带成员数」与「账号带 groupId」
/// 同时到位，分两次请求会出现「账号已挂到新分组但分组计数还是旧的」的闪动。
///
/// **整条链路用同一个 `variant`**：账号、分组、冷却三处都按变体分家，
/// 混用会让「冷却中 N 个」这个计数来自另一条产品线。
pub fn accounts_overview_for(variant: TraeVariant) -> Value {
    let accounts = account::list_account_views_for(variant);
    let groups = account::list_group_views_for(variant);
    let cooldowns = credits::load_cooldowns_for(variant);
    let now = chrono::Local::now().timestamp();
    let cooling = cooldowns
        .cooldowns
        .values()
        .filter(|entry| entry.until > now && !entry.error_type.is_empty())
        .count();
    let ungrouped = accounts
        .iter()
        .filter(|view| view.get("groupId").map(Value::is_null).unwrap_or(true))
        .count();
    json!({
        "accounts": accounts,
        "groups": groups,
        "total": accounts.len(),
        "cooling": cooling,
        "ungrouped": ungrouped,
    })
}

/// 签到状态：最近一次摘要 + 冷却明细 + 今日是否已有结果（默认变体，兼容壳）。
pub fn checkin_status() -> Value {
    checkin_status_for(TraeVariant::default())
}

/// 签到状态（按变体分家）：最近一次摘要 + 冷却明细 + 今日是否已有结果。
pub fn checkin_status_for(variant: TraeVariant) -> Value {
    let summary = credits::load_summary_for(variant);
    let cooldowns = credits::load_cooldowns_for(variant);
    let now = chrono::Local::now().timestamp();
    let today = store::today();
    let is_today = summary
        .time
        .as_ref()
        .map(|time| time.starts_with(&today))
        .unwrap_or(false);

    let cooldown_entries: Vec<Value> = cooldowns
        .cooldowns
        .iter()
        .filter(|(_, entry)| entry.until > now && !entry.error_type.is_empty())
        .map(|(uid, entry)| {
            json!({
                "userId": uid,
                "type": entry.error_type,
                "until": entry.until,
                "reason": entry.reason,
                "permanent": entry.until >= 9_999_999_999,
            })
        })
        .collect();

    json!({
        "summary": summary,
        "summaryIsToday": is_today,
        "cooldowns": cooldown_entries,
        "cooldownCount": cooldown_entries.len(),
        "logFile": paths::checkin_log_file_for(variant).to_string_lossy(),
    })
}

/// 积分总览：剩余积分、到期时间、签到明细、每日趋势、今日新增（默认变体，兼容壳）。
pub fn credits_overview() -> Value {
    credits_overview_for(TraeVariant::default())
}

/// 积分总览（按变体分家）：剩余积分、到期时间、签到明细、每日趋势、今日新增。
pub fn credits_overview_for(variant: TraeVariant) -> Value {
    let remaining = credits::load_remaining_for(variant);
    let history = credits::load_history_for(variant);
    let daily = credits::load_daily_for(variant);
    let today = store::today();

    // 每个账号只保留「最新日期、同日期取较大值」的一条，作为当前余额展示。
    let mut latest: std::collections::HashMap<String, &credits::CreditRecord> =
        std::collections::HashMap::new();
    for record in &history.records {
        match latest.get(&record.user_id) {
            None => {
                latest.insert(record.user_id.clone(), record);
            }
            Some(current) => {
                if record.date > current.date
                    || (record.date == current.date && record.credits > current.credits)
                {
                    latest.insert(record.user_id.clone(), record);
                }
            }
        }
    }
    let balances: Vec<Value> = latest
        .values()
        .map(|record| {
            json!({
                "userId": record.user_id,
                "credits": record.credits,
                "date": record.date,
            })
        })
        .collect();

    let today_earned: i64 = history
        .records
        .iter()
        .filter(|record| record.date == today)
        .map(|record| record.delta)
        .sum();

    json!({
        "remaining": remaining.credits,
        "expireTimes": remaining.expire_times,
        "updatedAt": remaining.updated_at,
        "balances": balances,
        "records": history.records.iter().map(|record| json!({
            "date": record.date,
            "userId": record.user_id,
            "credits": record.credits,
            "delta": record.delta,
        })).collect::<Vec<_>>(),
        "daily": daily.snapshots.iter().map(|snapshot| json!({
            "date": snapshot.date,
            "total": snapshot.total,
            "earned": snapshot.earned,
            "consumed": snapshot.consumed,
        })).collect::<Vec<_>>(),
        "todayEarned": today_earned,
        "historyDays": crate::modules::trae::TRAE_HISTORY_KEEP_DAYS,
    })
}

/// 新增账号（手动粘贴 JWT；默认变体，兼容壳）。
pub fn add_account(name: &str, jwt_value: &str, group_id: Option<&str>) -> Result<Value, String> {
    add_account_for(TraeVariant::default(), name, jwt_value, group_id)
}

/// 新增账号（按变体分家）。
pub fn add_account_for(
    variant: TraeVariant,
    name: &str,
    jwt_value: &str,
    group_id: Option<&str>,
) -> Result<Value, String> {
    let uid = account::add_manual_for(
        variant,
        name,
        jwt_value,
        group_id.map(|value| value.to_string()),
    )?;
    Ok(json!({ "userId": uid, "accounts": account::list_account_views_for(variant) }))
}

/// 更新账号（改名 / 换 JWT；默认变体，兼容壳）。
pub fn update_account(
    user_id: &str,
    name: Option<&str>,
    jwt_value: Option<&str>,
) -> Result<Value, String> {
    update_account_for(TraeVariant::default(), user_id, name, jwt_value)
}

/// 更新账号（按变体分家）。
pub fn update_account_for(
    variant: TraeVariant,
    user_id: &str,
    name: Option<&str>,
    jwt_value: Option<&str>,
) -> Result<Value, String> {
    account::update_for(
        variant,
        user_id,
        name.map(|value| value.to_string()),
        jwt_value.map(|value| value.to_string()),
    )?;
    Ok(json!({ "accounts": account::list_account_views_for(variant) }))
}

/// 删除账号（默认变体，兼容壳）。
pub fn delete_account(user_id: &str, delete_profile: bool) -> Result<Value, String> {
    delete_account_for(TraeVariant::default(), user_id, delete_profile)
}

/// 删除账号（按变体分家）。
pub fn delete_account_for(
    variant: TraeVariant,
    user_id: &str,
    delete_profile: bool,
) -> Result<Value, String> {
    account::delete_for(variant, user_id, delete_profile)?;
    Ok(json!({
        "deleted": user_id,
        "profileDeleted": delete_profile,
        "accounts": account::list_account_views_for(variant),
    }))
}

/// 从客户端登录态导入当前账号（对齐 WorkBuddy 的「导入本机账号」）。
///
/// 旧签名是无参封装（固定用 [`TraeVariant::default`]），既有调用点零改动。
/// 新增 [`import_local_account_for`] 以便按产品线变体指定要读哪条线的 userData。
pub fn import_local_account() -> Result<Value, String> {
    import_local_account_for(TraeVariant::default())
}

/// 按**产品线变体**从客户端登录态导入当前账号。
pub fn import_local_account_for(variant: TraeVariant) -> Result<Value, String> {
    let record = account::import_local_for(variant)?;
    let user_id = account::resolve_user_id(&record);
    Ok(json!({
        "userId": user_id,
        "name": record.name,
        "accounts": account::list_account_views_for(variant),
    }))
}

/// 导出账号：按 userId 列表回传完整记录（含 JWT）。
pub fn export_accounts(user_ids: &[String]) -> Result<Value, String> {
    export_accounts_for(TraeVariant::default(), user_ids)
}

/// 导出账号（按变体分家）：按 userId 列表回传完整记录（含 JWT）。
pub fn export_accounts_for(variant: TraeVariant, user_ids: &[String]) -> Result<Value, String> {
    let records = crate::modules::trae::export_import::export_accounts_for(variant, user_ids)?;
    Ok(json!({ "accounts": records }))
}

/// 导出账号到指定路径（桌面端保存对话框产物），返回写入路径。
pub fn export_accounts_to_path(user_ids: &[String], path: &str) -> Result<Value, String> {
    export_accounts_to_path_for(TraeVariant::default(), user_ids, path)
}

/// 导出账号到指定路径（按变体分家）。
pub fn export_accounts_to_path_for(
    variant: TraeVariant,
    user_ids: &[String],
    path: &str,
) -> Result<Value, String> {
    let written =
        crate::modules::trae::export_import::export_accounts_to_path_for(variant, user_ids, path)?;
    Ok(json!({ "path": written, "count": user_ids.len() }))
}

/// 解析导入文件并回传脱敏预览（与变体无关：只解析文本，不落库）。
pub fn preview_import_file(text: &str) -> Result<Value, String> {
    crate::modules::trae::export_import::preview_accounts(text)
}

/// 按选中索引导入账号，返回计数与最新账号视图（默认变体，兼容壳）。
pub fn import_accounts(text: &str, indexes: &[usize]) -> Result<Value, String> {
    import_accounts_for(TraeVariant::default(), text, indexes)
}

/// 按选中索引导入账号（按变体分家），返回计数与最新账号视图。
pub fn import_accounts_for(
    variant: TraeVariant,
    text: &str,
    indexes: &[usize],
) -> Result<Value, String> {
    let result = crate::modules::trae::export_import::import_accounts_for(variant, text, indexes)?;
    Ok(json!({
        "imported": result.imported,
        "skipped": result.skipped,
        "overwritten": result.overwritten,
        "accounts": account::list_account_views_for(variant),
    }))
}

/// 分组操作的分发（创建 / 更新 / 删除 / 移动；默认变体，兼容壳）。
pub fn group_op(action: &str, params: &Value) -> Result<Value, String> {
    group_op_for(TraeVariant::default(), action, params)
}

/// 分组操作的分发（按变体分家；创建 / 更新 / 删除 / 移动）。
///
/// 四个动作共用一次分发而不是四条独立函数：它们共享同一套入参解析与
/// 「操作后回传最新分组视图」的返回契约，分开写会重复四遍返回拼装。
///
/// **分组按变体分家**：`group_create` 的「同名分组已存在」判定只在变体内生效，
/// 两条产品线可以有同名分组而互不冲突。
pub fn group_op_for(
    variant: TraeVariant,
    action: &str,
    params: &Value,
) -> Result<Value, String> {
    let str_param = |key: &str| params.get(key).and_then(Value::as_str);
    match action {
        "create" => {
            let name = str_param("name").unwrap_or("");
            let color = str_param("color").unwrap_or("#8b5cf6");
            let id = account::group_create_for(variant, name, color)?;
            Ok(json!({ "id": id, "groups": account::list_group_views_for(variant) }))
        }
        "update" => {
            let id = str_param("id").ok_or("缺少分组 id")?;
            account::group_update_for(
                variant,
                id,
                str_param("name").map(str::to_string),
                str_param("color").map(str::to_string),
                params.get("order").and_then(Value::as_i64).map(|v| v as i32),
            )?;
            Ok(json!({ "groups": account::list_group_views_for(variant) }))
        }
        "delete" => {
            let id = str_param("id").ok_or("缺少分组 id")?;
            account::group_delete_for(variant, id)?;
            Ok(json!({ "groups": account::list_group_views_for(variant) }))
        }
        "move" => {
            let user_id = str_param("userId").ok_or("缺少 userId")?;
            let group_id = str_param("groupId").map(str::to_string);
            account::move_to_group_for(variant, user_id, group_id)?;
            Ok(json!({ "accounts": account::list_account_views_for(variant) }))
        }
        other => Err(format!("未知的分组操作: {other}")),
    }
}

/// 从请求参数里解析产品线变体（两条通道共用的唯一入口）。
///
/// ## 为什么缺省 / 非法一律回落默认变体，而不是报错
///
/// `variant` 是本轮新加的**可选**参数：老前端的请求里根本没有这个键，
/// 若把它做成必填或对未知值报错，用户升级后会立刻看到一片「参数非法」的红字，
/// 而真正的问题只是「客户端还没跟上」。因此规则是**宽容的**：
///
/// - `trae_work` → [`TraeVariant::TraeWork`]（同时也是缺省）
/// - `trae_cn` → [`TraeVariant::TraeCn`]
/// - 其它任何值（含缺失、空串、拼错、旧前端的 `null`）→ 默认变体
///
/// **默认变体沿用旧文件名**（存续层已如此设计），所以「回落默认」对老用户而言
/// 语义就是「行为和升级前一模一样」——这是安全的失败方向。
///
/// 反过来若回落成 `TraeCn`，老用户的账号库会瞬间看起来空了。
pub fn parse_variant_param(params: &Value) -> TraeVariant {
    params
        .get("variant")
        .and_then(Value::as_str)
        .and_then(TraeVariant::parse)
        .unwrap_or_default()
}

/// 签到网络失败重试次数的上限。
///
/// 无论来自请求参数还是设置，都必须夹到这个值：前端传 `1e9` 会造成事实上的死循环。
const MAX_CHECKIN_RETRY: u32 = 5;

/// 解析签到选项（两条通道共用的入参契约）。
///
/// `scope` 支持 `all` / `group:<id>` / `selected`；`selected` 时读 `userIds`。
/// `variant` 支持 `trae_work` / `trae_cn`，缺省 / 无法识别一律回落
/// [`TraeVariant::default`]（见 [`parse_variant_param`]）。
///
/// ## 三个签到参数都要回落设置（它们曾经全是假控件）
///
/// 设置页「签到期」暴露了三个控件，值都落在 `settings` 里：
/// `checkinSkipChecked` / `checkinSkipExpired` / `retry`。
/// 但本函数原先只读**当次请求**的同名参数（缺省硬编码 `true` / `true` / `1`），
/// **从不读设置** —— 于是这三个控件存了谁都不读的值：用户怎么切都没有任何效果。
///
/// 现在的优先级是：**请求参数 > 用户设置 > 内置默认**。
/// 这样既让设置真正生效（用户切了就该变），又保留了「单次调用可覆盖」的能力
/// （例如卡片上的单账号签到要显式跳过某些检查）。
///
/// 回落读取设置文件（一次小 JSON 读），因此带 HOME 覆盖的测试需自行隔离目录。
pub fn parse_checkin_options(params: &Value) -> Result<CheckinOptions, String> {
    let scope_raw = params
        .get("scope")
        .and_then(Value::as_str)
        .unwrap_or("all");
    let scope = Scope::parse(scope_raw)?;
    let user_ids = params.get("userIds").and_then(Value::as_array).map(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>()
    });

    // 只在真的需要回落时才读设置：三个参数都显式给了就完全不碰磁盘。
    let needs_settings = ["skipCheckedIn", "skipExpired", "retry"]
        .iter()
        .any(|key| params.get(*key).is_none());
    let settings = needs_settings.then(settings::load);

    Ok(CheckinOptions {
        variant: parse_variant_param(params),
        scope,
        user_ids,
        skip_checked_in: params
            .get("skipCheckedIn")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| settings.as_ref().is_none_or(|s| s.checkin_skip_checked)),
        skip_expired: params
            .get("skipExpired")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| settings.as_ref().is_none_or(|s| s.checkin_skip_expired)),
        retry: params
            .get("retry")
            .and_then(Value::as_u64)
            .map(|value| value as u32)
            // 设置里的 retry 是 i32：负数与超上限都要夹住，不能让脏设置把循环放大。
            .or_else(|| {
                settings
                    .as_ref()
                    .map(|s| u32::try_from(s.retry).unwrap_or(0))
            })
            .unwrap_or(1)
            .min(MAX_CHECKIN_RETRY),
    })
}

/// 刷新剩余积分：给了 `userId` 就刷单个，否则刷全部（默认变体，兼容壳）。
pub async fn refresh_credits(user_id: Option<&str>) -> Result<Value, String> {
    refresh_credits_for(TraeVariant::default(), user_id).await
}

/// 刷新剩余积分（按变体分家）：给了 `userId` 就刷单个，否则刷**该变体**全部。
pub async fn refresh_credits_for(
    variant: TraeVariant,
    user_id: Option<&str>,
) -> Result<Value, String> {
    match user_id.filter(|id| !id.is_empty()) {
        Some(user_id) => {
            let credits_value = credits::refresh_remaining_for_variant(variant, user_id).await?;
            Ok(json!({
                "scope": "single",
                "userId": user_id,
                "credits": credits_value,
                "refreshed": 1,
            }))
        }
        None => {
            let refreshed = credits::refresh_all_remaining_for(variant).await;
            Ok(json!({ "scope": "all", "refreshed": refreshed }))
        }
    }
}

/// 刷新某账号 JWT（默认变体，兼容壳）。
pub async fn refresh_jwt(user_id: &str) -> Result<Value, String> {
    refresh_jwt_for(TraeVariant::default(), user_id).await
}

/// 刷新某账号 JWT（按变体分家）。
pub async fn refresh_jwt_for(variant: TraeVariant, user_id: &str) -> Result<Value, String> {
    let jwt_value = account::refresh_jwt_for(variant, user_id).await?;
    let info = crate::modules::trae::jwt::parse(&jwt_value);
    Ok(json!({
        "userId": user_id,
        "jwt": jwt_value,
        "jwtExpTimestamp": info.exp_timestamp,
        "jwtExpHours": info.exp_hours,
        "accounts": account::list_account_views_for(variant),
    }))
}

/// 清除冷却：给了 `userId` 就清单个，否则全清（默认变体，兼容壳）。
pub fn clear_cooldown(user_id: Option<&str>) -> Result<Value, String> {
    clear_cooldown_for(TraeVariant::default(), user_id)
}

/// 清除冷却（按变体分家）：给了 `userId` 就清单个，否则清**该变体**全部。
pub fn clear_cooldown_for(
    variant: TraeVariant,
    user_id: Option<&str>,
) -> Result<Value, String> {
    match user_id.filter(|id| !id.is_empty()) {
        Some(user_id) => {
            credits::clear_cooldown_for(variant, user_id)?;
            Ok(json!({ "scope": "single", "userId": user_id }))
        }
        None => {
            let cleared = credits::clear_all_cooldowns_for(variant)?;
            Ok(json!({ "scope": "all", "cleared": cleared }))
        }
    }
}

/// 保存当前登录态到指定账号槽位（默认变体，兼容壳）。
pub fn save_login(user_id: &str) -> Result<Value, String> {
    save_login_for(TraeVariant::default(), user_id)
}

/// 保存当前登录态到指定账号槽位（按变体分家）。
pub fn save_login_for(variant: TraeVariant, user_id: &str) -> Result<Value, String> {
    let files = profile::save_current_login_for(variant, user_id)?;
    Ok(json!({ "userId": user_id, "fileCount": files }))
}

/// 备份当前登录态到指定账号槽位（等价于 [`save_login`]，语义更贴近前端的「备份」按钮）。
pub fn backup_profile(user_id: &str) -> Result<Value, String> {
    backup_profile_for(TraeVariant::default(), user_id)
}

/// 备份当前登录态到指定账号槽位（按变体分家）。
pub fn backup_profile_for(variant: TraeVariant, user_id: &str) -> Result<Value, String> {
    let files = profile::backup_to_slot_for(variant, user_id)?;
    Ok(json!({ "slot": user_id, "fileCount": files }))
}

/// 用指定槽位的快照覆盖客户端登录态（不关闭/不启动客户端）。
///
/// 与 [`switch_account`] 的区别：本操作**不做**「先保存当前」的兜底，
/// 因此只应在客户端已关闭时使用；界面上它是「高级操作」，默认入口一律走切换。
pub fn restore_profile(user_id: &str) -> Result<Value, String> {
    restore_profile_for(TraeVariant::default(), user_id)
}

/// 用指定槽位的快照覆盖客户端登录态（按变体分家）。
pub fn restore_profile_for(variant: TraeVariant, user_id: &str) -> Result<Value, String> {
    let files = profile::restore_from_slot_for(variant, user_id)?;
    Ok(json!({ "slot": user_id, "fileCount": files }))
}

/// 删除登录态快照（默认变体，兼容壳）。
pub fn delete_profile(slot: &str) -> Result<Value, String> {
    delete_profile_for(TraeVariant::default(), slot)
}

/// 删除登录态快照（按变体分家）。
pub fn delete_profile_for(variant: TraeVariant, slot: &str) -> Result<Value, String> {
    profile::delete_slot_for(variant, slot)?;
    Ok(json!({ "slot": slot }))
}

/// 解析切换选项（两条通道共用的入参契约）。
pub fn parse_switch_options(params: &Value) -> Result<SwitchOptions, String> {
    let user_id = params
        .get("userId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if user_id.is_empty() {
        return Err("缺少 userId".into());
    }
    Ok(SwitchOptions {
        user_id,
        launch: params.get("launch").and_then(Value::as_bool).unwrap_or(true),
        proxy_port: params
            .get("proxyPort")
            .and_then(Value::as_u64)
            .map(|port| port as u16),
        reset_device: params
            .get("resetDevice")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        // 变体与其他 handler 走同一个解析器：老请求没有这个键时回落默认变体，
        // 而默认变体沿用旧路径，故老调用点行为完全不变。
        variant: parse_variant_param(params),
    })
}

/// 重置 Trae 客户端设备标识（6 层；默认变体，兼容壳）。
pub fn reset_device() -> Result<Value, String> {
    platform::reset_device_identity()
}

/// 重置 Trae 客户端设备标识（6 层；按变体分家）。
pub fn reset_device_for(variant: TraeVariant) -> Result<Value, String> {
    platform::reset_device_identity_for(variant)
}

// ---------------------------------------------------------------------------
// OAuth 登录（浏览器授权 + 本地回调监听）
// ---------------------------------------------------------------------------

/// 发起 OAuth 登录：开本地回调监听并回传授权 URL。
///
/// `variant` 必须由调用方从请求参数解析后传入（见 [`parse_variant_param`]），
/// 它同时决定三件事：
/// 1. 授权 URL 里的设备身份（身份 A，`oauth_device.<变体>.json`，变体级持久稳定）；
/// 2. 会话归属——兑换成功后账号落进哪条产品线的账号库；
/// 3. 轮询回包里的 `variant` / `variantLabel`，前端据此决定刷哪个列表。
///
/// 返回 `{ loginId, verificationUri, expiresIn, port, variant, variantLabel,
/// deviceCredential }`。
/// **本函数不打开浏览器**：那是调用方（Tauri / 前端）的职责——
/// core 层不该知道「怎么在用户的系统上打开一个 URL」（三平台各一套，且要处理
/// 无 GUI 的 webui 场景）。
pub async fn oauth_login_start_for(variant: TraeVariant) -> Result<Value, String> {
    crate::modules::trae::oauth::login_start_for(variant).await
}

/// 轮询 OAuth 登录结果。
///
/// 形状与 WorkBuddy 侧 `oauth_status` 对齐：
/// `{ done, account?, accounts?, error?, port?, variant }`。
/// **不返回 Err**：与 [`crate::modules::trae::oauth::login_poll`] 同因——
/// 轮询是高频调用，「还没好」不该表达成异常。
///
/// 成功后顺带把最新账号视图附上：前端拿到 `done: true` 就要立刻刷列表，
/// 让它再发一次请求既慢又多一处不一致窗口。
///
/// 账号视图必须取自**该会话所属变体**：`login_poll` 会回传 `variant`，用它而不是
/// 默认变体——否则「Trae CN 登录成功却把 Trae Work 的账号列表刷出来」，
/// 用户会以为账号丢了（实际是读错了库）。
pub fn oauth_login_status(login_id: &str) -> Value {
    let mut value = crate::modules::trae::oauth::login_poll(login_id);
    if value.get("account").is_some() {
        let variant = value
            .get("variant")
            .and_then(Value::as_str)
            .and_then(TraeVariant::parse)
            .unwrap_or_default();
        if let Some(object) = value.as_object_mut() {
            object.insert(
                "accounts".into(),
                Value::Array(account::list_account_views_for(variant)),
            );
        }
    }
    value
}

/// 取消 OAuth 登录（用户关掉了对话框），释放监听端口。
pub fn oauth_login_cancel(login_id: &str) -> Value {
    let cancelled = crate::modules::trae::oauth::login_cancel(login_id);
    json!({ "cancelled": cancelled })
}

/// 保存 Trae 设置（局部更新）。
pub fn save_settings(patch: Value) -> Result<Value, String> {
    let updated = settings::patch(patch)?;
    serde_json::to_value(updated).map_err(|e| e.to_string())
}

/// Token 统计（聚合本机 Trae 网关请求日志）。
///
/// `days` 为统计窗口天数；`None` / `<= 0` 表示全部历史。
/// 解析细节与三条边界见 [`crate::modules::trae::token_stats`]。
pub fn token_statistics(days: Option<i64>) -> Value {
    crate::modules::trae::token_stats::get_statistics(days)
}

/// 运行日志（系统日志页的「运行日志」标签页）。
///
/// 只做转发：过滤 / 排序 / 截断 / 边界都在 [`crate::modules::trae::logs`]，
/// 两条通道因此不可能给出不同结果。
///
/// 变体从 `params.variant` 读取（与签到的处理一致：入参整体是一个 params 对象，
/// 不再单独加顶层参数，避免两条通道的契约分叉）。
pub fn logs(params: &Value) -> Value {
    let variant = parse_variant_param(params);
    crate::modules::trae::logs::query_logs_for(variant, params)
}

/// 签到并返回线上形态报告（HTTP 通道用；Tauri 通道改用 [`checkin::run_checkin`]
/// 并传入事件回调，以便逐条推送进度）。
pub async fn run_checkin_report(options: CheckinOptions) -> Value {
    let report = checkin::run_checkin(options, |_| {}).await;
    checkin::report_json(&report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_overview_has_stable_shape() {
        let value = accounts_overview();
        for key in ["accounts", "groups", "total", "cooling", "ungrouped"] {
            assert!(value.get(key).is_some(), "缺少字段 {key}");
        }
        assert!(value.get("accounts").unwrap().is_array());
        assert!(value.get("groups").unwrap().is_array());
        // 计数类字段必须是数字，不是字符串
        assert!(value.get("total").unwrap().is_u64());
        assert!(value.get("cooling").unwrap().is_u64());
    }

    #[test]
    fn checkin_status_shape_is_camel_case() {
        let value = checkin_status();
        for key in [
            "summary",
            "summaryIsToday",
            "cooldowns",
            "cooldownCount",
            "logFile",
        ] {
            assert!(value.get(key).is_some(), "缺少字段 {key}");
        }
        assert!(value.get("summary_is_today").is_none());
    }

    #[test]
    fn credits_overview_is_camel_case_and_numeric() {
        let value = credits_overview();
        for key in [
            "remaining",
            "expireTimes",
            "updatedAt",
            "balances",
            "records",
            "daily",
            "todayEarned",
            "historyDays",
        ] {
            assert!(value.get(key).is_some(), "缺少字段 {key}");
        }
        assert!(value.get("expire_times").is_none());
        assert!(value.get("todayEarned").unwrap().is_i64());
    }

    #[test]
    fn parse_checkin_options_defaults_match_module_defaults() {
        // 未显式传 `skipCheckedIn` 时会读设置文件，因此必须隔离 HOME：
        // 否则本用例的结果取决于跑测试的机器上那份真实设置，会随用户操作而红。
        // 走 `HomeOverrideGuard`（内部已持 env 锁并在 drop 时还原），不要手动加锁。
        let dir = std::env::temp_dir().join(format!("trae-parse-checkin-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let _guard = crate::modules::config::HomeOverrideGuard::set(&dir);

        let options = parse_checkin_options(&json!({})).unwrap();
        assert_eq!(options.scope, Scope::All);
        // 隔离目录里没有设置文件 → 落 `TraeSettings` 的默认值，与模块默认一致。
        assert_eq!(options.skip_checked_in, settings::TraeSettings::default().checkin_skip_checked);
        assert!(options.skip_expired);
        // 默认 retry 与 CheckinOptions::default 必须一致，否则两条通道行为不同
        assert_eq!(options.retry, CheckinOptions::default().retry);
    }

    /// 设置页的三个签到控件必须真的生效。
    ///
    /// 这是回归护栏 —— 修之前 `parse_checkin_options` 对这三个值全部硬编码
    /// （`true` / `true` / `1`），从不读设置，用户在设置页怎么改都没有效果。
    #[test]
    fn checkin_settings_actually_reach_checkin_options() {
        let dir = std::env::temp_dir().join(format!("trae-parse-skip-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let _guard = crate::modules::config::HomeOverrideGuard::set(&dir);

        // 默认（设置文件不存在）三个值都走内置默认。
        let defaults = parse_checkin_options(&json!({})).unwrap();
        assert!(defaults.skip_checked_in);
        assert!(defaults.skip_expired);
        assert_eq!(defaults.retry, 1);

        // 三个都改掉 → 必须全部传进签到选项。
        settings::patch(json!({
            "checkinSkipChecked": false,
            "checkinSkipExpired": false,
            "retry": 3
        }))
        .expect("写入设置不应失败");
        let patched = parse_checkin_options(&json!({})).unwrap();
        assert!(
            !patched.skip_checked_in,
            "checkinSkipChecked 没有传到签到选项，控件是假的"
        );
        assert!(
            !patched.skip_expired,
            "checkinSkipExpired 没有传到签到选项，控件是假的"
        );
        assert_eq!(patched.retry, 3, "retry 没有传到签到选项，控件是假的");

        // 改回来 → 必须跟着恢复（证明确实是读设置，而不是「一旦为 false 就卡住」）。
        settings::patch(json!({
            "checkinSkipChecked": true,
            "checkinSkipExpired": true,
            "retry": 1
        }))
        .expect("写入设置不应失败");
        let restored = parse_checkin_options(&json!({})).unwrap();
        assert!(restored.skip_checked_in);
        assert!(restored.skip_expired);
        assert_eq!(restored.retry, 1);

        // 请求参数优先于设置（单次调用可覆盖）。
        settings::patch(json!({
            "checkinSkipChecked": true,
            "checkinSkipExpired": true,
            "retry": 1
        }))
        .expect("写入设置不应失败");
        let overridden = parse_checkin_options(&json!({
            "skipCheckedIn": false,
            "skipExpired": false,
            "retry": 4
        }))
        .unwrap();
        assert!(!overridden.skip_checked_in, "请求参数应优先于设置");
        assert!(!overridden.skip_expired, "请求参数应优先于设置");
        assert_eq!(overridden.retry, 4, "请求参数应优先于设置");
    }

    /// 设置里的脏 `retry`（负数 / 超大）必须被夹住，不能把重试循环放大。
    #[test]
    fn checkin_retry_from_settings_is_clamped() {
        let dir = std::env::temp_dir().join(format!("trae-parse-retry-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let _guard = crate::modules::config::HomeOverrideGuard::set(&dir);

        settings::patch(json!({ "retry": -5 })).expect("写入设置不应失败");
        assert_eq!(
            parse_checkin_options(&json!({})).unwrap().retry,
            0,
            "负重试次数应归零，而不是当成无符号大数"
        );

        settings::patch(json!({ "retry": 9999 })).expect("写入设置不应失败");
        assert_eq!(
            parse_checkin_options(&json!({})).unwrap().retry,
            MAX_CHECKIN_RETRY,
            "超上限的重试次数必须被夹到 MAX_CHECKIN_RETRY"
        );
    }

    #[test]
    fn parse_checkin_options_reads_selected_scope() {
        let options = parse_checkin_options(&json!({
            "scope": "selected",
            "userIds": ["a", "b", 3],
            "skipCheckedIn": false,
            "retry": 99
        }))
        .unwrap();
        // `scope` 只承载「范围类型」；具体勾选的账号在 `user_ids` 里，
        // 由 `account::resolve_user_ids` 合并（Selected 支持两处来源）。
        assert_eq!(options.scope, Scope::Selected(Vec::new()));
        assert_eq!(options.user_ids, Some(vec!["a".into(), "b".into()]));
        // 非字符串项必须被丢弃，而不是变成 "3"
        assert!(!options.user_ids.as_ref().unwrap().contains(&"3".to_string()));
        assert!(!options.skip_checked_in);
        // retry 必须被夹到上限，避免前端传入 1e9 造成无限重试
        assert_eq!(options.retry, 5);
    }

    #[test]
    fn parse_checkin_options_rejects_unknown_scope() {
        assert!(parse_checkin_options(&json!({ "scope": "grup:x" })).is_err());
    }

    #[test]
    fn parse_switch_options_requires_user_id() {
        assert!(parse_switch_options(&json!({})).is_err());
        assert!(parse_switch_options(&json!({ "userId": "   " })).is_err());
        let options = parse_switch_options(&json!({ "userId": "u1" })).unwrap();
        assert_eq!(options.user_id, "u1");
        // 默认要启动客户端：切换后不启动等于让用户手动再点一次
        assert!(options.launch);
        assert!(!options.reset_device);
        assert_eq!(options.proxy_port, None);
    }

    #[test]
    fn parse_switch_options_reads_proxy_port_and_flags() {
        let options = parse_switch_options(&json!({
            "userId": "u1",
            "launch": false,
            "proxyPort": 8899,
            "resetDevice": true
        }))
        .unwrap();
        assert!(!options.launch);
        assert_eq!(options.proxy_port, Some(8899));
        assert!(options.reset_device);
    }

    #[test]
    fn group_op_rejects_unknown_action() {
        assert!(group_op("nope", &json!({})).is_err());
        assert!(group_op("create", &json!({})).is_err(), "空名分组应被拒绝");
        assert!(group_op("update", &json!({})).is_err(), "缺少 id");
        assert!(group_op("delete", &json!({})).is_err(), "缺少 id");
        assert!(group_op("move", &json!({})).is_err(), "缺少 userId");
    }

    #[test]
    fn clear_cooldown_single_vs_all_scope_is_explicit() {
        // 返回必须带上 scope，前端才能区分「清了一个」与「清了全部」。
        let single = clear_cooldown(Some("u1")).unwrap();
        assert_eq!(single.get("scope").unwrap().as_str(), Some("single"));
        let all = clear_cooldown(None).unwrap();
        assert_eq!(all.get("scope").unwrap().as_str(), Some("all"));
        // 空字符串等同未提供
        let all2 = clear_cooldown(Some("")).unwrap();
        assert_eq!(all2.get("scope").unwrap().as_str(), Some("all"));
    }
}
