//! workbuddy-switch CLI：npm 安装形态的入口。
//!
//! ```bash
//! workbuddy-switch              # 启动本地服务 + 打开浏览器 webui
//! workbuddy-switch serve        # 只起服务不开浏览器（--port / --no-open）
//! workbuddy-switch status       # 终端输出当前账号
//! workbuddy-switch version      # 版本号
//! ```

mod api;
mod gateway_host;

use serde_json::json;

use buddy_switch_core::modules::{
    account, activity, auth_file, cat, checkin, config, process, refresh, region::Region, rotate,
    schedule, school, travel, update,
};

fn default_port() -> u16 {
    57890
}

/// 执行某一类定时任务（到点后派发）。CN / Global 的 region 隔离在此收敛：
/// 签到 / 旅行 / 开学季 / 夜猫子只对 CN 执行；活跃上报 CN 与 Global 都执行；
/// 保活按 region 独立标志。
async fn run_scheduled_task(task: schedule::ScheduleTask) {
    match task {
        schedule::ScheduleTask::Checkin => {
            let _ = checkin::run_checkin_cycle_for(Region::Cn, checkin::CheckinCycleMode::PeriodicRecovery)
                .await;
        }
        schedule::ScheduleTask::Travel => {
            // 一趟派出 + 一趟领奖闭环。
            let _ = travel::run_travel_cycle().await;
            let _ = travel::run_travel_claim_cycle().await;
        }
        schedule::ScheduleTask::Activity => {
            for region in Region::all() {
                let _ = activity::run_activity_cycle_for(region).await;
            }
        }
        schedule::ScheduleTask::Keepalive => {
            for region in Region::all() {
                let _ = refresh::run_keepalive_cycle_for(region).await;
            }
        }
        schedule::ScheduleTask::School => {
            let _ = school::run_school_cycle_for(Region::Cn).await;
        }
        schedule::ScheduleTask::Cat => {
            let _ = cat::run_cat_cycle_for(Region::Cn).await;
        }
    }
}

/// 为某一类任务起一个独立排程循环：每轮重新计算**自己的** `next_fire` 并 sleep 到该时刻，
/// 不用统一 tick 轮询。六类各自独立开关、独立时点；同一整点上的多类任务因各占一个
/// tokio task 而天然并行、互不阻塞。
fn spawn_scheduled_task(task: schedule::ScheduleTask) {
    tokio::spawn(async move {
        loop {
            let cfg = schedule::load_schedule_config();
            let now = config::now_ms();
            let at = schedule::next_fire(task.hours(&cfg), now);
            let wait_ms = match at {
                Some(at) => (at - now).max(0) as u64,
                // 任务被禁用（hours 为空）：1 分钟后重查配置，避免无谓空转。
                None => 60_000,
            };
            tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
            if at.is_none() {
                continue;
            }
            run_scheduled_task(task).await;
        }
    });
}

