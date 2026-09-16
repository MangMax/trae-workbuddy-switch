//! HTTP API 层：把 buddy-switch-core 暴露为本地 REST 接口，供 webui（浏览器）调用。
//!
//! 路由设计对应 Python 版 server.py 与桌面端 commands.rs。仅绑定 127.0.0.1，
//! token 不出本机。

use std::sync::Mutex;
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::RawQuery;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use rust_embed::RustEmbed;
use serde_json::{json, Value};

use buddy_switch_core::modules::{
    account, auth_file, checkin, codebuddy_cli, codebuddy_cn_ide, config, credit_usage, credits, export_import,
    migrate, oauth, process, refresh, region::Region, region::RegionFilter, rotate, schedule, session, switch,
    token_stats, travel, update,
};
use buddy_switch_gateway::{GatewayConfig, GatewayStatusView};

/// WorkBuddy 运行状态缓存：Windows 上检测要跑 tasklist（慢），缓存几秒避免
/// 前端切 tab 频繁触发命令行导致卡顿/闪窗。缓存按 region 分别记录，避免两版串台。
#[cfg(target_os = "windows")]
static RUNNING_CACHE: Mutex<Option<(Region, Instant, bool)>> = Mutex::new(None);

fn cached_workbuddy_running(region: Region) -> bool {
    #[cfg(target_os = "windows")]
    {
        let mut cache = RUNNING_CACHE.lock().unwrap();
        if let Some((cached_region, t, v)) = cache.as_ref() {
            if *cached_region == region && t.elapsed() < Duration::from_secs(3) {
                return *v;
            }
        }
        let v = process::is_workbuddy_running_for(region);
        *cache = Some((region, Instant::now(), v));
        v
    }
    #[cfg(not(target_os = "windows"))]
    {
        process::is_workbuddy_running_for(region)
    }
}

#[derive(RustEmbed)]
#[folder = "../../dist/"]
struct Assets;

/// 切换进度缓存：webui 通过 GET /api/switch/progress 轮询。
static SWITCH_PROGRESS: Mutex<Option<String>> = Mutex::new(None);
static SWITCH_RUNNING: Mutex<bool> = Mutex::new(false);

/// webui 对外路由。
///
/// **merge 顺序（关键，A-1.3 / B-6 要点 12）**：先构造**不含 fallback** 的
/// `api_routes()`，再 `merge(gateway::router(state))`，**最后**才 `.fallback(...)`。
/// 否则 axum 会因 fallback 冲突直接 panic。
pub fn router() -> Router {
    let gateway_state = crate::gateway_host::shared_state();
    api_routes()
        .merge(buddy_switch_gateway::router(gateway_state))
        .fallback(static_handler)
}

/// 管理面 API 路由（**不含 fallback**）。
fn api_routes() -> Router {
    Router::new()
        .route("/api/status", get(api_status))
        .route("/api/accounts", get(api_accounts))
        .route("/api/accounts/open-dir", post(api_open_accounts_dir))
        .route("/api/codebuddy-cli/status", get(api_codebuddy_cli_status))
        .route(
            "/api/codebuddy-cli/install-helper",
            post(api_codebuddy_cli_install_helper),
        )
        .route("/api/codebuddy-cli/switch", post(api_codebuddy_cli_switch))
        .route("/api/codebuddy-cn-ide/status", get(api_codebuddy_cn_ide_status))
        .route("/api/codebuddy-cn-ide/switch", post(api_codebuddy_cn_ide_switch))
        .route("/api/codebuddy-cn-ide/detect", post(api_codebuddy_cn_ide_detect))
        .route("/api/delete", post(api_delete))
        .route("/api/oauth/start", post(api_oauth_start))
        .route("/api/oauth/status", post(api_oauth_status))
        .route("/api/import-local", post(api_import_local))
        .route("/api/export-accounts", post(api_export_accounts))
        .route(
            "/api/export-accounts-to-path",
            post(api_export_accounts_to_path),
        )
        .route("/api/import/preview", post(api_preview_import))
        .route("/api/import", post(api_import))
        .route("/api/switch", post(api_switch))
        .route("/api/switch/progress", get(api_switch_progress))
        .route("/api/sessions", get(api_sessions))
        .route("/api/sessions/copy", post(api_copy_sessions))
        .route("/api/migrate/account", post(api_migrate_account))
        .route("/api/checkin/status", get(api_checkin_status))
        .route("/api/credits", post(api_credits))
        .route("/api/credits/stats", get(api_credit_statistics))
        .route("/api/token-stats", get(api_token_statistics))
        .route("/api/checkin", post(api_checkin))
        .route("/api/checkin/all", post(api_checkin_all))
        .route(
            "/api/checkin/config",
            get(api_checkin_config).post(api_save_checkin_config),
        )
        .route("/api/checkin/logs", get(api_checkin_logs))
        .route("/api/travel/status", get(api_travel_status))
        .route(
            "/api/travel/config",
            get(api_travel_config).post(api_save_travel_config),
        )
        .route(
            "/api/rotate/config",
            get(api_rotate_config).post(api_save_rotate_config),
        )
        .route(
            "/api/schedule/config",
            get(api_schedule_config).post(api_save_schedule_config),
        )
        .route("/api/rotate/status", get(api_rotate_status))
        .route("/api/rotate/run", post(api_rotate_run))
        .route("/api/rotate/logs", get(api_rotate_logs))
        .route("/api/refresh-token", post(api_refresh_token))
        .route("/api/update/check", get(api_update_check))
        .route(
            "/api/update/config",
            get(api_update_config).post(api_save_update_config),
        )
        // —— 网关管理面（A-3.7）——
        .route(
            "/api/gateway/config",
            get(api_gateway_config).post(api_save_gateway_config),
        )
        .route("/api/gateway/status", get(api_gateway_status))
        .route(
            "/api/gateway/keys",
            get(api_gateway_list_keys).post(api_gateway_create_key),
        )
        .route("/api/gateway/keys/revoke", post(api_gateway_revoke_key))
        .route("/api/gateway/keys/delete", post(api_gateway_delete_key))
        .route("/api/gateway/models", get(api_gateway_models))
        .route(
            "/api/gateway/models/refresh",
            post(api_gateway_refresh_models),
        )
        .route(
            "/api/gateway/strategy",
            get(api_gateway_strategy).post(api_gateway_save_strategy),
        )
        .route("/api/gateway/logs", get(api_gateway_logs))
        .route("/api/gateway/logs/clear", post(api_gateway_clear_logs))
}

fn json_ok(v: Value) -> Response {
    Json(v).into_response()
}

fn json_err(e: String, code: StatusCode) -> Response {
    (code, Json(json!({ "ok": false, "error": e }))).into_response()
}

// ---------------------------------------------------------------------------
// 状态 / 账号
// ---------------------------------------------------------------------------

/// 从 query 中读取一个参数值（URL 解码为最简实现，仅处理 `%` 之外的常规字符）。
fn query_value(query: Option<&str>, name: &str) -> Option<String> {
    query.unwrap_or("").split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == name).then(|| value.to_string())
    })
}

/// 解析 region 参数，缺省为 `cn`（保证旧行为）。
fn parse_region(value: Option<&str>) -> Region {
    value.and_then(Region::parse).unwrap_or(Region::Cn)
}

/// 解析统计查询范围参数，缺省为 `cn`（保证旧行为）；额外支持 `"all"` 合并视图。
///
/// 复用 core 的 [`buddy_switch_core::modules::region::parse_region_filter`]，其绑定规则与
/// 本模块既有 [`parse_region`] 完全一致（`"ai"`/`"GLOBAL"` → Global、`""`/`"xx"` → Cn），
/// 并额外接受 `"all" | "*" | "合并" | "全部"`。**仅**用于 `/api/token-stats` 与
/// `/api/credits/stats` 两个统计路由。
fn parse_region_filter(value: Option<&str>) -> RegionFilter {
    buddy_switch_core::modules::region::parse_region_filter(value)
}

/// 该 region 客户端是否已安装。
///
/// 跨平台判定：解析出的应用路径（macOS bundle / Windows exe：进程→缓存→注册表→
/// 盘符扫描，最后回落默认安装路径）**真实存在**即为已安装。比 `identity::
/// installed_app_version` 更适合 Windows（后者只在 macOS 读 bundle 元数据）。
fn region_installed(region: Region) -> bool {
    auth_file::workbuddy_app_path_for(region).exists()
}

/// 把 core 的 [`auth_file::RegionMismatch`] 序列化为前端 `RegionMismatch` 契约（camelCase）。
fn mismatch_json(mismatch: &auth_file::RegionMismatch) -> Value {
    json!({
        "actualDomain": mismatch.actual_domain,
        "expectedFile": mismatch.expected_file,
        "envVar": mismatch.env_var,
        "actualRegion": mismatch.actual_region.as_str(),
        "expectedRegion": mismatch.expected_region.as_str(),
    })
}

