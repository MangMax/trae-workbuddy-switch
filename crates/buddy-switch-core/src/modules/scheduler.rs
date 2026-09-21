//! 六类定时任务的**运行时**调度：派发、按点循环、启动补跑。
//!
//! 与 [`super::schedule`] 的分工：`schedule` 只管配置与「下一次几点触发」这类**纯函数**
//! （可在无 tokio 运行时的单测里验证）；本模块管**真的去跑**。
//!
//! ⚠️ 历史缺陷（本模块存在的原因）：排程循环此前**只写在 `buddy-switch-server` 二进制里**，
//! 而桌面端（`src-tauri`）的 `spawn_background_loops` 是四个**写死周期**的循环（签到 30 分钟 /
//! 旅行 30 分钟 / 领取 15 分钟 / 保活每天一次）。于是设置页「定时任务排程」的 13 个控件
//! （六类开关 + 六份小时表 + 活跃上报次数）在桌面端**全部空转**——最典型的是「活跃地图」
//! 从未执行，用户在官网对照连登热力图发现始终没点亮。
//!
//! 现在桌面端与服务端**共用这一份**实现：两端只负责 `spawn`，排程语义只有一份，不会漂移。

use serde_json::{json, Value};
use std::time::Duration;

use super::{
    activity, cat, checkin, config, refresh,
    region::Region,
    schedule::{self, ScheduleConfig, ScheduleTask},
    school, travel,
};

/// 任务被禁用（`hours` 为空）时重查配置的间隔：1 分钟。
///
/// 禁用态不能退出循环（否则用户后来打开开关就再也不会生效），只需低频重查。
const DISABLED_RECHECK_MS: u64 = 60_000;

/// 哪些任务在**进程启动时补跑一轮**。
///
/// 只补「今天该做但还没做」的三类：签到核验、旅行派出+领取、保活。活跃上报 / 开学季 /
/// 夜猫子不在其中——它们是「每天至多一次」的积分动作，交给排程小时表即可，启动就跑会
/// 让「我明明设了 10 点」的语义失效。
const STARTUP_TASKS: [ScheduleTask; 3] = [
    ScheduleTask::Checkin,
    ScheduleTask::Travel,
    ScheduleTask::Keepalive,
];

/// 启动补跑的任务清单（**纯函数**）：受各自排程开关约束，关掉的任务一次都不跑。
///
/// 为什么必须显式按开关过滤：桌面端此前的启动补跑是**无条件**的，于是「关闭某类任务」
/// 的开关在桌面端形同虚设（关了照样跑）。抽成纯函数后可用单测钉住这一点。
pub fn startup_tasks(cfg: &ScheduleConfig) -> Vec<ScheduleTask> {
    STARTUP_TASKS
        .into_iter()
        .filter(|task| task.enabled(cfg))
        .collect()
}

/// 派发一类定时任务并返回它的执行结果。
///
/// 返回值供「立即执行」这类手动入口展示：各业务模块自身的返回原样内嵌，便于定位到具体
/// region / 账号（例如活跃上报会带回每个账号的 `reported` 与 `streakDays`）。
///
/// CN / Global 的 region 隔离在此收敛：签到 / 旅行 / 开学季 / 夜猫子只有 CN 有对应上游
/// （Global 侧由各模块自行返回结构化 `unsupported`，独立降级）；活跃上报与保活按 region
/// 各自执行。
pub async fn run_scheduled_task(task: ScheduleTask) -> Value {
    match task {
        ScheduleTask::Checkin => {
            let mut regions = Vec::new();
            for region in Region::all() {
                regions.push(
                    checkin::run_checkin_cycle_for(
                        region,
                        checkin::CheckinCycleMode::PeriodicRecovery,
                    )
                    .await,
                );
            }
            json!({ "task": task.as_str(), "regions": regions })
        }
        // 一趟派出 + 一趟领奖闭环。
        ScheduleTask::Travel => {
            let dispatched = travel::run_travel_cycle().await;
            let claimed = travel::run_travel_claim_cycle().await;
            json!({ "task": task.as_str(), "dispatched": dispatched, "claimed": claimed })
        }
        ScheduleTask::Activity => {
            let mut regions = Vec::new();
            for region in Region::all() {
                regions.push(activity::run_activity_cycle_for(region).await);
            }
            json!({ "task": task.as_str(), "regions": regions })
        }
        ScheduleTask::Keepalive => {
            let mut regions = Vec::new();
            for region in Region::all() {
                regions.push(refresh::run_keepalive_cycle_for(region).await);
            }
            json!({ "task": task.as_str(), "regions": regions })
        }
        ScheduleTask::School => {
            json!({ "task": task.as_str(), "result": school::run_school_cycle_for(Region::Cn).await })
        }
        ScheduleTask::Cat => {
            json!({ "task": task.as_str(), "result": cat::run_cat_cycle_for(Region::Cn).await })
        }
    }
}

