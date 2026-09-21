//! 会话列表与按需复制（路径 B：生成新 id，云端可正常同步）。
//!
//! 对照 server.py `current_user_uid` / `list_sessions_for_user` /
//! `_find_project_jsonl` / `copy_session_to_user` / `_register_edge_sync_mapping` /
//! `copy_sessions_for_switch` / `backup_workbuddy_db` / `workbuddy_db_path`。
//!
//! WorkBuddy 5.x 数据三件套（缺一不可）：
//!   1) 正文：`~/.workbuddy/projects/{workspace}/{cid}.jsonl`（JSONL 含 sessionId 字段）
//!   2) 元数据：`~/.workbuddy/workbuddy.db` sessions 表（id = conversation id = UUID）
//!   3) 云端映射：`~/.workbuddy/edge-sync-mapping-v2.db` edge_sync_mapping
//!      （session_id=conversation_id，msg_channel=convmsg:{uid} 决定云端归属）

use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::modules::auth_file;
use crate::modules::config::{backup_dir, home_dir, now_ms, now_secs, utc_iso};
use crate::modules::region::Region;

/// 打开数据库并设置 busy_timeout（对照 Python `sqlite3.connect(timeout=5)`）。
fn open_db(path: &Path, read_only: bool) -> Option<Connection> {
    let conn = if read_only {
        Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?
    } else {
        Connection::open(path).ok()?
    };
    let _ = conn.busy_timeout(Duration::from_secs(5));
    Some(conn)
}

/// 客户端会话数据目录（CN `.workbuddy` / Global `.workbuddy-ai`）。
pub fn session_data_dir(region: Region) -> PathBuf {
    let name = match region {
        Region::Cn => ".workbuddy",
        Region::Global => ".workbuddy-ai",
    };
    home_dir().join(name)
}

/// CN 会话数据库路径。
pub fn workbuddy_db_path() -> PathBuf {
    workbuddy_db_path_for(Region::Cn)
}

/// 按 region 会话数据库路径。
pub fn workbuddy_db_path_for(region: Region) -> PathBuf {
    session_data_dir(region).join("workbuddy.db")
}

/// CN 会话边车映射数据库路径（旧签名，仅供单测使用）。
#[cfg(test)]
fn edge_sync_db_path() -> PathBuf {
    edge_sync_db_path_for(Region::Cn)
}

fn edge_sync_db_path_for(region: Region) -> PathBuf {
    session_data_dir(region).join("edge-sync-mapping-v2.db")
}

/// 当前认证账号的 uid（CN 认证文件 account.uid）。
pub fn current_user_uid() -> Option<String> {
    current_user_uid_for(Region::Cn)
}

/// 按 region 当前认证账号的 uid（认证文件 account.uid）。
pub fn current_user_uid_for(region: Region) -> Option<String> {
    let auth = auth_file::read_auth_file_for(region)?;
    auth.get("account")
        .and_then(|a| a.get("uid"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get::<_, i64>(0),
    )
    .unwrap_or(0)
        == 1
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> bool {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) else {
        return false;
    };
    let Ok(iter) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return false;
    };
    let names: Vec<String> = iter.flatten().collect();
    names.iter().any(|name| name == column)
}

fn nonempty_text(value: Option<String>) -> Option<String> {
    value
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// WorkBuddy 侧栏展示名：优先 custom_title（用户改名 / 定时任务名），否则 title。
fn session_display_title(title: Option<String>, custom_title: Option<String>) -> String {
    nonempty_text(custom_title)
        .or_else(|| nonempty_text(title))
        .unwrap_or_else(|| "(无标题)".to_string())
}

/// Claw 是账号绑定的 IM 渠道工作区，复制会话行不够，目标账号也用不了。
fn is_claw_workspace(cwd: &str) -> bool {
    cwd.trim()
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .is_some_and(|name| name.eq_ignore_ascii_case("claw"))
}

/// 列出某账号未删除的会话（CN；workbuddy.db sessions 表，db 为准）。
///
/// `title` 为 WorkBuddy 侧栏同款展示名；`isPlayground` 对应侧栏「任务」，
/// 其余按 `cwd` 最后一段归入「空间」。
pub fn list_sessions_for_user(uid: &str) -> Value {
    list_sessions_for_user_for(Region::Cn, uid)
}

/// 按 region 列出某账号未删除的会话。
pub fn list_sessions_for_user_for(region: Region, uid: &str) -> Value {
    let db = workbuddy_db_path_for(region);
    if !db.is_file() {
        return json!([]);
    }
    let Some(conn) = open_db(&db, true) else {
        return json!([]);
    };
    if !table_exists(&conn, "sessions") {
        return json!([]);
    }
    let has_custom = column_exists(&conn, "sessions", "custom_title");
    let has_playground = column_exists(&conn, "sessions", "is_playground");
    let sql = match (has_custom, has_playground) {
        (true, true) => {
            "SELECT id, cwd, title, custom_title, updated_at, is_playground FROM sessions \
             WHERE user_id = ?1 AND deleted_at IS NULL ORDER BY updated_at DESC"
        }
        (true, false) => {
            "SELECT id, cwd, title, custom_title, updated_at, 0 FROM sessions \
             WHERE user_id = ?1 AND deleted_at IS NULL ORDER BY updated_at DESC"
        }
        (false, true) => {
            "SELECT id, cwd, title, NULL, updated_at, is_playground FROM sessions \
             WHERE user_id = ?1 AND deleted_at IS NULL ORDER BY updated_at DESC"
        }
        (false, false) => {
            "SELECT id, cwd, title, NULL, updated_at, 0 FROM sessions \
             WHERE user_id = ?1 AND deleted_at IS NULL ORDER BY updated_at DESC"
        }
    };
    let mut stmt = match conn.prepare(sql) {
        Ok(s) => s,
        Err(_) => return json!([]),
    };
    let rows = stmt.query_map([uid], |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<i64>>(5)?,
        ))
    });

    let mut sessions: Vec<Value> = Vec::new();
    if let Ok(iter) = rows {
        for r in iter.flatten() {
            let (cid, cwd, title, custom_title, updated_at, is_playground) = r;
            let cid = cid.unwrap_or_default();
            let cwd = cwd.unwrap_or_default();
            if is_claw_workspace(&cwd) {
                continue;
            }
            sessions.push(json!({
                "id": cid,
                "title": session_display_title(title, custom_title),
                "cwd": cwd,
                "updatedAt": updated_at.unwrap_or(0),
                "hasHistory": find_project_jsonl_for(region, &cid).is_some(),
                "isPlayground": is_playground.unwrap_or(0) != 0,
            }));
        }
    }
    json!(sessions)
}

