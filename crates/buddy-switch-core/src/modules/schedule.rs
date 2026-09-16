//! 定时任务排程配置与「按点独立排程」纯函数。
//!
//! 对照参考实现 `internal/config/schedule.go` + `internal/scheduler/scheduler.go`：
//! 六类任务（签到 / 旅行 / 活跃上报 / 保活 / 开学季 / 夜猫子）各自独立时点、独立开关，
//! 同一整点上的多类任务并行执行。
//!
//! **默认值预置 + 缺失键保留**：`ScheduleConfig` 先取默认值再被输入覆盖，因此键缺席
//! （或为 `null`）时保留默认——尤其 `*_enabled` 缺省必须为 `true`，否则老配置会因字段
//! 缺失被静默关闭。禁用一律走 `*_enabled=false`，不用哨兵小时（`[-1]` 之类）表意。
//!
//! 持久化到 `~/.buddy-switch/schedule_config.json`；读写沿用 config 模块的
//! `store_dir()` / `atomic_write` 约定。

use serde_json::{json, Value};
use std::path::PathBuf;

use crate::modules::config::{atomic_write, store_dir};

/// 六类定时任务的类型标识。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleTask {
    Checkin,
    Travel,
    Activity,
    Keepalive,
    School,
    Cat,
}

impl ScheduleTask {
    /// 稳定标识（用于日志 / 汇总）。
    pub fn as_str(self) -> &'static str {
        match self {
            ScheduleTask::Checkin => "checkin",
            ScheduleTask::Travel => "travel",
            ScheduleTask::Activity => "activity",
            ScheduleTask::Keepalive => "keepalive",
            ScheduleTask::School => "school",
            ScheduleTask::Cat => "cat",
        }
    }

    /// 该任务是否在配置中被显式启用。
    pub fn enabled(self, cfg: &ScheduleConfig) -> bool {
        match self {
            ScheduleTask::Checkin => cfg.checkin_enabled,
            ScheduleTask::Travel => cfg.travel_enabled,
            ScheduleTask::Activity => cfg.activity_enabled,
            ScheduleTask::Keepalive => cfg.keepalive_enabled,
            ScheduleTask::School => cfg.school_enabled,
            ScheduleTask::Cat => cfg.cat_enabled,
        }
    }

    /// 该任务配置的小时列表；任务被禁用时返回空切片（`next_fire` 对空列表返回 `None`）。
    pub fn hours<'a>(self, cfg: &'a ScheduleConfig) -> &'a [u32] {
        if !self.enabled(cfg) {
            return &[];
        }
        match self {
            ScheduleTask::Checkin => &cfg.checkin_hours,
            ScheduleTask::Travel => &cfg.travel_hours,
            ScheduleTask::Activity => &cfg.activity_hours,
            ScheduleTask::Keepalive => &cfg.keepalive_hours,
            ScheduleTask::School => &cfg.school_hours,
            ScheduleTask::Cat => &cfg.cat_hours,
        }
    }

    /// 全部六类任务（固定顺序，供遍历用）。
    pub fn all() -> [ScheduleTask; 6] {
        [
            ScheduleTask::Checkin,
            ScheduleTask::Travel,
            ScheduleTask::Activity,
            ScheduleTask::Keepalive,
            ScheduleTask::School,
            ScheduleTask::Cat,
        ]
    }
}

/// 排程配置（`~/.buddy-switch/schedule_config.json`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleConfig {
    pub checkin_hours: Vec<u32>,
    pub travel_hours: Vec<u32>,
    pub activity_hours: Vec<u32>,
    pub keepalive_hours: Vec<u32>,
    pub school_hours: Vec<u32>,
    pub cat_hours: Vec<u32>,
    pub checkin_enabled: bool,
    pub travel_enabled: bool,
    pub activity_enabled: bool,
    pub keepalive_enabled: bool,
    pub school_enabled: bool,
    pub cat_enabled: bool,
    pub activity_report_count: u32,
}

