//! Token 刷新与保活。
//!
//! 对照 server.py `refresh_account_token` / `ensure_fresh_token` /
//! `run_keepalive_cycle`。
//!
//! **region 化**：新增 `*_for(region, …)` 变体；旧 CN 签名保留为薄包装。

use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;

use crate::modules::account::{build_auth_headers, load_accounts_for, upsert_account_for};
use crate::modules::config::{
    http_request, load_checkin_config, norm_ts, now_ms, RunFlagGuard, WORKBUDDY_API_PREFIX,
};
use crate::modules::region::{region_spec, Region};

// 保活运行标志按 region 独立：CN / Global 都会真正执行保活，若共用一个全局标志，
// 一版的保活周期会把另一版误报为 skipped/already_running（违反 PRD G1「两版互不污染」）。
static KEEPALIVE_RUNNING_CN: AtomicBool = AtomicBool::new(false);
static KEEPALIVE_RUNNING_GLOBAL: AtomicBool = AtomicBool::new(false);

/// 取该 region 独立的保活运行标志。
fn keepalive_running_flag(region: Region) -> &'static AtomicBool {
    match region {
        Region::Cn => &KEEPALIVE_RUNNING_CN,
        Region::Global => &KEEPALIVE_RUNNING_GLOBAL,
    }
}

/// 刷新单账号 token（CN）。
pub async fn refresh_account_token(account: Value) -> Value {
    refresh_account_token_for(Region::Cn, account).await
}

/// 刷新单账号 token（POST `{billing_base}/v2/plugin/auth/token/refresh`），成功则落盘并返回新账号。
///
/// 刷新失败（refresh token 失效等）时给账号标记 needs_relogin，避免无限重试。
pub async fn refresh_account_token_for(region: Region, mut account: Value) -> Value {
    let previous_access_token = account
        .get("access_token")
        .and_then(|value| value.as_str())
        .map(str::to_string);
    let rt = account
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    if rt.is_empty() {
        account["needs_relogin"] = json!(true);
        account["needs_relogin_reason"] = json!("缺少 refresh token，无法刷新，需重新登录");
        let _ = upsert_account_for(region, &account);
        return account;
    }

    let mut headers = build_auth_headers(&account);
    headers.insert("X-Refresh-Token".to_string(), rt.clone());
    let url = format!(
        "{}{WORKBUDDY_API_PREFIX}/auth/token/refresh",
        region_spec(region).billing_base
    );
    let resp = http_request(&url, "POST", Some(json!({})), Some(&headers)).await;
    let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 && code != 200 {
        account["needs_relogin"] = json!(true);
        account["needs_relogin_reason"] = json!(format!(
            "刷新失败(code={code}): {}",
            resp.get("message")
                .or_else(|| resp.get("msg"))
                .and_then(|v| v.as_str())
                .unwrap_or("未知错误")
        ));
        let _ = upsert_account_for(region, &account);
        return account;
    }

    let data = resp.get("data").cloned().unwrap_or_else(|| json!({}));
    let new_at = data
        .get("accessToken")
        .and_then(|v| v.as_str())
        .or_else(|| data.get("access_token").and_then(|v| v.as_str()))
        .map(|s| s.to_string());
    let Some(new_at) = new_at else {
        account["needs_relogin"] = json!(true);
        account["needs_relogin_reason"] = json!("刷新响应缺少 accessToken");
        let _ = upsert_account_for(region, &account);
        return account;
    };

    account["access_token"] = json!(new_at);
    if let Some(new_rt) = data
        .get("refreshToken")
        .and_then(|v| v.as_str())
        .or_else(|| data.get("refresh_token").and_then(|v| v.as_str()))
    {
        account["refresh_token"] = json!(new_rt);
    }
    // 官方接口只返回相对 expiresIn（秒），需换算为绝对时间戳
    let new_exp = norm_ts(data.get("expiresAt").or_else(|| data.get("expires_at")));
    let new_exp = match new_exp {
        Some(v) => Some(v),
        None => data
            .get("expiresIn")
            .and_then(|v| v.as_i64())
            .map(|e| now_ms() + e * 1000),
    };
    if let Some(v) = new_exp {
        account["expiresAt"] = json!(v);
    }
    let fallback_rt_exp = norm_ts(
        account
            .get("auth_raw")
            .and_then(|a| a.get("refreshExpiresAt")),
    );
    let mut new_rt_exp = norm_ts(
        data.get("refreshExpiresAt")
            .or_else(|| data.get("refresh_expires_at")),
    );
    if new_rt_exp.is_none() {
        new_rt_exp = fallback_rt_exp;
    }
    let new_rt_exp = match new_rt_exp {
        Some(v) => Some(v),
        None => data
            .get("refreshExpiresIn")
            .and_then(|v| v.as_i64())
            .map(|e| now_ms() + e * 1000),
    };
    if let Some(v) = new_rt_exp {
        account["refreshExpiresAt"] = json!(v);
    }
    account["refreshedAt"] = json!(now_ms());
    let map = account.as_object_mut().unwrap();
    map.remove("needs_relogin");
    map.remove("needs_relogin_reason");
    let _ = upsert_account_for(region, &account);
    // Windows 不执行 apiKeyHelper；当前 CLI 账号刷新后同步 settings env。
    // 同步失败不阻断 WorkBuddy 保活；状态接口会根据 settings 与账号库是否
    // 一致显示“待同步”，避免把认证配置错误混入账号数据。
    if cfg!(windows) {
        let _ = crate::modules::codebuddy_cli::sync_windows_env_for_account(
            &account,
            previous_access_token.as_deref(),
        );
    }
    account
}

