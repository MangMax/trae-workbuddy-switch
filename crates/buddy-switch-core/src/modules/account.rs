//! 账号存储：读取/写入 `~/.buddy-switch/accounts.json`，与 Python 版共享数据目录。
//!
//! 对照 server.py `load_accounts` / `save_accounts` / `find_account` /
//! `account_display_name` / `account_meta`。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

use crate::modules::config::{atomic_write, now_ms};
use crate::modules::region::{accounts_file_for, region_display, region_of, region_spec, Region};

/// 是否持有未过期的明文 `access_token`（OAuth 扫码所得形态）。
/// 无 `expiresAt` 时视为有效（保守：不因缺字段丢弃明文凭据）。
fn has_unexpired_plain_token(acc: &Value) -> bool {
    let Some(Value::String(s)) = acc.get("access_token") else {
        return false;
    };
    if s.trim().is_empty() {
        return false;
    }
    match acc.get("expiresAt").and_then(|v| v.as_i64()) {
        Some(exp) => exp > now_ms(),
        None => true,
    }
}

fn load_accounts_from_path(path: &Path) -> Vec<Value> {
    if let Ok(text) = std::fs::read_to_string(path) {
        if let Ok(Value::Array(accounts)) = serde_json::from_str::<Value>(&text) {
            return accounts;
        }
    }
    vec![]
}

fn save_accounts_to_path(path: &Path, accounts: &[Value]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(accounts).unwrap_or_default();
    atomic_write(path, &content)
}

fn find_account_in(accounts: &[Value], account_id: &str) -> Option<Value> {
    accounts
        .iter()
        .find(|account| {
            account.get("id").and_then(Value::as_str) == Some(account_id)
                || account.get("uid").and_then(Value::as_str) == Some(account_id)
        })
        .cloned()
}

fn delete_account_from_path(path: &Path, account_id: &str) -> Result<(), String> {
    let mut accounts = load_accounts_from_path(path);
    let before = accounts.len();
    accounts.retain(|account| account.get("id").and_then(Value::as_str) != Some(account_id));
    if accounts.len() == before {
        return Err("账号不存在".to_string());
    }
    save_accounts_to_path(path, &accounts).map_err(|error| error.to_string())
}

/// 读取账号库（CN）；文件缺失或损坏返回空列表。
pub fn load_accounts() -> Vec<Value> {
    load_accounts_for(Region::Cn)
}

/// 按 region 读取账号库；文件缺失或损坏返回空列表。
pub fn load_accounts_for(region: Region) -> Vec<Value> {
    load_accounts_from_path(&accounts_file_for(region))
}

/// 写回 CN 账号库（原子写），保持原 JSON 数组结构。
pub fn save_accounts(accounts: &[Value]) -> std::io::Result<()> {
    save_accounts_for(Region::Cn, accounts)
}

/// 按 region 写回账号库（原子写）。
pub fn save_accounts_for(region: Region, accounts: &[Value]) -> std::io::Result<()> {
    save_accounts_to_path(&accounts_file_for(region), accounts)
}

/// 按 id 或 uid 在 CN 账号库查找账号。
pub fn find_account(account_id: &str) -> Option<Value> {
    find_account_for(Region::Cn, account_id)
}

/// 按 region 在账号库查找账号。
pub fn find_account_for(region: Region, account_id: &str) -> Option<Value> {
    find_account_in(&load_accounts_for(region), account_id)
}

/// 账号展示名（email → nickname → uid → unknown）。
pub fn account_display_name(acc: &Value) -> String {
    get_str(acc, "email")
        .or_else(|| get_str(acc, "nickname"))
        .or_else(|| get_str(acc, "uid"))
        .unwrap_or_else(|| "unknown".to_string())
}

/// 账号的展示元数据（不泄露 token）。对照 server.py `account_meta`。
///
/// 展示字段一律走 [`display_value`]：WorkBuddy 5.6 起 `nickname` / `phoneNumber`
/// 等可能是加密信封对象，裸透传会让前端把它当 React 子节点渲染，
/// 触发 error #31 整树卸载（白屏）。
pub fn account_meta(acc: &Value) -> Value {
    json!({
        "id": display_value(acc, "id"),
        "uid": display_value(acc, "uid"),
        "email": display_value(acc, "email"),
        "nickname": display_value(acc, "nickname"),
        "enterpriseName": display_value(acc, "enterpriseName"),
        "expiresAt": display_value(acc, "expiresAt"),
        "refreshExpiresAt": display_value(acc, "refreshExpiresAt"),
        "refreshedAt": display_value(acc, "refreshedAt"),
        "createdAt": display_value(acc, "createdAt"),
        "needsRelogin": acc.get("needs_relogin").and_then(|v| v.as_bool()) == Some(true),
        "needsReloginReason": display_value(acc, "needs_relogin_reason"),
    })
}

