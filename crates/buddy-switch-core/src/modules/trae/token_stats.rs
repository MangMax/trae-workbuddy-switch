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
        })
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

/// Trae Token 统计（`days` 为统计窗口天数，`None` 表示全部历史）。
///
/// 返回体形状与 WorkBuddy 的 `get_statistics` **刻意不同**：那边是「多源 + 会话/项目维度」，
/// 这边只有网关一个源，且天然带账号维度。强行对齐字段只会得到一堆恒为 0 的键。
pub fn get_statistics(days: Option<i64>) -> Value {
    let (entries, parse_errors) = load_entries();
    let cutoff = cutoff_ms(days);

    let mut summary = Bucket::default();
    let mut by_model: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_account: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_date: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_hour: BTreeMap<String, Bucket> = BTreeMap::new();
    let mut by_status: BTreeMap<String, u64> = BTreeMap::new();
    let mut coverage_start: Option<i64> = None;
    let mut coverage_end: Option<i64> = None;
    let mut kept = 0usize;

    for entry in entries.iter().filter(|entry| {
        cutoff.map(|minimum| entry.ts >= minimum).unwrap_or(true)
    }) {
        kept += 1;
        summary.add(entry);
        by_model
            .entry(if entry.model.is_empty() {
                "（未指定）".to_string()
            } else {
                entry.model.clone()
            })
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
        // 与 WorkBuddy 侧同名，便于共用「数据来源说明」组件。
        "filesScanned": if kept > 0 { 1 } else { 0 },
        "parseErrors": parse_errors,
        "coverageStartAt": coverage_start,
        "coverageEndAt": coverage_end,
        "note": "只统计经过本机 Trae 网关的调用；直接在 Trae IDE 里对话不产生记录。",
    })
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
        }
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
