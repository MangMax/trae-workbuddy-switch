//! Trae Token 统计：聚合本机 Trae API 网关的请求日志。
//!
//! ## 为什么数据源只有一个，以及它带来的边界
//!
//! WorkBuddy 的 Token 统计能扫 `.workbuddy/projects/*.jsonl`——那是客户端自己落的
//! 会话记录。Trae **没有**等价的可解析落盘：TRAE SOLO 的对话历史在 IDE 的私有
//! 存储里（加密 + 二进制索引），本模块不去碰它。
//!
//! 于是 Trae 的 Token 用量只有一个诚实可得的观测点：本机网关的
//! `~/.buddy-switch/trae/api_gateway_logs.json`。这决定了三条必须写在页面上的边界：
//!
//! 1. 只统计**经过本网关**的调用——直接在 Trae IDE 里对话不产生记录；
//! 2. 网关未启用（或日志被清空）时统计恒为空；
//! 3. 上游只在流结束时回报 `token_usage`；中途断流的请求没有用量，只记 `records`。
//!
//! 与其拼一个看起来完整、实则混入猜测的「全量用量」，不如把边界讲清楚。

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::handlers;
use super::variant::TraeVariant;
use super::{account, paths};

/// 统计窗口的默认天数（与 WorkBuddy 侧一致）。
pub const DEFAULT_RANGE_DAYS: i64 = 30;

/// 一个聚合桶的累加器。
#[derive(Debug, Default, Clone)]
struct Bucket {
    input: u64,
    output: u64,
    records: u64,
    errors: u64,
    stream_requests: u64,
    latency_sum: i64,
    /// 用于 p95：只保留原始延迟（日志上限 200 条量级，直接存全量足够）。
    latencies: Vec<i64>,
}

impl Bucket {
    fn add(&mut self, entry: &LogEntry) {
        self.input += entry.prompt_tokens;
        self.output += entry.completion_tokens;
        self.records += 1;
        if entry.status >= 400 || entry.error.is_some() {
            self.errors += 1;
        }
        if entry.stream {
            self.stream_requests += 1;
        }
        if entry.latency_ms > 0 {
            self.latency_sum += entry.latency_ms;
            self.latencies.push(entry.latency_ms);
        }
    }

    fn total(&self) -> u64 {
        self.input + self.output
    }

    fn avg_latency(&self) -> i64 {
        if self.records == 0 {
            0
        } else {
            self.latency_sum / self.records as i64
        }
    }

    /// 最近秩次 p95（`ceil(0.95 * n) - 1`）。样本不足时退化为最大值。
    fn p95_latency(&self) -> i64 {
        if self.latencies.is_empty() {
            return 0;
        }
        let mut sorted = self.latencies.clone();
        sorted.sort_unstable();
        let index = ((sorted.len() as f64 * 0.95).ceil() as usize)
            .saturating_sub(1)
            .min(sorted.len() - 1);
        sorted[index]
    }

    /// 汇总字段（`total` / `input` / `output` 三件套 + 请求侧指标）。
    fn summary_value(&self) -> Value {
        json!({
            "total": self.total(),
            "input": self.input,
            "output": self.output,
            "records": self.records,
            "errors": self.errors,
            "streamRequests": self.stream_requests,
            "avgLatencyMs": self.avg_latency(),
            "p95LatencyMs": self.p95_latency(),
        })
    }

    /// 分组字段（比 summary 多一个 `key`）。
    fn group_value(&self, key: &str, extra: Value) -> Value {
        let mut value = self.summary_value();
        if let Some(object) = value.as_object_mut() {
            object.insert("key".into(), json!(key));
            if let Some(extra) = extra.as_object() {
                for (name, item) in extra {
                    object.insert(name.clone(), item.clone());
                }
            }
        }
        value
    }
}

/// 日志里一条记录（只取本模块用得到的字段）。
#[derive(Debug, Default, Clone)]
struct LogEntry {
    ts: i64,
    model: String,
    account: String,
    status: u16,
    latency_ms: i64,
    prompt_tokens: u64,
    completion_tokens: u64,
    stream: bool,
    error: Option<String>,
    /// 归属产品线（网关记录日志时写入）。**旧日志没有该键 → `None`（未标注）**。
    variant: Option<TraeVariant>,
}