/// 某一类任务的排程循环：**永不返回**，由调用方 `spawn` 到后台。
///
/// 每轮**重新读取配置**并重算自己的 `next_fire`，因此运行期间改动小时表或开关、
/// **下一轮即生效**，无需重启应用。任务被禁用（`hours` 为空）时不派发，只按 1 分钟重查。
pub async fn schedule_loop(task: ScheduleTask) {
    loop {
        let cfg = schedule::load_schedule_config();
        let now = config::now_ms();
        let at = schedule::next_fire(task.hours(&cfg), now);
        let wait_ms = match at {
            Some(at) => (at - now).max(0) as u64,
            // 任务被禁用：1 分钟后重查配置，避免无谓空转。
            None => DISABLED_RECHECK_MS,
        };
        tokio::time::sleep(Duration::from_millis(wait_ms)).await;
        if at.is_none() {
            continue;
        }
        let _ = run_scheduled_task(task).await;
    }
}

/// 启动补跑：进程刚起来时先跑一轮「今天该做但还没做」的事，
/// 避免「排程定在 10 点、但应用 11 点才打开」导致当天整天不跑。
///
/// 每类是否补跑**受它自己的排程开关约束**：关掉的任务一次都不跑。
pub async fn run_startup_maintenance() {
    if let Err(error) = config::compact_checkin_logs() {
        eprintln!("[签到] 历史日志整理失败: {error}");
    }
    let cfg = schedule::load_schedule_config();
    for task in startup_tasks(&cfg) {
        let _ = run_scheduled_task(task).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 启动补跑必须**逐类**受自己的排程开关约束。
    ///
    /// 可证伪性：若把 `startup_tasks` 的开关过滤去掉（回到「无条件补跑」的旧行为），
    /// 第二条断言（全关 → 空）会红；若改成「任一开关关闭就全部不补跑」，
    /// 第三条断言（只关 travel → 仍补 checkin/keepalive）会红。
    #[test]
    fn startup_tasks_are_gated_per_task_by_its_own_switch() {
        let cfg = schedule::default_schedule();
        assert_eq!(
            startup_tasks(&cfg),
            vec![
                ScheduleTask::Checkin,
                ScheduleTask::Travel,
                ScheduleTask::Keepalive,
            ],
            "默认全启用时，启动补跑应恰好覆盖这三类"
        );

        let mut all_off = cfg.clone();
        all_off.checkin_enabled = false;
        all_off.travel_enabled = false;
        all_off.keepalive_enabled = false;
        assert!(
            startup_tasks(&all_off).is_empty(),
            "三类全关时启动补跑必须为空（否则开关形同虚设）"
        );

        let mut travel_off = cfg.clone();
        travel_off.travel_enabled = false;
        assert_eq!(
            startup_tasks(&travel_off),
            vec![ScheduleTask::Checkin, ScheduleTask::Keepalive],
            "gating 必须是逐类的：关 travel 不影响另外两类"
        );

        // 活跃上报 / 开学季 / 夜猫子**不在**启动补跑里（交给排程小时表）。
        let only_others = schedule::ScheduleConfig {
            checkin_enabled: false,
            travel_enabled: false,
            keepalive_enabled: false,
            ..cfg.clone()
        };
        assert!(startup_tasks(&only_others).is_empty());
    }

    /// 启动补跑的三类必须**恰好**是签到 / 旅行 / 保活；新增第四类进来必须有意为之。
    #[test]
    fn startup_task_set_is_exactly_three_task_families() {
        let labels: Vec<&str> = STARTUP_TASKS.iter().map(|t| t.as_str()).collect();
        assert_eq!(labels, vec!["checkin", "travel", "keepalive"]);
    }
}