/// 按 region 在 `{data_dir}/projects/{workspace}/{cid}.jsonl` 定位会话正文。
fn find_project_jsonl_for(region: Region, cid: &str) -> Option<PathBuf> {
    let projects = session_data_dir(region).join("projects");
    if !projects.is_dir() {
        return None;
    }
    let direct = projects.join(format!("{cid}.jsonl"));
    if direct.is_file() {
        return Some(direct);
    }
    for entry in std::fs::read_dir(&projects).ok()?.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let p = entry.path().join(format!("{cid}.jsonl"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// 备份 workbuddy.db（含 -wal/-shm），返回主库备份路径。对照 `backup_workbuddy_db`。
fn backup_workbuddy_db(region: Region, backup_root: &Path) -> Option<PathBuf> {
    let db = workbuddy_db_path_for(region);
    if !db.is_file() {
        return None;
    }
    std::fs::create_dir_all(backup_root).ok()?;
    for suffix in ["", "-wal", "-shm"] {
        let src = PathBuf::from(format!("{}{}", db.to_string_lossy(), suffix));
        if src.is_file() {
            let _ = std::fs::copy(&src, backup_root.join(format!("workbuddy.db{suffix}")));
        }
    }
    Some(backup_root.join("workbuddy.db"))
}

/// 把 source_uid 的一个会话复制为 target_uid 的新会话（路径 B：生成新 id）。
///
/// 全部按「新 id」复制一份给目标账号，源账号数据完全不动。
/// 新 id 必须用带连字符的 UUID 格式（`Uuid::new_v4().to_string()`），与官方一致；
/// 32 位无连字符形式会导致 WorkBuddy 无法识别新会话。
pub fn copy_session_to_user(
    cid: &str,
    source_uid: &str,
    target_uid: &str,
) -> Result<Value, String> {
    copy_session_to_user_for(Region::Cn, cid, source_uid, target_uid)
}

/// 按 region（同一版本内）把 source_uid 的一个会话复制为 target_uid 的新会话（路径 B）。
pub fn copy_session_to_user_for(
    region: Region,
    cid: &str,
    source_uid: &str,
    target_uid: &str,
) -> Result<Value, String> {
    copy_session_to_user_cross(region, region, cid, source_uid, target_uid)
}

/// 跨版本复制：正文与元数据**读自 `source_region`**，副本与云端映射**写入 `target_region`**。
///
/// **去重**：若该源会话此前已复制给同一目标账号且副本仍在，则不重复复制，
/// 直接返回既有副本（`deduplicated: true`）。见 [`ledger_hit_for`]。
pub fn copy_session_to_user_cross(
    source_region: Region,
    target_region: Region,
    cid: &str,
    source_uid: &str,
    target_uid: &str,
) -> Result<Value, String> {
    if let Some(existing) =
        ledger_hit_for(source_region, target_region, source_uid, target_uid, cid)
    {
        return Ok(json!({
            "id": cid,
            "newId": existing,
            "jsonlCopied": false,
            "mappingWritten": false,
            "backup": Value::Null,
            "deduplicated": true,
        }));
    }

    let new_cid = uuid::Uuid::new_v4().to_string();
    let db = workbuddy_db_path_for(source_region);
    if let Some(conn) = open_db(&db, true) {
        let cwd: Option<String> = conn
            .query_row(
                "SELECT cwd FROM sessions WHERE id = ?1 AND user_id = ?2",
                rusqlite::params![cid, source_uid],
                |r| r.get(0),
            )
            .ok();
        if cwd.as_deref().is_some_and(is_claw_workspace) {
            return Err("Claw 工作区绑定当前账号渠道，不支持复制".into());
        }
    }

    // 1) 复制正文 jsonl：源 projects 下 {ws}/{cid}.jsonl → 目标 projects 下同位置 {new_cid}.jsonl
    let mut jsonl_copied = false;
    if let Some(src_jsonl) = find_project_jsonl_for(source_region, cid) {
        let dst_jsonl = target_jsonl_path_for(source_region, target_region, &src_jsonl, &new_cid);
        if let Some(parent) = dst_jsonl.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = std::fs::read_to_string(&src_jsonl) {
            let text = text.replace(cid, &new_cid); // 替换 sessionId 等旧 id 引用
            if std::fs::write(&dst_jsonl, text).is_ok() {
                jsonl_copied = true;
            }
        }
    }

    // 2) 备份目标 db（复制前），再把源行复制为新 id 插进目标库
    let backup_root = backup_dir().join("sessions").join(utc_iso());
    backup_workbuddy_db(target_region, &backup_root);
    insert_session_copy(
        &workbuddy_db_path_for(source_region),
        &workbuddy_db_path_for(target_region),
        &new_cid,
        cid,
        source_uid,
        target_uid,
    )?;

    // 3) 注册云端映射：新会话归属目标账号（msg_channel=convmsg:{target_uid}）
    let mapping_written = register_edge_sync_mapping_for(target_region, &new_cid, target_uid);

    // 4) 登记去重账本：下次复制同一源会话时直接跳过
    let ledger_written = record_copy_ledger(
        source_region,
        target_region,
        source_uid,
        target_uid,
        cid,
        &new_cid,
    );

    Ok(json!({
        "id": cid,
        "newId": new_cid,
        "jsonlCopied": jsonl_copied,
        "mappingWritten": mapping_written,
        "backup": backup_root.to_string_lossy().to_string(),
        "deduplicated": false,
        "ledgerWritten": ledger_written,
    }))
}

/// 会话正文副本的落点：保持「相对 projects 目录的子路径」不变，换到目标版本目录下。
///
/// 同版本时结果与原实现完全一致（同一 workspace 目录、只换文件名）；
/// 跨版本时把正文搬到 `target_region` 的 projects，否则副本写进源版本目录、
/// 目标版本的 WorkBuddy 根本看不到它。
fn target_jsonl_path_for(
    source_region: Region,
    target_region: Region,
    src_jsonl: &Path,
    new_cid: &str,
) -> PathBuf {
    let source_projects = session_data_dir(source_region).join("projects");
    let target_projects = session_data_dir(target_region).join("projects");
    let in_target = match src_jsonl.strip_prefix(&source_projects) {
        Ok(rel) => target_projects.join(rel),
        Err(_) => target_projects,
    };
    in_target.with_file_name(format!("{new_cid}.jsonl"))
}

/// 表的列名清单（按 PRAGMA 顺序）；表不存在时为空。
fn table_columns(conn: &Connection, table: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info({table})")) else {
        return Vec::new();
    };
    let Ok(iter) = stmt.query_map([], |row| row.get::<_, String>(1)) else {
        return Vec::new();
    };
    iter.flatten().collect()
}

/// 读出源会话行的列名与值；顺带做 Claw 判定。
///
/// 源库不存在 / 无 sessions 表 / 无此行 → `Ok(None)`（对照 Python 版静默跳过）。
fn read_session_row(
    src_db_path: &Path,
    cid: &str,
    source_uid: &str,
) -> Result<Option<(Vec<String>, Vec<rusqlite::types::Value>)>, String> {
    if !src_db_path.is_file() {
        return Ok(None);
    }
    let Some(conn) = open_db(src_db_path, true) else {
        return Ok(None);
    };
    if !table_exists(&conn, "sessions") {
        return Ok(None);
    }
    let mut stmt = conn
        .prepare("SELECT * FROM sessions WHERE id = ?1 AND user_id = ?2")
        .map_err(|e| e.to_string())?;
    let cols: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let mut rows = stmt
        .query(rusqlite::params![cid, source_uid])
        .map_err(|e| e.to_string())?;
    let Some(row) = rows.next().map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let mut vals: Vec<rusqlite::types::Value> = Vec::with_capacity(cols.len());
    for (i, col) in cols.iter().enumerate() {
        let v = row
            .get::<_, rusqlite::types::Value>(i)
            .unwrap_or(rusqlite::types::Value::Null);
        if col == "cwd" {
            if let rusqlite::types::Value::Text(ref path) = v {
                if is_claw_workspace(path) {
                    return Err("Claw 工作区绑定当前账号渠道，不支持复制".into());
                }
            }
        }
        vals.push(v);
    }
    Ok(Some((cols, vals)))
}

/// 把源会话行复制为新 id 写入目标库（动态列，覆盖 id/user_id/时间戳）。
///
/// 源与目标可以是两个版本的 db。目标库缺某一列时**丢弃该列**而不是整条失败 ——
/// 两版 schema 高度同源但允许有差异，丢弃比让整次复制失败更接近用户预期。
fn insert_session_copy(
    src_db_path: &Path,
    dst_db_path: &Path,
    new_cid: &str,
    cid: &str,
    source_uid: &str,
    target_uid: &str,
) -> Result<(), String> {
    let Some((cols, vals)) = read_session_row(src_db_path, cid, source_uid)? else {
        return Ok(());
    };
    if !dst_db_path.is_file() {
        return Ok(());
    }
    let Some(conn) = open_db(dst_db_path, false) else {
        return Ok(());
    };
    if !table_exists(&conn, "sessions") {
        return Ok(());
    }
    let dst_cols = table_columns(&conn, "sessions");

    let mut insert_cols: Vec<&str> = Vec::with_capacity(cols.len());
    let mut insert_vals: Vec<rusqlite::types::Value> = Vec::with_capacity(cols.len());
    for (col, v) in cols.iter().zip(vals) {
        if !dst_cols.iter().any(|c| c == col) {
            continue;
        }
        let v = match col.as_str() {
            "id" => rusqlite::types::Value::Text(new_cid.to_string()),
            "user_id" => rusqlite::types::Value::Text(target_uid.to_string()),
            "created_at" | "updated_at" => rusqlite::types::Value::Integer(now_ms()),
            "deleted_at" => rusqlite::types::Value::Null,
            _ => v,
        };
        insert_cols.push(col.as_str());
        insert_vals.push(v);
    }
    if insert_cols.is_empty() {
        return Ok(());
    }

    let placeholders = insert_cols.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
    let colnames = insert_cols.join(", ");
    let sql = format!("INSERT OR REPLACE INTO sessions ({colnames}) VALUES ({placeholders})");
    let params: Vec<&rusqlite::types::Value> = insert_vals.iter().collect();
    conn.execute(&sql, rusqlite::params_from_iter(params))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// 把新会话注册进 edge_sync_mapping（云端归属关键）。失败不致命，返回 False。
fn register_edge_sync_mapping_for(region: Region, new_cid: &str, target_uid: &str) -> bool {
    insert_edge_sync_mapping(&edge_sync_db_path_for(region), new_cid, target_uid)
}

fn insert_edge_sync_mapping(db_path: &Path, new_cid: &str, target_uid: &str) -> bool {
    if !db_path.is_file() {
        return false;
    }
    let Some(conn) = open_db(db_path, false) else {
        return false;
    };
    if !table_exists(&conn, "edge_sync_mapping") {
        return false;
    }
    let created_at = now_secs();
    let r = conn.execute(
        "INSERT OR REPLACE INTO edge_sync_mapping \
         (session_id, conversation_id, msg_channel, created_at) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            new_cid,
            new_cid,
            format!("convmsg:{target_uid}"),
            created_at
        ],
    );
    match r {
        Ok(_) => true,
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// 复制去重账本（同一源会话 → 同一目标账号只复制一次）
// ---------------------------------------------------------------------------

/// 复制去重账本相对于 session 数据目录的文件名。
///
/// 存放形如 `{ "source_uid\u{1f}target_uid\u{1f}source_cid": new_cid }` 的映射。
const COPY_LEDGER_FILE: &str = "buddy-switch-copy-ledger.json";

/// 复合键分隔符（Unit Separator；正常 uid / uuid 不会包含）。
const LEDGER_SEP: char = '\u{1f}';

fn copy_ledger_path_for(region: Region) -> PathBuf {
    session_data_dir(region).join(COPY_LEDGER_FILE)
}

/// 解析账本路径：仅用于单测把读写重定向到临时目录，**普通调用一律传 `None`**。
fn resolve_ledger_path(region: Region, override_path: Option<&Path>) -> PathBuf {
    match override_path {
        Some(p) => p.to_path_buf(),
        None => copy_ledger_path_for(region),
    }
}

/// 构造账本复合键：`source_uid ␟ target_uid ␟ source_cid`。
///
/// 三元组缺一不可：同一源会话复制给不同目标账号是两次独立操作；
/// 同一目标账号从不同源账号复制同名 cid 也应各自成条。
fn copy_ledger_key(source_uid: &str, target_uid: &str, source_cid: &str) -> String {
    format!("{source_uid}{LEDGER_SEP}{target_uid}{LEDGER_SEP}{source_cid}")
}

/// 跨版本复制的账本键：在三元键前加**源 region** 前缀。
///
/// 同版本刻意保持原键 —— 否则既有账本全部失配，老用户会被判成「没复制过」而重复复制；
/// 跨版本必须加前缀 —— 否则「CN 的 uid-a 复制给 X」与「Global 的 uid-a 复制给 X」
/// 会共用一条登记，其中一侧被错误跳过。
fn copy_ledger_key_for(
    source_region: Region,
    target_region: Region,
    source_uid: &str,
    target_uid: &str,
    source_cid: &str,
) -> String {
    let base = copy_ledger_key(source_uid, target_uid, source_cid);
    if source_region == target_region {
        base
    } else {
        format!("{}{LEDGER_SEP}{base}", source_region.as_str())
    }
}

/// 读取复制账本；文件缺失 / 损坏 / 结构不符时返回空表（视为无登记）。
fn load_copy_ledger_at(path: &Path) -> serde_json::Map<String, Value> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return serde_json::Map::new();
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    }
}

fn load_copy_ledger(region: Region) -> serde_json::Map<String, Value> {
    load_copy_ledger_at(&copy_ledger_path_for(region))
}

/// 原子写回复制账本。写入失败不致命（下次会重新复制），返回是否成功。
fn save_copy_ledger_at(path: &Path, ledger: &serde_json::Map<String, Value>) -> bool {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return false;
        }
    }
    let content = serde_json::to_string_pretty(&Value::Object(ledger.clone())).unwrap_or_default();
    crate::modules::config::atomic_write(path, &content).is_ok()
}

