//! webui / CLI 形态的网关宿主：按配置起独立监听（默认 `127.0.0.1:57891`）。
//!
//! - [`shared_state`] 提供一份进程内共享的 [`GatewayState`]，供 `api.rs` 的
//!   `merge(gateway::router(state))` 与网关管理路由复用（配置/Key/日志一致）。
//! - [`apply`] / [`status`] 管理独立监听的生命周期。独立监听与 `api.rs` 的管理面
//!   **共享同一份** [`GatewayState`]（`spawn_listener_with_state`），因此监听端口上的
//!   真实请求日志 / 目录刷新在管理页可见；`apply` 在启用前会先停掉旧监听，因此配置
//!   改为 disabled 即等价于停止。

use std::sync::OnceLock;

use tokio::sync::Mutex;

use buddy_switch_gateway::{spawn_listener_with_state, GatewayConfig, GatewayHandle, GatewayState};

static SHARED_STATE: OnceLock<GatewayState> = OnceLock::new();
static HOST: OnceLock<Mutex<GatewayHost>> = OnceLock::new();

/// 进程内共享的网关状态（惰性初始化，读一次配置）。
pub fn shared_state() -> GatewayState {
    SHARED_STATE
        .get_or_init(|| GatewayState::new(GatewayConfig::load()))
        .clone()
}

/// 独立监听生命周期管理。
#[derive(Default)]
pub struct GatewayHost {
    handle: Option<GatewayHandle>,
}

impl GatewayHost {
    /// 新建（未启动）。
    pub fn new() -> Self {
        Self { handle: None }
    }

    /// 按当前配置启动/重启独立监听。返回实际监听地址；未启用时返回 `None`。
    ///
    /// **共享状态（E4）**：独立监听与 `api.rs` 的管理面复用**同一份** [`GatewayState`]
    /// （[`shared_state`]），因此监听 57891 上的真实请求日志与 `/v1/models` 目录刷新
    /// 结果都能在管理页看到；不再让监听自建状态。
    pub async fn start(&mut self) -> Result<Option<String>, String> {
        self.stop().await;
        let state = shared_state();
        // 以共享状态内的配置为准（管理面保存配置时会同步写回该状态）。
        let config = state.config_snapshot().await;
        if !config.enabled {
            return Ok(None);
        }
        let handle = spawn_listener_with_state(state)
            .await
            .map_err(|error| error.to_string())?;
        let addr = handle.addr().to_string();
        self.handle = Some(handle);
        Ok(Some(addr))
    }

    /// 停止独立监听。
    pub async fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.shutdown().await;
        }
    }

    /// 是否运行中。
    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    /// 运行中的监听地址。
    pub fn addr(&self) -> Option<String> {
        self.handle.as_ref().map(|handle| handle.addr().to_string())
    }
}

fn host() -> &'static Mutex<GatewayHost> {
    HOST.get_or_init(|| Mutex::new(GatewayHost::new()))
}

/// 按当前配置应用（启动/重启）独立监听。
pub async fn apply() -> Result<Option<String>, String> {
    let mut host = host().lock().await;
    host.start().await
}

/// 查询运行状态（运行中, 监听地址）。
pub async fn status() -> (bool, Option<String>) {
    let host = host().lock().await;
    (host.is_running(), host.addr())
}
