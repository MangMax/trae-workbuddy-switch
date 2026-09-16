//! 账号存储：读取/写入 `~/.buddy-switch/accounts.json`，与 Python 版共享数据目录。
//!
//! 对照 server.py `load_accounts` / `save_accounts` / `find_account` /
//! `account_display_name` / `account_meta`。

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;

use crate::modules::config::atomic_write;
use crate::modules::region::{accounts_file_for, region_spec, Region};

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
pub fn account_meta(acc: &Value) -> Value {
    json!({
        "id": acc.get("id"),
        "uid": acc.get("uid"),
        "email": acc.get("email"),
        "nickname": acc.get("nickname"),
        "enterpriseName": acc.get("enterpriseName"),
        "expiresAt": acc.get("expiresAt"),
        "refreshExpiresAt": acc.get("refreshExpiresAt"),
        "refreshedAt": acc.get("refreshedAt"),
        "createdAt": acc.get("createdAt"),
        "needsRelogin": acc.get("needs_relogin").and_then(|v| v.as_bool()) == Some(true),
        "needsReloginReason": acc.get("needs_relogin_reason"),
    })
}

/// 取非空字符串字段；空/缺失返回 None。
pub fn get_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
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

/// 使用统一身份规则保存采集到的账号（CN）。
pub fn save_collected_account(collected: Value) -> std::io::Result<Value> {
    save_collected_account_for(Region::Cn, collected)
}

/// 按 region 使用统一身份规则保存采集到的账号。
pub fn save_collected_account_for(region: Region, collected: Value) -> std::io::Result<Value> {
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
    let acc = crate::modules::auth_file::import_from_auth_file_for(region)
        .ok_or("未读取到本地 WorkBuddy 登录信息")?;
    let saved = save_collected_account_for(region, acc).map_err(|e| e.to_string())?;
    Ok(account_meta(&saved))
}

// 手动添加账号（token 方式）已随 UI 入口「手动添加」一并下线；
// `identity_email` 中的 "手动添加" 占位过滤保留，用于兼容历史手动添加的旧账号。