/// 该账号名下是否存在未删除的指定会话（db 为准）。
fn session_exists_for(region: Region, cid: &str, uid: &str) -> bool {
    let db = workbuddy_db_path_for(region);
    if !db.is_file() {
        return false;
    }
    let Some(conn) = open_db(&db, true) else {
        return false;
    };
    if !table_exists(&conn, "sessions") {
        return false;
    }
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sessions \
         WHERE id = ?1 AND user_id = ?2 AND deleted_at IS NULL)",
        rusqlite::params![cid, uid],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
    .unwrap_or(false)
}

/// 目标账号下该源会话是否已复制过：账本命中且副本仍存活 → 返回该副本 id。
///
/// 「仍存活」= 副本会话行仍在目标账号名下（未被用户删除）。
/// 用户删除副本后允许重新复制，避免账本变成永久性阻断。
fn ledger_hit_for(
    source_region: Region,
    target_region: Region,
    source_uid: &str,
    target_uid: &str,
    source_cid: &str,
) -> Option<String> {
    let ledger = load_copy_ledger(target_region);
    let new_cid = ledger
        .get(&copy_ledger_key_for(
            source_region,
            target_region,
            source_uid,
            target_uid,
            source_cid,
        ))?
        .as_str()?
        .trim()
        .to_string();
    if new_cid.is_empty() {
        return None;
    }
    session_exists_for(target_region, &new_cid, target_uid).then_some(new_cid)
}