/// 惰性刷新（CN）：expiresAt 缺失或剩余 < lazy_refresh_hours 则刷新。返回最新账号。
pub async fn ensure_fresh_token(account: Value, cfg: &Value) -> Value {
    ensure_fresh_token_for(Region::Cn, account, cfg).await
}

/// 按 region 惰性刷新：expiresAt 缺失或剩余 < lazy_refresh_hours 则刷新。
pub async fn ensure_fresh_token_for(region: Region, mut account: Value, cfg: &Value) -> Value {
    let lazy_h = cfg
        .get("lazy_refresh_hours")
        .and_then(|v| v.as_i64())
        .unwrap_or(24);
    let exp = account.get("expiresAt").and_then(|v| v.as_i64());
    let stale = match exp {
        Some(e) => now_ms() >= e || e - now_ms() < lazy_h * 3600 * 1000,
        None => true,
    };
    let has_rt = !account
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .is_empty();
    if stale && has_rt {
        account = refresh_account_token_for(region, account).await;
    }
    account
}

/// 保活检查（CN）：每天由后台循环调用一次。
pub async fn run_keepalive_cycle() -> Value {
    run_keepalive_cycle_for(Region::Cn).await
}

/// 按 region 保活检查：每天由后台循环调用一次，默认（keepalive_days <= 0）无条件刷新
/// 全部带 refresh token 的账号；keepalive_days > 0 时仅刷新剩余不足该天数的账号。
///
/// 高频保活是为了避免官方服务端清理闲置的 refresh 会话——曾出现闲置数天后
/// 刷新返回 12153 invalid_grant（Session doesn't have required client）导致
/// 账号被迫重新登录。
pub async fn run_keepalive_cycle_for(region: Region) -> Value {
    let Some(_guard) = RunFlagGuard::try_acquire(keepalive_running_flag(region)) else {
        return json!({"skipped": "already_running"});
    };
    let cfg = load_checkin_config();
    let keep_days = cfg
        .get("keepalive_days")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let accounts = load_accounts_for(region);
    let total = accounts.len();
    let mut results: Vec<Value> = Vec::new();
    for mut acc in accounts {
        let exp = acc.get("expiresAt").and_then(|v| v.as_i64());
        let stale = keep_days <= 0
            || match exp {
                Some(e) => now_ms() >= e || e - now_ms() < keep_days * 24 * 3600 * 1000,
                None => true,
            };
        if !stale {
            continue;
        }
        if acc
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
        {
            acc["needs_relogin"] = json!(true);
            acc["needs_relogin_reason"] = json!("缺少 refresh token，无法保活，需重新登录");
            let _ = upsert_account_for(region, &acc);
            results.push(json!({
                "email": crate::modules::account::account_display_name(&acc),
                "status": "missing_rt",
            }));
            continue;
        }
        let fresh = refresh_account_token_for(region, acc).await;
        let failed = fresh.get("needs_relogin").and_then(|v| v.as_bool()) == Some(true);
        results.push(json!({
            "email": crate::modules::account::account_display_name(&fresh),
            "status": if failed { "failed" } else { "ok" },
            "error": if failed {
                fresh.get("needs_relogin_reason").and_then(|v| v.as_str()).map(|s| s.to_string())
            } else {
                None
            },
        }));
    }
    json!({"checked": total, "refreshed": results})
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 保活运行标志必须按 region 独立（PRD G1）。
    ///
    /// 只断言「两个 `KEEPALIVE_RUNNING_*` 常量不同」是不够的——常量不同 ≠
    /// `keepalive_running_flag` 用对了常量。这里从两个角度钉住契约：
    /// ① 指针身份（纯断言，不碰状态）；② 获取语义（CN 持有时 Global 仍可获取）。
    ///
    /// 两版若共用一把标志，一版的保活周期会把另一版误报为 `skipped/already_running`。
    #[test]
    fn keepalive_running_flag_is_region_scoped() {
        // ① 指针身份：同一 region 稳定返回同一把标志，跨 region 必须是不同对象。
        assert!(std::ptr::eq(
            keepalive_running_flag(Region::Cn),
            keepalive_running_flag(Region::Cn)
        ));
        assert!(std::ptr::eq(
            keepalive_running_flag(Region::Global),
            keepalive_running_flag(Region::Global)
        ));
        assert!(
            !std::ptr::eq(
                keepalive_running_flag(Region::Cn),
                keepalive_running_flag(Region::Global)
            ),
            "CN / Global 保活标志不得共用同一把锁"
        );

        // ② 获取语义：持有 CN 时 Global 仍能独立获取（跨版不互相阻塞）。
        // 守卫是 RAII 的，panic 展开时也会释放，不会给后续用例留下脏状态。
        let cn = RunFlagGuard::try_acquire(keepalive_running_flag(Region::Cn))
            .expect("CN 保活标志初始应为空闲");
        // CN 已持有：再取 CN 必须失败（同版互斥仍然有效）……
        assert!(RunFlagGuard::try_acquire(keepalive_running_flag(Region::Cn)).is_none());
        // ……但取 Global 必须成功（跨版不互相阻塞）。
        let global = RunFlagGuard::try_acquire(keepalive_running_flag(Region::Global));
        assert!(global.is_some(), "CN 保活进行中不应阻塞 Global 保活");

        drop(global);
        drop(cn);
        // 释放后应可重新获取。
        assert!(RunFlagGuard::try_acquire(keepalive_running_flag(Region::Cn)).is_some());
    }
}
