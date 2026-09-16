//! CST 时间助手 —— **兼容层**。唯一实现在 [`buddy_switch_core::modules::cst`]。
//!
//! 历史：网关曾先行自实现一套 CST 助手（`next_day_4am` 供 402 硬冷却、`next_midnight`
//! 供内容拦截降级门）；随后 core 因「签到跨天 / 夜猫窗口 / 冷却跨天」也需要同一口径而
//! 复制了一份。两份实现并存意味着同一个时区口径有两种写法，**任何一处修正都可能只改到
//! 一边**（漂移风险）。现收敛为「core 为唯一来源、网关只再导出」：改 core 等于同时改网关。
//!
//! 依赖方向保持不变（gateway → core），未引入任何反向依赖。
//!
//! 本模块的测试刻意**只保留网关自身依赖的三条语义**（`next_day_4am` / `next_midnight` /
//! `in_night_window`），完整边界矩阵由 core 的 `cst` 模块覆盖——同一纯函数不需要在两个
//! crate 里各维护一份边界用例。

pub use buddy_switch_core::modules::cst::{
    cst_date, cst_date_str, cst_datetime, cst_hour, cst_hour_ms, cst_naive, cst_offset,
    cst_yesterday_str, in_night_window, next_day_4am, next_midnight, CST_OFFSET_SECONDS,
};

/// 当前时刻的毫秒时间戳（UTC 纪元）。转发到 core 的时钟访问器，保持单一来源。
pub fn now_ms() -> i64 {
    buddy_switch_core::modules::config::now_ms()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, TimeZone};

    /// 构造 CST 某时刻的毫秒时间戳（core 的测试助手是 crate 私有，此处就地实现）。
    fn cst_ms(hour: u32, minute: u32) -> i64 {
        let naive = NaiveDate::from_ymd_opt(2026, 9, 16)
            .expect("合法日期")
            .and_hms_opt(hour, minute, 0)
            .expect("合法时刻");
        cst_offset()
            .from_local_datetime(&naive)
            .single()
            .expect("无歧义 CST 时刻")
            .timestamp_millis()
    }

    #[test]
    fn next_day_4am_anchor_is_stable_for_hard_cooldown() {
        // `hour < 4` 严格小于：03:30 → 当天 04:00；04:00 整点 → 次日 04:00。
        assert_eq!(
            next_day_4am(cst_ms(3, 30)),
            cst_ms(4, 0),
            "03:30 应锚定当天 04:00"
        );
        assert_eq!(
            next_day_4am(cst_ms(4, 0)),
            cst_ms(4, 0) + 24 * 3600 * 1000,
            "04:00 整点应锚定次日 04:00"
        );
        assert_eq!(
            next_day_4am(cst_ms(14, 26)),
            cst_ms(4, 0) + 24 * 3600 * 1000,
            "14:26 应锚定次日 04:00"
        );
    }

    #[test]
    fn next_midnight_anchor_ends_the_same_cst_day() {
        assert_eq!(
            next_midnight(cst_ms(14, 26)),
            cst_ms(0, 0) + 24 * 3600 * 1000
        );
        assert_eq!(next_midnight(cst_ms(0, 1)), cst_ms(0, 0) + 24 * 3600 * 1000);
    }

    #[test]
    fn night_window_is_23_to_08_cst() {
        assert!(in_night_window(cst_ms(23, 0)));
        assert!(in_night_window(cst_ms(7, 59)));
        assert!(!in_night_window(cst_ms(8, 0)));
        assert!(!in_night_window(cst_ms(22, 59)));
        assert!(!in_night_window(cst_ms(12, 0)));
    }

    #[test]
    fn now_ms_returns_millisecond_epoch() {
        let value = now_ms();
        // 2020-01-01 与 2100-01-01 的毫秒纪元区间——足以证明单位是毫秒而非秒。
        assert!(value > 1_577_836_800_000, "过小，可能不是毫秒: {value}");
        assert!(value < 4_102_444_800_000, "过大，可能不是毫秒: {value}");
    }
}
