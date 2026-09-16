//! Tauri commands：前端调用的薄包装，对应 Python 版 HTTP API。
//!
//! 阶段 1 覆盖：get_status / get_accounts / delete_account / oauth_start /
//! oauth_status / import_local。

use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Mutex;

use tauri::{Emitter, Manager};
use buddy_switch_core::modules::{
    account, auth_file, checkin, codebuddy_cli, codebuddy_cn_ide, credit_usage, credits, export_import, migrate,
    oauth, process, refresh, region::Region, region::RegionFilter, rotate, session, switch, token_stats, travel,
    update,
};
use buddy_switch_gateway::{AccountStrategy, GatewayConfig, GatewayStatusView};

use crate::gateway;

/// 解析 region 参数，缺省为 `cn`（保证旧行为）。
fn parse_region(value: Option<&str>) -> Region {
    value.and_then(Region::parse).unwrap_or(Region::Cn)
}

/// 解析统计查询范围参数，缺省为 `cn`（保证旧行为）；额外支持 `"all"` 合并视图。
///
/// 复用 core 的 `parse_region_filter`，绑定规则与既有 [`parse_region`] 一致，
/// 并额外接受 `"all" | "*" | "合并" | "全部"`。**仅**用于两个统计命令。
fn parse_region_filter(value: Option<&str>) -> RegionFilter {
    buddy_switch_core::modules::region::parse_region_filter(value)
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

/// 该 region 客户端是否已安装（跨平台：解析出的应用路径真实存在）。
fn region_installed(region: Region) -> bool {
    auth_file::workbuddy_app_path_for(region).exists()
}

/// 切换进度缓存：桌面端也可通过 `switch_progress` 轮询（与 webui 的
/// `/api/switch/progress` 同契约）。事件 `switch-progress` 仍照常派发。
static SWITCH_PROGRESS: Mutex<Option<String>> = Mutex::new(None);
static SWITCH_RUNNING: Mutex<bool> = Mutex::new(false);

/// 应用状态（序列化为 **camelCase**，与 server `/api/status` 契约一致）。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    running: bool,
    region: String,
    auth_file: String,
    current: Option<Value>,
    app_path: String,
    version: String,
    /// 该 region 客户端是否已安装（前端据此判定 Tab 空态）。
    installed: bool,
    /// 认证文件 domain 与目标 region 不符时返回（安全红线 F），否则为 null。
    region_mismatch: Option<Value>,
}

/// GET /api/status —— WorkBuddy 运行状态 + 当前账号（可按 region）。
#[tauri::command]
pub async fn get_status(region: Option<String>) -> Result<AppStatus, String> {
    // Windows 的运行状态检测会启动 tasklist 子进程。同步 command 默认在
    // Tauri 主线程执行，标题栏拖拽期间一旦焦点事件触发状态刷新，就会阻塞
    // 原生窗口消息循环。放入 blocking 线程，保持窗口移动与 IPC 查询解耦。
    let region = parse_region(region.as_deref());
    tauri::async_runtime::spawn_blocking(move || build_app_status(region))
        .await
        .map_err(|error| format!("查询应用状态失败: {error}"))
}

fn build_app_status(region: Region) -> AppStatus {
    // 安全红线 F：读取后校验凭据域归属；不匹配即拒绝使用（current 置空）。
    let (auth, region_mismatch) = match auth_file::read_auth_file_checked_for(region) {
        Ok(auth) => (auth, None),
        Err(mismatch) => (None, Some(mismatch_json(&mismatch))),
    };
    let current = auth.as_ref().and_then(|a| {
        let acct = a.get("account").cloned().unwrap_or_else(|| json!({}));
        Some(json!({
            "uid": acct.get("uid"),
            "nickname": acct.get("nickname"),
            "email": acct.get("email"),
        }))
    });
    AppStatus {
        running: process::is_workbuddy_running_for(region),
        region: region.as_str().to_string(),
        auth_file: auth_file::auth_file_path_for(region)
            .to_string_lossy()
            .to_string(),
        current,
        app_path: auth_file::workbuddy_app_path_for(region)
            .to_string_lossy()
            .to_string(),
        version: update::APP_VERSION.to_string(),
        installed: region_installed(region),
        region_mismatch,
    }
}

/// GET /api/accounts —— 账号列表（account_meta，不含 token；可按 region）。
#[tauri::command]
pub fn get_accounts(region: Option<String>) -> Value {
    let region = parse_region(region.as_deref());
    let metas: Vec<Value> = account::load_accounts_for(region)
        .iter()
        .map(account::account_meta)
        .collect();
    json!({ "region": region, "accounts": metas })
}