/// 默认排程：与参考实现 `DefaultSchedule` 逐字一致。
pub fn default_schedule() -> ScheduleConfig {
    ScheduleConfig {
        checkin_hours: vec![9, 21],
        travel_hours: vec![9, 21],
        activity_hours: vec![10],
        keepalive_hours: vec![22],
        school_hours: vec![12],
        cat_hours: vec![1],
        checkin_enabled: true,
        travel_enabled: true,
        activity_enabled: true,
        keepalive_enabled: true,
        school_enabled: true,
        cat_enabled: true,
        // 领猫前置需 5 次对话；5 连发把 chat_5 刷满。显式缺省 = 5，但 0/负数归一为 1（旧行为）。
        activity_report_count: 5,
    }
}

/// `schedule_config.json` 路径。
pub fn schedule_config_file() -> PathBuf {
    store_dir().join("schedule_config.json")
}

/// 从已解析的数组读小时；`null` / 非数组 / 空数组一律视为「未配置」→ 保留默认。
///
/// 数组内的每个值**必须是 0–23 的整数**：负数、浮点（非整数）、非数字或越界一律**报错**
/// （错误文案指向对应 `*_enabled` 开关），绝不静默丢弃——否则「全部小时非法」会得到空表，
/// 使该任务被静默关闭、永不触发。
fn hours_from(
    map: &serde_json::Map<String, Value>,
    field: &str,
    switch_key: &str,
    default: &[u32],
) -> Result<Vec<u32>, String> {
    let Some(Value::Array(items)) = map.get(field) else {
        return Ok(default.to_vec());
    };
    if items.is_empty() {
        return Ok(default.to_vec());
    }
    let mut hours = Vec::with_capacity(items.len());
    for item in items {
        // `as_i64` 仅对「可表示为 i64 的整数」返回 Some；浮点 / 非数字 / 超范围 → None → 报错。
        match item.as_i64() {
            Some(n) if (0..=23).contains(&n) => hours.push(n as u32),
            _ => return Err(hour_error(field, switch_key, item)),
        }
    }
    Ok(hours)
}

/// 小时非法的统一错误文案：指出字段与合法区间，并**指向对应的 `*_enabled` 开关**
/// （关闭任务走开关，而不是靠哨兵小时值猜）。
fn hour_error(field: &str, switch_key: &str, value: impl std::fmt::Display) -> String {
    format!("{field}: {value} 不是合法小时（0-23）；如要关闭该任务请设 schedule.{switch_key}=false")
}