/// 取非空字符串字段；空/缺失返回 None。
pub fn get_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 字段是否为 WorkBuddy 5.6 加密信封对象（`{$wbEncrypted, envelope}`）。
///
/// 信封在本机同一 keyblob 下可由 WorkBuddy 自行解密，因此**不需要**我们实现解密：
/// 导入与切换写回时原样保留即可；只有需要明文 token 的接口（签到 / 积分 / 旅行）
/// 才必须拦截。
pub fn is_envelope(v: &Value, key: &str) -> bool {
    matches!(v.get(key), Some(Value::Object(map)) if map.contains_key("$wbEncrypted"))
}

/// 展示型字段安全读取：标量原样返回；对象/数组（如加密信封）折叠为 `Null`。
///
/// 这是**唯一**的展示字段标量化规则，`account_meta`、`/api/status` 的 `current`
/// 与桌面端 `get_status` 都必须走它。裸透传对象到前端会触发 React error #31
/// （Objects are not valid as a React child）导致整树卸载、整页白屏。
pub fn display_value(acc: &Value, key: &str) -> Value {
    match acc.get(key) {
        Some(v @ (Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null)) => v.clone(),
        _ => Value::Null,
    }
}

/// 凭据型字段读取：接受明文字符串或加密信封对象，其他类型返回 `None`。
///
/// 与 [`get_str`] 的区别是**信封不被丢弃**。`get_str` 遇信封返回 `None`，调用方
/// 一旦用 `unwrap_or_default()` 兜底就会把信封静默降级成空串——导入侧表现为
/// `/api/import-local` 恒 400，写回侧表现为登录态被悄悄毁掉。
pub fn secret_value(v: &Value, key: &str) -> Option<Value> {
    match v.get(key) {
        Some(s @ Value::String(_)) => Some(s.clone()),
        Some(o @ Value::Object(map)) if map.contains_key("$wbEncrypted") => Some(o.clone()),
        _ => None,
    }
}

/// 加密信封凭据的可读错误：`access_token` 为信封形态时返回提示文案。
///
/// 信封 token 解不出明文，不能用于签到 / 积分 / 旅行等接口；此前会经
/// [`build_auth_headers`] 的 `unwrap_or_default()` 兜底成空 `Bearer`，被网关 401
/// 后再把错误页原样回显到界面。需要账号身份的请求发出前应先用本函数短路。
pub fn envelope_token_error(account: &Value) -> Option<String> {
    if is_envelope(account, "access_token") {
        return Some(
            "该账号凭据为 WorkBuddy 加密信封态，无法直接调用签到 / 积分 / Token 统计等接口；\
             切换功能不受影响，如需上述功能请删除该账号后改用「OAuth 扫码添加」获取明文凭据。"
                .to_string(),
        );
    }
    None
}

/// 返回可用于 UID 缺失场景的真实邮箱。历史展示占位值不参与身份匹配。
fn identity_email(account: &Value) -> Option<String> {
    let email = get_str(account, "email")?;
    if !email.contains('@')
        || email.eq_ignore_ascii_case("unknown")
        || email == "手动添加"
        || get_str(account, "nickname").as_deref() == Some(email.as_str())
        || get_str(account, "uid").as_deref() == Some(email.as_str())
    {
        return None;
    }
    Some(email.to_ascii_lowercase())
}