/// GET /api/codebuddy-cli/status —— CodeBuddy CLI helper 轮换状态（不含 token）。
///
/// async + spawn_blocking：状态检测可能执行 ps / helper 定位等子进程，
/// 避免在账号页挂载刷新时阻塞主线程造成页面卡顿。
#[tauri::command]
pub async fn get_codebuddy_cli_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(codebuddy_cli::status)
        .await
        .map_err(|error| format!("查询 CodeBuddy CLI 状态失败: {error}"))
}

/// POST /api/codebuddy-cli/install-helper —— 显式安装/升级 CLI helper。
#[tauri::command]
pub async fn install_codebuddy_cli_helper() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(codebuddy_cli::install_helper)
        .await
        .map_err(|e| e.to_string())?
}

/// POST /api/codebuddy-cli/switch —— 只切换 CodeBuddy CLI，不重启 WorkBuddy。
///
/// async + spawn_blocking：切换会用登录 shell 定位 node 并执行 apiKeyHelper
/// 校验账号（子进程无超时），同步 command 会阻塞主线程造成 UI 卡顿。
#[tauri::command(rename_all = "camelCase")]
pub async fn switch_codebuddy_cli_account(account_id: String) -> Result<Value, String> {
    if account_id.trim().is_empty() {
        return Err("缺少 accountId".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || codebuddy_cli::set_active_account(&account_id))
        .await
        .map_err(|e| e.to_string())?
}

/// GET /api/codebuddy-cn-ide/status —— CodeBuddy IDE 安装/运行/当前账号。
///
/// async + spawn_blocking：状态检测会跑 ps / mdfind 等子进程（mdfind 可能
/// 耗时数秒），账号页每次挂载都会刷新，若在主线程执行会造成页面卡顿。
#[tauri::command]
pub async fn get_codebuddy_cn_ide_status() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(codebuddy_cn_ide::status)
        .await
        .map_err(|error| format!("查询 CodeBuddy IDE 状态失败: {error}"))
}