/// 后台任务：
/// - 启动：整理历史签到日志 + 对 CN 做一次启动即核验；
/// - 自动轮换按配置间隔执行（CodeBuddy CLI 为 CN 专有，保持 CN）；
/// - 六类积分任务（签到 / 旅行 / 活跃上报 / 保活 / 开学季 / 夜猫子）按 `schedule` 配置
///   **按点独立排程**。
fn spawn_background_loops() {
    tokio::spawn(async move {
        if let Err(error) = config::compact_checkin_logs() {
            eprintln!("[签到] 历史日志整理失败: {error}");
        }
        // 启动即核验一次（CN）；Global 无签到体系，跳过。
        let _ = checkin::run_checkin_cycle_for(Region::Cn, checkin::CheckinCycleMode::StartupVerify).await;
    });

    tokio::spawn(async move {
        let mut last_cycle_at: i64 = 0;
        loop {
            let cfg = config::load_auto_rotate_config();
            if cfg.get("enabled").and_then(|v| v.as_bool()) == Some(true) {
                let interval_minutes = cfg
                    .get("check_interval_minutes")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(5)
                    .max(1);
                let now = config::now_ms();
                if now - last_cycle_at >= interval_minutes * 60_000 {
                    last_cycle_at = now;
                    let _ = rotate::run_rotate_cycle().await;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });

    // 六类任务各自独立排程。
    for task in schedule::ScheduleTask::all() {
        spawn_scheduled_task(task);
    }

    // 账号池余额刷新：**独立**循环（不与其它循环合并成一个 tick），首次启动先跑一次，
    // 之后按池配置 `credits_refresh_interval_ms`（默认 30 分钟）周期刷新。
    //
    // 为什么必须是独立后台循环：余额是慢变数据，在请求路径上同步拉余额会直接拉高
    // 每次请求的延迟。刷新语义见 `buddy_switch_gateway::credits_refresh`。
    tokio::spawn(async move {
        let state = gateway_host::shared_state();
        // 启动即刷一次：补齐上次进程遗留的「从未取过余额」账号。
        let _ = buddy_switch_gateway::credits_refresh::refresh_once(&state).await;
        loop {
            let interval_ms = state
                .pool
                .read()
                .await
                .config()
                .credits_refresh_interval_ms
                .max(1);
            tokio::time::sleep(std::time::Duration::from_millis(interval_ms as u64)).await;
            let _ = buddy_switch_gateway::credits_refresh::refresh_once(&state).await;
        }
    });
}

fn print_status() {
    let auth = auth_file::read_auth_file();
    let current = auth.as_ref().and_then(|a| {
        let acct = a.get("account").cloned().unwrap_or_else(|| json!({}));
        Some(json!({
            "uid": acct.get("uid"),
            "nickname": acct.get("nickname"),
            "email": acct.get("email"),
        }))
    });
    let running = process::is_workbuddy_running();
    println!("BuddySwitch v{}", update::APP_VERSION);
    println!("WorkBuddy 运行中: {}", if running { "是" } else { "否" });
    match current {
        Some(c) => {
            let name = c
                .get("nickname")
                .and_then(|v| v.as_str())
                .or_else(|| c.get("email").and_then(|v| v.as_str()))
                .unwrap_or("未知");
            println!("当前账号: {name}");
        }
        None => println!("当前账号: 未登录"),
    }
    println!("账号数: {}", account::load_accounts().len());
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("serve");
    match cmd {
        "status" => print_status(),
        "version" | "--version" | "-V" => {
            println!("BuddySwitch {}", env!("CARGO_PKG_VERSION"));
        }
        "serve" | _ => serve(&args).await,
    }
}

async fn serve(args: &[String]) {
    let mut port = default_port();
    if let Some(i) = args.iter().position(|a| a == "--port") {
        if let Some(p) = args.get(i + 1).and_then(|p| p.parse::<u16>().ok()) {
            port = p;
        }
    }

    let app = api::router();
    let addr = format!("127.0.0.1:{port}");
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("启动失败: 端口 {port} 被占用或不可用（{e}）。可用 --port 指定其他端口。");
            std::process::exit(1);
        }
    };

    println!("BuddySwitch v{}", update::APP_VERSION);
    println!("webui: http://{addr}");
    println!("按 Ctrl+C 停止服务。");

    let no_open = args.iter().any(|a| a == "--no-open");
    if !no_open {
        open_browser(&addr);
    }

    spawn_background_loops();

    // 按配置启动 API 网关独立监听（默认关闭；默认 127.0.0.1:57891）。
    match gateway_host::apply().await {
        Ok(Some(addr)) => println!("API 网关: http://{addr}"),
        Ok(None) => {}
        Err(error) => eprintln!("[gateway] 启动失败: {error}"),
    }

    axum::serve(listener, app).await.unwrap();
}

fn open_browser(addr: &str) {
    let url = format!("http://{addr}");
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&url).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let mut c = std::process::Command::new("cmd");
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW：开浏览器不闪 cmd 窗
        }
        let _ = c.args(["/C", "start", &url]).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }
}