impl LogEntry {
    /// 从日志条目解析；`ts` 缺失的记录直接丢弃（无法归入任何时间桶）。
    fn parse(value: &Value) -> Option<Self> {
        let ts = value.get("ts").and_then(Value::as_i64)?;
        Some(Self {
            ts,
            model: text(value, "model"),
            account: text(value, "account"),
            status: value
                .get("status")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(u16::MAX as u64) as u16,
            latency_ms: value.get("latencyMs").and_then(Value::as_i64).unwrap_or(0),
            prompt_tokens: value
                .get("promptTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            completion_tokens: value
                .get("completionTokens")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            stream: value
                .get("stream")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            error: value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|text| !text.is_empty()),
            // 变体用 as_str() 的下划线形态；未知值 → None（归入「未标注」）。
            variant: value
                .get("variant")
                .and_then(Value::as_str)
                .and_then(TraeVariant::parse),
        })
    }
}

/// Token 统计的**区域**查询范围（筛选维度，非第三种区域实体）。
///
/// **不得并入 [`TraeRegion`] / [`TraeVariant`]**：那两个是「实体」，而这里是「查询范围」，
/// 多了「未标注」（旧日志）与「全部」两档 —— 合并会让 `TraeRegion::all()` 之类的
/// 既有约定失去定义（`MEMORY.md §一`）。
///
/// ⚠️ 档名在 2026-09-21 由**产品线**改为**区域**：网关的账号池、冷却、日志归属都按
/// 区域分家（国内 / 国际是两套互不相通的账号体系），而「Trae Work / Trae CN」只是
/// **国内区域下的两条程序** —— 拿它们当统计维度会名不副实。
///
/// 历史标识 `work` / `trae_work` / `trae_cn` **一律归入国内版**（它们都是国内构建），
/// 因此历史日志与老链接既不会变成「未标注」，也不会串到国际版去。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraeTokenScope {
    /// 仅**国内版**（`variant` 属于国内区域的任何取值）。
    Cn,
    /// 仅**国际版**（另一套账号体系）。
    Global,
    /// 仅**未标注**（旧日志：无 `variant` 键）。
    Unlabeled,
    /// 全部（国内 ∪ 国际 ∪ 未标注）。
    All,
}

impl Default for TraeTokenScope {
    fn default() -> Self {
        Self::All
    }
}

impl TraeTokenScope {
    /// 从字符串解析（两条通道共用；缺省 / 未知 → [`Self::All`]）。
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            // 国内的三种写法：新标识 `cn`，以及全部历史标识（两条旧产品线都是国内构建）。
            "cn" | "work" | "trae_work" | "trae_cn" => Self::Cn,
            "global" | "intl" | "international" => Self::Global,
            "unlabeled" | "none" | "unknown" | "未标注" => Self::Unlabeled,
            _ => Self::All,
        }
    }

    /// 线上标识（前端 `TraeTokenScope = "cn" | "global" | "unlabeled" | "all"`）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cn => "cn",
            Self::Global => "global",
            Self::Unlabeled => "unlabeled",
            Self::All => "all",
        }
    }

    /// 某条日志（其变体）是否落在本范围内。
    ///
    /// 判据是**区域**而不是变体本身：国内的两条程序位共享同一批账号与同一份用量来源，
    /// 按变体分档只会把同一区域的调用劈成两半。
    fn matches(self, variant: Option<TraeVariant>) -> bool {
        match self {
            Self::All => true,
            Self::Cn => variant.map(|v| v.region() == super::region::TraeRegion::Cn) == Some(true),
            Self::Global => {
                variant.map(|v| v.region() == super::region::TraeRegion::Global) == Some(true)
            }
            Self::Unlabeled => variant.is_none(),
        }
    }
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// 读取日志文件。缺失/损坏一律视为「没有记录」，**不报错**——
/// 网关没启用过时文件本来就不存在，那不是异常。
fn load_entries() -> (Vec<LogEntry>, usize) {
    let file = paths::api_gateway_log_file();
    let Ok(text) = std::fs::read_to_string(&file) else {
        return (Vec::new(), 0);
    };
    let Ok(values) = serde_json::from_str::<Vec<Value>>(&text) else {
        return (Vec::new(), 1);
    };
    let mut parse_errors = 0;
    let entries = values
        .iter()
        .filter_map(|value| match LogEntry::parse(value) {
            Some(entry) => Some(entry),
            None => {
                parse_errors += 1;
                None
            }
        })
        .collect();
    (entries, parse_errors)
}