/// POST /api/codebuddy-cn-ide/switch —— 注入凭证并可选重启 CodeBuddy CN IDE。
///
/// async + spawn_blocking：切换会关闭并重启 CodeBuddy CN，可能阻塞数十秒，
/// 与 WorkBuddy 切换同理，若在同步 command（主线程）执行会卡死整个 UI。
#[tauri::command(rename_all = "camelCase")]
pub async fn switch_codebuddy_cn_ide_account(
    account_id: String,
    restart: Option<bool>,
) -> Result<Value, String> {
    if account_id.trim().is_empty() {
        return Err("缺少 accountId".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        codebuddy_cn_ide::switch_account(&account_id, restart.unwrap_or(true))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// POST /api/codebuddy-cn-ide/detect —— 读取本机 CN IDE 当前登录并尝试匹配账号库。
///
/// async + spawn_blocking：会通过 Keychain/secret 读取子进程，避免阻塞主线程。
#[tauri::command]
pub async fn detect_codebuddy_cn_ide_account() -> Result<Value, String> {
    tauri::async_runtime::spawn_blocking(codebuddy_cn_ide::detect_current_account)
        .await
        .map_err(|e| e.to_string())?
}


/// POST /api/delete —— 删除账号（按 region）。
#[tauri::command(rename_all = "camelCase")]
pub fn delete_account(account_id: String, region: Option<String>) -> Result<Value, String> {
    let region = parse_region(region.as_deref());
    account::delete_account_for(region, &account_id)?;
    Ok(json!({ "ok": true }))
}

/// POST /api/oauth/start —— 发起 OAuth 扫码登录（按 region）。
#[tauri::command]
pub async fn oauth_start(region: Option<String>) -> Result<Value, String> {
    let region = parse_region(region.as_deref());
    oauth::oauth_start_for(region).await
}

/// GET /api/oauth/status —— 轮询采集结果（按 region）。
#[tauri::command(rename_all = "camelCase")]
pub async fn oauth_status(login_id: String, region: Option<String>) -> Value {
    let region = parse_region(region.as_deref());
    oauth::oauth_poll_for(region, &login_id).await
}

/// POST /api/import-local —— 导入本机当前账号（按 region）。
#[tauri::command]
pub fn import_local(region: Option<String>) -> Result<Value, String> {
    let region = parse_region(region.as_deref());
    account::import_local_for(region).map(|acc| json!({ "ok": true, "account": acc }))
}

// ---------------------------------------------------------------------------
// 导出 / 导入账号
// ---------------------------------------------------------------------------

/// POST /api/export-accounts —— 按账号 id 列表导出完整记录（含 token，按 region）。
#[tauri::command(rename_all = "camelCase")]
pub fn export_accounts(account_ids: Vec<String>, region: Option<String>) -> Result<Value, String> {
    let region = parse_region(region.as_deref());
    export_import::export_accounts_for(region, &account_ids)
        .map(|records| json!({ "ok": true, "accounts": records }))
}

/// POST /api/export-accounts-to-path —— 把勾选账号的完整记录写入用户选择的路径（保存对话框产物，按 region）。
#[tauri::command(rename_all = "camelCase")]
pub fn export_accounts_to_path(
    account_ids: Vec<String>,
    path: String,
    region: Option<String>,
) -> Result<Value, String> {
    let region = parse_region(region.as_deref());
    export_import::export_accounts_to_path_for(region, &account_ids, &path)
        .map(|path| json!({ "ok": true, "path": path }))
}

/// POST /api/import/preview —— 解析导入文件并返回脱敏预览（含文件内索引）。
///
/// **无需 region**：预览是纯函数（只解析请求里的文件文本，不触及任何 region 账号库），
/// 因此显式忽略前端可能一并带来的 `region` 参数。
#[tauri::command(rename_all = "camelCase")]
pub fn preview_import_accounts(file_text: String, _region: Option<String>) -> Result<Value, String> {
    export_import::preview_accounts(&file_text)
}

/// POST /api/import —— 按选中索引把账号导入该 region 账号库，返回导入/跳过/覆盖计数。
#[tauri::command(rename_all = "camelCase")]
pub fn import_accounts(
    file_text: String,
    indexes: Vec<usize>,
    region: Option<String>,
) -> Result<Value, String> {
    let region = parse_region(region.as_deref());
    let result = export_import::import_accounts_for(region, &file_text, &indexes)?;
    Ok(json!({
        "ok": true,
        "imported": result.imported,
        "skipped": result.skipped,
        "overwritten": result.overwritten,
    }))
}

/// 打开系统设置授权面板。默认「完全磁盘访问」（该 anchor 各版本均有效）；
/// 传 `target="app_management"` 尝试「App 管理」（macOS 15+，部分版本不支持深链）。
///
/// 使用 macOS 13+ 深链接格式（`com.apple.settings.PrivacySecurity.extension?Privacy_*`）。
#[tauri::command]
pub fn open_permission_settings(target: Option<String>) -> Result<(), String> {
    let t = target.unwrap_or_else(|| "all_files".to_string());
    let url = match t.as_str() {
        "app_management" => {
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AppManagement"
        }
        _ => {
            "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_AllFiles"
        }
    };
    let _ = std::process::Command::new("open").arg(url).spawn();
    Ok(())
}

/// 权限自检：尝试在认证文件目录写/删探针文件，确认完全磁盘访问等授权是否生效。
#[tauri::command]
pub fn check_auth_permission() -> Value {
    let path = auth_file::auth_file_path();
    let probe = path.with_file_name("workbuddy-desktop.info.probe");
    match std::fs::write(&probe, "probe") {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            json!({ "ok": true, "message": "认证目录可写，权限正常" })
        }
        Err(e) => json!({
            "ok": false,
            "error": e.to_string(),
            "dir": path.parent().map(|p| p.to_string_lossy().to_string()),
            "hint": "请在 系统设置→隐私与安全性 中授权：优先「App 管理」开启 buddy-switch，若没有则去「完全磁盘访问」把 buddy-switch 拖进去；授权后需重启 App 生效",
        }),
    }
}

/// 在 Finder 中显示当前 App（便于拖拽到「完全磁盘访问」授权框）。
#[tauri::command]
pub fn reveal_app_in_finder() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(&exe)
        .spawn();
    Ok(())
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
/// 因此这里统一打开账号文件所在目录；目录不存在时先创建，避免「打开失败」。
#[tauri::command]
pub fn open_accounts_dir(region: Option<String>) -> Result<Value, String> {
    let region = parse_region(region.as_deref());
    let file = buddy_switch_core::modules::region::accounts_file_for(region);
    let dir = file
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| file.clone());
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(|error| format!("创建账号库目录失败: {error}"))?;
    }
    reveal_dir(&dir)?;
    Ok(json!({ "ok": true, "region": region.as_str(), "dir": dir.to_string_lossy() }))
}

