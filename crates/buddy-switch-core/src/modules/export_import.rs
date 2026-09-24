//! 账号导出/导入：解析导入文件、按 uid 去重合并、计数。
//!
//! 纯逻辑（`parse_accounts_json` / `merge_import_records` / `select_export_records`）
//! 不依赖文件系统，便于无 UI 环境单测；`export_accounts` / `import_accounts`
//! 负责读写账号库（CN `~/.buddy-switch/accounts.json` / Global `accounts.global.json`）。
//!
//! **region 化**：导出/导入按 region 作用域（`*_for(region, …)`）；旧 CN 签名保留为
//! 薄包装，内部转调 `Region::Cn`，保证 P0-1「CN 行为零变化」。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::modules::account;
use crate::modules::region::Region;

/// 该记录是否带可用凭据。
///
/// 不能用 `get_str(..).is_some()`：WorkBuddy 5.6 起 `access_token` 可能是加密信封
/// 对象，`get_str` 取不到值会把「有凭据」误判成「没凭据」——预览显示无 token，
/// 合并阶段则直接 `Skipped`，导出的账号再也导不回来（静默丢账号）。
/// 空字符串仍视为无凭据，保持既有语义。
fn has_credential(item: &Value) -> bool {
    match account::secret_value(item, "access_token") {
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(_) => true,
        None => false,
    }
}

/// 导入结果计数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ImportResult {
    /// 成功合入账号库的数量（含覆盖与新增）。
    pub imported: usize,
    /// 未导入的数量（缺 access_token / 索引越界）。
    pub skipped: usize,
    /// 其中覆盖了同 uid 本地账号的数量。
    pub overwritten: usize,
}

/// 解析导出/导入文件文本：必须是 JSON 数组，且每项为 JSON 对象。
///
/// 失败时返回带位置的明确错误文案（非法 JSON / 非数组 / 元素不是对象）。
pub fn parse_accounts_json(text: &str) -> Result<Vec<Value>, String> {
    if text.trim().is_empty() {
        return Err("文件内容为空".to_string());
    }
    let parsed: Value =
        serde_json::from_str(text).map_err(|e| format!("文件不是合法的 JSON：{e}"))?;
    let array = parsed
        .as_array()
        .ok_or_else(|| "文件内容应为 JSON 数组（账号列表）".to_string())?;
    for (index, item) in array.iter().enumerate() {
        if !item.is_object() {
            return Err(format!("文件第 {} 项不是合法的账号对象", index + 1));
        }
    }
    Ok(array.clone())
}

/// 生成导入文件的脱敏预览（含文件内索引，不含 token）。
pub fn preview_accounts(text: &str) -> Result<Value, String> {
    let array = parse_accounts_json(text)?;
    let items: Vec<Value> = array
        .iter()
        .enumerate()
        .map(|(index, item)| {
            json!({
                "index": index,
                // 展示字段走 display_value：加密信封必须折叠为 null，裸透传会让
                // 导入预览弹窗把它当 React 子节点渲染，触发 error #31（白屏）。
                "uid": account::display_value(item, "uid"),
                "nickname": account::display_value(item, "nickname"),
                "email": account::display_value(item, "email"),
                "hasToken": has_credential(item),
            })
        })
        .collect();
    Ok(json!({ "accounts": items, "total": array.len() }))
}

/// 单条导入记录的合并动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeOutcome {
    /// 追加为新账号。
    Appended,
    /// 覆盖同 uid 的本地账号（保留导入记录原样）。
    Overwritten,
    /// 缺少 access_token，跳过。
    Skipped,
}

/// 纯函数：把一条导入记录合并进账号列表。
///
/// 按 uid 去重：同 uid 覆盖（保留导入记录原样）；uid 缺失或无法匹配则追加。
/// 缺少 access_token 的记录跳过，不进入账号库（信封形态算「有」，见 [`has_credential`]）。
fn merge_import_record(accounts: &mut Vec<Value>, item: &Value) -> MergeOutcome {
    if !has_credential(item) {
        return MergeOutcome::Skipped;
    }
    if let Some(uid) = account::get_str(item, "uid").as_deref() {
        if let Some(existing) = accounts
            .iter_mut()
            .find(|a| account::get_str(a, "uid").as_deref() == Some(uid))
        {
            let mut replaced = item.clone();
            // 导入记录缺 id 时保留本地 id：账号库不允许出现无 id 记录
            //（删除按 id、列表 key、导出选择都依赖 id）。
            if account::get_str(&replaced, "id").is_none() {
                if let Some(id) = existing.get("id").cloned() {
                    replaced["id"] = id;
                }
            }
            *existing = replaced;
            return MergeOutcome::Overwritten;
        }
    }
    accounts.push(item.clone());
    MergeOutcome::Appended
}