/// 按 `days` 过滤（`None` 表示不限；`days <= 0` 同样视为不限）。
fn cutoff_ms(days: Option<i64>) -> Option<i64> {
    let days = days?;
    if days <= 0 {
        return None;
    }
    let now = chrono::Local::now().timestamp_millis();
    Some(now - days * 24 * 60 * 60 * 1000)
}

/// 毫秒时间戳 → 本地日期 `YYYY-MM-DD`。
fn local_date(ts_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ts_ms)
        .map(|utc| utc.with_timezone(&chrono::Local).format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "未知".to_string())
}

/// 毫秒时间戳 → 本地小时 `00`..`23`。
fn local_hour(ts_ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ts_ms)
        .map(|utc| utc.with_timezone(&chrono::Local).format("%H").to_string())
        .unwrap_or_else(|| "--".to_string())
}

/// Trae Token 统计（`days` 为统计窗口天数，`None` 表示全部历史；`scope` 为变体范围）。
///
/// 返回体形状与 WorkBuddy 的 `get_statistics` **刻意不同**：那边是「多源 + 会话/项目维度」，
/// 这边只有网关一个源，且天然带账号维度。强行对齐字段只会得到一堆恒为 0 的键。
///
/// 新增（本轮）：
/// - `variantCounts: { work, cn, unlabeled, all }`：**时间窗口内**各档条数（供范围条徽标）；
/// - `modelDaily: [{ date, model, total, input, output, records }]`：按天×模型的堆叠柱数据源；
/// - `unsupported: [ { capability, label, supportedOn, reason } ]`：平台做不到的维度（置灰卡）。
///
/// `scope` 只影响**展示聚合**（summary/models/accounts/daily/hours/statuses/modelDaily），
/// **不影响** `variantCounts`（范围条要显示「切换后各档各有多少条」）。
pub fn get_statistics(days: Option<i64>, scope: TraeTokenScope) -> Value {
    let (entries, parse_errors) = load_entries();
    let cutoff = cutoff_ms(days);

    let mut summary = Bucket::default();
    let mut by_model: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_account: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_date: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_hour: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_status: BTreeMap<String, u64> = BTreeMap::new();
    let mut by_model_daily: BTreeMap<(String, String), Bucket> = BTreeMap::new();
    let mut variant_counts = VariantCounts::default();
    let mut coverage_start: Option<i64> = None;
    let mut coverage_end: Option<i64> = None;
    let mut kept = 0usize;

    for entry in entries.iter() {
        // 时间窗口过滤（变体计数也只看窗口内）。
        if cutoff.map(|minimum| entry.ts < minimum).unwrap_or(false) {
            continue;
        }
        variant_counts.bump(entry.variant);

        // 变体范围过滤：只影响下面的展示聚合，不影响 range 计数。
        if !scope.matches(entry.variant) {
            continue;
        }

        kept += 1;
        summary.add(entry);
        let model_key = if entry.model.is_empty() {
            "（未指定）".to_string()
        } else {
            entry.model.clone()
        };
        by_model.entry(model_key.clone()).or_default().add(entry);
        by_model_daily
            .entry((local_date(entry.ts), model_key))
            .or_default()
            .add(entry);
        by_account
            .entry(if entry.account.is_empty() {
                "（无账号）".to_string()
            } else {
                entry.account.clone()
            })
            .or_default()
            .add(entry);
        by_date.entry(local_date(entry.ts)).or_default().add(entry);
        by_hour.entry(local_hour(entry.ts)).or_default().add(entry);
        *by_status.entry(entry.status.to_string()).or_insert(0) += 1;
        coverage_start = Some(coverage_start.map_or(entry.ts, |current| current.min(entry.ts)));
        coverage_end = Some(coverage_end.map_or(entry.ts, |current| current.max(entry.ts)));
    }

    // 账号名回表：日志只存 uid，展示必须给人看的名字。
    let names: BTreeMap<String, String> = account::entries()
        .into_iter()
        .map(|(uid, raw)| {
            let name = if raw.name.trim().is_empty() {
                uid.clone()
            } else {
                raw.name.clone()
            };
            (uid, name)
        })
        .collect();

    let models = descending(by_model, |key, bucket| {
        bucket.group_value(key, json!(null))
    });
    let accounts = descending(by_account, |key, bucket| {
        let name = names.get(key).cloned().unwrap_or_else(|| key.to_string());
        let tail = key.get(key.len().saturating_sub(8)..).unwrap_or(key);
        bucket.group_value(key, json!({ "name": name, "shortId": tail }))
    });
    let daily = ascending(by_date, |key, bucket| bucket.group_value(key, json!(null)));
    let hours = ascending(by_hour, |key, bucket| bucket.group_value(key, json!(null)));
    let statuses: Vec<Value> = by_status
        .iter()
        .map(|(key, count)| json!({ "key": key, "records": count }))
        .collect();
    // modelDaily：按键升序（`(date, model)` 天然有序），供「按模型×按天堆叠柱」直接渲染。
    let model_daily: Vec<Value> = by_model_daily
        .iter()
        .map(|((date, model), bucket)| {
            json!({
                "date": date,
                "model": model,
                "total": bucket.total(),
                "input": bucket.input,
                "output": bucket.output,
                "records": bucket.records,
            })
        })
        .collect();

    json!({
        "source": "trae-gateway",
        "label": "Trae API 网关",
        "generatedAt": chrono::Local::now().timestamp_millis(),
        "rangeDays": days.filter(|value| *value > 0),
        "logFile": paths::api_gateway_log_file().to_string_lossy(),
        "summary": summary.summary_value(),
        "models": models,
        "accounts": accounts,
        "daily": daily,
        "hours": hours,
        "statuses": statuses,
        "modelDaily": model_daily,
        "variantCounts": {
            // 按**区域**分档（国内 / 国际）：国内两条程序位合计一档，
            // 因为账号库与用量来源在区域层面才分开（见 `TraeTokenScope` 的说明）。
            "cn": variant_counts.cn,
            "global": variant_counts.global,
            "unlabeled": variant_counts.unlabeled,
            "all": variant_counts.all,
        },
        "unsupported": unsupported_notes(),
        // 与 WorkBuddy 侧同名，便于共用「数据来源说明」组件。
        "filesScanned": if kept > 0 { 1 } else { 0 },
        "parseErrors": parse_errors,
        "coverageStartAt": coverage_start,
        "coverageEndAt": coverage_end,
        "note": "只统计经过本机 Trae 网关的调用；直接在 Trae IDE 里对话不产生记录。",
    })
}