/// POST /api/switch —— 切换账号（备份 → 关进程 → 复制会话 → 写认证 → 重启，按 region）。
///
/// async + spawn_blocking：切换中关闭/启动 WorkBuddy 会阻塞数十秒，
/// 若在同步 command（主线程）执行会卡死整个 UI（loading 遮罩无法渲染）。
#[tauri::command(rename_all = "camelCase")]
pub async fn switch_account(
    app: tauri::AppHandle,
    account_id: String,
    region: Option<String>,
    restart: Option<bool>,
    share_sessions: Option<bool>,
    copy_session_ids: Option<Vec<String>>,
) -> Result<Value, String> {
    if account_id.trim().is_empty() {
        return Err("缺少 accountId".to_string());
    }
    let region = parse_region(region.as_deref());
    let restart = restart.unwrap_or(true);
    let share_sessions = share_sessions.unwrap_or(false);
    let copy_ids = copy_session_ids.unwrap_or_default();

    // 与 webui 的 `/api/switch/progress` 同契约：同步写入进程内进度缓存，
    // 供 `switch_progress` 轮询；事件 `switch-progress` 仍照常派发。
    {
        let mut running = SWITCH_RUNNING.lock().unwrap();
        if *running {
            return Err("已有切换任务进行中".to_string());
        }
        *running = true;
        *SWITCH_PROGRESS.lock().unwrap() = Some("开始切换账号…".to_string());
    }

    let progress: switch::ProgressFn = Box::new(move |message| {
        *SWITCH_PROGRESS.lock().unwrap() = Some(message.to_string());
        let _ = app.emit("switch-progress", json!({ "message": message }));
    });
    let result = tauri::async_runtime::spawn_blocking(move || {
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

    result.map_err(|e| e.to_string())?
}

/// GET /api/switch/progress —— 轮询切换进度（与 webui 同契约）。
#[tauri::command]
pub fn switch_progress() -> Value {
    let progress = SWITCH_PROGRESS.lock().unwrap().clone();
    let running = *SWITCH_RUNNING.lock().unwrap();
    json!({ "running": running, "progress": progress })
}

/// GET /api/sessions —— 当前账号的会话列表（按 region）。
#[tauri::command]
pub fn list_sessions(region: Option<String>) -> Value {
    let region = parse_region(region.as_deref());
    match session::current_user_uid_for(region) {
        Some(uid) => json!({
            "sessions": session::list_sessions_for_user_for(region, &uid),
            "current": uid,
        }),
        None => json!({"sessions": [], "current": Value::Null}),
    }
}

/// POST /api/sessions/copy —— 把勾选会话复制到指定账号（路径 B，按 region）。
#[tauri::command(rename_all = "camelCase")]
pub async fn copy_sessions(
    target_account_id: String,
    session_ids: Vec<String>,
    region: Option<String>,
) -> Result<Value, String> {
    if target_account_id.trim().is_empty() {
        return Err("缺少 targetAccountId".to_string());
    }
    if session_ids.is_empty() {
        return Err("缺少 sessionIds".to_string());
    }
    let region = parse_region(region.as_deref());
    tauri::async_runtime::spawn_blocking(move || {
        let target =
            account::find_account_for(region, &target_account_id).ok_or("目标账号不存在")?;
        Ok(session::copy_sessions_for_switch_for(region, &target, &session_ids)
            .unwrap_or_else(|| json!({})))
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------------------
// 账号数据迁移（Memory / Connector 合并去重）
// ---------------------------------------------------------------------------

/// POST /api/migrate/account —— 把源账号的 Memory / Connector 合并到目标账号（带去重）。
///
/// 只处理普通文件，不触碰 `workbuddy.db`，因此无需关闭 WorkBuddy。
#[tauri::command(rename_all = "camelCase")]
pub async fn migrate_account_data(
    target_account_id: String,
    source_account_id: Option<String>,
    memory: Option<bool>,
    connectors: Option<bool>,
    region: Option<String>,
) -> Result<Value, String> {
    if target_account_id.trim().is_empty() {
        return Err("缺少 targetAccountId".to_string());
    }
    let region = parse_region(region.as_deref());
    let source_account_id = source_account_id.unwrap_or_default().trim().to_string();

    tauri::async_runtime::spawn_blocking(move || {
        let scope = migrate::MigrateScope {
            memory: memory.unwrap_or(true),
            connectors: connectors.unwrap_or(true),
        };
        if scope.is_empty() {
            return Err("未指定任何迁移范围".to_string());
        }

        let target = account::find_account_for(region, &target_account_id)
            .ok_or("目标账号不存在")?;
        let target_uid = target
            .get("uid")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if target_uid.is_empty() {
            return Err("目标账号缺少 uid".to_string());
        }

        let source_uid = if source_account_id.is_empty() {
            session::current_user_uid_for(region).unwrap_or_default()
        } else {
            let acc = account::find_account_for(region, &source_account_id)
                .ok_or("源账号不存在")?;
            acc.get("uid")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        if source_uid.is_empty() {
            return Err("无法确定源账号 uid（未登录或账号缺少 uid）".to_string());
        }

        migrate::migrate_account_data_for(region, &source_uid, &target_uid, scope)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------------------
// 阶段 3：签到 + token 刷新
// ---------------------------------------------------------------------------

/// GET /api/checkin/status —— 查询单账号签到状态（按 region）。
#[tauri::command(rename_all = "camelCase")]
pub async fn get_checkin_status(account_id: String, region: Option<String>) -> Result<Value, String> {
    let region = parse_region(region.as_deref());
    let acc = account::find_account_for(region, &account_id).ok_or("账号不存在")?;
    Ok(checkin::get_checkin_status_for(region, &acc).await)
}

/// POST /api/credits —— 查询单账号积分资源及到期时间（按 region）。
#[tauri::command(rename_all = "camelCase")]
pub async fn get_credit_expiry(account_id: String, region: Option<String>) -> Result<Value, String> {
    let region = parse_region(region.as_deref());
    let acc = account::find_account_for(region, &account_id).ok_or("账号不存在")?;
    Ok(credits::get_credit_expiry_for(region, &acc).await)
}

/// GET /api/credits/stats —— 本地快照与官方请求用量统计（按 region）。
/// `refresh = true` 时才重新请求官方用量；默认读缓存。
#[tauri::command]
pub async fn get_credit_statistics(refresh: Option<bool>, region: Option<String>) -> Value {
    let filter = parse_region_filter(region.as_deref());
    credit_usage::get_statistics_for_filter(filter, refresh.unwrap_or(false)).await
}

#[tauri::command]
pub async fn get_token_statistics(days: Option<i64>, region: Option<String>) -> Result<Value, String> {
    let filter = parse_region_filter(region.as_deref());
    tauri::async_runtime::spawn_blocking(move || token_stats::get_statistics_for_filter(filter, days))
        .await
        .map_err(|error| format!("扫描 Token 统计失败: {error}"))
}

/// POST /api/checkin —— 单账号立即签到（按 region）。
#[tauri::command(rename_all = "camelCase")]
pub async fn checkin(account_id: String, region: Option<String>) -> Result<Value, String> {
    let region = parse_region(region.as_deref());
    let acc = account::find_account_for(region, &account_id).ok_or("账号不存在")?;
    Ok(checkin::checkin_account_for(region, &acc).await)
}

/// POST /api/checkin/all —— 全部账号立即签到（按 region）。
#[tauri::command]
pub async fn checkin_all(region: Option<String>) -> Value {
    let region = parse_region(region.as_deref());
    checkin::run_checkin_all_for(region).await
}

/// GET /api/checkin/config —— 自动签到配置。
#[tauri::command]
pub fn get_auto_checkin_config() -> Value {
    crate::modules::config::load_checkin_config()
}

/// POST /api/checkin/config —— 保存自动签到配置。
#[tauri::command]
pub fn save_auto_checkin_config(config: Value) -> Result<Value, String> {
    crate::modules::config::save_checkin_config(&config).map_err(|e| e.to_string())?;
    Ok(crate::modules::config::load_checkin_config())
}

/// GET /api/checkin/logs —— 签到日志。
#[tauri::command]
pub fn get_checkin_logs() -> Value {
    json!({ "logs": crate::modules::config::load_checkin_logs() })
}

// ---------------------------------------------------------------------------
// 派猫猫旅行
// ---------------------------------------------------------------------------

/// GET /api/travel/status —— 查询单账号今日旅行状态标签。
#[tauri::command]
pub async fn get_travel_status(account_id: String) -> Result<Value, String> {
    account::find_account(&account_id).ok_or("账号不存在")?;
    travel::reconcile_due_travel(Some(account_id.as_str())).await;
    Ok(travel::travel_display(&account_id))
}

/// GET /api/travel/config —— 自动旅行配置。
#[tauri::command]
pub fn get_auto_travel_config() -> Value {
    crate::modules::config::load_travel_config()
}

/// POST /api/travel/config —— 保存自动旅行配置。开启时立刻跑一轮派发/领取。
#[tauri::command]
pub fn save_auto_travel_config(config: Value) -> Result<Value, String> {
    crate::modules::config::save_travel_config(&config).map_err(|e| e.to_string())?;
    let saved = crate::modules::config::load_travel_config();
    if saved.get("enabled").and_then(Value::as_bool) == Some(true) {
        tauri::async_runtime::spawn(async {
            let _ = travel::run_travel_cycle().await;
            let _ = travel::run_travel_claim_cycle().await;
        });
    }
    Ok(saved)
}

// ---------------------------------------------------------------------------
// 定时任务排程（六类积分任务，全局单份，无需 region）
// ---------------------------------------------------------------------------

/// GET /api/schedule/config —— 六类定时任务的排程配置。
#[tauri::command]
pub fn get_schedule_config() -> Value {
    crate::modules::schedule::schedule_to_value(&crate::modules::schedule::load_schedule_config())
}

/// POST /api/schedule/config —— 保存排程配置；小时越界返回 Err（文案指向对应 `*_enabled`）。
#[tauri::command]
pub fn save_schedule_config(config: Value) -> Result<Value, String> {
    let cfg = crate::modules::schedule::save_schedule_config(&config)?;
    Ok(crate::modules::schedule::schedule_to_value(&cfg))
}

// ---------------------------------------------------------------------------
// 自动轮换（CodeBuddy CLI）
// ---------------------------------------------------------------------------

/// GET /api/rotate/config —— 自动轮换配置。
#[tauri::command]
pub fn get_auto_rotate_config() -> Value {
    crate::modules::config::load_auto_rotate_config()
}

/// POST /api/rotate/config —— 保存自动轮换配置。
#[tauri::command]
pub fn save_auto_rotate_config(config: Value) -> Result<Value, String> {
    crate::modules::config::save_auto_rotate_config(&config).map_err(|e| e.to_string())?;
    Ok(crate::modules::config::load_auto_rotate_config())
}

/// GET /api/rotate/status —— 轮换状态（配置 + 上次检查/切换）。
#[tauri::command]
pub fn rotate_status() -> Value {
    rotate::rotate_status()
}

/// POST /api/rotate/run —— 手动触发一次轮换检查。
#[tauri::command]
pub async fn run_rotate() -> Value {
    rotate::run_rotate_cycle().await
}

/// GET /api/rotate/logs —— 最近轮换日志。
#[tauri::command]
pub fn get_rotate_logs() -> Value {
    json!({ "logs": rotate::rotate_logs() })
}

/// POST /api/refresh-token —— 单账号刷新 token。
#[tauri::command]
pub async fn refresh_account_token(
    account_id: String,
    region: Option<String>,
) -> Result<Value, String> {
    let region = parse_region(region.as_deref());
    let acc = account::find_account_for(region, &account_id).ok_or("账号不存在")?;
    let fresh = refresh::refresh_account_token_for(region, acc).await;
    Ok(account::account_meta(&fresh))
}

// ---------------------------------------------------------------------------
// 阶段 4：自动更新
// ---------------------------------------------------------------------------

/// GET /api/update/config —— 更新源配置（owner/repo/token）。
#[tauri::command]
pub fn get_github_config() -> Value {
    update::load_github_config()
}

/// POST /api/update/config —— 保存更新源配置。
#[tauri::command]
pub fn save_github_config(config: Value) -> Result<Value, String> {
    update::save_github_config(&config).map_err(|e| e.to_string())?;
    Ok(update::load_github_config())
}

/// GET /api/update/check —— 检查 GitHub Releases 是否有新版本。
/// force=true 时绕过缓存强制刷新（设置页手动检查）。
#[tauri::command]
pub async fn check_update(proxy: Option<String>, force: Option<bool>) -> Value {
    update::update_check(proxy.as_deref(), force.unwrap_or(false)).await
}

/// 启动当前应用的新进程并退出旧进程，用于更新安装完成后的立即重启。
#[tauri::command]
pub fn relaunch_app() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|e| format!("无法定位应用程序: {e}"))?;
    // 更新重启是普通启动路径；不要把系统自启专用参数带给新进程。
    let args = std::env::args_os().skip(1).filter(|arg| {
        #[cfg(desktop)]
        {
            should_forward_relaunch_arg(arg.as_os_str())
        }
        #[cfg(not(desktop))]
        {
            true
        }
    });
    std::process::Command::new(executable)
        .args(args)
        .spawn()
        .map_err(|e| format!("启动应用失败: {e}"))?;
    std::process::exit(0);
}

// ---------------------------------------------------------------------------
// 开机自启（仅桌面端；webui 不提供同名接口）
// ---------------------------------------------------------------------------

/// GET /api/launch-at-login —— 查询系统当前的开机自启注册状态。
///
/// 以 tauri-plugin-autostart 的 OS 状态为唯一事实来源，不另存本地布尔值。
#[tauri::command]
pub fn get_launch_at_login_enabled(_app: tauri::AppHandle) -> Result<bool, String> {
    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        return _app
            .autolaunch()
            .is_enabled()
            .map_err(|e| format!("查询开机自启状态失败：{e}"));
    }
    #[cfg(not(desktop))]
    {
        Err("当前平台不支持开机自启".to_string())
    }
}

#[cfg(desktop)]
fn should_forward_relaunch_arg(arg: &std::ffi::OsStr) -> bool {
    arg != std::ffi::OsStr::new(crate::tray::SILENT_STARTUP_ARG)
}

#[cfg(all(test, desktop))]
mod relaunch_tests {
    use super::should_forward_relaunch_arg;
    use std::ffi::OsStr;

    #[test]
    fn update_relaunch_drops_only_the_exact_silent_startup_arg() {
        assert!(!should_forward_relaunch_arg(OsStr::new("--hidden")));
        assert!(should_forward_relaunch_arg(OsStr::new("--hidden-x")));
        assert!(should_forward_relaunch_arg(OsStr::new("x--hidden")));
        assert!(should_forward_relaunch_arg(OsStr::new("--debug")));
    }
}

/// POST /api/launch-at-login —— 注册 / 移除系统开机自启，并回读权威状态。
///
/// 回读结果与请求值不一致时按失败处理并返回当前真实状态，避免假装设置成功。
#[tauri::command]
pub fn set_launch_at_login_enabled(_app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        let autostart = _app.autolaunch();
        let action = if enabled { "开启" } else { "关闭" };
        let result = if enabled {
            autostart.enable()
        } else {
            autostart.disable()
        };
        if let Err(e) = result {
            return Err(format!("{action}开机自启失败：{e}"));
        }
        let authoritative = autostart
            .is_enabled()
            .map_err(|e| format!("开机自启设置后回读状态失败：{e}"))?;
        if authoritative != enabled {
            return Err(format!(
                "{action}开机自启未生效（系统当前状态：{}），请稍后重试",
                if authoritative {
                    "已开启"
                } else {
                    "未开启"
                }
            ));
        }
        Ok(authoritative)
    }
    #[cfg(not(desktop))]
    {
        let _ = enabled;
        Err("当前平台不支持开机自启".to_string())
    }
}

// ---------------------------------------------------------------------------
// API 网关（管理面）
// ---------------------------------------------------------------------------

/// GET /api/gateway/config —— 网关配置。
#[tauri::command]
pub fn get_gateway_config() -> Value {
    serde_json::to_value(GatewayConfig::load()).unwrap_or(Value::Null)
}

/// POST /api/gateway/config —— 保存配置并应用（启动/重启独立监听）。
#[tauri::command]
pub async fn save_gateway_config(app: tauri::AppHandle, config: Value) -> Result<Value, String> {
    let submitted = config.get("config").cloned().unwrap_or(config);
    let parsed: GatewayConfig =
        serde_json::from_value(submitted).map_err(|error| format!("配置格式错误: {error}"))?;
    parsed.save()?;

    let state = gateway::shared_state();
    *state.config.write().await = parsed.clone();
    state.log.set_keep(parsed.log_keep);
    state.log.set_log_bodies(parsed.log_bodies);

    let runtime = app.state::<gateway::GatewayRuntime>();
    let addr = runtime.apply().await?;
    Ok(json!({
        "ok": true,
        "config": parsed,
        "running": addr.is_some(),
        "addr": addr,
    }))
}

/// GET /api/gateway/status —— 运行状态。
///
/// **E3.1**：统一复用 [`GatewayStatusView`]（**snake_case**，与 server 的
/// `/api/gateway/status` 契约一致），不再手工拼 camelCase。
#[tauri::command]
pub fn gateway_status(app: tauri::AppHandle) -> Value {
    let config = GatewayConfig::load();
    let runtime = app.state::<gateway::GatewayRuntime>();
    let mut view = GatewayStatusView::from(&config);
    view.running = runtime.is_running();
    view.addr = runtime.addr();
    view.version = update::APP_VERSION.to_string();
    serde_json::to_value(view).unwrap_or(Value::Null)
}

/// GET /api/gateway/keys —— Key 列表（脱敏）。
#[tauri::command]
pub fn list_api_keys() -> Value {
    let state = gateway::shared_state();
    let keys: Vec<Value> = state.keys.list().iter().map(|record| record.masked()).collect();
    json!({ "keys": keys })
}

/// POST /api/gateway/keys —— 创建 Key（返回一次性明文）。
#[tauri::command]
pub fn create_api_key(name: Option<String>, region: Option<String>) -> Value {
    let state = gateway::shared_state();
    let region = parse_region(region.as_deref());
    let name = name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "未命名".to_string());
    let (record, plaintext) = state.keys.create(name, region);
    json!({ "ok": true, "key": plaintext, "record": record.masked() })
}

/// POST /api/gateway/keys/revoke —— 吊销 Key。
#[tauri::command]
pub fn revoke_api_key(id: String) -> Result<Value, String> {
    let state = gateway::shared_state();
    state.keys.revoke(&id)?;
    Ok(json!({ "ok": true }))
}

/// POST /api/gateway/keys/delete —— 删除已吊销 Key。
#[tauri::command]
pub fn delete_api_key(id: String) -> Result<Value, String> {
    let state = gateway::shared_state();
    state.keys.delete(&id)?;
    Ok(json!({ "ok": true }))
}

/// GET /api/gateway/models —— 模型列表 + 来源（按 region）。
#[tauri::command]
pub fn get_gateway_models(region: Option<String>) -> Value {
    let region = parse_region(region.as_deref());
    let state = gateway::shared_state();
    let snapshot = state.catalogs.current(region);
    serde_json::to_value(snapshot).unwrap_or(Value::Null)
}

/// POST /api/gateway/models/refresh —— 手动刷新目录（按 region）。
#[tauri::command]
pub async fn refresh_gateway_models(region: Option<String>) -> Result<Value, String> {
    let region = parse_region(region.as_deref());
    let state = gateway::shared_state();
    let strategy = state.strategy_for(region).await;
    let account = buddy_switch_gateway::AccountSelector
        .select(region, &strategy)
        .await
        .map_err(|error| error.message())?;
    let snapshot = state
        .catalogs
        .refresh(region, &state.upstream, &account)
        .await;
    Ok(serde_json::to_value(snapshot).unwrap_or(Value::Null))
}

/// GET /api/gateway/strategy —— 各 region 账号策略。
#[tauri::command]
pub async fn get_account_strategy() -> Value {
    let state = gateway::shared_state();
    // 先取快照，避免跨 await 持有读锁。
    let strategies = state.strategies.read().await.clone();
    let cn = strategies.get(&Region::Cn).cloned().unwrap_or_default();
    let global = strategies.get(&Region::Global).cloned().unwrap_or_default();
    json!({
        "cn": buddy_switch_gateway::account_strategy::describe_strategy(Region::Cn, &cn).await,
        "global": buddy_switch_gateway::account_strategy::describe_strategy(Region::Global, &global).await,
    })
}

/// POST /api/gateway/strategy —— 保存某 region 账号策略。
#[tauri::command]
pub async fn save_account_strategy(region: Option<String>, strategy: Value) -> Result<Value, String> {
    let region = parse_region(region.as_deref());
    let parsed: AccountStrategy =
        serde_json::from_value(strategy).map_err(|error| format!("策略格式错误: {error}"))?;
    let state = gateway::shared_state();
    {
        let mut strategies = state.strategies.write().await;
        strategies.insert(region, parsed);
        buddy_switch_gateway::account_strategy::save_strategies(&strategies)?;
    }
    Ok(json!({ "ok": true }))
}

/// GET /api/gateway/logs —— 最近 N 条请求日志（元数据）。
#[tauri::command]
pub fn get_gateway_logs() -> Value {
    let state = gateway::shared_state();
    json!({ "logs": state.log.list() })
}

/// POST /api/gateway/logs/clear —— 清空日志。
#[tauri::command]
pub fn clear_gateway_logs() -> Value {
    let state = gateway::shared_state();
    state.log.clear();
    json!({ "ok": true })
}