/// 纯函数：解析文件文本并按选中索引把记录合并进账号列表，返回计数。
///
/// 不做文件读写，便于单测。索引越界视为跳过。
pub fn merge_import_records(
    accounts: &mut Vec<Value>,
    text: &str,
    indexes: &[usize],
) -> Result<ImportResult, String> {
    let array = parse_accounts_json(text)?;
    let mut result = ImportResult::default();
    for &index in indexes {
        match array.get(index) {
            None => result.skipped += 1,
            Some(item) => match merge_import_record(accounts, item) {
                MergeOutcome::Appended => result.imported += 1,
                MergeOutcome::Overwritten => {
                    result.imported += 1;
                    result.overwritten += 1;
                }
                MergeOutcome::Skipped => result.skipped += 1,
            },
        }
    }
    Ok(result)
}

/// 导入（CN）：读账号库 → 合并 → 写回，返回计数。
pub fn import_accounts(text: &str, indexes: &[usize]) -> Result<ImportResult, String> {
    import_accounts_for(Region::Cn, text, indexes)
}

/// 检查选中的账号记录是否属于目标 region。
///
/// 缺 token 的记录仍由后续合并阶段计为 skipped；只有会实际进入账号库的记录需要
/// 经过区域校验。索引越界也保持既有 skipped 语义。
fn validate_records_for_region(
    region: Region,
    array: &[Value],
    indexes: &[usize],
) -> Result<(), String> {
    for &index in indexes {
        if let Some(item) = array.get(index) {
            if has_credential(item) {
                account::ensure_account_region(region, item)?;
            }
        }
    }
    Ok(())
}

/// 按 region 导入：读该 region 账号库 → 合并 → 写回，返回计数。
pub fn import_accounts_for(
    region: Region,
    text: &str,
    indexes: &[usize],
) -> Result<ImportResult, String> {
    // 先做区域预检，再创建/写回目标账号库。这样即使导入文件来自国际版，
    // 也不会因为调用方把 region 传成 cn 而把它落入 `accounts.json`。
    let array = parse_accounts_json(text)?;
    validate_records_for_region(region, &array, indexes)?;

    let mut accounts = account::load_accounts_for(region);
    let result = merge_import_records(&mut accounts, text, indexes)?;
    account::save_accounts_for(region, &accounts).map_err(|e| format!("保存账号库失败：{e}"))?;
    Ok(result)
}

/// 纯函数：从账号列表中挑出 id 命中的完整记录（含 token）。
pub fn select_export_records(accounts: &[Value], ids: &[String]) -> Result<Vec<Value>, String> {
    let ids: Vec<&str> = ids
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return Err("请先选择要导出的账号".to_string());
    }
    let mut exported: Vec<Value> = Vec::new();
    for id in ids {
        if let Some(acc) = accounts
            .iter()
            .find(|a| a.get("id").and_then(|v| v.as_str()) == Some(id))
        {
            exported.push(acc.clone());
        }
    }
    if exported.is_empty() {
        return Err("未找到要导出的账号".to_string());
    }
    Ok(exported)
}

/// 导出（CN）：按账号 id 列表返回完整记录（含 token）。
pub fn export_accounts(ids: &[String]) -> Result<Vec<Value>, String> {
    export_accounts_for(Region::Cn, ids)
}

/// 按 region 导出：按账号 id 列表返回完整记录（含 token）。
pub fn export_accounts_for(region: Region, ids: &[String]) -> Result<Vec<Value>, String> {
    select_export_records(&account::load_accounts_for(region), ids)
}

/// 导出文件名白名单：只允许 `buddy-switch-accounts-YYYY-MM-DD.json` 这类由前端生成的文件名。
fn validate_export_file_name(file_name: &str) -> Result<(), String> {
    let name = file_name.trim();
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || !name.ends_with(".json")
    {
        return Err("导出文件名不合法".to_string());
    }
    Ok(())
}

/// 校验导出目标路径：必须是绝对路径且以 `.json` 结尾（保存对话框产物）。
fn validate_export_path(path: &str) -> Result<(), String> {
    let p = Path::new(path.trim());
    if !p.is_absolute() {
        return Err("导出路径必须是绝对路径".to_string());
    }
    if !p
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("json"))
        .unwrap_or(false)
    {
        return Err("导出文件名必须以 .json 结尾".to_string());
    }
    Ok(())
}