/// 时间窗口内各变体档位的条数累加器。
#[derive(Debug, Default)]
struct VariantCounts {
    /// 国内版一档（旧日志里的 `trae_work` 与 `trae_cn` **都算它** —— 同属国内区域）。
    cn: u64,
    /// 国际版一档（另一套账号体系）。
    global: u64,
    unlabeled: u64,
    all: u64,
}

impl VariantCounts {
    /// 按**区域**分档计数。
    ///
    /// 与 [`TraeTokenScope::matches`] 同一判据：国内的两条程序位共享同一批账号与
    /// 同一份用量来源，按变体分档会把同一区域的调用劈成两半。
    fn bump(&mut self, variant: Option<TraeVariant>) {
        match variant.map(|v| v.region()) {
            Some(super::region::TraeRegion::Cn) => self.cn += 1,
            Some(super::region::TraeRegion::Global) => self.global += 1,
            None => self.unlabeled += 1,
        }
        self.all += 1;
    }
}

/// Token 统计里「平台做不到」的维度清单（形状与文案由 [`handlers::unsupported_note`] 唯一产出）。
fn unsupported_notes() -> Vec<Value> {
    vec![
        handlers::unsupported_note(
            "cache_metrics",
            "缓存读取 / 写入 / 命中率",
            "Trae 网关日志与上传链路都没有 cache 字段，上游也不回传——无从记录",
        ),
        handlers::unsupported_note(
            "project_dimension",
            "按项目维度统计",
            "网关日志的 project_id / session_id 是每请求新生成的 uuid，不对应客户端项目",
        ),
        handlers::unsupported_note(
            "session_cost",
            "调用最贵的会话",
            "无稳定会话标识，无法把多次请求归并成一个会话成本",
        ),
    ]
}