/// 把输入 JSON 归一化为 [`ScheduleConfig`]（默认值预置 → 缺失键保留 → 校验小时范围）。
///
/// 失败仅有一种原因：某任务的小时非法（负数 / 浮点 / 越界 > 23）。此时错误文案**指向该任务的
/// `*_enabled` 开关**，引导用户改用开关而不是靠哨兵值猜。
pub fn schedule_from_value(input: &Value) -> Result<ScheduleConfig, String> {
    let defaults = default_schedule();
    let mut cfg = defaults.clone();
    if let Some(map) = input.as_object() {
        cfg.checkin_hours = hours_from(map, "checkin_hours", "checkin_enabled", &defaults.checkin_hours)?;
        cfg.travel_hours = hours_from(map, "travel_hours", "travel_enabled", &defaults.travel_hours)?;
        cfg.activity_hours = hours_from(map, "activity_hours", "activity_enabled", &defaults.activity_hours)?;
        cfg.keepalive_hours = hours_from(map, "keepalive_hours", "keepalive_enabled", &defaults.keepalive_hours)?;
        cfg.school_hours = hours_from(map, "school_hours", "school_enabled", &defaults.school_hours)?;
        cfg.cat_hours = hours_from(map, "cat_hours", "cat_enabled", &defaults.cat_hours)?;

        for (key, slot) in [
            ("checkin_enabled", &mut cfg.checkin_enabled),
            ("travel_enabled", &mut cfg.travel_enabled),
            ("activity_enabled", &mut cfg.activity_enabled),
            ("keepalive_enabled", &mut cfg.keepalive_enabled),
            ("school_enabled", &mut cfg.school_enabled),
            ("cat_enabled", &mut cfg.cat_enabled),
        ] {
            if let Some(value) = map.get(key).and_then(Value::as_bool) {
                *slot = value;
            }
        }

        // 0 / 负数 → 1（兼容旧行为：每号每天 1 条上报点亮连登）；缺省保留默认 5。
        if let Some(count) = map.get("activity_report_count").and_then(Value::as_i64) {
            cfg.activity_report_count = if count <= 0 { 1 } else { count as u32 };
        }
    }

    // 末道防线：`hours_from` 已逐项校验，这里再对组装后的配置整体断言（防止默认值被误配）。
    validate_hours("schedule.checkin_hours", "checkin_enabled", &cfg.checkin_hours)?;
    validate_hours("schedule.travel_hours", "travel_enabled", &cfg.travel_hours)?;
    validate_hours("schedule.activity_hours", "activity_enabled", &cfg.activity_hours)?;
    validate_hours("schedule.keepalive_hours", "keepalive_enabled", &cfg.keepalive_hours)?;
    validate_hours("schedule.school_hours", "school_enabled", &cfg.school_hours)?;
    validate_hours("schedule.cat_hours", "cat_enabled", &cfg.cat_hours)?;
    Ok(cfg)
}

/// 校验小时落在 0–23；越界报错并提示对应的 `schedule.<switchKey>=false`。
fn validate_hours(field: &str, switch_key: &str, hours: &[u32]) -> Result<(), String> {
    for &h in hours {
        if h > 23 {
            return Err(hour_error(field, switch_key, h));
        }
    }
    Ok(())
}

/// 把 [`ScheduleConfig`] 序列化为落盘 / 接口返回用的 JSON（只含已知字段）。
pub fn schedule_to_value(cfg: &ScheduleConfig) -> Value {
    json!({
        "checkin_hours": cfg.checkin_hours.clone(),
        "travel_hours": cfg.travel_hours.clone(),
        "activity_hours": cfg.activity_hours.clone(),
        "keepalive_hours": cfg.keepalive_hours.clone(),
        "school_hours": cfg.school_hours.clone(),
        "cat_hours": cfg.cat_hours.clone(),
        "checkin_enabled": cfg.checkin_enabled,
        "travel_enabled": cfg.travel_enabled,
        "activity_enabled": cfg.activity_enabled,
        "keepalive_enabled": cfg.keepalive_enabled,
        "school_enabled": cfg.school_enabled,
        "cat_enabled": cfg.cat_enabled,
        "activity_report_count": cfg.activity_report_count,
    })
}

/// 读取排程配置；文件缺失 / 损坏 / 含非法小时时回落到默认，保证调度器始终可用。
pub fn load_schedule_config() -> ScheduleConfig {
    let file = schedule_config_file();
    if file.exists() {
        if let Ok(text) = std::fs::read_to_string(&file) {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if let Ok(cfg) = schedule_from_value(&value) {
                    return cfg;
                }
            }
        }
    }
    default_schedule()
}

/// 保存排程配置：先校验归一化，成功才落盘（只保留已知字段）；返回归一化后的配置。
pub fn save_schedule_config(input: &Value) -> Result<ScheduleConfig, String> {
    let cfg = schedule_from_value(input)?;
    std::fs::create_dir_all(store_dir()).map_err(|e| e.to_string())?;
    let content = serde_json::to_string_pretty(&schedule_to_value(&cfg)).unwrap_or_default();
    atomic_write(&schedule_config_file(), &content).map_err(|e| e.to_string())?;
    Ok(cfg)
}