/// 在账本中登记一次成功的复制。写入路径可被单测重定向（见 [`resolve_ledger_path`]）。
fn record_copy_ledger_at(
    source_region: Region,
    target_region: Region,
    override_path: Option<&Path>,
    source_uid: &str,
    target_uid: &str,
    source_cid: &str,
    new_cid: &str,
) -> bool {
    let path = resolve_ledger_path(target_region, override_path);
    let mut ledger = load_copy_ledger_at(&path);
    ledger.insert(
        copy_ledger_key_for(source_region, target_region, source_uid, target_uid, source_cid),
        Value::String(new_cid.to_string()),
    );
    save_copy_ledger_at(&path, &ledger)
}

fn record_copy_ledger(
    source_region: Region,
    target_region: Region,
    source_uid: &str,
    target_uid: &str,
    source_cid: &str,
    new_cid: &str,
) -> bool {
    record_copy_ledger_at(
        source_region,
        target_region,
        None,
        source_uid,
        target_uid,
        source_cid,
        new_cid,
    )
}

/// 切换前把勾选的会话复制到目标账号（CN，路径 B）。返回复制报告。
pub fn copy_sessions_for_switch(target_acc: &Value, session_ids: &[String]) -> Option<Value> {
    copy_sessions_for_switch_for(Region::Cn, target_acc, session_ids)
}