/// 纯函数：把记录数组写入指定目录下的 JSON 文件，返回完整路径。
pub fn write_records_to_file(
    dir: &Path,
    records: &[Value],
    file_name: &str,
) -> Result<String, String> {
    validate_export_file_name(file_name)?;
    let content = serde_json::to_string_pretty(records).map_err(|e| e.to_string())?;
    let path = dir.join(file_name);
    crate::modules::config::atomic_write(&path, &content)
        .map_err(|e| format!("写入导出文件失败：{e}"))?;
    Ok(path.to_string_lossy().into_owned())
}

/// 导出（CN）：按账号 id 列表把完整记录写入用户选择的路径（保存对话框产物），返回该路径。
pub fn export_accounts_to_path(ids: &[String], path: &str) -> Result<String, String> {
    export_accounts_to_path_for(Region::Cn, ids, path)
}

/// 按 region 导出：按账号 id 列表把完整记录写入用户选择的路径，返回该路径。
pub fn export_accounts_to_path_for(
    region: Region,
    ids: &[String],
    path: &str,
) -> Result<String, String> {
    validate_export_path(path)?;
    let records = select_export_records(&account::load_accounts_for(region), ids)?;
    let path_buf = PathBuf::from(path.trim());
    let content = serde_json::to_string_pretty(&records).map_err(|e| e.to_string())?;
    crate::modules::config::atomic_write(&path_buf, &content)
        .map_err(|e| format!("写入导出文件失败：{e}"))?;
    Ok(path_buf.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        id: &str,
        uid: Option<&str>,
        nickname: &str,
        email: Option<&str>,
        token: bool,
    ) -> Value {
        json!({
            "id": id,
            "uid": uid,
            "nickname": nickname,
            "email": email,
            "access_token": if token { format!("token-{id}") } else { String::new() },
            "createdAt": 1,
        })
    }

    #[test]
    fn parse_rejects_empty_invalid_and_non_array() {
        assert!(parse_accounts_json("").is_err());
        assert!(parse_accounts_json("not json").is_err());
        assert!(parse_accounts_json(r#"{ "a": 1 }"#).is_err());
    }

    #[test]
    fn parse_rejects_non_object_element() {
        let err = parse_accounts_json(r#"[{ "uid": "u1" }, 42]"#).unwrap_err();
        assert!(err.contains("第 2 项"), "错误应带位置：{err}");
    }

    #[test]
    fn parse_accepts_object_array() {
        let parsed = parse_accounts_json(r#"[{ "uid": "u1" }, {}]"#).unwrap();
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn merge_overwrites_same_uid_preserving_imported_record() {
        let mut accounts = vec![record("local", Some("u1"), "旧名称", None, true)];
        let text = r#"[{ "id": "imported", "uid": "u1", "nickname": "新名称", "access_token": "tok-new" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(result.imported, 1);
        assert_eq!(result.overwritten, 1);
        assert_eq!(result.skipped, 0);
        assert_eq!(accounts.len(), 1, "同 uid 应覆盖而不是新增");
        assert_eq!(accounts[0]["id"], "imported", "覆盖保留导入记录原样");
        assert_eq!(accounts[0]["nickname"], "新名称");
        assert_eq!(accounts[0]["access_token"], "tok-new");
    }

    #[test]
    fn merge_preserves_local_id_when_imported_record_has_none() {
        let mut accounts = vec![record("local-id", Some("u1"), "旧名称", None, true)];
        let text = r#"[{ "uid": "u1", "nickname": "新名称", "access_token": "tok" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(result.imported, 1);
        assert_eq!(result.overwritten, 1);
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["id"], "local-id", "导入记录缺 id 时保留本地 id");
        assert_eq!(accounts[0]["nickname"], "新名称");
        assert_eq!(accounts[0]["access_token"], "tok");
    }

    #[test]
    fn merge_appends_when_uid_missing_or_unmatched() {
        let mut accounts = vec![record("local", Some("u1"), "甲", None, true)];
        let text = r#"[
            { "id": "n1", "nickname": "无uid", "access_token": "t1" },
            { "id": "n2", "uid": "u-other", "nickname": "uid不匹配", "access_token": "t2" }
        ]"#;
        let result = merge_import_records(&mut accounts, text, &[0, 1]).unwrap();
        assert_eq!(result.imported, 2);
        assert_eq!(result.overwritten, 0);
        assert_eq!(accounts.len(), 3);
    }

    #[test]
    fn merge_skips_missing_token() {
        let mut accounts: Vec<Value> = vec![];
        let text = r#"[{ "id": "no-token", "uid": "u9", "nickname": "缺token" }]"#;
        let result = merge_import_records(&mut accounts, text, &[0]).unwrap();
        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 1);
        assert!(accounts.is_empty(), "缺 token 的记录不得进入账号库");
    }

    #[test]
    fn merge_counts_out_of_range_index_as_skipped() {
        let mut accounts: Vec<Value> = vec![];
        let result =
            merge_import_records(&mut accounts, r#"[{ "access_token": "t" }]"#, &[5]).unwrap();
        assert_eq!(result.imported, 0);
        assert_eq!(result.skipped, 1);
    }

    #[test]
    fn merge_deduplicates_within_file() {
        let mut accounts: Vec<Value> = vec![];
        let text = r#"[
            { "id": "f1", "uid": "u9", "access_token": "t1" },
            { "id": "f2", "uid": "u9", "access_token": "t2" }
        ]"#;
        let result = merge_import_records(&mut accounts, text, &[0, 1]).unwrap();
        assert_eq!(result.imported, 2);
        assert_eq!(result.overwritten, 1);
        assert_eq!(accounts.len(), 1, "文件内重复 uid 也不得产生重复账号");
        assert_eq!(accounts[0]["id"], "f2");
    }

    #[test]
    fn export_requires_selection() {
        let accounts = vec![record("a1", Some("u1"), "甲", None, true)];
        assert!(select_export_records(&accounts, &[]).is_err());
        assert!(select_export_records(&accounts, &["missing".to_string()]).is_err());
    }

    #[test]
    fn region_scoped_export_requires_selection() {
        // 空选择在加载账号库前即被拒绝；两版行为一致，且不触碰写路径。
        assert!(export_accounts_for(Region::Global, &[]).is_err());
        assert!(export_accounts_for(Region::Cn, &[]).is_err());
        assert!(export_accounts_to_path_for(Region::Global, &[], "/tmp/out.json").is_err());
    }

    #[test]
    fn export_returns_full_records_with_tokens() {
        let accounts = vec![
            record("a1", Some("u1"), "甲", Some("a@b.c"), true),
            record("a2", Some("u2"), "乙", None, true),
        ];
        let exported =
            select_export_records(&accounts, &["a1".to_string(), "missing".to_string()]).unwrap();
        assert_eq!(exported.len(), 1);
        assert_eq!(exported[0]["id"], "a1");
        assert_eq!(exported[0]["access_token"], "token-a1");
    }

    #[test]
    fn exported_records_roundtrip_import() {
        let accounts = vec![json!({
            "id": "a1",
            "uid": "u1",
            "nickname": "甲",
            "email": "a@b.c",
            "access_token": "tok-1",
            "refresh_token": "ref-1",
            "auth_raw": { "k": "v" },
            "createdAt": 1,
        })];
        let exported = select_export_records(&accounts, &["a1".to_string()]).unwrap();
        let text = serde_json::to_string(&exported).unwrap();
        let mut target: Vec<Value> = vec![];
        let result = merge_import_records(&mut target, &text, &[0]).unwrap();
        assert_eq!(result.imported, 1);
        assert_eq!(target.len(), 1);
        assert_eq!(
            target[0]["access_token"], "tok-1",
            "round-trip 保留 access_token"
        );
        assert_eq!(target[0]["refresh_token"], "ref-1");
        assert_eq!(target[0]["auth_raw"]["k"], "v");
    }

    #[test]
    fn preview_exposes_only_desensitized_fields() {
        let text = r#"[{
            "id": "a1",
            "uid": "u1",
            "nickname": "小明",
            "email": "x@y.z",
            "access_token": "SECRET"
        }]"#;
        let preview = preview_accounts(text).unwrap();
        assert_eq!(preview["total"], 1);
        assert_eq!(preview["accounts"][0]["index"], 0);
        assert_eq!(preview["accounts"][0]["uid"], "u1");
        assert_eq!(preview["accounts"][0]["nickname"], "小明");
        assert_eq!(preview["accounts"][0]["email"], "x@y.z");
        assert_eq!(preview["accounts"][0]["hasToken"], true);
        assert!(
            preview["accounts"][0].get("access_token").is_none(),
            "预览不得泄露 token"
        );
    }

    #[test]
    fn export_file_name_rejects_traversal_and_non_json() {
        assert!(validate_export_file_name("../../etc/passwd.json").is_err());
        assert!(validate_export_file_name("a/b.json").is_err());
        assert!(validate_export_file_name("a\\b.json").is_err());
        assert!(validate_export_file_name("out.txt").is_err());
        assert!(validate_export_file_name("").is_err());
        assert!(validate_export_file_name("buddy-switch-accounts-2026-08-21.json").is_ok());
    }

    #[test]
    fn export_path_validation() {
        // 用当前平台的绝对路径构造用例（Windows 上 `/tmp/...` 并非绝对路径）。
        let dir = std::env::temp_dir();
        assert!(
            validate_export_path("relative/out.json").is_err(),
            "必须绝对路径"
        );
        assert!(
            validate_export_path(&dir.join("out.txt").to_string_lossy()).is_err(),
            "必须 .json"
        );
        assert!(
            validate_export_path(&dir.join("out.JSON").to_string_lossy()).is_ok(),
            "扩展名不区分大小写"
        );
        assert!(validate_export_path(&dir.join("out.json").to_string_lossy()).is_ok());
    }

    #[test]
    fn export_accounts_to_file_writes_json_with_tokens() {
        let dir = std::env::temp_dir();
        let file_name = format!(
            "buddy-switch-accounts-test-{}.json",
            uuid::Uuid::new_v4().simple()
        );
        let records = vec![record("a1", Some("u1"), "甲", None, true)];
        let path = write_records_to_file(&dir, &records, &file_name).unwrap();
        let written: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(written.len(), 1);
        assert_eq!(written[0]["access_token"], "token-a1", "导出文件保留 token");
    }

    #[test]
    fn write_records_to_file_rejects_bad_name() {
        let dir = std::env::temp_dir();
        let records = vec![record("a1", Some("u1"), "甲", None, true)];
        assert!(write_records_to_file(&dir, &records, "../escape.json").is_err());
        assert!(write_records_to_file(&dir, &records, "no-ext").is_err());
    }

    /// 信封 token 的账号**有**凭据：预览不得报 `hasToken:false`，
    /// 且展示字段必须标量化（否则预览弹窗 React #31 白屏）。
    #[test]
    fn preview_scalarizes_envelope_and_reports_has_token() {
        let envelope = json!({"$wbEncrypted": 1, "envelope": "…"});
        let text = json!([{
            "uid": "u-enc",
            "nickname": envelope,
            "email": "enc@example.com",
            "access_token": envelope,
        }])
        .to_string();

        let preview = preview_accounts(&text).expect("信封账号应能生成预览");
        let item = &preview["accounts"][0];
        assert_eq!(item["hasToken"], json!(true), "信封算有凭据：{item}");
        assert!(item["nickname"].is_null(), "信封昵称必须折叠为 null：{item}");
        assert_eq!(item["uid"], "u-enc");
        assert_eq!(item["email"], "enc@example.com");
    }

    /// 信封 token 的导出记录必须能导回来。修复前 `get_str` 取不到值 → `Skipped`，
    /// 导出的账号静默丢失。
    #[test]
    fn merge_imports_envelope_token_record_instead_of_skipping() {
        let envelope = json!({"$wbEncrypted": 1, "envelope": "…"});
        let mut accounts: Vec<Value> = vec![];
        let outcome = merge_import_record(
            &mut accounts,
            &json!({
                "id": "a-enc",
                "uid": "u-enc",
                "access_token": envelope,
                "refresh_token": envelope,
            }),
        );

        assert_eq!(outcome, MergeOutcome::Appended, "信封记录不得被跳过");
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0]["access_token"], envelope, "信封原样入库");
    }

    /// 既有语义不能被信封改造破坏：空串 / 缺失 token 仍然跳过。
    #[test]
    fn merge_still_skips_blank_or_missing_token() {
        let mut accounts: Vec<Value> = vec![];
        for item in [
            json!({"uid": "u1", "access_token": ""}),
            json!({"uid": "u2"}),
            json!({"uid": "u3", "access_token": "   "}),
        ] {
            assert_eq!(
                merge_import_record(&mut accounts, &item),
                MergeOutcome::Skipped,
                "空/缺失凭据必须跳过：{item}"
            );
        }
        assert!(accounts.is_empty());
    }

    /// 账号文件导入必须拒绝把 Global 记录写进 CN 账号库；这是 UI 传错 region
    /// 或用户选择了另一版本导出文件时的最后一道纯逻辑防线。
    #[test]
    fn region_mismatch_is_rejected_before_file_import() {
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

        let records = vec![global.clone(), cn.clone()];
        assert!(validate_records_for_region(Region::Global, &records, &[0]).is_ok());
        let err = validate_records_for_region(Region::Cn, &records, &[0])
            .expect_err("Global 记录不得导入 CN");
        assert!(err.contains("账号库"), "错误应能指导用户：{err}");
        assert!(validate_records_for_region(Region::Cn, &records, &[1]).is_ok());
    }
}