/// 在给定小时列表里挑「**现在之后**最近的整点」的毫秒时间戳。
///
/// - 今天该整点仍晚于 `now_ms` → 取今天；
/// - 该整点已过（或恰等于 `now_ms`）→ 取次日同一整点；
/// - 跨月 / 跨年 / 闰日由 `NaiveDate::succ_opt` 处理，不做手工进位；
/// - `hours` 为空（任务禁用）→ `None`。
pub fn next_fire(hours: &[u32], now_ms: i64) -> Option<i64> {
    let today = crate::modules::cst::cst_date(now_ms);
    let mut earliest: Option<i64> = None;
    for &h in hours {
        let mut candidate = crate::modules::cst::cst_hour_ms(today, h);
        if !matches!(candidate, Some(ts) if ts > now_ms) {
            candidate = today
                .succ_opt()
                .and_then(|next_day| crate::modules::cst::cst_hour_ms(next_day, h));
        }
        if let Some(ts) = candidate {
            earliest = Some(match earliest {
                Some(existing) => existing.min(ts),
                None => ts,
            });
        }
    }
    earliest
}

/// 汇总六类时点，返回「最近一次唤醒时刻」与该时刻到期的任务列表。
///
/// 同一整点上的多类任务一并返回（调用方据此**并行**派发，互不阻塞）；全部任务被
/// 显式禁用时返回 `(None, [])`。
pub fn next_wake(cfg: &ScheduleConfig, now_ms: i64) -> (Option<i64>, Vec<ScheduleTask>) {
    let slots: Vec<(i64, ScheduleTask)> = ScheduleTask::all()
        .into_iter()
        .filter_map(|task| next_fire(task.hours(cfg), now_ms).map(|at| (at, task)))
        .collect();
    let Some(earliest) = slots.iter().map(|(at, _)| *at).min() else {
        return (None, Vec::new());
    };
    let kinds = slots
        .into_iter()
        .filter(|(at, _)| *at == earliest)
        .map(|(_, task)| task)
        .collect();
    (Some(earliest), kinds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::cst::cst_ms;

    fn hours_of(kinds: &[ScheduleTask]) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = kinds.iter().map(|k| k.as_str()).collect();
        names.sort_unstable();
        names
    }

    #[test]
    fn defaults_match_reference_and_enable_all_tasks() {
        let cfg = default_schedule();
        assert_eq!(cfg.checkin_hours, vec![9, 21]);
        assert_eq!(cfg.travel_hours, vec![9, 21]);
        assert_eq!(cfg.activity_hours, vec![10]);
        assert_eq!(cfg.keepalive_hours, vec![22]);
        assert_eq!(cfg.school_hours, vec![12]);
        assert_eq!(cfg.cat_hours, vec![1]);
        assert_eq!(cfg.activity_report_count, 5);
        for task in ScheduleTask::all() {
            assert!(task.enabled(&cfg), "默认应全部启用: {}", task.as_str());
        }
    }

    #[test]
    fn missing_keys_keep_defaults_especially_enabled_flags() {
        // 空对象：缺失键必须保留默认（尤其 *_enabled 不能因缺失变 false）。
        let cfg = schedule_from_value(&json!({})).expect("empty config is valid");
        assert_eq!(cfg, default_schedule());

        // 只给一个无关键，其余仍全部默认启用。
        let cfg = schedule_from_value(&json!({"activity_report_count": 2})).unwrap();
        assert_eq!(cfg.activity_report_count, 2);
        assert!(cfg.checkin_enabled && cfg.travel_enabled && cfg.school_enabled);
    }

    #[test]
    fn empty_and_null_hours_fall_back_to_defaults() {
        let cfg = schedule_from_value(&json!({
            "checkin_hours": [],
            "travel_hours": null,
            "school_hours": [],
        }))
        .unwrap();
        assert_eq!(cfg.checkin_hours, vec![9, 21]);
        assert_eq!(cfg.travel_hours, vec![9, 21]);
        assert_eq!(cfg.school_hours, vec![12]);
    }

    #[test]
    fn explicit_false_wins_for_every_switch() {
        let cfg = schedule_from_value(&json!({
            "checkin_enabled": false,
            "travel_enabled": false,
            "activity_enabled": false,
            "keepalive_enabled": false,
            "school_enabled": false,
            "cat_enabled": false,
        }))
        .unwrap();
        for task in ScheduleTask::all() {
            assert!(!task.enabled(&cfg), "显式 false 必须生效: {}", task.as_str());
            assert!(task.hours(&cfg).is_empty());
        }
    }

    #[test]
    fn out_of_range_hour_errors_and_points_to_enabled_switch() {
        let err = schedule_from_value(&json!({"checkin_hours": [9, 24]}))
            .expect_err("24 必须报错");
        assert!(err.contains("checkin_hours"), "{err}");
        assert!(
            err.contains("checkin_enabled"),
            "错误文案必须指向 *_enabled 开关: {err}"
        );

        let err = schedule_from_value(&json!({"cat_hours": [99]})).unwrap_err();
        assert!(err.contains("cat_enabled"), "{err}");
    }

    #[test]
    fn activity_report_count_non_positive_normalizes_to_one() {
        assert_eq!(
            schedule_from_value(&json!({"activity_report_count": 0}))
                .unwrap()
                .activity_report_count,
            1
        );
        assert_eq!(
            schedule_from_value(&json!({"activity_report_count": -3}))
                .unwrap()
                .activity_report_count,
            1
        );
        assert_eq!(
            schedule_from_value(&json!({"activity_report_count": 8}))
                .unwrap()
                .activity_report_count,
            8
        );
        // 缺省保留默认 5。
        assert_eq!(
            schedule_from_value(&json!({})).unwrap().activity_report_count,
            5
        );
    }

    #[test]
    fn next_fire_picks_nearest_upcoming_hour_today() {
        // 今天 08:00 → [9,21] 今天 09:00。
        let now = cst_ms(2026, 9, 16, 8, 0);
        assert_eq!(next_fire(&[9, 21], now), Some(cst_ms(2026, 9, 16, 9, 0)));
    }

    #[test]
    fn next_fire_wraps_to_next_day_when_all_points_passed() {
        // 今天 22:00 → 今天时点（9/21）全过 → 次日 09:00。
        let now = cst_ms(2026, 9, 16, 22, 0);
        assert_eq!(next_fire(&[9, 21], now), Some(cst_ms(2026, 9, 17, 9, 0)));

        // 恰好等于某整点 → 不算「之后」，取次日（与参考实现 !After 语义一致）。
        let exact = cst_ms(2026, 9, 16, 9, 0);
        assert_eq!(next_fire(&[9], exact), Some(cst_ms(2026, 9, 17, 9, 0)));
    }

    #[test]
    fn next_fire_crosses_month_and_year_boundaries() {
        assert_eq!(
            next_fire(&[9], cst_ms(2026, 9, 30, 23, 59)),
            Some(cst_ms(2026, 10, 1, 9, 0))
        );
        assert_eq!(
            next_fire(&[1], cst_ms(2026, 12, 31, 20, 0)),
            Some(cst_ms(2027, 1, 1, 1, 0))
        );
        // 空列表（禁用）→ None。
        assert_eq!(next_fire(&[], cst_ms(2026, 9, 16, 8, 0)), None);
    }

    #[test]
    fn next_wake_groups_multiple_tasks_on_same_hour() {
        // 默认配置下 checkin/travel 都在 9 点；08:00 唤醒时二者同槽。
        let cfg = default_schedule();
        let (at, kinds) = next_wake(&cfg, cst_ms(2026, 9, 16, 8, 0));
        assert_eq!(at, Some(cst_ms(2026, 9, 16, 9, 0)));
        assert_eq!(hours_of(&kinds), vec!["checkin", "travel"]);
    }

    #[test]
    fn next_wake_returns_earliest_across_task_families() {
        // 08:00：cat=1(次日) / activity=10 / school=12 / checkin&travel=9 / keepalive=22 → 9 点。
        let cfg = default_schedule();
        let (at, kinds) = next_wake(&cfg, cst_ms(2026, 9, 16, 8, 0));
        assert_eq!(at, Some(cst_ms(2026, 9, 16, 9, 0)));
        assert_eq!(hours_of(&kinds), vec!["checkin", "travel"]);
    }

    #[test]
    fn disabling_activity_does_not_affect_school_or_cat() {
        // 六类开关互相独立：单独关闭 activity 不应影响其它任务的排程。
        let mut cfg = default_schedule();
        cfg.activity_hours = vec![10];
        let (_, all_kinds) = next_wake(&cfg, cst_ms(2026, 9, 16, 9, 30));
        assert!(all_kinds.contains(&ScheduleTask::Activity));

        cfg.activity_enabled = false;
        let (_, kinds) = next_wake(&cfg, cst_ms(2026, 9, 16, 9, 30));
        assert!(
            !kinds.contains(&ScheduleTask::Activity),
            "关闭 activity 后不应再出现该任务"
        );
        // school / cat 仍在候选（其自身开关未动）。
        assert!(ScheduleTask::School.enabled(&cfg));
        assert!(ScheduleTask::Cat.enabled(&cfg));
        assert!(ScheduleTask::School.hours(&cfg).contains(&12));
        assert_eq!(ScheduleTask::Cat.hours(&cfg), &[1]);
        // 最近的唤醒时点 = 各启用任务的最近时点，school(12) 与 cat(次日1) 均在其中。
        let (at, _) = next_wake(&cfg, cst_ms(2026, 9, 16, 9, 30));
        assert_eq!(at, Some(cst_ms(2026, 9, 16, 12, 0)));
    }

    #[test]
    fn all_tasks_disabled_yields_no_wake() {
        let mut cfg = default_schedule();
        cfg.checkin_enabled = false;
        cfg.travel_enabled = false;
        cfg.activity_enabled = false;
        cfg.keepalive_enabled = false;
        cfg.school_enabled = false;
        cfg.cat_enabled = false;
        let (at, kinds) = next_wake(&cfg, cst_ms(2026, 9, 16, 8, 0));
        assert_eq!(at, None);
        assert!(kinds.is_empty());
    }

    #[test]
    fn save_rejects_invalid_and_persists_valid() {
        // save 的纯校验路径（不触盘）：值非法直接 Err。
        assert!(save_schedule_config(&json!({"travel_hours": [30]})).is_err());
    }

    #[test]
    fn all_invalid_hours_error_instead_of_silent_empty() {
        // 负数：全部非法 → 必须报错，而不是静默丢弃得到空表（任务会被静默关闭、永不触发）。
        let err = schedule_from_value(&json!({"checkin_hours": [-1, -2]}))
            .expect_err("负数小时必须报错");
        assert!(err.contains("checkin_hours"), "{err}");
        assert!(
            err.contains("checkin_enabled"),
            "错误文案必须指向 *_enabled 开关: {err}"
        );

        // 浮点（非整数）：同样报错并指向开关。
        let err = schedule_from_value(&json!({"cat_hours": [1.5, 2.5]}))
            .expect_err("浮点小时必须报错");
        assert!(err.contains("cat_hours"), "{err}");
        assert!(err.contains("cat_enabled"), "{err}");

        // 混合（一个合法 + 一个非法）：仍报错，绝不静默丢弃非法项。
        assert!(
            schedule_from_value(&json!({"travel_hours": [9, -1]})).is_err(),
            "含非法项必须整体报错"
        );

        // 合法整数边界 0 / 23 必须通过。
        let cfg = schedule_from_value(&json!({"school_hours": [0, 23]})).unwrap();
        assert_eq!(cfg.school_hours, vec![0, 23]);
    }
}