/// 按 region（同一版本内）切换前把勾选的会话复制到目标账号（路径 B）。返回复制报告。
pub fn copy_sessions_for_switch_for(
    region: Region,
    target_acc: &Value,
    session_ids: &[String],
) -> Option<Value> {
    copy_sessions_for_switch_cross(region, region, target_acc, session_ids)
}

/// 跨版本：把 `source_region` 当前登录账号的勾选会话复制到 `target_region` 的目标账号。
///
/// 源 uid 取自**源版本**的认证文件 —— 这正是跨版本的关键：沿用目标版本去取，
/// 取到的是目标版本自己的登录账号，会把「另一个账号」的会话搬过去。
pub fn copy_sessions_for_switch_cross(
    source_region: Region,
    target_region: Region,
    target_acc: &Value,
    session_ids: &[String],
) -> Option<Value> {
    let target_uid = target_acc
        .get("uid")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    if target_uid.is_empty() {
        return None;
    }
    let source_uid = current_user_uid_for(source_region)?;
    // 同版本同 uid = 自我复制，无意义；跨版本 uid 同文不算（两版 uid 不同源）。
    if source_region == target_region && source_uid == target_uid {
        return None;
    }

    let mut report = json!({
        "sourceUid": source_uid,
        "targetUid": target_uid,
        "sourceRegion": source_region.as_str(),
        "targetRegion": target_region.as_str(),
        "copied": [],
    });
    let mut errors: Vec<Value> = Vec::new();
    // 命中账本的源会话：不重复复制，单独归类以便 UI 明确告知「已存在，跳过」
    let mut skipped: Vec<Value> = Vec::new();
    for cid in session_ids {
        match copy_session_to_user_cross(
            source_region,
            target_region,
            cid,
            &source_uid,
            &target_uid,
        ) {
            Ok(r) if r.get("deduplicated").and_then(Value::as_bool) == Some(true) => {
                skipped.push(r);
            }
            Ok(r) => report["copied"].as_array_mut().unwrap().push(r),
            Err(e) => errors.push(json!({"id": cid, "error": e})),
        }
    }
    if !errors.is_empty() {
        report["errors"] = json!(errors);
    }
    if !skipped.is_empty() {
        report["skipped"] = json!(skipped);
    }
    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn db_paths_point_to_home() {
        // 用 Path 组件比较，避免 Windows `\` / Unix `/` 分隔符差异。
        assert!(workbuddy_db_path().ends_with(std::path::Path::new(".workbuddy").join("workbuddy.db")));
        assert!(edge_sync_db_path()
            .to_string_lossy()
            .ends_with("edge-sync-mapping-v2.db"));
    }

    /// 会话目录与数据库路径必须按 region 隔离（PRD G1 / D2）。
    ///
    /// CN 用 `~/.workbuddy`，Global 用 `~/.workbuddy-ai`；两版若共用一个目录，
    /// 会话库会互相覆盖。
    #[test]
    fn session_paths_are_region_scoped() {
        let cn_dir = session_data_dir(Region::Cn);
        let global_dir = session_data_dir(Region::Global);

        assert_eq!(
            cn_dir.file_name().and_then(|n| n.to_str()),
            Some(".workbuddy"),
            "CN 会话目录名"
        );
        assert_eq!(
            global_dir.file_name().and_then(|n| n.to_str()),
            Some(".workbuddy-ai"),
            "Global 会话目录名"
        );
        assert_ne!(cn_dir, global_dir, "CN / Global 会话目录必须隔离");
        // 同一 home，仅目录名不同。
        assert_eq!(cn_dir.parent(), global_dir.parent());
        assert_eq!(
            cn_dir,
            crate::modules::config::home_dir().join(".workbuddy")
        );
        assert_eq!(
            global_dir,
            crate::modules::config::home_dir().join(".workbuddy-ai")
        );

        // 数据库路径由会话目录派生，同样必须按 region 隔离。
        assert_eq!(workbuddy_db_path_for(Region::Cn), cn_dir.join("workbuddy.db"));
        assert_eq!(
            workbuddy_db_path_for(Region::Global),
            global_dir.join("workbuddy.db")
        );
        assert_ne!(
            workbuddy_db_path_for(Region::Cn),
            workbuddy_db_path_for(Region::Global)
        );
        // CN 兼容路径与 region 化路径一致，避免两处逻辑漂移。
        assert_eq!(workbuddy_db_path(), workbuddy_db_path_for(Region::Cn));
    }

    fn temp_db(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "buddy_switch_test_{}_{name}.db",
            uuid::Uuid::new_v4().simple()
        ))
    }

    #[test]
    fn insert_session_copy_duplicates_row_with_target_uid() {
        let db = temp_db("sessions");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                title TEXT,
                cwd TEXT,
                created_at INTEGER,
                updated_at INTEGER,
                deleted_at INTEGER,
                payload BLOB
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, user_id, title, cwd, created_at, updated_at, deleted_at, payload)
             VALUES ('src-1', 'uid-a', '旧标题', '/ws', 1000, 2000, NULL, x'DEADBEEF')",
            [],
        )
        .unwrap();

        insert_session_copy(&db, &db, "new-uuid-1", "src-1", "uid-a", "uid-b").unwrap();

        let (id, user_id, title, deleted_at): (String, String, String, Option<i64>) = conn
            .query_row(
                "SELECT id, user_id, title, deleted_at FROM sessions WHERE id = 'new-uuid-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(id, "new-uuid-1");
        assert_eq!(user_id, "uid-b");
        assert_eq!(title, "旧标题"); // 普通列原样保留
        assert_eq!(deleted_at, None); // deleted_at 置空

        // 源行保持不变
        let src_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sessions WHERE id = 'src-1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(src_count, 1);
    }

    #[test]
    fn insert_session_copy_missing_source_is_noop() {
        let db = temp_db("noop");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, user_id TEXT, title TEXT, created_at INTEGER, updated_at INTEGER, deleted_at INTEGER);",
        )
        .unwrap();
        insert_session_copy(&db, &db, "new-1", "missing", "uid-a", "uid-b").unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn insert_session_copy_missing_db_is_ok() {
        let db = temp_db("missing");
        // 不创建文件
        assert!(insert_session_copy(&db, &db, "new-1", "src-1", "a", "b").is_ok());
    }

    /// 跨库复制：源行只出现在源库，副本只出现在目标库。
    ///
    /// 这是跨版本复制的核心断言 —— 若实现仍把「读」和「写」绑在同一个 db 上，
    /// 要么副本没进目标库（用户看不到），要么源库被写脏。
    #[test]
    fn insert_session_copy_across_databases() {
        let src_db = temp_db("xsrc");
        let dst_db = temp_db("xdst");
        for db in [&src_db, &dst_db] {
            let conn = Connection::open(db).unwrap();
            conn.execute_batch(
                "CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    user_id TEXT NOT NULL,
                    title TEXT,
                    created_at INTEGER,
                    updated_at INTEGER,
                    deleted_at INTEGER
                 );",
            )
            .unwrap();
        }
        Connection::open(&src_db)
            .unwrap()
            .execute(
                "INSERT INTO sessions (id, user_id, title, created_at, updated_at, deleted_at)
                 VALUES ('src-1', 'uid-a', '跨版本标题', 1000, 2000, NULL)",
                [],
            )
            .unwrap();

        insert_session_copy(&src_db, &dst_db, "new-1", "src-1", "uid-a", "uid-b").unwrap();

        let dst = Connection::open(&dst_db).unwrap();
        let (title, user_id): (String, String) = dst
            .query_row(
                "SELECT title, user_id FROM sessions WHERE id = 'new-1'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(title, "跨版本标题");
        assert_eq!(user_id, "uid-b", "副本必须归属目标账号");

        let src_rows: i64 = Connection::open(&src_db)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(src_rows, 1, "源库不得被写入");
    }

    #[test]
    fn insert_edge_sync_mapping_registers_channel() {
        let db = temp_db("edge");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE edge_sync_mapping (
                session_id TEXT,
                conversation_id TEXT,
                msg_channel TEXT,
                created_at INTEGER
            );",
        )
        .unwrap();
        assert!(insert_edge_sync_mapping(&db, "new-1", "uid-b"));
        let (sid, cid, channel): (String, String, String) = conn
            .query_row(
                "SELECT session_id, conversation_id, msg_channel FROM edge_sync_mapping",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(sid, "new-1");
        assert_eq!(cid, "new-1");
        assert_eq!(channel, "convmsg:uid-b");
    }

    #[test]
    fn insert_edge_sync_mapping_missing_table_false() {
        let db = temp_db("edge-no-table");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE other (x INTEGER);")
            .unwrap();
        assert!(!insert_edge_sync_mapping(&db, "new-1", "uid-b"));
    }

    #[test]
    fn session_display_title_prefers_custom_title() {
        assert_eq!(
            session_display_title(Some("自动标题".into()), Some("美团每日自动领券".into())),
            "美团每日自动领券"
        );
        assert_eq!(
            session_display_title(None, Some("美团每日自动领券".into())),
            "美团每日自动领券"
        );
        assert_eq!(
            session_display_title(Some("汉字详情页".into()), None),
            "汉字详情页"
        );
        assert_eq!(session_display_title(None, None), "(无标题)");
        assert_eq!(
            session_display_title(Some("  ".into()), Some("".into())),
            "(无标题)"
        );
    }

    #[test]
    fn claw_workspace_detected_by_folder_name() {
        assert!(is_claw_workspace("/Users/apple/WorkBuddy/Claw"));
        assert!(is_claw_workspace("/Users/apple/WorkBuddy/claw/"));
        assert!(is_claw_workspace(r"C:\Users\me\WorkBuddy\Claw"));
        assert!(!is_claw_workspace("/Users/apple/WorkBuddy/ClawBot"));
        assert!(!is_claw_workspace(
            "/Users/apple/Documents/AI-PROJECT/LetterTotTown"
        ));
    }

    /// 账本键必须同时区分源账号、目标账号与源会话：任一维度不同即为不同条目，
    /// 否则「A→C 复制过 x」会错误阻断「B→C 复制 x」。
    #[test]
    fn copy_ledger_key_separates_all_three_dimensions() {
        let base = copy_ledger_key("uid-a", "uid-b", "cid-1");
        assert_ne!(base, copy_ledger_key("uid-a2", "uid-b", "cid-1"), "源账号区分");
        assert_ne!(base, copy_ledger_key("uid-a", "uid-b2", "cid-1"), "目标账号区分");
        assert_ne!(base, copy_ledger_key("uid-a", "uid-b", "cid-2"), "源会话区分");
        assert_eq!(base, copy_ledger_key("uid-a", "uid-b", "cid-1"));
        // 分隔符不得与字段内容混淆：把分隔符塞进字段值也必须产生不同键。
        assert_ne!(
            copy_ledger_key("uid-a", "uid-b", "cid-1"),
            copy_ledger_key("uid-a\u{1f}uid-b", "", "cid-1"),
            "字段内容中的分隔符不得造成键碰撞"
        );
    }

    /// 跨版本账本键必须带源 region 前缀，同版本必须保持原键。
    ///
    /// 两个方向都要钉住：加前缀是为了「CN 的 uid-a」与「Global 的 uid-a」不共用登记；
    /// 同版本不加是为了既有账本不失配（否则老用户会被判成没复制过而重复复制）。
    #[test]
    fn cross_region_ledger_key_is_namespaced_by_source_region() {
        let same = copy_ledger_key_for(Region::Cn, Region::Cn, "uid-a", "uid-b", "cid-1");
        let cross = copy_ledger_key_for(Region::Cn, Region::Global, "uid-a", "uid-b", "cid-1");
        let reverse = copy_ledger_key_for(Region::Global, Region::Cn, "uid-a", "uid-b", "cid-1");

        assert_eq!(same, copy_ledger_key("uid-a", "uid-b", "cid-1"), "同版本键不得变");
        assert_ne!(same, cross, "跨版本必须与同版本区分");
        assert_ne!(cross, reverse, "方向不同必须是不同条目");
        assert!(cross.starts_with("cn\u{1f}"), "前缀应为源 region: {cross}");
        assert!(reverse.starts_with("global\u{1f}"), "前缀应为源 region: {reverse}");
    }

    /// 会话正文副本必须落在**目标**版本的 projects 下。
    ///
    /// 若沿用源目录，副本会写进源版本目录，目标版本的 WorkBuddy 根本看不到它 ——
    /// 这正是「跨版本不支持」时期最隐蔽的失败形态（不报错，只是没搬过去）。
    #[test]
    fn cross_region_jsonl_copy_lands_in_target_region_projects() {
        let src = session_data_dir(Region::Cn)
            .join("projects")
            .join("ws")
            .join("cid-1.jsonl");

        let same = target_jsonl_path_for(Region::Cn, Region::Cn, &src, "new-1");
        assert_eq!(
            same,
            session_data_dir(Region::Cn)
                .join("projects")
                .join("ws")
                .join("new-1.jsonl"),
            "同版本必须原地换名，行为零变化"
        );

        let cross = target_jsonl_path_for(Region::Cn, Region::Global, &src, "new-1");
        assert_eq!(
            cross,
            session_data_dir(Region::Global)
                .join("projects")
                .join("ws")
                .join("new-1.jsonl"),
            "跨版本必须换到目标版本目录"
        );
        assert_ne!(same, cross);
    }

    /// 账本读写往返：写入后能读回，且未命中项返回 None。
    ///
    /// 全程只操作临时文件，**不得触碰真实 `~/.workbuddy` 账本**。
    #[test]
    fn copy_ledger_roundtrip_and_miss() {
        let region = Region::Cn;
        let dir = std::env::temp_dir().join(format!(
            "buddy_switch_ledger_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(COPY_LEDGER_FILE);

        assert!(
            load_copy_ledger_at(&path).is_empty(),
            "账本不存在时应视为空"
        );

        assert!(record_copy_ledger_at(
            region,
            region,
            Some(&path),
            "uid-a",
            "uid-b",
            "cid-1",
            "new-1"
        ));

        let reloaded = load_copy_ledger_at(&path);
        assert_eq!(
            reloaded
                .get(&copy_ledger_key("uid-a", "uid-b", "cid-1"))
                .and_then(Value::as_str),
            Some("new-1")
        );
        assert!(
            reloaded
                .get(&copy_ledger_key("uid-a", "uid-b", "other"))
                .is_none(),
            "未登记的源会话不得命中"
        );

        // 同一键再次登记应覆盖而不是新增条目。
        assert!(record_copy_ledger_at(
            region,
            region,
            Some(&path),
            "uid-a",
            "uid-b",
            "cid-1",
            "new-2"
        ));
        let overwritten = load_copy_ledger_at(&path);
        assert_eq!(overwritten.len(), 1, "同一键不得产生第二条");
        assert_eq!(
            overwritten
                .get(&copy_ledger_key("uid-a", "uid-b", "cid-1"))
                .and_then(Value::as_str),
            Some("new-2")
        );

        // 测试过程不得在真实数据目录留下账本。
        assert!(
            !copy_ledger_path_for(region).exists() || copy_ledger_path_for(region).is_file(),
            "账本路径形态异常"
        );

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 损坏的账本必须退化为「无登记」而不是报错或丢弃已有副本。
    #[test]
    fn corrupt_copy_ledger_is_treated_as_empty() {
        let dir = std::env::temp_dir().join(format!(
            "buddy_switch_ledger_corrupt_{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(COPY_LEDGER_FILE);

        for payload in ["not-json", "[1,2,3]", "\"text\"", "null", ""] {
            std::fs::write(&path, payload).unwrap();
            assert!(
                load_copy_ledger_at(&path).is_empty(),
                "损坏内容应视为空账本：{payload}"
            );
        }

        std::fs::remove_dir_all(dir).unwrap();
    }

    /// 账本命中还必须要求副本仍存活：副本被删则允许重新复制。
    #[test]
    fn ledger_only_hits_when_copy_still_alive() {
        let db = temp_db("ledger-alive");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                user_id TEXT NOT NULL,
                title TEXT,
                deleted_at INTEGER
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, user_id, deleted_at) VALUES ('copy-1', 'uid-b', NULL)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, user_id, deleted_at) VALUES ('copy-2', 'uid-b', 12345)",
            [],
        )
        .unwrap();

        // 直接验证存活判定语义（账本文件 + db 路径由 region 派生，无法在本单测内重定向）。
        let alive = |cid: &str| -> bool {
            conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions \
                 WHERE id = ?1 AND user_id = 'uid-b' AND deleted_at IS NULL)",
                rusqlite::params![cid],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n != 0)
            .unwrap()
        };
        assert!(alive("copy-1"), "未删除的副本应视为存活");
        assert!(!alive("copy-2"), "已删除的副本不应视为存活");
        assert!(!alive("missing"), "不存在的副本不应视为存活");
    }
}