async fn api_status(RawQuery(query): RawQuery) -> Response {
    let region = parse_region(query_value(query.as_deref(), "region").as_deref());
    // 安全红线 F：读取后校验凭据域归属；不匹配即拒绝使用该凭据（current 置空），
    // 并把结构化冲突信息交给前端渲染修复指引。
    let (auth, region_mismatch) = match auth_file::read_auth_file_checked_for(region) {
        Ok(auth) => (auth, Value::Null),
        Err(mismatch) => (None, mismatch_json(&mismatch)),
    };
    let current = auth.as_ref().and_then(|a| {
        let acct = a.get("account").cloned().unwrap_or_else(|| json!({}));
        Some(json!({
            "uid": acct.get("uid"),
            "nickname": acct.get("nickname"),
            "email": acct.get("email"),
        }))
    });
    json_ok(json!({
        "running": cached_workbuddy_running(region),
        "region": region,
        "authFile": auth_file::auth_file_path_for(region).to_string_lossy(),
        "current": current,
        "appPath": auth_file::workbuddy_app_path_for(region).to_string_lossy(),
        "version": update::APP_VERSION,
        "installed": region_installed(region),
        "regionMismatch": region_mismatch,
    }))
}

async fn api_accounts(RawQuery(query): RawQuery) -> Response {
    let region = parse_region(query_value(query.as_deref(), "region").as_deref());
    json_ok(json!({
        "region": region,
        "accounts": account::load_accounts_for(region)
            .iter()
            .map(account::account_meta)
            .collect::<Vec<_>>(),
        "current": auth_file::read_auth_file_for(region)
            .and_then(|a| a.get("account").and_then(|x| x.get("uid")).and_then(|x| x.as_str()).map(String::from)),
    }))
}