/// 按稳定身份将采集结果合并到账号列表，并返回最终持久化的账号。
///
/// 非空 UID 始终优先；仅当新账号没有 UID 时，才使用真实邮箱兜底。
/// 命中已有身份时保留本地 id，避免调用方持有的账号引用失效。
pub fn upsert_collected_account(accounts: &mut Vec<Value>, mut collected: Value) -> Value {
    let collected_uid = get_str(&collected, "uid");
    let collected_email = identity_email(&collected);
    let matches_identity = |existing: &Value| {
        if let Some(uid) = collected_uid.as_deref() {
            return get_str(existing, "uid").as_deref() == Some(uid);
        }
        collected_email
            .as_deref()
            .is_some_and(|email| identity_email(existing).as_deref() == Some(email))
    };

    let matching_indexes: Vec<usize> = accounts
        .iter()
        .enumerate()
        .filter_map(|(index, existing)| matches_identity(existing).then_some(index))
        .collect();

    if let Some(&first_index) = matching_indexes.first() {
        let existing = &accounts[first_index];

        // WorkBuddy 5.6 加密态保护：本机重导入得到的是加密信封 token；若已有记录
        // 仍持有未过期的明文 token（OAuth 扫码所得），不得让信封覆盖明文 —— 否则
        // 账号页每次加载自动 importLocal 都会把扫码凭据冲掉，签到 / 积分等需要明文
        // token 的功能随之失效（且 `build_auth_headers` 只会发出空 Bearer）。
        // 明文过期后才放行信封接管。
        if is_envelope(&collected, "access_token") && has_unexpired_plain_token(existing) {
            return existing.clone();
        }
        // 展示字段兜底：新采集为信封时保留已有记录的明文展示值，避免昵称/邮箱
        // 从可读文本退化成 null。
        for key in ["nickname", "email", "enterpriseName"] {
            if is_envelope(&collected, key) {
                if let Some(v) = existing.get(key) {
                    collected[key] = v.clone();
                }
            }
        }

        if let Some(existing_id) = existing.get("id").cloned() {
            collected["id"] = existing_id;
        }
        if get_str(&collected, "uid").is_none() {
            if let Some(existing_uid) = existing.get("uid").cloned() {
                collected["uid"] = existing_uid;
            }
        }
        if let Some(created_at) = existing.get("createdAt").cloned() {
            collected["createdAt"] = created_at;
        }

        for index in matching_indexes.into_iter().rev() {
            accounts.remove(index);
        }
        accounts.insert(first_index.min(accounts.len()), collected.clone());
    } else {
        accounts.push(collected.clone());
    }

    collected
}

/// 校验账号记录的凭据域是否属于目标 region。
///
/// 账号库是按文件分区的，但账号 JSON 自身仍可能来自错误的导入入口；只按目标
/// 文件落库会把国际账号写进 CN 的 `accounts.json`。CN 旧账号可能没有 `domain`，
/// 为保持兼容允许这种历史记录；Global 没有 domain 时则拒绝，因为无法证明它
/// 属于国际版。只要 domain 存在，就必须严格匹配目标 region。
pub fn ensure_account_region(region: Region, account: &Value) -> Result<(), String> {
    let domain = get_str(account, "domain");
    if domain.is_none() {
        return if region == Region::Cn {
            Ok(())
        } else {
            Err("国际版账号缺少凭据 domain，无法安全写入国际版账号库".to_string())
        };
    }

    let domain = domain.unwrap_or_default();
    let actual = region_of(&domain);
    if actual == region {
        return Ok(());
    }

    let actual_name = region_display(actual);
    let expected_name = region_display(region);
    let actual_label = if actual == Region::Global {
        format!("{actual_name}（国际版）")
    } else {
        format!("{actual_name}（国内版）")
    };
    let expected_label = if region == Region::Global {
        format!("{expected_name}（国际版）")
    } else {
        format!("{expected_name}（国内版）")
    };
    Err(format!(
        "账号凭据属于{}（domain: {}），不能写入{}账号库",
        actual_label, domain, expected_label,
    ))
}

/// 使用统一身份规则保存采集到的账号（CN）。
pub fn save_collected_account(collected: Value) -> std::io::Result<Value> {
    save_collected_account_for(Region::Cn, collected)
}

/// 按 region 使用统一身份规则保存采集到的账号。
///
/// 这是所有 OAuth / 本机导入等采集入口的最后一道区域边界；在读取目标账号库
/// 或写盘之前校验 domain，防止任何调用方因丢失 region 参数把跨区域账号落到 CN。
pub fn save_collected_account_for(region: Region, collected: Value) -> std::io::Result<Value> {
    ensure_account_region(region, &collected)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let mut accounts = load_accounts_for(region);
    let saved = upsert_collected_account(&mut accounts, collected);
    save_accounts_for(region, &accounts)?;
    Ok(saved)
}

/// 按 id 覆盖写入 CN 账号库（不存在则追加）。对照 server.py `_upsert_account`。
pub fn upsert_account(updated: &Value) -> std::io::Result<()> {
    upsert_account_for(Region::Cn, updated)
}

/// 按 region 覆盖写入账号库（不存在则追加）。
pub fn upsert_account_for(region: Region, updated: &Value) -> std::io::Result<()> {
    ensure_account_region(region, updated)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    let mut accounts = load_accounts_for(region);
    let id = updated.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let mut replaced = false;
    for a in accounts.iter_mut() {
        if a.get("id").and_then(|v| v.as_str()) == Some(id) {
            *a = updated.clone();
            replaced = true;
            break;
        }
    }
    if !replaced {
        accounts.push(updated.clone());
    }
    save_accounts_for(region, &accounts)
}

