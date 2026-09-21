//! 账号切换：备份 → 关进程 → 复制会话（可选）→ 写认证 → 启动。
//!
//! 对照 server.py `switch_account`。切换过程中通过进度回调向前端推送实时进度，
//! 避免界面长时间无反馈被误认为卡死。core 不依赖 Tauri，进度回调由宿主适配
//! （桌面端转发为 `switch-progress` 事件，HTTP 端写入轮询/SSE）。
//!
//! **region 化**：新增 [`switch_account_for`]；旧 CN 签名保留为薄包装。

use serde_json::{json, Value};

use crate::modules::account;
use crate::modules::auth_file;
use crate::modules::process::{close_workbuddy_for, launch_workbuddy_for};
use crate::modules::region::Region;
use crate::modules::session;

/// 切换进度回调（宿主注入，如 Tauri `app.emit` 或 HTTP 进度缓存）。
pub type ProgressFn = Box<dyn Fn(&str) + Send + Sync>;

/// 切换账号（CN）。copy_session_ids 非空时按路径 B 复制勾选会话（新 id，云端可同步）。
pub fn switch_account(
    progress_fn: Option<&ProgressFn>,
    account_id: &str,
    restart: bool,
    share_sessions: bool,
    copy_session_ids: &[String],
) -> Result<Value, String> {
    switch_account_for(
        Region::Cn,
        progress_fn,
        account_id,
        restart,
        share_sessions,
        copy_session_ids,
    )
}

/// 按 region 切换账号；会话复制的源版本与切换目标版本相同。
pub fn switch_account_for(
    region: Region,
    progress_fn: Option<&ProgressFn>,
    account_id: &str,
    restart: bool,
    share_sessions: bool,
    copy_session_ids: &[String],
) -> Result<Value, String> {
    switch_account_cross(
        region,
        region,
        progress_fn,
        account_id,
        restart,
        share_sessions,
        copy_session_ids,
    )
}

/// 跨版本切换：`source_region` 的当前登录账号是会话复制的**源**，`region` 是切换目标。
///
/// 只有会话复制关心源版本 —— 记忆与连接器迁移走 `migrate_account_data_cross`，
/// 由调用方单独传源 region（见 [`crate::modules::migrate`]）。
pub fn switch_account_cross(
    region: Region,
    source_region: Region,
    progress_fn: Option<&ProgressFn>,
    account_id: &str,
    restart: bool,
    share_sessions: bool,
    copy_session_ids: &[String],
) -> Result<Value, String> {
    let progress = |message: &str| {
        eprintln!("[switch] progress: {message}");
        if let Some(p) = progress_fn {
            p(message);
        }
    };

    progress("开始切换账号…");
    let acc = account::find_account_for(region, account_id)
        .ok_or_else(|| format!("账号不存在: {account_id}"))?;
    let backup = auth_file::backup_auth_file_for(region);

    let mut copy_report: Option<Value> = None;
    let mut session_report: Option<Value> = None;
    if restart {
        progress("正在关闭 WorkBuddy…");
        close_workbuddy_for(region, 20)?;
        // 只有重启场景才做会话操作（数据库在运行中不宜写入）
        if !copy_session_ids.is_empty() {
            progress("正在复制会话到目标账号…");
            copy_report = session::copy_sessions_for_switch_cross(
                source_region,
                region,
                &acc,
                copy_session_ids,
            );
        }
        if share_sessions {
            // 旧的「全体转移」兼容路径（默认关闭），Rust 版暂未实现
            session_report = Some(json!({"error": "share_sessions 兼容路径暂未在 Rust 版实现"}));
        }
    }
    progress("正在写入认证文件…");
    auth_file::write_account_to_auth_file_for(region, &acc)?;
    if restart {
        progress("正在启动 WorkBuddy…");
        launch_workbuddy_for(region, Some(&progress))?;
    }
    progress("切换完成");

    let mut result = json!({
        "ok": true,
        "account": account::account_display_name(&acc),
        "backup": backup.map(|p| p.to_string_lossy().to_string()),
    });
    if let Some(c) = copy_report {
        result["sessionCopy"] = c;
    }
    if let Some(s) = session_report {
        result["sessionShare"] = s;
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch_account_missing_id_returns_before_auth_side_effects() {
        let missing_id = "switch-test-account-that-does-not-exist";
        let error = switch_account(None, missing_id, false, false, &[]).unwrap_err();
        assert!(error.contains("账号不存在"));
        assert!(error.contains(missing_id));
        // This deliberately does not prove the CN wrapper's Region::Cn binding:
        // proving that requires distinct CN/Global fixture stores, while a found
        // account would write the real auth file. That boundary belongs to an
        // isolated HOME integration test.
    }
}