/// 按 `total` 降序（同名则按键升序，保证渲染顺序稳定）。
fn descending<F>(map: BTreeMap<String, Bucket>, build: F) -> Vec<Value>
where
    F: Fn(&str, &Bucket) -> Value,
{
    let mut items: Vec<(&String, &Bucket)> = map.iter().collect();
    items.sort_by(|left, right| {
        right
            .1
            .total()
            .cmp(&left.1.total())
            .then_with(|| left.0.cmp(right.0))
    });
    items
        .into_iter()
        .map(|(key, bucket)| build(key, bucket))
        .collect()
}

/// 按键升序（日期 / 小时天然有序）。
fn ascending<F>(map: BTreeMap<String, Bucket>, build: F) -> Vec<Value>
where
    F: Fn(&str, &Bucket) -> Value,
{
    map.iter()
        .map(|(key, bucket)| build(key, bucket))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ts: i64, model: &str, account: &str, input: u64, output: u64) -> LogEntry {
        LogEntry {
            ts,
            model: model.to_string(),
            account: account.to_string(),
            status: 200,
            latency_ms: 100,
            prompt_tokens: input,
            completion_tokens: output,
            stream: true,
            error: None,
            variant: None,
        }
    }

    #[test]
    fn scope_matches_the_expected_region_tiers() {
        assert!(TraeTokenScope::All.matches(None));
        assert!(TraeTokenScope::All.matches(Some(TraeVariant::TraeWork)));
        assert!(TraeTokenScope::All.matches(Some(TraeVariant::Trae)));
        assert!(TraeTokenScope::All.matches(Some(TraeVariant::Global)));

        // 国内版一档**同时**覆盖两条国内程序位 —— 它们共享账号库与用量来源，
        // 按变体分档会把同一区域的调用劈成两半。
        assert!(TraeTokenScope::Cn.matches(Some(TraeVariant::TraeWork)));
        assert!(TraeTokenScope::Cn.matches(Some(TraeVariant::Trae)));
        assert!(!TraeTokenScope::Cn.matches(Some(TraeVariant::Global)));
        assert!(!TraeTokenScope::Cn.matches(None));

        // 国际版是另一套账号体系，两者互不重叠。
        assert!(TraeTokenScope::Global.matches(Some(TraeVariant::Global)));
        assert!(!TraeTokenScope::Global.matches(Some(TraeVariant::TraeWork)));
        assert!(!TraeTokenScope::Global.matches(Some(TraeVariant::Trae)));

        assert!(TraeTokenScope::Unlabeled.matches(None));
        assert!(!TraeTokenScope::Unlabeled.matches(Some(TraeVariant::TraeWork)));
        assert!(!TraeTokenScope::Unlabeled.matches(Some(TraeVariant::Global)));
    }

    #[test]
    fn scope_parse_and_default_cover_all_tiers() {
        assert_eq!(TraeTokenScope::default(), TraeTokenScope::All);
        // 历史标识（两条旧产品线）**都**归国内版 —— 老链接与旧前端值不会串到国际版。
        for legacy in ["work", "trae_work", "cn", "trae_cn"] {
            assert_eq!(
                TraeTokenScope::parse(legacy),
                TraeTokenScope::Cn,
                "历史标识 {legacy} 必须归国内版"
            );
        }
        assert_eq!(TraeTokenScope::parse("global"), TraeTokenScope::Global);
        assert_eq!(TraeTokenScope::parse("unlabeled"), TraeTokenScope::Unlabeled);
        assert_eq!(TraeTokenScope::parse("all"), TraeTokenScope::All);
        // 缺失 / 未知 → All（宽容失败方向）。
        assert_eq!(TraeTokenScope::parse(""), TraeTokenScope::All);
        assert_eq!(TraeTokenScope::parse("bogus"), TraeTokenScope::All);
        assert_eq!(TraeTokenScope::Cn.as_str(), "cn");
        assert_eq!(TraeTokenScope::Global.as_str(), "global");
        assert_eq!(TraeTokenScope::Unlabeled.as_str(), "unlabeled");
    }

    #[test]
    fn variant_counts_keep_unlabeled_and_sum_to_all() {
        let mut counts = VariantCounts::default();
        counts.bump(Some(TraeVariant::TraeWork));
        counts.bump(Some(TraeVariant::Trae));
        counts.bump(Some(TraeVariant::Global));
        counts.bump(None);
        assert_eq!(counts.cn, 2, "两条国内程序位同属国内版");
        assert_eq!(counts.global, 1);
        assert_eq!(counts.unlabeled, 1, "旧日志（无 variant）必须计入「未标注」");
        assert_eq!(counts.all, 4, "All = 三档之和");
    }

    #[test]
    fn entry_parse_reads_variant_or_none() {
        let work =
            LogEntry::parse(&json!({"ts": 1, "variant": "trae_work"})).expect("解析 work");
        assert_eq!(work.variant, Some(TraeVariant::TraeWork));
        let cn = LogEntry::parse(&json!({"ts": 1, "variant": "trae_cn"})).expect("解析 cn");
        assert_eq!(cn.variant, Some(TraeVariant::Trae));
        // 旧日志无 variant → None（未标注），不得被丢弃或误判。
        let old = LogEntry::parse(&json!({"ts": 1})).expect("解析旧日志");
        assert!(old.variant.is_none());
        // 未知值 → None。
        let unknown = LogEntry::parse(&json!({"ts": 1, "variant": "doubao"})).expect("解析未知");
        assert!(unknown.variant.is_none());
    }

    #[test]
    fn get_statistics_filters_by_scope_and_counts_unlabeled() {
        let home = std::env::temp_dir().join(format!("trae-token-scope-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(home.join(".buddy-switch").join("trae")).expect("创建隔离目录");
        // 走 HomeOverrideGuard（内部已持 env 锁并在 drop 时还原），不要手动加锁。
        let _guard = crate::modules::config::HomeOverrideGuard::set(&home);

        let now = chrono::Local::now().timestamp_millis();
        let entries = json!([
            { "ts": now, "model": "m", "account": "u", "status": 200, "promptTokens": 1, "completionTokens": 1, "stream": true, "variant": "trae_work" },
            { "ts": now, "model": "m", "account": "u", "status": 200, "promptTokens": 1, "completionTokens": 1, "stream": true, "variant": "trae_cn" },
            { "ts": now, "model": "m", "account": "u", "status": 200, "promptTokens": 1, "completionTokens": 1, "stream": true, "variant": "global" },
            { "ts": now, "model": "m", "account": "u", "status": 200, "promptTokens": 1, "completionTokens": 1, "stream": true },
        ]);
        std::fs::write(paths::api_gateway_log_file(), entries.to_string()).expect("写入日志");

        let all = get_statistics(None, TraeTokenScope::All);
        assert_eq!(
            all["variantCounts"]["cn"], 2,
            "两条国内程序位同属国内版（旧日志的 trae_work / trae_cn 都算它）"
        );
        assert_eq!(all["variantCounts"]["global"], 1);
        assert_eq!(all["variantCounts"]["unlabeled"], 1);
        assert_eq!(all["variantCounts"]["all"], 4);
        assert_eq!(all["summary"]["records"], 4, "All 应计入全部三档");
        assert!(all["modelDaily"].as_array().map(|v| !v.is_empty()).unwrap_or(false));
        assert!(all["unsupported"].as_array().map(|v| !v.is_empty()).unwrap_or(false));

        let cn = get_statistics(None, TraeTokenScope::Cn);
        assert_eq!(
            cn["summary"]["records"], 2,
            "scope=cn 必须**同时**计入国内的两条程序位（按区域，不按产品线）"
        );
        // variantCounts 不受 scope 影响（范围条要显示各档总数）。
        assert_eq!(cn["variantCounts"]["all"], 4);

        let global = get_statistics(None, TraeTokenScope::Global);
        assert_eq!(
            global["summary"]["records"], 1,
            "scope=global 只应计入国际版，不得混入国内调用"
        );

        let unlabeled = get_statistics(None, TraeTokenScope::Unlabeled);
        assert_eq!(
            unlabeled["summary"]["records"], 1,
            "旧日志（无 variant）在 Unlabeled 下必须可见，不得被静默丢弃（R9）"
        );

        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn bucket_totals_add_input_and_output() {
        let mut bucket = Bucket::default();
        bucket.add(&entry(0, "m", "u", 10, 5));
        bucket.add(&entry(0, "m", "u", 1, 2));
        assert_eq!(bucket.total(), 18);
        assert_eq!(bucket.input, 11);
        assert_eq!(bucket.output, 7);
        assert_eq!(bucket.records, 2);
    }

    #[test]
    fn bucket_counts_errors_from_status_or_message() {
        let mut bucket = Bucket::default();
        let mut failed = entry(0, "m", "u", 0, 0);
        failed.status = 429;
        bucket.add(&failed);
        let mut streamed = entry(0, "m", "u", 0, 0);
        streamed.error = Some("流内错误".to_string());
        bucket.add(&streamed);
        bucket.add(&entry(0, "m", "u", 0, 0));
        assert_eq!(bucket.errors, 2);
    }

    #[test]
    fn p95_uses_nearest_rank_and_falls_back_to_max() {
        let mut bucket = Bucket::default();
        for latency in [10, 20, 30, 40, 50] {
            let mut item = entry(0, "m", "u", 0, 0);
            item.latency_ms = latency;
            bucket.add(&item);
        }
        // ceil(0.95 * 5) - 1 = 4 → 最大值。
        assert_eq!(bucket.p95_latency(), 50);

        let mut single = Bucket::default();
        single.add(&entry(0, "m", "u", 0, 0));
        assert_eq!(single.p95_latency(), 100);

        assert_eq!(Bucket::default().p95_latency(), 0);
    }

    #[test]
    fn avg_latency_is_zero_without_records() {
        assert_eq!(Bucket::default().avg_latency(), 0);
    }

    #[test]
    fn cutoff_is_disabled_for_none_and_non_positive() {
        assert!(cutoff_ms(None).is_none());
        assert!(cutoff_ms(Some(0)).is_none());
        assert!(cutoff_ms(Some(-3)).is_none());
        let cutoff = cutoff_ms(Some(7)).expect("7 天窗口应有截止时间");
        let now = chrono::Local::now().timestamp_millis();
        assert!(cutoff < now);
        assert!(now - cutoff >= 6 * 24 * 3600 * 1000);
    }

    #[test]
    fn local_date_and_hour_are_stable_strings() {
        let ts = chrono::Local::now().timestamp_millis();
        assert_eq!(local_date(ts).len(), 10, "YYYY-MM-DD");
        assert_eq!(local_hour(ts).len(), 2, "HH");
        // 极端时间戳不 panic，退化为占位符。
        assert_eq!(local_date(i64::MIN), "未知");
        assert_eq!(local_hour(i64::MIN), "--");
    }

    #[test]
    fn entry_parse_rejects_records_without_timestamp() {
        assert!(LogEntry::parse(&json!({"model": "m"})).is_none());
        let parsed = LogEntry::parse(&json!({
            "ts": 1000, "model": "glm-5.3", "account": "u1", "status": 200,
            "latencyMs": 42, "promptTokens": 7, "completionTokens": 3,
            "stream": true, "error": null,
        }))
        .expect("完整条目必须解析成功");
        assert_eq!(parsed.model, "glm-5.3");
        assert_eq!(parsed.prompt_tokens, 7);
        assert_eq!(parsed.completion_tokens, 3);
        assert!(parsed.stream);
        assert!(parsed.error.is_none());
    }

    #[test]
    fn entry_parse_tolerates_missing_optional_fields() {
        let parsed = LogEntry::parse(&json!({"ts": 1})).expect("只有 ts 也要能解析");
        assert_eq!(parsed.model, "");
        assert_eq!(parsed.status, 0);
        assert_eq!(parsed.latency_ms, 0);
        assert!(!parsed.stream);
    }

    #[test]
    fn descending_sorts_by_total_then_key() {
        let mut map = BTreeMap::new();
        let mut low = Bucket::default();
        low.add(&entry(0, "a", "u", 1, 1));
        let mut high = Bucket::default();
        high.add(&entry(0, "b", "u", 10, 10));
        let mut tie_a = Bucket::default();
        tie_a.add(&entry(0, "c", "u", 5, 5));
        let mut tie_b = Bucket::default();
        tie_b.add(&entry(0, "d", "u", 5, 5));
        map.insert("zz".to_string(), tie_a);
        map.insert("aa".to_string(), tie_b);
        map.insert("low".to_string(), low);
        map.insert("high".to_string(), high);

        let keys: Vec<String> = descending(map, |key, _| json!(key))
            .into_iter()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect();
        // total 降序；同 total 按键升序。
        assert_eq!(keys, vec!["high", "aa", "zz", "low"]);
    }
}