/// 构造与官方对齐的请求头。对照 server.py `build_auth_headers`。
pub fn build_auth_headers(account: &Value) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    headers.insert(
        "Authorization".to_string(),
        format!(
            // 注意：`access_token` 为加密信封对象时 `get_str` 取不到值，这里会产出
            // 空 `Bearer`。调用方必须先用 `envelope_token_error` 拦截，不要把空凭据
            // 真的发出去（否则换回网关 401，错误页还会被原样回显到界面）。
            "Bearer {}",
            get_str(account, "access_token").unwrap_or_default()
        ),
    );
    headers.insert("Accept".to_string(), "application/json".to_string());
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    if let Some(uid) = get_str(account, "uid") {
        headers.insert("X-User-Id".to_string(), uid);
    }
    if let Some(eid) =
        get_str(account, "enterpriseId").or_else(|| get_str(account, "enterprise_id"))
    {
        headers.insert("X-Enterprise-Id".to_string(), eid.clone());
        headers.insert("X-Tenant-Id".to_string(), eid);
    }
    if let Some(domain) = get_str(account, "domain") {
        headers.insert("X-Domain".to_string(), domain);
    }
    headers
}

/// 构造 chat 请求头（region 化，含 X-No-* 缺省约定与 `X-Product: SaaS`）。
///
/// 对照参考实现 `chatHeaders`。**安全红线：chat 请求绝不携带 refresh token。**
/// 缺失的身份字段用官方 CLI 的 `X-No-*` 约定表达，而非省略 header。
///
/// 与 [`build_auth_headers`] 的差异（对齐官方客户端行为）：
/// - `Accept` 声明 `text/event-stream`（chat 端点恒为 SSE）；
/// - 附带 `Accept-Language`（CN `zh-CN` / Global `en-US`）；
/// - 附带 `X-CodeBuddy-Request: 1` 与 `X-Agent-Purpose: conversation` 归属头。
pub fn build_chat_headers(region: Region, account: &Value) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    let origin = region_spec(region).billing_base;
    let accept_language = match region {
        Region::Global => "en-US",
        Region::Cn => "zh-CN",
    };
    headers.insert(
        "Accept".to_string(),
        "application/json, text/event-stream".to_string(),
    );
    headers.insert("Accept-Language".to_string(), accept_language.to_string());
    headers.insert("X-CodeBuddy-Request".to_string(), "1".to_string());
    headers.insert(
        "X-Agent-Purpose".to_string(),
        "conversation".to_string(),
    );
    headers.insert("X-Requested-With".to_string(), "XMLHttpRequest".to_string());
    headers.insert("Origin".to_string(), origin.to_string());
    headers.insert("Referer".to_string(), format!("{origin}/"));
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert(
        "Authorization".to_string(),
        format!(
            "Bearer {}",
            get_str(account, "access_token").unwrap_or_default()
        ),
    );
    match get_str(account, "uid") {
        Some(uid) => {
            headers.insert("X-User-Id".to_string(), uid);
        }
        None => {
            headers.insert("X-No-User-Id".to_string(), "1".to_string());
        }
    }
    match get_str(account, "enterpriseId").or_else(|| get_str(account, "enterprise_id")) {
        Some(eid) => {
            headers.insert("X-Enterprise-Id".to_string(), eid);
        }
        None => {
            headers.insert("X-No-Enterprise-Id".to_string(), "1".to_string());
        }
    }
    match get_str(account, "domain") {
        Some(domain) => {
            headers.insert("X-Domain".to_string(), domain);
        }
        None => {
            headers.insert("X-No-Department-Info".to_string(), "1".to_string());
        }
    }
    headers.insert("X-Product".to_string(), "SaaS".to_string());
    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 展示字段标量化：标量原样保留，加密信封折叠为 null。
    ///
    /// 裸透传信封会让前端 `nickname || email || uid` 选中对象并当 React 子节点渲染，
    /// 触发 error #31 整树卸载（白屏）。
    #[test]
    fn display_value_folds_envelope_to_null_but_keeps_scalars() {
        let acc = json!({
            "uid": "u1",
            "nickname": {"$wbEncrypted": 1, "envelope": "…"},
            "enterpriseName": null,
            "expiresAt": 1_800_000_000_000_i64,
            "needsRelogin": false,
            "nested": ["a"],
        });
        assert_eq!(display_value(&acc, "uid"), json!("u1"));
        assert_eq!(display_value(&acc, "expiresAt"), json!(1_800_000_000_000_i64));
        assert_eq!(display_value(&acc, "needsRelogin"), json!(false));
        assert!(display_value(&acc, "nickname").is_null(), "信封必须折叠为 null");
        assert!(display_value(&acc, "nested").is_null(), "数组同样不得透传");
        assert!(display_value(&acc, "missing").is_null(), "缺失字段为 null");
    }

    /// `account_meta` 是账号列表 / OAuth 结果 / 网关下发的统一出口，必须已标量化。
    #[test]
    fn account_meta_scalarizes_envelope_display_fields() {
        let meta = account_meta(&json!({
            "id": "a1",
            "uid": "u1",
            "nickname": {"$wbEncrypted": 1, "envelope": "…"},
            "enterpriseName": {"$wbEncrypted": 1, "envelope": "…"},
            "access_token": "SECRET",
        }));
        assert!(meta["nickname"].is_null(), "信封昵称不得透传：{meta}");
        assert!(meta["enterpriseName"].is_null(), "信封企业名不得透传：{meta}");
        assert_eq!(meta["uid"], json!("u1"));
        assert!(meta.get("access_token").is_none(), "不得泄露 token：{meta}");
    }

    /// `secret_value` 与 `get_str` 的差别就是**不丢信封** —— 这是导入能修好的前提。
    #[test]
    fn secret_value_accepts_plain_and_envelope_but_rejects_others() {
        let envelope = json!({"$wbEncrypted": 1, "envelope": "…"});
        let acc = json!({
            "access_token": envelope,
            "refresh_token": "plain-refresh",
            "bad": 42,
            "other": {"not": "envelope"},
        });
        assert_eq!(secret_value(&acc, "access_token"), Some(envelope.clone()));
        assert_eq!(secret_value(&acc, "refresh_token"), Some(json!("plain-refresh")));
        assert!(secret_value(&acc, "bad").is_none(), "数字不是凭据");
        assert!(
            secret_value(&acc, "other").is_none(),
            "普通对象不是信封，不得当凭据"
        );
        assert!(secret_value(&acc, "missing").is_none());
        // 对照：get_str 遇信封返回 None —— 正是导入恒 400 的成因。
        assert!(get_str(&acc, "access_token").is_none());
    }

    /// 信封凭据要能被识别并给出可读错误；明文 / 缺字段不误报。
    #[test]
    fn envelope_token_error_only_fires_on_envelope_access_token() {
        let envelope = json!({
            "id": "a1",
            "access_token": {"$wbEncrypted": true, "envelope": "…"},
            "refresh_token": {"$wbEncrypted": true, "envelope": "…"},
        });
        let err = envelope_token_error(&envelope).expect("信封 access_token 应返回错误");
        assert!(err.contains("信封"), "错误文案应可读：{err}");
        assert!(err.contains("OAuth"), "应给出扫码重新添加的指引：{err}");

        let plain = json!({"id": "a2", "access_token": "SECRET", "refresh_token": "R"});
        assert!(envelope_token_error(&plain).is_none(), "明文凭据不应报错");

        let legacy = json!({"id": "a3"});
        assert!(
            envelope_token_error(&legacy).is_none(),
            "缺 access_token 的历史账号不在此拦截（保持既有行为）"
        );
    }

    #[test]
    fn envelope_reimport_keeps_unexpired_plain_oauth_token() {
        let envelope = json!({"$wbEncrypted": 1, "envelope": "enc"});
        let mut accounts = vec![json!({
            "id": "a-1",
            "uid": "uid-1",
            "nickname": "明文昵称",
            "access_token": "plain-token",
            "refresh_token": "plain-refresh",
            "expiresAt": now_ms() + 86_400_000_i64,
        })];
        // 账号页自动 importLocal 会拿本机加密态重采集同一 uid：
        // 不得让信封覆盖仍未过期的明文 token（否则签到 / 积分失效）。
        let collected = json!({
            "uid": "uid-1",
            "nickname": envelope,
            "access_token": {"$wbEncrypted": 1, "envelope": "a"},
            "refresh_token": {"$wbEncrypted": 1, "envelope": "r"},
        });
        let saved = upsert_collected_account(&mut accounts, collected);

        assert_eq!(accounts.len(), 1);
        assert_eq!(saved["access_token"], "plain-token");
        assert_eq!(saved["refresh_token"], "plain-refresh");
        assert_eq!(saved["nickname"], "明文昵称");
    }

    #[test]
    fn envelope_reimport_takes_over_after_plain_token_expired() {
        let mut accounts = vec![json!({
            "id": "a-1",
            "uid": "uid-1",
            "access_token": "stale-plain",
            "expiresAt": now_ms() - 1_000_i64,
        })];
        let collected = json!({
            "uid": "uid-1",
            "access_token": {"$wbEncrypted": 1, "envelope": "a"},
        });
        let saved = upsert_collected_account(&mut accounts, collected);

        assert_eq!(accounts.len(), 1);
        // 明文已过期：信封接管（切换仍可用，由 WorkBuddy 自解）。
        assert!(saved.get("access_token").and_then(|v| v.as_str()).is_none());
    }

    #[test]
    fn fresh_plain_oauth_token_replaces_envelope_record() {
        let mut accounts = vec![json!({
            "id": "a-1",
            "uid": "uid-1",
            "access_token": {"$wbEncrypted": 1, "envelope": "old"},
        })];
        // 重新扫码得到新明文：应正常替换。
        let collected = json!({
            "uid": "uid-1",
            "access_token": "fresh-plain",
            "expiresAt": now_ms() + 86_400_000_i64,
        });
        let saved = upsert_collected_account(&mut accounts, collected);

        assert_eq!(accounts.len(), 1);
        assert_eq!(saved["access_token"], "fresh-plain");
    }

    #[test]
    fn account_meta_strips_tokens() {
        let acc = json!({
            "id": "a1",
            "uid": "u1",
            "email": "x@y.z",
            "nickname": "小明",
            "enterpriseName": "某公司",
            "access_token": "SECRET_ACCESS",
            "refresh_token": "SECRET_REFRESH",
            "expiresAt": 123456,
            "needs_relogin": true,
            "needs_relogin_reason": "刷新失败",
        });
        let meta = account_meta(&acc);
        assert_eq!(meta["id"], "a1");
        assert_eq!(meta["needsRelogin"], true);
        assert_eq!(meta["needsReloginReason"], "刷新失败");
        assert!(meta.get("access_token").is_none(), "不得泄露 token");
        assert!(meta.get("refresh_token").is_none(), "不得泄露 token");
    }

    #[test]
    fn account_display_name_priority() {
        assert_eq!(
            account_display_name(&json!({"email": "a@b.c", "nickname": "n"})),
            "a@b.c"
        );
        assert_eq!(
            account_display_name(&json!({"nickname": "n", "uid": "u"})),
            "n"
        );
        assert_eq!(account_display_name(&json!({"uid": "u"})), "u");
        assert_eq!(account_display_name(&json!({})), "unknown");
    }

    /// 账号采集结果落库前必须按 domain 再做一次 region 校验，不能只相信调用方参数。
    #[test]
    fn ensure_account_region_rejects_cross_region_records() {
        let global = json!({
            "uid": "global-1",
            "domain": "www.workbuddy.ai",
            "access_token": "token",
        });
        let cn = json!({
            "uid": "cn-1",
            "domain": "www.codebuddy.cn",
            "access_token": "token",
        });

        assert!(ensure_account_region(Region::Global, &global).is_ok());
        let err = ensure_account_region(Region::Cn, &global)
            .expect_err("国际账号不得落入 CN 账号库");
        assert!(err.contains("国际版"), "错误应指出实际 region：{err}");
        assert!(err.contains("国内版"), "错误应指出目标 region：{err}");

        assert!(ensure_account_region(Region::Cn, &cn).is_ok());
        assert!(ensure_account_region(Region::Global, &cn).is_err());
    }

    /// 历史 CN 账号可能没有 domain；保持兼容。Global 缺 domain 则不能安全判定归属。
    #[test]
    fn ensure_account_region_handles_missing_domain_conservatively() {
        assert!(ensure_account_region(Region::Cn, &json!({"uid": "legacy-cn"})).is_ok());
        let err = ensure_account_region(Region::Global, &json!({"uid": "unknown"}))
            .expect_err("缺 domain 的记录不能写入 Global 账号库");
        assert!(err.contains("缺少"), "错误应说明缺少 domain：{err}");
    }

    #[test]
    fn get_str_trims_and_filters_empty() {
        assert_eq!(get_str(&json!({"k": "  v  "}), "k"), Some("v".to_string()));
        assert_eq!(get_str(&json!({"k": "  "}), "k"), None);
        assert_eq!(get_str(&json!({"k": 123}), "k"), None);
    }

    #[test]
    fn build_chat_headers_uses_no_star_conventions_and_never_carries_refresh_token() {
        let acc = json!({
            "access_token": "AT",
            "refresh_token": "SECRET_REFRESH",
            "uid": "u1",
            "domain": "www.codebuddy.cn",
        });
        let headers = build_chat_headers(Region::Cn, &acc);
        assert_eq!(headers.get("X-User-Id").map(String::as_str), Some("u1"));
        assert_eq!(headers.get("X-Domain").map(String::as_str), Some("www.codebuddy.cn"));
        assert_eq!(headers.get("X-No-Enterprise-Id").map(String::as_str), Some("1"));
        assert_eq!(headers.get("X-Product").map(String::as_str), Some("SaaS"));
        assert_eq!(headers.get("Origin").map(String::as_str), Some("https://www.codebuddy.cn"));
        assert_eq!(headers.get("Authorization").map(String::as_str), Some("Bearer AT"));
        // 安全红线：chat 头绝不携带 refresh token。
        assert!(!headers.contains_key("X-Refresh-Token"));
        assert!(!headers.values().any(|v| v == "SECRET_REFRESH"));
    }

    #[test]
    fn build_chat_headers_marks_missing_identity_with_no_flags() {
        let acc = json!({"access_token": "AT"});
        let headers = build_chat_headers(Region::Global, &acc);
        assert_eq!(headers.get("X-No-User-Id").map(String::as_str), Some("1"));
        assert_eq!(headers.get("X-No-Enterprise-Id").map(String::as_str), Some("1"));
        assert_eq!(headers.get("X-No-Department-Info").map(String::as_str), Some("1"));
        assert_eq!(headers.get("Origin").map(String::as_str), Some("https://www.workbuddy.ai"));
    }

    fn account(id: &str, uid: Option<&str>, nickname: &str, email: Option<&str>) -> Value {
        json!({
            "id": id,
            "uid": uid,
            "nickname": nickname,
            "email": email,
            "access_token": format!("token-{id}"),
            "createdAt": 1,
        })
    }

    /// WorkBuddy 账号库（JSON 数组）的旧记录必须仍可读。
    ///
    /// 账号记录整体是 `serde_json::Value`，因此天然前向/后向兼容；这条测试钉住
    /// 「历史字段缺失、以及未来新增未知字段都不影响读取」，防止有人日后收紧成
    /// 强类型 struct 而把老账号库读空。
    #[test]
    fn workbuddy_accounts_tolerate_legacy_and_unknown_fields() {
        let dir = std::env::temp_dir().join(format!(
            "buddy-switch-accounts-compat-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join("accounts.json");

        // 混合：完整记录 / 仅最小字段的历史记录 / 带未知新字段的记录。
        let text = r#"[
          {
            "id": "a1", "uid": "u1", "email": "full@example.com", "nickname": "完整",
            "access_token": "AT", "refresh_token": "RT",
            "expiresAt": 123456, "createdAt": 1
          },
          { "id": "a2", "uid": "u2", "nickname": "最小历史记录", "access_token": "AT2" },
          { "id": "a3", "uid": "u3", "email": "new@example.com", "access_token": "AT3",
            "brand_new_field": {"nested": true}, "another": [1, 2, 3] }
        ]"#;
        std::fs::write(&path, text).expect("write accounts");

        let accounts = load_accounts_from_path(&path);
        assert_eq!(accounts.len(), 3, "三条记录都必须被读出");
        assert_eq!(
            find_account_in(&accounts, "a2").unwrap()["nickname"],
            "最小历史记录"
        );
        assert_eq!(find_account_in(&accounts, "a3").unwrap()["uid"], "u3");

        // 旧的 `needs_relogin` 布尔标志仍要能映射到线上 camelCase
        let meta = account_meta(&json!({
            "id": "a4", "uid": "u4",
            "needs_relogin": true, "needs_relogin_reason": "刷新失败"
        }));
        assert_eq!(meta["needsRelogin"], true);
        assert_eq!(meta["needsReloginReason"], "刷新失败");

        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn same_nickname_with_different_uids_is_retained() {
        let mut accounts = vec![account("old", Some("uid-1"), "同名", Some("同名"))];
        let saved =
            upsert_collected_account(&mut accounts, account("new", Some("uid-2"), "同名", None));

        assert_eq!(accounts.len(), 2);
        assert_eq!(saved["id"], "new");
    }

    #[test]
    fn same_uid_refresh_preserves_local_id_and_removes_duplicates() {
        let mut accounts = vec![
            account("stable", Some("uid-1"), "旧名称", Some("old@example.com")),
            account("duplicate", Some("uid-1"), "重复记录", None),
        ];
        let saved = upsert_collected_account(
            &mut accounts,
            account("generated", Some("uid-1"), "新名称", None),
        );

        assert_eq!(accounts.len(), 1);
        assert_eq!(saved["id"], "stable");
        assert_eq!(saved["nickname"], "新名称");
        assert_eq!(saved["access_token"], "token-generated");
    }

    #[test]
    fn different_uids_with_same_real_email_are_retained() {
        let mut accounts = vec![account(
            "old",
            Some("uid-1"),
            "账号一",
            Some("shared@example.com"),
        )];
        upsert_collected_account(
            &mut accounts,
            account("new", Some("uid-2"), "账号二", Some("shared@example.com")),
        );

        assert_eq!(accounts.len(), 2);
    }

    #[test]
    fn real_email_is_fallback_only_when_collected_uid_is_missing() {
        let mut accounts = vec![account("stable", None, "旧名称", Some("user@example.com"))];
        let saved = upsert_collected_account(
            &mut accounts,
            account("generated", None, "新名称", Some("USER@example.com")),
        );

        assert_eq!(accounts.len(), 1);
        assert_eq!(saved["id"], "stable");
        assert_eq!(saved["nickname"], "新名称");
    }

    #[test]
    fn legacy_synthetic_email_does_not_merge_accounts() {
        let mut accounts = vec![account("old", None, "同名", Some("同名"))];
        upsert_collected_account(&mut accounts, account("new", None, "同名", Some("同名")));

        assert_eq!(accounts.len(), 2);
    }

    #[test]
    fn persisted_same_name_accounts_can_be_found_and_deleted_independently() {
        let test_dir = std::env::temp_dir().join(format!(
            "buddy-switch-same-name-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let path = test_dir.join("accounts.json");
        let mut accounts = vec![];
        upsert_collected_account(
            &mut accounts,
            account("account-1", Some("uid-1"), "同名用户", None),
        );
        upsert_collected_account(
            &mut accounts,
            account("account-2", Some("uid-2"), "同名用户", None),
        );
        save_accounts_to_path(&path, &accounts).expect("same-name accounts should persist");

        let persisted = load_accounts_from_path(&path);
        assert_eq!(
            find_account_in(&persisted, "account-1").unwrap()["uid"],
            "uid-1"
        );
        assert_eq!(
            find_account_in(&persisted, "account-2").unwrap()["uid"],
            "uid-2"
        );

        delete_account_from_path(&path, "account-1").expect("first account should delete");
        let after_first_delete = load_accounts_from_path(&path);
        assert!(find_account_in(&after_first_delete, "account-1").is_none());
        assert_eq!(
            find_account_in(&after_first_delete, "account-2").unwrap()["uid"],
            "uid-2"
        );

        delete_account_from_path(&path, "account-2").expect("second account should delete");
        assert!(load_accounts_from_path(&path).is_empty());
        std::fs::remove_dir_all(&test_dir).expect("temporary account store should clean up");
    }

    /// 两版账号库必须落到**不同文件**（PRD D2 头号硬约束）。
    ///
    /// `region.rs` 只断言了两个 `accounts_filename` 常量不同，但**常量不同 ≠
    /// `accounts_file_for` 用对了常量**——若该函数写死用 CN 的文件名，常量测试照样绿，
    /// 而两版账号库会互相覆盖。这里直接钉住函数产出的文件名。
    #[test]
    fn accounts_file_for_is_region_scoped() {
        let cn = accounts_file_for(Region::Cn);
        let global = accounts_file_for(Region::Global);

        assert_eq!(
            cn.file_name().and_then(|n| n.to_str()),
            Some("accounts.json"),
            "CN 账号库文件名"
        );
        assert_eq!(
            global.file_name().and_then(|n| n.to_str()),
            Some("accounts.global.json"),
            "Global 账号库文件名"
        );
        assert_ne!(cn, global, "CN / Global 账号库不得指向同一文件");

        // 同一 store 目录，仅文件名不同。
        assert_eq!(cn.parent(), global.parent());
        assert_eq!(cn, crate::modules::config::store_dir().join("accounts.json"));
        assert_eq!(
            global,
            crate::modules::config::store_dir().join("accounts.global.json")
        );

        // 与既有的 CN 兼容路径保持一致，避免两处逻辑漂移。
        assert_eq!(cn, crate::modules::config::accounts_file());
    }
}

/// 删除账号（按 id，CN）。
pub fn delete_account(account_id: &str) -> Result<(), String> {
    delete_account_for(Region::Cn, account_id)
}

/// 按 region 删除账号（按 id）。
pub fn delete_account_for(region: Region, account_id: &str) -> Result<(), String> {
    delete_account_from_path(&accounts_file_for(region), account_id)
}

/// 导入本机当前账号（从认证文件读取，CN）。
pub fn import_local() -> Result<Value, String> {
    import_local_for(Region::Cn)
}

/// 按 region 导入本机当前账号（从该 region 认证文件读取）。
pub fn import_local_for(region: Region) -> Result<Value, String> {
    let acc = crate::modules::auth_file::import_from_auth_file_checked_for(region)
        .map_err(|mismatch| mismatch.message())?
        .ok_or("未读取到本地 WorkBuddy 登录信息")?;
    let saved = save_collected_account_for(region, acc).map_err(|e| e.to_string())?;
    Ok(account_meta(&saved))
}

// 手动添加账号（token 方式）已随 UI 入口「手动添加」一并下线；
// `identity_email` 中的 "手动添加" 占位过滤保留，用于兼容历史手动添加的旧账号。
