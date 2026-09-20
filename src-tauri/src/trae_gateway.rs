//! 桌面宿主（Tauri）的 **Trae** 网关接入：独立监听句柄 + 进程内共享状态。
//!
//! ## 为什么不复用 [`crate::gateway`]
//!
//! 两者持有的类型完全不同：WorkBuddy 侧是 `GatewayState`（Key 库 / region / 目录缓存），
//! Trae 侧是 `TraeGatewayState`（Trae 账号池 / 单一 API Key / 独立日志文件）。共用一个
//! `OnceLock` 只会得到「两个 Option 字段里总有一个是 None」的结构。
//!
//! 代价是重复约 40 行生命周期代码，换来的是**改 Trae 网关不会碰到 WorkBuddy 网关**。
//! 这正是 `buddy_switch_gateway::trae` 模块的整体设计取向。

use std::sync::{Mutex, OnceLock};

use buddy_switch_gateway::trae::{spawn_listener, TraeGatewayConfig, TraeGatewayState};
use buddy_switch_gateway::GatewayHandle;

static SHARED_STATE: OnceLock<TraeGatewayState> = OnceLock::new();

/// 进程内共享的 Trae 网关状态（惰性初始化，读一次配置）。
///
/// 独立监听与管理命令复用**同一份**状态，因此「API 服务」页看到的请求日志、
/// 账号池与真实打进来的请求完全一致。
pub fn shared_state() -> TraeGatewayState {
    SHARED_STATE
        .get_or_init(|| TraeGatewayState::new(TraeGatewayConfig::load()))
        .clone()
}

/// 独立监听生命周期（Tauri managed state）。
#[derive(Default)]
pub struct TraeGatewayRuntime {
    handle: Mutex<Option<GatewayHandle>>,
}

impl TraeGatewayRuntime {
    /// 新建（未启动）。
    pub fn new() -> Self {
        Self {
            handle: Mutex::new(None),
        }
    }

    /// 按当前配置启动/重启独立监听。返回实际监听地址；未启用时返回 `None`。
    pub async fn apply(&self) -> Result<Option<String>, String> {
        // 先停掉旧监听（内部会在锁外 await，避免跨 await 持有 std 锁）。
        self.stop().await;

        let state = shared_state();
        let config = state.config_snapshot().await;
        if !config.enabled {
            return Ok(None);
        }
        let handle = spawn_listener(state)
            .await
            .map_err(|error| error.to_string())?;
        let addr = handle.addr().to_string();
        {
            let mut guard = self.handle.lock().unwrap();
            *guard = Some(handle);
        }
        Ok(Some(addr))
    }

    /// 停止独立监听。
    pub async fn stop(&self) {
        let existing = {
            let mut guard = self.handle.lock().unwrap();
            guard.take()
        };
        if let Some(handle) = existing {
            handle.shutdown().await;
        }
    }

    /// 是否运行中。
    pub fn is_running(&self) -> bool {
        self.handle.lock().unwrap().is_some()
    }

    /// 运行中的监听地址。
    pub fn addr(&self) -> Option<String> {
        self.handle
            .lock()
            .unwrap()
            .as_ref()
            .map(|handle| handle.addr().to_string())
    }
}