/// 在系统文件管理器中打开目录（跨平台）。
fn reveal_dir(dir: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";

    #[cfg(any(target_os = "windows", target_os = "macos", unix))]
    {
        std::process::Command::new(program)
            .arg(dir)
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("打开目录失败: {error}"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
    {
        let _ = dir;
        Err("当前平台不支持打开目录".to_string())
    }
}

/// POST /api/accounts/open-dir —— 在文件管理器中打开该 region 的账号库目录。
///
/// 账号库落在 `~/.buddy-switch/`（`accounts.json` / `accounts.global.json`），
/// 目录不存在时先创建，避免「打开失败」。
async fn api_open_accounts_dir(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let file = buddy_switch_core::modules::region::accounts_file_for(region);
    let dir = file
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| file.clone());
    if !dir.exists() {
        if let Err(error) = std::fs::create_dir_all(&dir) {
            return json_err(
                format!("创建账号库目录失败: {error}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            );
        }
    }
    match reveal_dir(&dir) {
        Ok(()) => json_ok(json!({
            "ok": true,
            "region": region.as_str(),
            "dir": dir.to_string_lossy(),
        })),
        Err(error) => json_err(error, StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_codebuddy_cli_status() -> Response {
    json_ok(codebuddy_cli::status())
}

async fn api_codebuddy_cli_install_helper() -> Response {
    match codebuddy_cli::install_helper() {
        Ok(result) => json_ok(result),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

async fn api_codebuddy_cli_switch(Json(body): Json<Value>) -> Response {
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    match codebuddy_cli::set_active_account(id) {
        Ok(result) => json_ok(result),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

async fn api_codebuddy_cn_ide_status() -> Response {
    json_ok(codebuddy_cn_ide::status())
}

async fn api_codebuddy_cn_ide_switch(Json(body): Json<Value>) -> Response {
    let account_id = body
        .get("accountId")
        .or_else(|| body.get("account_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let restart = body.get("restart").and_then(|v| v.as_bool()).unwrap_or(true);
    match codebuddy_cn_ide::switch_account(account_id, restart) {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

async fn api_codebuddy_cn_ide_detect() -> Response {
    match codebuddy_cn_ide::detect_current_account() {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}


async fn api_delete(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    match account::delete_account_for(region, id) {
        Ok(()) => json_ok(json!({ "ok": true })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

async fn api_import_local(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    match account::import_local_for(region) {
        Ok(acc) => json_ok(json!({ "ok": true, "account": acc })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// 导出 / 导入账号
// ---------------------------------------------------------------------------

async fn api_export_accounts(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let ids: Vec<String> = body
        .get("accountIds")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    match export_import::export_accounts_for(region, &ids) {
        Ok(records) => json_ok(json!({ "ok": true, "accounts": records })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

async fn api_export_accounts_to_path(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let ids: Vec<String> = body
        .get("accountIds")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let path = body
        .get("path")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    match export_import::export_accounts_to_path_for(region, &ids, &path) {
        Ok(path) => json_ok(json!({ "ok": true, "path": path })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

/// POST /api/import/preview —— 解析导入文件并返回脱敏预览。
///
/// **无需 region**：`preview_accounts` 是纯函数，只解析请求里的文件文本，不触及任何
/// region 账号库（core 也未提供 `_for` 变体）。region 由后续 `/api/import` 决定落库位置。
async fn api_preview_import(Json(body): Json<Value>) -> Response {
    let text = body
        .get("fileText")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    match export_import::preview_accounts(&text) {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

async fn api_import(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let text = body
        .get("fileText")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let indexes: Vec<usize> = body
        .get("indexes")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_u64().map(|n| n as usize))
                .collect()
        })
        .unwrap_or_default();
    match export_import::import_accounts_for(region, &text, &indexes) {
        Ok(result) => json_ok(json!({
            "ok": true,
            "imported": result.imported,
            "skipped": result.skipped,
            "overwritten": result.overwritten,
        })),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// OAuth 登录
// ---------------------------------------------------------------------------

async fn api_oauth_start(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    match oauth::oauth_start_for(region).await {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

async fn api_oauth_status(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let login_id = body
        .get("loginId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    json_ok(oauth::oauth_poll_for(region, &login_id).await)
}

// ---------------------------------------------------------------------------
// 切换
// ---------------------------------------------------------------------------

async fn api_switch(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let account_id = body
        .get("accountId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if account_id.trim().is_empty() {
        return json_err("缺少 accountId".to_string(), StatusCode::BAD_REQUEST);
    }
    let restart = body
        .get("restart")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let share_sessions = body
        .get("shareSessions")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let copy_ids: Vec<String> = body
        .get("copySessionIds")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    {
        let mut running = SWITCH_RUNNING.lock().unwrap();
        if *running {
            return json_err("已有切换任务进行中".to_string(), StatusCode::CONFLICT);
        }
        *running = true;
        *SWITCH_PROGRESS.lock().unwrap() = Some("开始切换账号…".to_string());
    }

    let progress: switch::ProgressFn = Box::new(|msg| {
        *SWITCH_PROGRESS.lock().unwrap() = Some(msg.to_string());
    });

    let result = tokio::task::spawn_blocking(move || {
        switch::switch_account_for(
            region,
            Some(&progress),
            &account_id,
            restart,
            share_sessions,
            &copy_ids,
        )
    })
    .await;

    *SWITCH_RUNNING.lock().unwrap() = false;

    match result {
        Ok(Ok(v)) => json_ok(v),
        Ok(Err(e)) => json_err(e, StatusCode::BAD_REQUEST),
        Err(e) => json_err(e.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_switch_progress() -> Response {
    let p = SWITCH_PROGRESS.lock().unwrap().clone();
    let running = *SWITCH_RUNNING.lock().unwrap();
    json_ok(json!({ "running": running, "progress": p }))
}

// ---------------------------------------------------------------------------
// 会话
// ---------------------------------------------------------------------------

async fn api_sessions(RawQuery(query): RawQuery) -> Response {
    let region = parse_region(query_value(query.as_deref(), "region").as_deref());
    match session::current_user_uid_for(region) {
        Some(uid) => json_ok(json!({
            "sessions": session::list_sessions_for_user_for(region, &uid),
            "current": uid,
        })),
        None => json_ok(json!({ "sessions": [], "current": null })),
    }
}

async fn api_copy_sessions(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let target_account_id = body
        .get("targetAccountId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let session_ids: Vec<String> = body
        .get("sessionIds")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let Some(target) = account::find_account_for(region, &target_account_id) else {
        return json_err("目标账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    // copy_sessions_for_switch_for 已返回完整报告（sourceUid / targetUid / copied /
    // skipped? / errors?）。这里必须**原样透传**，不能再包一层 "copied" —— 否则
    // 响应会变成 {copied:{copied:[...]}}，与 Tauri 通道及前端 CopyResult[] 契约不一致。
    match session::copy_sessions_for_switch_for(region, &target, &session_ids) {
        Some(report) => json_ok(report),
        None => json_ok(json!({
            "sourceUid": Value::Null,
            "targetUid": target.get("uid"),
            "copied": [],
        })),
    }
}

// ---------------------------------------------------------------------------
// 账号数据迁移（Memory / Connector 合并去重）
// ---------------------------------------------------------------------------

/// POST /api/migrate/account —— 把源账号的 Memory / Connector 合并到目标账号（带去重）。
///
/// 请求体：`{ sourceAccountId?, targetAccountId, memory?, connectors?, region? }`
/// `sourceAccountId` 缺省时取当前登录账号。两个范围默认都启用。
/// 只处理普通文件，不触碰 `workbuddy.db`，因此**无需关闭 WorkBuddy**。
async fn api_migrate_account(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let target_account_id = body
        .get("targetAccountId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if target_account_id.is_empty() {
        return json_err("缺少 targetAccountId".to_string(), StatusCode::BAD_REQUEST);
    }

    let source_account_id = body
        .get("sourceAccountId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let scope = migrate::MigrateScope {
        memory: body.get("memory").and_then(Value::as_bool).unwrap_or(true),
        connectors: body
            .get("connectors")
            .and_then(Value::as_bool)
            .unwrap_or(true),
    };
    if scope.is_empty() {
        return json_err("未指定任何迁移范围".to_string(), StatusCode::BAD_REQUEST);
    }

    let Some(target) = account::find_account_for(region, &target_account_id) else {
        return json_err("目标账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    let target_uid = target
        .get("uid")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if target_uid.is_empty() {
        return json_err("目标账号缺少 uid".to_string(), StatusCode::BAD_REQUEST);
    }

    // 源账号：显式指定则解析其 uid，否则退回「当前登录账号」。
    let source_uid = if source_account_id.is_empty() {
        session::current_user_uid_for(region).unwrap_or_default()
    } else {
        match account::find_account_for(region, &source_account_id) {
            Some(acc) => acc
                .get("uid")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            None => return json_err("源账号不存在".to_string(), StatusCode::BAD_REQUEST),
        }
    };
    if source_uid.is_empty() {
        return json_err(
            "无法确定源账号 uid（未登录或账号缺少 uid）".to_string(),
            StatusCode::BAD_REQUEST,
        );
    }

    match migrate::migrate_account_data_for(region, &source_uid, &target_uid, scope) {
        Ok(v) => json_ok(v),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// 签到 / 保活
// ---------------------------------------------------------------------------

async fn api_checkin_status(RawQuery(query): RawQuery) -> Response {
    let region = parse_region(query_value(query.as_deref(), "region").as_deref());
    let list = account::load_accounts_for(region);
    let mut items = Vec::new();
    for acc in &list {
        let status = checkin::get_checkin_status_for(region, acc).await;
        items.push(checkin_status_item(acc, status));
    }
    json_ok(json!({ "accounts": items }))
}

fn checkin_status_item(account: &Value, mut status: Value) -> Value {
    status["accountId"] = account.get("id").cloned().unwrap_or(Value::Null);
    status["email"] = json!(account::account_display_name(account));
    status
}

async fn api_credits(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    let Some(acc) = account::find_account_for(region, id) else {
        return json_err("账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    json_ok(credits::get_credit_expiry_for(region, &acc).await)
}

fn query_flag_enabled(query: Option<&str>, name: &str) -> bool {
    query.unwrap_or("").split('&').any(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, "true"));
        key == name && matches!(value, "" | "1" | "true" | "yes")
    })
}

async fn api_credit_statistics(RawQuery(query): RawQuery) -> Response {
    let filter = parse_region_filter(query_value(query.as_deref(), "region").as_deref());
    let refresh = query_flag_enabled(query.as_deref(), "refresh");
    json_ok(credit_usage::get_statistics_for_filter(filter, refresh).await)
}

async fn api_token_statistics(RawQuery(query): RawQuery) -> Response {
    let filter = parse_region_filter(query_value(query.as_deref(), "region").as_deref());
    let days = query.as_deref().and_then(|value| {
        value.split('&').find_map(|part| {
            part.strip_prefix("days=")?.parse::<i64>().ok()
        })
    });
    match tokio::task::spawn_blocking(move || token_stats::get_statistics_for_filter(filter, days))
        .await
    {
        Ok(statistics) => json_ok(statistics),
        Err(error) => json_err(
            format!("扫描 Token 统计失败: {error}"),
            StatusCode::INTERNAL_SERVER_ERROR,
        ),
    }
}

async fn api_checkin(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    let Some(acc) = account::find_account_for(region, id) else {
        return json_err("账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    json_ok(checkin::checkin_account_for(region, &acc).await)
}

async fn api_checkin_all(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    json_ok(checkin::run_checkin_all_for(region).await)
}

/// GET /api/checkin/config —— 自动签到开关。
///
/// **无需 region**：签到配置是**全局单份**（`auto_checkin_config.json`），
/// 两个 region 共用同一套「是否自动签到」开关；签到日志亦为全局。
async fn api_checkin_config() -> Response {
    json_ok(config::load_checkin_config())
}

async fn api_save_checkin_config(Json(body): Json<Value>) -> Response {
    let submitted = body.get("config").unwrap_or(&body);
    match config::save_checkin_config(submitted) {
        Ok(()) => json_ok(config::load_checkin_config()),
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}

/// GET /api/checkin/logs —— 签到日志（全局单份，按账号 id 归档，无需 region）。
async fn api_checkin_logs() -> Response {
    json_ok(json!({ "logs": config::load_checkin_logs() }))
}

/// GET /api/travel/status —— 派猫猫旅行状态（**CN 专有**）。
///
/// core 的 `travel` 模块没有 `_for(region, …)` 变体（旅行状态机与缓存按账号 id
/// 全局单份，region 化改动面大、风险高），因此本路由保持 CN 语义。国际版无对应
/// 上游能力时按「该功能独立降级」处理：前端按 region 账号 id 过滤后自然为空，
/// 不影响同页其它功能。已知缺口，见发布报告。
async fn api_travel_status() -> Response {
    travel::reconcile_due_travel(None).await;
    let items = account::load_accounts()
        .iter()
        .map(|acc| {
            let id = acc.get("id").and_then(Value::as_str).unwrap_or("");
            let mut value = travel::travel_display(id);
            value["accountId"] = acc.get("id").cloned().unwrap_or(Value::Null);
            value["email"] = json!(account::account_display_name(acc));
            value
        })
        .collect::<Vec<_>>();
    json_ok(json!({ "accounts": items }))
}

async fn api_travel_config() -> Response {
    json_ok(config::load_travel_config())
}

async fn api_save_travel_config(Json(body): Json<Value>) -> Response {
    let submitted = body.get("config").unwrap_or(&body);
    match config::save_travel_config(submitted) {
        Ok(()) => {
            let saved = config::load_travel_config();
            if saved.get("enabled").and_then(Value::as_bool) == Some(true) {
                tokio::spawn(async {
                    let _ = travel::run_travel_cycle().await;
                    let _ = travel::run_travel_claim_cycle().await;
                });
            }
            json_ok(saved)
        }
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}

async fn api_refresh_token(Json(body): Json<Value>) -> Response {
    let id = body.get("accountId").and_then(|v| v.as_str()).unwrap_or("");
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let Some(acc) = account::find_account_for(region, id) else {
        return json_err("账号不存在".to_string(), StatusCode::BAD_REQUEST);
    };
    json_ok(refresh::refresh_account_token_for(region, acc).await)
}

// ---------------------------------------------------------------------------
// 自动轮换（CodeBuddy CLI）
//
// **CN 专有**：自动轮换驱动的是 CodeBuddy CLI（国内版工具链），core 的 `rotate`
// 模块无 `_for(region, …)` 变体，因此本组路由保持 CN 语义。已知缺口，见发布报告。
// ---------------------------------------------------------------------------

async fn api_rotate_config() -> Response {
    json_ok(config::load_auto_rotate_config())
}

async fn api_save_rotate_config(Json(body): Json<Value>) -> Response {
    match config::save_auto_rotate_config(&body) {
        Ok(()) => json_ok(json!({ "ok": true, "config": config::load_auto_rotate_config() })),
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}

async fn api_rotate_status() -> Response {
    json_ok(rotate::rotate_status())
}

async fn api_rotate_run() -> Response {
    json_ok(rotate::run_rotate_cycle().await)
}

async fn api_rotate_logs() -> Response {
    json_ok(json!({ "logs": rotate::rotate_logs() }))
}

// ---------------------------------------------------------------------------
// 定时任务排程（六类积分任务，全局单份，无需 region）
// ---------------------------------------------------------------------------

/// GET /api/schedule/config —— 六类定时任务的排程配置。
async fn api_schedule_config() -> Response {
    json_ok(schedule::schedule_to_value(&schedule::load_schedule_config()))
}

/// POST /api/schedule/config —— 写排程配置；小时越界返回 400（错误文案指向对应 `*_enabled`）。
async fn api_save_schedule_config(Json(body): Json<Value>) -> Response {
    let submitted = body.get("config").unwrap_or(&body);
    match schedule::save_schedule_config(submitted) {
        Ok(cfg) => json_ok(schedule::schedule_to_value(&cfg)),
        Err(e) => json_err(e, StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// 更新
// ---------------------------------------------------------------------------

async fn api_update_check() -> Response {
    json_ok(update::update_check(None, false).await)
}

async fn api_update_config() -> Response {
    json_ok(update::load_github_config())
}

async fn api_save_update_config(Json(body): Json<Value>) -> Response {
    match update::save_github_config(&body) {
        Ok(()) => json_ok(json!({ "ok": true, "config": update::load_github_config() })),
        Err(e) => json_err(e.to_string(), StatusCode::BAD_REQUEST),
    }
}

// ---------------------------------------------------------------------------
// API 网关（管理面，A-3.7）
// ---------------------------------------------------------------------------

async fn api_gateway_config() -> Response {
    match serde_json::to_value(GatewayConfig::load()) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_save_gateway_config(Json(body): Json<Value>) -> Response {
    let submitted = body.get("config").cloned().unwrap_or(body);
    let config: GatewayConfig = match serde_json::from_value(submitted) {
        Ok(config) => config,
        Err(error) => return json_err(format!("配置格式错误: {error}"), StatusCode::BAD_REQUEST),
    };
    if let Err(error) = config.save() {
        return json_err(error, StatusCode::BAD_REQUEST);
    }
    let state = crate::gateway_host::shared_state();
    *state.config.write().await = config.clone();
    state.log.set_keep(config.log_keep);
    state.log.set_log_bodies(config.log_bodies);
    let addr = match crate::gateway_host::apply().await {
        Ok(addr) => addr,
        Err(error) => return json_err(error, StatusCode::BAD_REQUEST),
    };
    json_ok(json!({
        "ok": true,
        "config": config,
        "running": addr.is_some(),
        "addr": addr,
    }))
}

async fn api_gateway_status() -> Response {
    let config = GatewayConfig::load();
    let (running, addr) = crate::gateway_host::status().await;
    // 统一走显式契约结构体（snake_case，与 GatewayConfig 一致），不再手工拼 json!。
    let mut view = GatewayStatusView::from(&config);
    view.running = running;
    view.addr = addr;
    view.version = update::APP_VERSION.to_string();
    match serde_json::to_value(view) {
        Ok(value) => json_ok(value),
        Err(error) => json_err(error.to_string(), StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn api_gateway_list_keys() -> Response {
    let state = crate::gateway_host::shared_state();
    let keys: Vec<Value> = state.keys.list().iter().map(|record| record.masked()).collect();
    json_ok(json!({ "keys": keys }))
}

async fn api_gateway_create_key(Json(body): Json<Value>) -> Response {
    let state = crate::gateway_host::shared_state();
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let name = body
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("未命名")
        .to_string();
    let (record, plaintext) = state.keys.create(name, region);
    json_ok(json!({ "ok": true, "key": plaintext, "record": record.masked() }))
}

async fn api_gateway_revoke_key(Json(body): Json<Value>) -> Response {
    let state = crate::gateway_host::shared_state();
    let id = body.get("id").and_then(Value::as_str).unwrap_or("");
    match state.keys.revoke(id) {
        Ok(()) => json_ok(json!({ "ok": true })),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

async fn api_gateway_delete_key(Json(body): Json<Value>) -> Response {
    let state = crate::gateway_host::shared_state();
    let id = body.get("id").and_then(Value::as_str).unwrap_or("");
    match state.keys.delete(id) {
        Ok(()) => json_ok(json!({ "ok": true })),
        Err(error) => json_err(error, StatusCode::BAD_REQUEST),
    }
}

async fn api_gateway_models(RawQuery(query): RawQuery) -> Response {
    let region = parse_region(query_value(query.as_deref(), "region").as_deref());
    let state = crate::gateway_host::shared_state();
    let snapshot = state.catalogs.current(region);
    json_ok(serde_json::to_value(snapshot).unwrap_or(Value::Null))
}

async fn api_gateway_refresh_models(Json(body): Json<Value>) -> Response {
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let state = crate::gateway_host::shared_state();
    let strategy = state.strategy_for(region).await;
    let account = match buddy_switch_gateway::AccountSelector
        .select(region, &strategy)
        .await
    {
        Ok(account) => account,
        Err(error) => return json_err(error.message(), StatusCode::BAD_REQUEST),
    };
    let snapshot = state
        .catalogs
        .refresh(region, &state.upstream, &account)
        .await;
    json_ok(serde_json::to_value(snapshot).unwrap_or(Value::Null))
}

async fn api_gateway_strategy() -> Response {
    let state = crate::gateway_host::shared_state();
    // 先取快照，避免跨 await 持有读锁。
    let strategies = state.strategies.read().await.clone();
    let cn = strategies.get(&Region::Cn).cloned().unwrap_or_default();
    let global = strategies.get(&Region::Global).cloned().unwrap_or_default();
    json_ok(json!({
        "cn": buddy_switch_gateway::account_strategy::describe_strategy(Region::Cn, &cn).await,
        "global": buddy_switch_gateway::account_strategy::describe_strategy(Region::Global, &global).await,
    }))
}

async fn api_gateway_save_strategy(Json(body): Json<Value>) -> Response {
    let state = crate::gateway_host::shared_state();
    let region = parse_region(body.get("region").and_then(Value::as_str));
    let strategy_value = body.get("strategy").cloned().unwrap_or_else(|| {
        let mut clone = body.clone();
        if let Some(object) = clone.as_object_mut() {
            object.remove("region");
        }
        clone
    });
    let strategy: buddy_switch_gateway::AccountStrategy = match serde_json::from_value(strategy_value) {
        Ok(strategy) => strategy,
        Err(error) => return json_err(format!("策略格式错误: {error}"), StatusCode::BAD_REQUEST),
    };
    {
        let mut strategies = state.strategies.write().await;
        strategies.insert(region, strategy);
        if let Err(error) = buddy_switch_gateway::account_strategy::save_strategies(&strategies) {
            return json_err(error, StatusCode::BAD_REQUEST);
        }
    }
    json_ok(json!({ "ok": true }))
}

async fn api_gateway_logs() -> Response {
    let state = crate::gateway_host::shared_state();
    json_ok(json!({ "logs": state.log.list() }))
}

async fn api_gateway_clear_logs() -> Response {
    let state = crate::gateway_host::shared_state();
    state.log.clear();
    json_ok(json!({ "ok": true }))
}

// ---------------------------------------------------------------------------
// 静态前端
// ---------------------------------------------------------------------------

fn content_type(path: &str) -> &'static str {
    if path.ends_with(".js") || path.ends_with(".mjs") {
        "text/javascript"
    } else if path.ends_with(".css") {
        "text/css"
    } else if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".json") {
        "application/json"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".ico") {
        "image/x-icon"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    }
}

async fn static_handler(uri: Uri) -> Response {
    let mut path = uri.path().trim_start_matches('/').to_string();
    if path.is_empty() || path == "index.html" {
        path = "index.html".to_string();
    }
    // 前端路由回退到 index.html。
    //
    // Content-Type 必须由**实际被服务的资源名**推导，而不是请求路径：SPA 深链
    // （如 `/accounts`）在 embed 里落空后会回退到 index.html 的内容，但若仍按请求
    // 路径（无扩展名）算 MIME，会得到 `application/octet-stream`，浏览器会把 HTML
    // 当成二进制**下载**而不是渲染应用（`App.tsx` 用的是 `BrowserRouter`，深链是常态）。
    let (data, served_path) = match Assets::get(&path) {
        Some(file) => (Some(file), path),
        None => match Assets::get("index.html") {
            Some(file) => (Some(file), "index.html".to_string()),
            None => (None, path),
        },
    };
    match data {
        Some(f) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type(&served_path))
            .body(Body::from(f.data.into_owned()))
            .unwrap(),
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("not found"))
            .unwrap(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        api_routes, checkin_status_item, parse_region, parse_region_filter, query_flag_enabled,
        query_value, router, Assets,
    };
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use serde_json::{json, Value};
    use std::collections::BTreeSet;
    use std::sync::{Mutex, MutexGuard, OnceLock};
    use tower::ServiceExt;
    use buddy_switch_core::modules::config::BUDDY_SWITCH_HOME_ENV;
    use buddy_switch_core::modules::region::{Region, RegionFilter};

    // -----------------------------------------------------------------------
    // 路由级测试脚手架
    //
    // 本 crate 是**二进制 crate**（`[[bin]]`，无 lib target），集成测试无法 import
    // `api_routes()`，故路由级测试放在此处的单元测试里（可访问私有 `api_routes`）。
    //
    // 关键：`gateway_host::shared_state()` 是进程级 `OnceLock`，且 `GatewayConfig::load()`
    // 每次实时读取 `home_dir()`。因此必须在**任何**触及 home 的调用之前，把
    // `BUDDY_SWITCH_HOME` 指向隔离目录——否则会读写真实 `~/.buddy-switch/`。
    // -----------------------------------------------------------------------

    /// 串行化所有路由测试：既避免并行修改进程级环境变量，也避免多个写操作测试
    /// 争抢同一份共享网关状态（Key 文件 / 配置）。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 进程内唯一的隔离 home（首次调用时创建并把 `BUDDY_SWITCH_HOME` 指向它）。
    static ISOLATED_HOME: OnceLock<std::path::PathBuf> = OnceLock::new();

    fn test_guard() -> MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// 返回隔离的 home 目录。必须作为每个测试的**第一步**调用。
    fn isolated_home() -> &'static std::path::Path {
        ISOLATED_HOME
            .get_or_init(|| {
                let dir = std::env::temp_dir().join(format!(
                    "buddy-switch-server-api-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0)
                ));
                std::fs::create_dir_all(&dir).expect("create isolated home");
                // OnceLock 保证恰好设置一次，且对并发调用串行化。
                std::env::set_var(BUDDY_SWITCH_HOME_ENV, &dir);
                dir
            })
            .as_path()
    }

    fn exact_keys(value: &Value) -> BTreeSet<String> {
        value
            .as_object()
            .expect("value must be a JSON object")
            .keys()
            .cloned()
            .collect()
    }

    fn set_of(keys: &[&str]) -> BTreeSet<String> {
        keys.iter().map(|key| key.to_string()).collect()
    }

    fn assert_has_keys(value: &Value, keys: &[&str]) {
        let actual = exact_keys(value);
        for key in keys {
            assert!(actual.contains(*key), "response is missing key `{key}`: {actual:?}");
        }
    }

    /// 是否含有连续 `minimum` 个十六进制字符的片段（用于抓 64 位 sha256 摘要）。
    fn contains_hex_run(text: &str, minimum: usize) -> bool {
        let mut run = 0usize;
        for ch in text.chars() {
            if ch.is_ascii_hexdigit() {
                run += 1;
                if run >= minimum {
                    return true;
                }
            } else {
                run = 0;
            }
        }
        false
    }

    /// 驱动 `api_routes()`（**不含 fallback**）并返回 (状态码, JSON)。
    async fn call_api(method: Method, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = match body {
            Some(value) => Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&value).unwrap()))
                .unwrap(),
            None => Request::builder()
                .method(method)
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        };
        let response = api_routes().oneshot(request).await.expect("router call");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }

    /// 驱动完整 `router()`（含静态 fallback），返回 (状态码, content-type, body 文本)。
    async fn call_full(method: Method, uri: &str) -> (StatusCode, String, String) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let response = router().oneshot(request).await.expect("router call");
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read body");
        (status, content_type, String::from_utf8_lossy(&bytes).to_string())
    }

    // -----------------------------------------------------------------------
    // 纯函数
    // -----------------------------------------------------------------------

    #[test]
    fn parse_region_defaults_to_cn_and_binds_known_values() {
        assert_eq!(parse_region(None), Region::Cn, "missing region must default to CN");
        assert_eq!(parse_region(Some("cn")), Region::Cn);
        assert_eq!(parse_region(Some("global")), Region::Global);
        assert_eq!(parse_region(Some("ai")), Region::Global);
        assert_eq!(parse_region(Some("GLOBAL")), Region::Global);
        // 未知 / 空值回落到 CN（保证旧行为）。
        assert_eq!(parse_region(Some("")), Region::Cn);
        assert_eq!(parse_region(Some("xx")), Region::Cn);
        assert_ne!(parse_region(None), Region::Global);
    }

    #[test]
    fn parse_region_filter_supports_all_and_keeps_legacy_bindings() {
        // 与既有 `parse_region` 绑定规则一致。
        assert_eq!(parse_region_filter(None), RegionFilter::Cn);
        assert_eq!(parse_region_filter(Some("")), RegionFilter::Cn);
        assert_eq!(parse_region_filter(Some("xx")), RegionFilter::Cn);
        assert_eq!(parse_region_filter(Some("cn")), RegionFilter::Cn);
        assert_eq!(parse_region_filter(Some("ai")), RegionFilter::Global);
        assert_eq!(parse_region_filter(Some("GLOBAL")), RegionFilter::Global);
        // 新增：合并视图。
        assert_eq!(parse_region_filter(Some("all")), RegionFilter::All);
        assert_eq!(parse_region_filter(Some("ALL")), RegionFilter::All);
        assert_eq!(parse_region_filter(Some("合并")), RegionFilter::All);
    }

    #[test]
    fn query_value_reads_first_match_and_handles_missing() {
        assert_eq!(query_value(Some("region=global"), "region").as_deref(), Some("global"));
        assert_eq!(
            query_value(Some("a=1&region=global&b=2"), "region").as_deref(),
            Some("global")
        );
        assert_eq!(query_value(Some("a=1"), "region"), None);
        assert_eq!(query_value(None, "region"), None);
        assert_eq!(query_value(Some("flag"), "flag").as_deref(), Some(""));
        assert_eq!(query_value(Some("x=1&x=2"), "x").as_deref(), Some("1"));
    }

    #[test]
    fn query_flag_enabled_matches_truthy_forms() {
        assert!(query_flag_enabled(Some("refresh"), "refresh"));
        assert!(query_flag_enabled(Some("refresh="), "refresh"));
        assert!(query_flag_enabled(Some("refresh=1"), "refresh"));
        assert!(query_flag_enabled(Some("refresh=true"), "refresh"));
        assert!(query_flag_enabled(Some("refresh=yes"), "refresh"));
        assert!(!query_flag_enabled(Some("refresh=false"), "refresh"));
        assert!(!query_flag_enabled(Some("refresh=0"), "refresh"));
        assert!(!query_flag_enabled(Some("refresh=no"), "refresh"));
        assert!(!query_flag_enabled(Some("other=1"), "refresh"));
        assert!(!query_flag_enabled(None, "refresh"));
        assert!(query_flag_enabled(Some("a=1&refresh=yes"), "refresh"));
    }

    // -----------------------------------------------------------------------
    // 只读路由
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn core_read_only_routes_return_expected_shapes() {
        let _guard = test_guard();
        isolated_home();

        let (status, body) = call_api(Method::GET, "/api/status", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_has_keys(
            &body,
            &[
                "running",
                "region",
                "authFile",
                "current",
                "appPath",
                "version",
                "installed",
                "regionMismatch",
            ],
        );
        assert_eq!(body["region"], json!("cn"));
        // E3.3：契约必须产出 `installed` / `regionMismatch` 两个字段（前端读点）。
        assert!(
            body["installed"].is_boolean(),
            "status.installed 必须是布尔：{body}"
        );
        assert!(
            body["regionMismatch"].is_null(),
            "隔离 home 无认证文件，regionMismatch 必须为 null：{body}"
        );

        let (status, body) = call_api(Method::GET, "/api/accounts", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_has_keys(&body, &["region", "accounts", "current"]);
        assert_eq!(body["region"], json!("cn"));
        // 空库上的 `== []` 无法证伪「账号映射」是否真的生效（数据层回归不可见），
        // 故此处仅断言形状；内容映射由
        // `accounts_route_returns_seeded_account_contents` 播种后精确断言。
        assert!(body["accounts"].is_array(), "accounts must be a JSON array: {body}");
        assert_eq!(body["current"], Value::Null);

        let (status, body) = call_api(Method::GET, "/api/sessions", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_has_keys(&body, &["sessions", "current"]);
        // 空库上的 `== []` 无法证伪「uid → sessions 表 → 响应」这条链路，
        // 故此处仅断言形状；内容映射由
        // `sessions_route_returns_seeded_session_rows` 播种真实 workbuddy.db 后精确断言。
        assert!(body["sessions"].is_array(), "sessions must be a JSON array: {body}");

        let (status, body) = call_api(Method::GET, "/api/checkin/status", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(exact_keys(&body), set_of(&["accounts"]));
        assert_eq!(body["accounts"], json!([]));

        let (status, body) = call_api(Method::GET, "/api/switch/progress", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(exact_keys(&body), set_of(&["running", "progress"]));
        assert_eq!(body["running"], json!(false));
    }

    /// Q1 修复：`/api/accounts` 的「账号库 → 响应」映射必须在**有数据**时断言，
    /// 否则空隔离库上的 `== []` 恒真、无法发现数据层回归。此处播种 1 条 CN 账号，
    /// 断言返回**恰好该条**且具体字段正确；断言前清理，保持其它用例的空库前置
    /// （否则 `/api/checkin/status` 会因此对播种账号发起真实网络请求）。
    #[tokio::test]
    async fn accounts_route_returns_seeded_account_contents() {
        let _guard = test_guard();
        let home = isolated_home();

        let accounts_file = home.join(".buddy-switch").join("accounts.json");
        std::fs::create_dir_all(accounts_file.parent().unwrap()).expect("create store dir");
        std::fs::write(
            &accounts_file,
            r#"[{"id":"seeded-cn-1","uid":"uid-seeded","nickname":"Seeded CN","email":"seeded@example.com"}]"#,
        )
        .expect("seed accounts");

        let (status, body) = call_api(Method::GET, "/api/accounts", None).await;
        // 先清理，避免种子泄漏到其它用例（即便断言随后失败也不会污染后续用例）。
        std::fs::remove_file(&accounts_file).expect("remove seeded accounts");

        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["region"], json!("cn"));
        let accounts = body["accounts"].as_array().expect("accounts array");
        assert_eq!(
            accounts.len(),
            1,
            "a seeded store must surface exactly one account: {body}"
        );
        assert_eq!(accounts[0]["id"], json!("seeded-cn-1"));
        assert_eq!(accounts[0]["uid"], json!("uid-seeded"));
        assert_eq!(accounts[0]["email"], json!("seeded@example.com"));
        assert_eq!(accounts[0]["nickname"], json!("Seeded CN"));
        assert_eq!(accounts[0]["needsRelogin"], json!(false));
    }

    /// 修复 `/api/sessions` 的空态断言（原 `sessions == []` 跑在「没有 workbuddy.db」的
    /// 隔离 home 上，**恒真、打不红**，无法发现「认证 uid → sessions 表 → 响应」这条
    /// 链路上的任何回归）。此处播种一个真实的 `workbuddy.db` 与认证文件，断言**恰好**
    /// 返回应返回的两条及其字段值，并覆盖 4 类必须被过滤 / 跳过的行。
    #[tokio::test]
    async fn sessions_route_returns_seeded_session_rows() {
        const UID: &str = "uid-sessions-seeded";

        let _guard = test_guard();
        let home = isolated_home();

        // 1) 播种 CN 会话库 `<home>/.workbuddy/workbuddy.db`。
        let data_dir = home.join(".workbuddy");
        let project_dir = data_dir.join("projects").join("ws-beta");
        std::fs::create_dir_all(&project_dir).expect("create projects dir");

        let db_path = data_dir.join("workbuddy.db");
        {
            let conn = rusqlite::Connection::open(&db_path).expect("open seeded db");
            conn.execute_batch(
                "CREATE TABLE sessions (
                    id TEXT, user_id TEXT, cwd TEXT, title TEXT, custom_title TEXT,
                    updated_at INTEGER, deleted_at INTEGER, is_playground INTEGER
                 );",
            )
            .expect("create sessions table");
            let insert = "INSERT INTO sessions \
                 (id, user_id, cwd, title, custom_title, updated_at, deleted_at, is_playground) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";
            let seed = |id: &str,
                        uid: &str,
                        cwd: &str,
                        title: &str,
                        custom: Option<&str>,
                        updated_at: i64,
                        deleted_at: Option<i64>,
                        playground: i64| {
                conn.execute(
                    insert,
                    rusqlite::params![
                        id,
                        uid,
                        cwd,
                        title,
                        custom,
                        updated_at,
                        deleted_at,
                        playground
                    ],
                )
                .expect("insert seeded session");
            };
            // 可见 1：普通会话，无 custom_title；无正文 jsonl → hasHistory=false。
            seed("cid-alpha", UID, "/home/u/ws-alpha", "Alpha 原标题", None, 2_000, None, 0);
            // 可见 2：custom_title 应**覆盖** title；is_playground=1 → isPlayground=true。
            seed("cid-beta", UID, "/home/u/ws-beta", "Beta 原标题", Some("Beta 自定义名"), 3_000, None, 1);
            // 过滤 3：已软删除（updated_at 最新，若漏过滤会排在首位，最易被发现）。
            seed("cid-deleted", UID, "/home/u/ws-alpha", "已删除", None, 4_000, Some(1_000), 0);
            // 过滤 4：属于其它账号。
            seed("cid-otheruser", "uid-other", "/home/u/ws-alpha", "别人的", None, 5_000, None, 0);
            // 跳过 5：claw 工作区（账号绑定 IM 渠道，换账号后不可用）。
            seed("cid-claw", UID, "/home/u/claw", "Claw", None, 6_000, None, 0);
        }
        // cid-beta 有正文 jsonl → hasHistory=true；cid-alpha 没有 → false。
        std::fs::write(
            project_dir.join("cid-beta.jsonl"),
            "{\"sessionId\":\"cid-beta\"}\n",
        )
        .expect("seed session jsonl");

        // 2) 播种 CN 认证文件：`current_user_uid()` 正是从这里取 uid。
        let auth_file = buddy_switch_core::modules::auth_file::auth_file_path_for(Region::Cn);
        std::fs::create_dir_all(auth_file.parent().unwrap()).expect("create auth dir");
        std::fs::write(&auth_file, format!(r#"{{"account":{{"uid":"{UID}"}}}}"#))
            .expect("seed auth file");

        // 3) 驱动路由，然后**先清理**再断言——即便断言失败也不污染后续用例
        //    （`core_read_only_routes_return_expected_shapes` 仍以空库为前提）。
        let (status, body) = call_api(Method::GET, "/api/sessions", None).await;
        let _ = std::fs::remove_dir_all(&data_dir);
        let _ = std::fs::remove_file(&auth_file);

        assert_eq!(status, StatusCode::OK);
        assert_eq!(exact_keys(&body), set_of(&["sessions", "current"]));
        assert_eq!(body["current"], json!(UID), "current must come from the seeded auth file");

        let sessions = body["sessions"].as_array().expect("sessions array");
        let ids: Vec<&str> = sessions.iter().filter_map(|s| s["id"].as_str()).collect();
        // 恰好两条：软删除 / 其它账号被过滤，claw 被跳过。
        assert_eq!(
            ids,
            vec!["cid-beta", "cid-alpha"],
            "must return exactly the visible, non-claw sessions ordered by updated_at DESC: {body}"
        );

        let beta = &sessions[0];
        assert_eq!(
            exact_keys(beta),
            set_of(&["id", "title", "cwd", "updatedAt", "hasHistory", "isPlayground"]),
            "session item key set must be pinned"
        );
        assert_eq!(beta["title"], json!("Beta 自定义名"), "custom_title must win over title");
        assert_eq!(beta["cwd"], json!("/home/u/ws-beta"));
        assert_eq!(beta["updatedAt"], json!(3_000));
        assert_eq!(beta["hasHistory"], json!(true), "cid-beta has a project jsonl");
        assert_eq!(beta["isPlayground"], json!(true));

        let alpha = &sessions[1];
        assert_eq!(alpha["title"], json!("Alpha 原标题"), "no custom_title → fall back to title");
        assert_eq!(alpha["updatedAt"], json!(2_000));
        assert_eq!(alpha["hasHistory"], json!(false), "cid-alpha has no project jsonl");
        assert_eq!(alpha["isPlayground"], json!(false));
    }

    #[tokio::test]
    async fn gateway_read_only_routes_expose_pinned_contracts() {
        let _guard = test_guard();
        isolated_home();

        // status：键集合精确匹配（E3 契约，snake_case）。
        let (status, body) = call_api(Method::GET, "/api/gateway/status", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            exact_keys(&body),
            set_of(&[
                "enabled",
                "running",
                "addr",
                "base_url",
                "bind_addr",
                "port",
                "allow_non_loopback",
                "version",
            ]),
            "gateway_status key set must be pinned (snake_case)"
        );
        // 前端归一化读取 base_url；不得再出现 camelCase 键。
        assert!(body["base_url"].is_string(), "base_url must be present for the frontend");
        assert!(!body.as_object().unwrap().contains_key("baseUrl"));

        // config：snake_case 键集合。
        let (status, body) = call_api(Method::GET, "/api/gateway/config", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            exact_keys(&body),
            set_of(&[
                "enabled",
                "bind_addr",
                "port",
                "allow_non_loopback",
                "dual_port",
                "log_keep",
                "log_bodies",
                "per_key_rate_limit",
                "max_rotate",
                "sanitize_fingerprints",
                "prompt_mode",
                "prompt_file",
                "sticky_ttl_ms",
                "pool",
                "allow_model_region_prefix",
                "max_body_mb",
            ])
        );

        // keys：空列表 + 仅 keys 键。
        let (status, body) = call_api(Method::GET, "/api/gateway/keys", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(exact_keys(&body), set_of(&["keys"]));
        assert_eq!(body["keys"], json!([]));

        // models：CatalogSnapshot 顶层键。
        let (status, body) = call_api(Method::GET, "/api/gateway/models", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            exact_keys(&body),
            set_of(&["region", "source", "fetched_at", "models", "note"])
        );

        // logs
        let (status, body) = call_api(Method::GET, "/api/gateway/logs", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(exact_keys(&body), set_of(&["logs"]));
        assert_eq!(body["logs"], json!([]));

        // strategy
        let (status, body) = call_api(Method::GET, "/api/gateway/strategy", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(exact_keys(&body), set_of(&["cn", "global"]));
    }

    #[tokio::test]
    async fn unknown_paths_fall_back_to_static_index() {
        let _guard = test_guard();
        isolated_home();

        // api_routes() 无 fallback：未知 /api 路径 → 404。
        let request = Request::builder()
            .method(Method::GET)
            .uri("/api/definitely-missing")
            .body(Body::empty())
            .unwrap();
        let response = api_routes().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        // router() 有静态 fallback：未知静态路径 → SPA 回退 index.html（F1 修复后
        // MIME 由**实际服务的资源**推导，未知路径也回退为 HTML）。
        let (status, content_type, body) = call_full(Method::GET, "/definitely-missing").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type, "text/html; charset=utf-8");
        assert!(
            body.to_lowercase().contains("<!doctype html"),
            "unknown static path should fall back to index.html"
        );

        // 未知 /api 路径在完整 router() 上同样回退到静态处理器（不是 JSON 404）。
        let (status, content_type, body) = call_full(Method::GET, "/api/definitely-missing").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type, "text/html; charset=utf-8");
        assert!(
            body.to_lowercase().contains("<!doctype html"),
            "unknown /api path should fall back to the static index, not a JSON 404"
        );

        // 命中 static_handler 的已知资源。
        let (status, content_type, body) = call_full(Method::GET, "/index.html").await;
        assert_eq!(status, StatusCode::OK);
        assert!(content_type.contains("text/html"), "content-type: {content_type}");
        assert!(body.to_lowercase().contains("<!doctype html"));

        let (status, content_type, _) = call_full(Method::GET, "/vite.svg").await;
        assert_eq!(status, StatusCode::OK);
        assert!(content_type.contains("image/svg+xml"), "content-type: {content_type}");
    }

    // -----------------------------------------------------------------------
    // F1 回归：静态资源的 Content-Type 必须由「**实际被服务的资源**」推导
    //
    // 生产 Bug：`static_handler` 曾用**请求路径**推导 MIME。SPA 深链（如 `/accounts`）
    // 在 embed 里落空 → 回退成 index.html 的**内容**，但 MIME 仍按无扩展名的请求路径
    // 计算，得到 `application/octet-stream`，浏览器把 HTML 当二进制**下载**而不是
    // 渲染应用（`App.tsx` 用的是 `BrowserRouter`，深链是常态）。
    // -----------------------------------------------------------------------

    /// F1：SPA 深链必须作为 HTML 返回——状态码 200、MIME 严格为 `text/html; charset=utf-8`、
    /// 且响应体确实是 index.html 的内容。
    #[tokio::test]
    async fn spa_deep_links_are_served_as_html_index() {
        let _guard = test_guard();
        isolated_home();

        // 基准：真实 index.html 的响应体，用来证明深链返回的是**同一份**内容。
        let (index_status, index_type, index_body) = call_full(Method::GET, "/index.html").await;
        assert_eq!(index_status, StatusCode::OK);
        assert_eq!(index_type, "text/html; charset=utf-8");
        assert!(index_body.to_lowercase().contains("<!doctype html"));

        for deep_link in ["/accounts", "/gateway", "/settings/nested/deep"] {
            let (status, content_type, body) = call_full(Method::GET, deep_link).await;
            assert_eq!(status, StatusCode::OK, "SPA deep link {deep_link} must be 200");
            assert_eq!(
                content_type, "text/html; charset=utf-8",
                "SPA deep link {deep_link} must be served as HTML, not an octet-stream download"
            );
            assert_eq!(
                body, index_body,
                "SPA deep link {deep_link} must serve the exact index.html body"
            );
        }

        // 根路径同样回退 index.html，且是 HTML。
        let (status, content_type, body) = call_full(Method::GET, "/").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type, "text/html; charset=utf-8");
        assert_eq!(body, index_body);
    }

    /// F1：**真实存在的**静态资源 MIME 不受影响——仍由其自身扩展名推导。
    #[tokio::test]
    async fn real_static_asset_mime_is_derived_from_asset_name() {
        let _guard = test_guard();
        isolated_home();

        // 稳定的、非哈希命名资源。
        let (status, content_type, _) = call_full(Method::GET, "/vite.svg").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type, "image/svg+xml");

        let (status, content_type, _) = call_full(Method::GET, "/icon.png").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type, "image/png");

        let (status, content_type, _) = call_full(Method::GET, "/index.html").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(content_type, "text/html; charset=utf-8");

        // 哈希命名的构建产物：从嵌入清单里取**真实**路径，避免硬编码哈希名失稳。
        let css = Assets::iter()
            .find(|path| path.ends_with(".css"))
            .expect("dist must embed a css asset");
        let (status, content_type, _) = call_full(Method::GET, &format!("/{css}")).await;
        assert_eq!(status, StatusCode::OK, "embedded asset {css} must be served");
        assert_eq!(content_type, "text/css", "asset {css} mime");

        let js = Assets::iter()
            .find(|path| path.ends_with(".js"))
            .expect("dist must embed a js asset");
        let (status, content_type, _) = call_full(Method::GET, &format!("/{js}")).await;
        assert_eq!(status, StatusCode::OK, "embedded asset {js} must be served");
        assert_eq!(content_type, "text/javascript", "asset {js} mime");
    }

    /// Q2 边界：SPA 回退 MIME 与请求**大小写 / query** 无关——`uri.path()` 已剥离 query，
    /// 大小写不同的深链同样「embed 落空 → index.html」回退，仍必须是 HTML。
    #[tokio::test]
    async fn spa_fallback_mime_is_case_and_query_insensitive() {
        let _guard = test_guard();
        isolated_home();

        let (_, _, index_body) = call_full(Method::GET, "/index.html").await;
        for uri in ["/Accounts", "/GATEWAY", "/accounts?tab=keys", "/settings/"] {
            let (status, content_type, body) = call_full(Method::GET, uri).await;
            assert_eq!(status, StatusCode::OK, "{uri} must be 200");
            assert_eq!(
                content_type, "text/html; charset=utf-8",
                "{uri} must be served as HTML regardless of case/query"
            );
            assert_eq!(body, index_body, "{uri} must serve the exact index.html body");
        }
    }

    // -----------------------------------------------------------------------
    // 构建产物一致性护栏：`dist/` 只能装 WebUI 构建
    //
    // 背景（真实事故）：`npm run build`（Vite base `/`，供 rust-embed / Tauri
    // `frontendDist` / `scripts/fix-app.sh` 使用）与 `npm run build:demo`
    // （base `/workbuddy-switch/`，仅供 GitHub Pages）**曾共同输出到 `dist/`**。
    // 若编译期 `dist/` 恰好是演示构建，`index.html` 会请求
    // `/workbuddy-switch/assets/index-*.js`——该前缀在 embed 里不存在 →
    // `static_handler` 回退成 HTML → 浏览器模块脚本 MIME 校验失败：
    //   Failed to load module script: Expected a JavaScript-or-Wasm module script
    //   but the server responded with a MIME type of "text/html".
    // → **webui 与桌面端双双空白页**（实测 `#root` 子节点数为 0）。
    //
    // 该约定已由 `build:demo --outDir dist-demo` 保证，下面的测试是它的护栏：
    // 一旦有人把演示构建塞回 `dist/`，本测试立刻变红，而不是等到用户看到白屏。
    // -----------------------------------------------------------------------

    /// 取出 HTML 中所有**根绝对**资源引用（`src="/…"` / `href="/…"`）。
    ///
    /// 只收根绝对路径：跳过 `//cdn…`（协议相对）、`https://…`、`data:`、`#hash`。
    fn asset_refs_in(html: &str) -> Vec<String> {
        let mut refs: Vec<String> = Vec::new();
        for attr in ["src=\"", "href=\""] {
            let mut rest = html;
            while let Some(found) = rest.find(attr) {
                let tail = &rest[found + attr.len()..];
                let Some(end) = tail.find('"') else { break };
                let value = &tail[..end];
                if value.starts_with('/') && !value.starts_with("//") {
                    refs.push(value.to_string());
                }
                rest = &tail[end..];
            }
        }
        refs.sort();
        refs.dedup();
        refs
    }

    #[test]
    fn asset_refs_in_extracts_only_root_absolute_paths() {
        let html = concat!(
            r#"<link rel="icon" href="/icon.png">"#,
            r#"<script src="/assets/a.js"></script>"#,
            r#"<link href="//cdn.example.com/x.css">"#,
            r#"<img src="https://example.com/y.png">"#,
            "<a href=\"#top\">top</a>",
            r#"<a href="/relative-ok.css">css</a>"#,
        );
        assert_eq!(asset_refs_in(html), vec!["/assets/a.js", "/icon.png", "/relative-ok.css"]);
        assert!(asset_refs_in("<p>no refs</p>").is_empty());
    }

    /// 护栏：内嵌 `index.html` 引用的每个根绝对资源都必须能在 embed 中命中。
    ///
    /// 这条断言同时覆盖三类真实故障：① `dist/` 被演示构建覆盖（base 前缀不匹配）；
    /// ② 前端删了资源但 embed 未重编；③ `dist/` 与 embed 不同步。
    #[test]
    fn embedded_index_html_references_only_embedded_assets() {
        let index = Assets::get("index.html").expect("dist/index.html must be embedded");
        let html = String::from_utf8_lossy(&index.data).to_string();
        let refs = asset_refs_in(&html);
        assert!(
            !refs.is_empty(),
            "dist/index.html must reference at least one root-absolute asset: {html}"
        );
        for reference in &refs {
            let key = reference.trim_start_matches('/');
            assert!(
                Assets::get(key).is_some(),
                "dist/index.html references `{reference}` which is NOT embedded. \
                 `dist/` most likely holds a **demo** build (Vite base `/workbuddy-switch/`). \
                 Run `npm run build` (NOT `npm run build:demo`) before compiling the server / Tauri app. \
                 Embedded paths: {:?}",
                Assets::iter().collect::<Vec<_>>()
            );
        }
    }

    /// 护栏的对照面：**正确**构建下，`index.html` 的脚本预加载同源路径也必须命中。
    /// （`/` 与 `/index.html` 两个入口都要能命中，避免只修一个出口。）
    #[test]
    fn embedded_index_html_is_servable_at_root_and_index() {
        assert!(Assets::get("index.html").is_some(), "dist/index.html must be embedded");
        let files: Vec<String> = Assets::iter().map(|path| path.to_string()).collect();
        assert!(
            files.iter().any(|f| f.starts_with("assets/")),
            "dist must embed at least one assets/* file, got {files:?}"
        );
    }

    /// Q2 独立证明：F2 护栏要求覆盖目录**已存在**，否则 `set_var` 会被静默忽略
    /// 并回落真实 home。这里直接断言生产 `home_dir()` 解析到隔离临时目录——
    /// 若隔离失效则 `home_dir()` 会返回真实 home（不在 temp 下），本断言即变红。
    #[test]
    fn isolated_home_override_is_actually_effective() {
        let _guard = test_guard();
        let home = isolated_home();
        let resolved = buddy_switch_core::modules::config::home_dir();
        assert!(
            resolved.starts_with(std::env::temp_dir()),
            "home_dir() must resolve inside the temp dir, got {resolved:?}"
        );
        assert_eq!(
            resolved, home,
            "BUDDY_SWITCH_HOME override must resolve home_dir() to the isolated temp dir"
        );
    }

    // -----------------------------------------------------------------------
    // 写操作路由（隔离 home 下串行）
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn gateway_key_lifecycle_never_leaks_secrets_over_http() {
        let _guard = test_guard();
        isolated_home();

        // create：一次性明文必须返回（且只在 create 响应里）。
        let (status, body) = call_api(
            Method::POST,
            "/api/gateway/keys",
            Some(json!({"name": "e2-lifecycle", "region": "cn"})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        let plaintext = body["key"]
            .as_str()
            .expect("create must return once-only plaintext")
            .to_string();
        assert!(plaintext.starts_with("sk-wb-"), "plaintext: {plaintext}");
        assert_eq!(plaintext.len(), "sk-wb-".len() + 32);
        let record_id = body["record"]["id"]
            .as_str()
            .expect("create record id")
            .to_string();

        // list：脱敏红线——仅 prefix，绝无 hash / 明文 / 64 位摘要。
        let (status, body) = call_api(Method::GET, "/api/gateway/keys", None).await;
        assert_eq!(status, StatusCode::OK);
        let keys = body["keys"].as_array().expect("keys array");
        assert_eq!(keys.len(), 1);
        assert_eq!(
            exact_keys(&keys[0]),
            set_of(&[
                "id",
                "name",
                "region",
                "prefix",
                "createdAt",
                "revokedAt",
                "revoked",
                "lastUsedAt",
            ]),
            "masked record must be an explicit non-secret whitelist"
        );
        assert!(!keys[0].as_object().unwrap().contains_key("hash"));

        let serialized = serde_json::to_string(&body).unwrap();
        assert!(!serialized.contains("hash"), "list body must not contain a `hash` field");
        assert!(
            !serialized.contains(&plaintext),
            "list body must never echo the plaintext key"
        );
        assert!(
            !serialized.contains(&buddy_switch_gateway::apikey::sha256_hex(&plaintext)),
            "list body must not contain the sha256 digest"
        );
        assert!(
            !contains_hex_run(&serialized, 64),
            "list body must not contain any 64-char hex digest: {serialized}"
        );

        // 未吊销直接删除 → 400 + 可读错误。
        let (status, body) = call_api(
            Method::POST,
            "/api/gateway/keys/delete",
            Some(json!({"id": record_id})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], json!(false));
        assert!(
            body["error"].as_str().unwrap_or("").contains("吊销"),
            "error should instruct to revoke first: {body}"
        );

        // 吊销 → 删除成功 → 列表清空。
        let (status, body) = call_api(
            Method::POST,
            "/api/gateway/keys/revoke",
            Some(json!({"id": record_id})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));

        let (_, body) = call_api(Method::GET, "/api/gateway/keys", None).await;
        assert_eq!(body["keys"][0]["revoked"], json!(true));

        let (status, body) = call_api(
            Method::POST,
            "/api/gateway/keys/delete",
            Some(json!({"id": record_id})),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));

        let (_, body) = call_api(Method::GET, "/api/gateway/keys", None).await;
        assert_eq!(body["keys"], json!([]));
    }

    #[tokio::test]
    async fn gateway_save_config_rejects_invalid_and_persists_disabled() {
        let _guard = test_guard();
        isolated_home();

        // 端口类型错误 → 400（而非 500）。
        let (status, body) = call_api(
            Method::POST,
            "/api/gateway/config",
            Some(json!({"port": "not-a-number"})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], json!(false));
        assert!(
            body["error"].as_str().unwrap_or("").contains("配置格式错误"),
            "error: {body}"
        );

        // 合法但禁用监听：不得真的绑定端口。
        let (status, body) = call_api(
            Method::POST,
            "/api/gateway/config",
            Some(json!({
                "enabled": false,
                "bind_addr": "127.0.0.1",
                "port": 57891,
                "allow_non_loopback": false,
                "log_keep": 50,
                "log_bodies": false
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["running"], json!(false));
        assert_eq!(body["addr"], Value::Null);

        // 配置必须落盘到隔离 home（证明读写未触碰真实用户目录）。
        let config_file = isolated_home().join(".buddy-switch").join("gateway_config.json");
        assert!(config_file.is_file(), "config must persist to the isolated home");
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(&config_file).unwrap()).unwrap();
        assert_eq!(saved["enabled"], json!(false));
        assert_eq!(saved["log_keep"], json!(50));
    }

    #[test]
    fn web_checkin_status_keeps_account_identity() {
        let item = checkin_status_item(
            &json!({"id": "account-1", "email": "user@example.com"}),
            json!({"ok": true, "todayCheckedIn": true}),
        );

        assert_eq!(item["accountId"], "account-1");
        assert_eq!(item["email"], "user@example.com");
        assert_eq!(item["todayCheckedIn"], true);
    }

    #[test]
    fn web_checkin_status_preserves_failure_state() {
        let item = checkin_status_item(
            &json!({"id": "account-2"}),
            json!({"ok": false, "todayCheckedIn": false, "error": "status failed"}),
        );

        assert_eq!(item["accountId"], "account-2");
        assert_eq!(item["ok"], false);
        assert_eq!(item["error"], "status failed");
    }

    /// 排程配置路由：GET 返回默认值；POST 越界小时返回 400 且错误指向 `*_enabled`。
    #[tokio::test]
    async fn schedule_config_route_validates_hours() {
        let _guard = test_guard();
        isolated_home();

        let (status, body) = call_api(Method::GET, "/api/schedule/config", None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["checkin_hours"], json!([9, 21]));
        assert_eq!(body["activity_hours"], json!([10]));
        assert_eq!(body["activity_report_count"], json!(5));
        assert_eq!(body["checkin_enabled"], json!(true));

        let (status, body) = call_api(
            Method::POST,
            "/api/schedule/config",
            Some(json!({"checkin_hours": [25]})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], json!(false));
        assert!(
            body["error"].as_str().unwrap_or("").contains("checkin_enabled"),
            "错误文案必须指向 *_enabled：{body}"
        );
    }

    // -----------------------------------------------------------------------
    // region 隔离（E1，可证伪断言）
    //
    // 目的：证明宿主**真的按请求里的 region 分派**到 core 的 `*_for(region, …)`
    // 变体，而不是忽略 region 恒按 CN。以下用例都是「改坏即变红」的形态：
    //   · 若把 `region` 从 args 里丢掉 / 恒按 CN 处理 → 断言立刻失败。
    // -----------------------------------------------------------------------

    /// 播种某 region 的账号库（`~/.buddy-switch/accounts.json` / `accounts.global.json`）。
    fn seed_accounts(region: Region, accounts: Value) {
        let file = buddy_switch_core::modules::region::accounts_file_for(region);
        std::fs::create_dir_all(file.parent().unwrap()).expect("create store dir");
        std::fs::write(&file, serde_json::to_string(&accounts).unwrap()).expect("seed accounts");
    }

    /// 清理某 region 的账号库，避免种子泄漏到其它用例。
    fn clear_accounts(region: Region) {
        let file = buddy_switch_core::modules::region::accounts_file_for(region);
        let _ = std::fs::remove_file(&file);
    }

    fn account_ids(body: &Value) -> Vec<String> {
        body["accounts"]
            .as_array()
            .expect("accounts array")
            .iter()
            .filter_map(|a| a["id"].as_str().map(String::from))
            .collect()
    }

    /// 两套独立账号库各自只对本 region 可见：`?region=cn` 只看得到 CN，
    /// `?region=global` 只看得到 Global；缺省 region 仍等价于 cn。
    #[tokio::test]
    async fn accounts_route_is_region_isolated() {
        let _guard = test_guard();
        isolated_home();

        seed_accounts(
            Region::Cn,
            json!([{"id": "cn-only", "uid": "uid-cn", "email": "cn@example.com"}]),
        );
        seed_accounts(
            Region::Global,
            json!([{"id": "global-only", "uid": "uid-global", "email": "global@example.com"}]),
        );

        let (status_cn, body_cn) = call_api(Method::GET, "/api/accounts?region=cn", None).await;
        let (status_global, body_global) =
            call_api(Method::GET, "/api/accounts?region=global", None).await;
        let (status_default, body_default) = call_api(Method::GET, "/api/accounts", None).await;

        // 先清理，再断言：即便断言失败也不会把种子留给后续用例。
        clear_accounts(Region::Cn);
        clear_accounts(Region::Global);

        assert_eq!(status_cn, StatusCode::OK);
        assert_eq!(status_global, StatusCode::OK);
        assert_eq!(status_default, StatusCode::OK);

        assert_eq!(body_cn["region"], json!("cn"));
        assert_eq!(
            account_ids(&body_cn),
            vec!["cn-only"],
            "region=cn 只能看到 CN 账号库：{body_cn}"
        );

        assert_eq!(body_global["region"], json!("global"));
        assert_eq!(
            account_ids(&body_global),
            vec!["global-only"],
            "region=global 只能看到 Global 账号库：{body_global}"
        );

        // 旧行为不破：缺省 region 等价于 cn。
        assert_eq!(body_default["region"], json!("cn"));
        assert_eq!(
            account_ids(&body_default),
            vec!["cn-only"],
            "缺省 region 必须等价于 cn：{body_default}"
        );
    }

    /// 对一个**只存在于 Global 库**的账号执行删除：`{region:"cn"}` 必须失败，
    /// `{region:"global"}` 必须成功。
    ///
    /// 这是「宿主真的按 region 分派」的可证伪断言——若宿主忽略 region、恒按 CN 删除，
    /// 则成功分支会因 CN 库里没有该 id 而 400，本用例必然变红。
    #[tokio::test]
    async fn delete_route_dispatches_by_region() {
        let _guard = test_guard();
        isolated_home();

        seed_accounts(Region::Cn, json!([]));
        seed_accounts(
            Region::Global,
            json!([{"id": "global-victim", "uid": "uid-g", "email": "g@example.com"}]),
        );

        // region=cn：该账号在 CN 库不存在 → 400，且不得影响 Global 库。
        let (status, body) = call_api(
            Method::POST,
            "/api/delete",
            Some(json!({"accountId": "global-victim", "region": "cn"})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "CN 库不得删除 Global 账号：{body}");
        assert_eq!(body["ok"], json!(false));

        let (_, still_there) = call_api(Method::GET, "/api/accounts?region=global", None).await;
        assert_eq!(
            account_ids(&still_there),
            vec!["global-victim"],
            "失败的删除不得改动 Global 库：{still_there}"
        );

        // region=global：命中 → 200 + ok。
        let (status, body) = call_api(
            Method::POST,
            "/api/delete",
            Some(json!({"accountId": "global-victim", "region": "global"})),
        )
        .await;
        let (_, emptied) = call_api(Method::GET, "/api/accounts?region=global", None).await;

        clear_accounts(Region::Cn);
        clear_accounts(Region::Global);

        assert_eq!(status, StatusCode::OK, "Global 必须能删除 Global 账号：{body}");
        assert_eq!(body["ok"], json!(true));
        assert!(
            account_ids(&emptied).is_empty(),
            "删除后 Global 库应为空：{emptied}"
        );
    }
}
