//! Trae 模块设置。
//!
//! 字段**统一使用 camelCase**（含持久化文件 `trae/settings.json`）。
//!
//! 与账号库（`checkin_accounts.json`，刻意保持参考实现的 `UserID` 等键名以兼容既有
//! 数据）不同，设置文件里装的是端口、客户端路径、日志保留天数这类**本机专有**配置，
//! 换一台机器就没有迁移价值，因此不值得为「与参考实现同名」付出
//! 「磁盘一套命名、响应体另一套命名」的双形态维护成本。
//!
//! 结果：本模块的设置对象在磁盘、Tauri 响应、HTTP 响应三处同名，
//! 前端 `TraeSettings` 类型可直接映射，无需任何字段转换。
//!
//! 新增字段一律给 `#[serde(default = "...")]`：字段缺失会回落默认值而非报错，
//! 使旧版本配置文件在升级后仍可读。

use serde::{Deserialize, Serialize};

use crate::modules::trae::{paths, store, TRAE_DEFAULT_API_PORT, TRAE_DEFAULT_PROXY_PORT};

/// Trae 模块设置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TraeSettings {
    /// 本地代理监听端口。
    #[serde(default = "default_proxy_port")]
    pub proxy_port: u16,
    /// 主题：`system` / `light` / `dark`。
    #[serde(default = "default_theme")]
    pub theme: String,
    /// 启动时最小化到托盘。
    #[serde(default)]
    pub launch_minimized: bool,
    /// 应用启动时自动开启代理。
    #[serde(default = "default_true")]
    pub auto_start_proxy: bool,
    /// 启用托盘图标。
    #[serde(default = "default_true")]
    pub tray: bool,
    /// 界面语言。
    #[serde(default = "default_language")]
    pub language: String,
    /// 批量签到时跳过今日已签到账号。
    #[serde(default = "default_true")]
    pub checkin_skip_checked: bool,
    /// 批量签到时跳过 JWT 已过期账号。
    #[serde(default = "default_true")]
    pub checkin_skip_expired: bool,
    /// 签到网络异常时的重试次数。
    #[serde(default = "default_retry")]
    pub retry: i32,
    /// 通知方式：`toast` / `system` / `none`。
    #[serde(default = "default_notify")]
    pub notify: String,
    /// 用户指定的 Trae 客户端可执行文件路径（覆盖自动探测）。
    #[serde(default)]
    pub trae_path: Option<String>,
    /// 用户指定的浏览器可执行文件路径（浏览器提取 JWT 用）。
    #[serde(default)]
    pub browser_path: Option<String>,
    /// 日志保留天数。
    #[serde(default = "default_log_retention_days")]
    pub log_retention_days: i64,
    /// 代理需要解密（MITM）的域名白名单，逗号分隔。
    #[serde(default = "default_proxy_domains")]
    pub proxy_domains: String,
    /// API 网关监听端口。
    #[serde(default = "default_api_port")]
    pub api_port: u16,
    /// API 网关 Bearer Token；留空则不鉴权。
    ///
    /// ## ★ 多 Key 化后本字段降级为「**兼容读**」，**不得删除**
    ///
    /// 网关的 API Key 已迁移到独立的 `trae/api_gateway_keys.json`（多 Key + 归属产品线，
    /// 见 `buddy_switch_gateway::trae::apikey`）。但**旧用户**的 Key 只存在这里：
    /// `TraeApiKeyStore::load()` 在 Key 库缺失/损坏时会回落本字段，合成一条
    /// `variant = TraeWork`（= `TraeVariant::default()`）的 legacy 记录，从而让
    /// 「升级前能用的 Key，升级后仍然能用」（R1：多 Key 迁移失败 → 全量 401）。
    ///
    /// 本字段**原地保留、不清理**：删掉它会让老用户在升级后立刻 401。
    /// 它不再被任何写路径更新（旧 `patch({apiKey})` 已随 `regenerate_api_key` 一并删除）。
    #[serde(default)]
    pub api_key: String,
    /// API 网关默认模型。
    #[serde(default = "default_api_model")]
    pub api_default_model: String,
}

fn default_proxy_port() -> u16 {
    TRAE_DEFAULT_PROXY_PORT
}

fn default_api_port() -> u16 {
    TRAE_DEFAULT_API_PORT
}

fn default_api_model() -> String {
    "deepseek-v4-flash".into()
}

fn default_theme() -> String {
    "system".into()
}

fn default_true() -> bool {
    true
}

fn default_language() -> String {
    "zh-CN".into()
}

fn default_retry() -> i32 {
    1
}

fn default_notify() -> String {
    "toast".into()
}

fn default_log_retention_days() -> i64 {
    30
}

/// 代理默认解密域名白名单。
///
/// 只解密这些域名才能捕获 Trae 的登录态；其余流量走隧道直连（见第 C 组代理实现），
/// 避免把用户所有 HTTPS 流量都置于 MITM 之下。
pub fn default_proxy_domains() -> String {
    "trae.cn,trae.com.cn,mchost.guru,zijieapi.com,bytedance.com,volcengine.com,volces.com,treecode.com"
        .into()
}

impl Default for TraeSettings {
    fn default() -> Self {
        Self {
            proxy_port: default_proxy_port(),
            theme: default_theme(),
            launch_minimized: false,
            auto_start_proxy: default_true(),
            tray: default_true(),
            language: default_language(),
            checkin_skip_checked: default_true(),
            checkin_skip_expired: default_true(),
            retry: default_retry(),
            notify: default_notify(),
            trae_path: None,
            browser_path: None,
            log_retention_days: default_log_retention_days(),
            proxy_domains: default_proxy_domains(),
            api_port: default_api_port(),
            api_key: String::new(),
            api_default_model: default_api_model(),
        }
    }
}

impl TraeSettings {
    /// 代理白名单解析为小写域名列表（忽略空白项）。
    ///
    /// 匹配时按「等于该域名或是其子域」判断，因此列表里写 `trae.cn` 即可覆盖
    /// `api.trae.cn`。统一小写避免大小写差异导致漏匹配。
    pub fn proxy_domain_list(&self) -> Vec<String> {
        self.proxy_domains
            .split(',')
            .map(|part| part.trim().to_ascii_lowercase())
            .filter(|part| !part.is_empty())
            .collect()
    }

    /// 判断某主机是否属于需要 MITM 解密的域名。
    pub fn should_intercept(&self, host: &str) -> bool {
        let host = host.split(':').next().unwrap_or(host).trim().to_ascii_lowercase();
        if host.is_empty() {
            return false;
        }
        self.proxy_domain_list().iter().any(|domain| {
            host == *domain
                || (host.len() > domain.len()
                    && host.ends_with(domain.as_str())
                    && host.as_bytes()[host.len() - domain.len() - 1] == b'.')
        })
    }
}

/// 读取设置；文件缺失或损坏时回落全默认值。
pub fn load() -> TraeSettings {
    let mut settings: TraeSettings = store::read_json(&paths::settings_file());
    // 白名单为空时回填默认值，避免设置页展示空白、且代理退化为「什么都不解密」。
    if settings.proxy_domains.trim().is_empty() {
        settings.proxy_domains = default_proxy_domains();
    }
    settings
}

/// 全量写入设置。
pub fn save(settings: &TraeSettings) -> Result<(), String> {
    store::write_json(&paths::settings_file(), settings)
}

/// 局部更新：仅覆盖 `patch` 中出现的字段，其余保持原值。
///
/// 走 JSON 中间层而非直接反序列化成 `TraeSettings`，是为了区分
/// 「字段未提供」与「字段显式设为 null」——后者用于清空 `trae_path` / `browser_path`
/// 这类可选设置。前端只需提交要改的键。
pub fn patch(patch: serde_json::Value) -> Result<TraeSettings, String> {
    let mut current = serde_json::to_value(load()).map_err(|e| e.to_string())?;
    let (Some(base), Some(delta)) = (current.as_object_mut(), patch.as_object()) else {
        return Err("设置必须是 JSON 对象".into());
    };
    for (key, value) in delta {
        base.insert(key.clone(), value.clone());
    }
    let merged: TraeSettings =
        serde_json::from_value(current).map_err(|e| format!("设置字段无效: {e}"))?;
    save(&merged)?;
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_use_trae_ports_not_workbuddy_ports() {
        let settings = TraeSettings::default();
        assert_eq!(settings.proxy_port, TRAE_DEFAULT_PROXY_PORT);
        assert_eq!(settings.api_port, TRAE_DEFAULT_API_PORT);
        // WorkBuddy 网关默认 57891，两者不能相同，否则同时启用会抢端口。
        assert_ne!(settings.api_port, 57891);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // 模拟旧版本配置文件：只含极少数键，其余必须自动补默认值。
        let partial = serde_json::json!({ "proxyPort": 18899 });
        let settings: TraeSettings = serde_json::from_value(partial).unwrap();
        assert_eq!(settings.proxy_port, 18899);
        assert_eq!(settings.api_port, TRAE_DEFAULT_API_PORT);
        assert!(settings.auto_start_proxy);
        assert_eq!(settings.retry, 1);
        assert!(!settings.proxy_domains.is_empty());
    }

    #[test]
    fn unknown_fields_are_ignored_not_fatal() {
        let value = serde_json::json!({ "proxyPort": 1, "legacy_field": "x" });
        assert!(serde_json::from_value::<TraeSettings>(value).is_ok());
    }

    #[test]
    fn settings_use_camel_case_on_disk_and_on_wire() {
        // 关键护栏：磁盘、Tauri 响应、HTTP 响应必须是同一套字段名。
        let settings = TraeSettings::default();
        let wire = serde_json::to_value(&settings).unwrap();
        for key in ["proxyPort", "autoStartProxy", "traePath", "proxyDomains", "apiPort", "apiKey"] {
            assert!(wire.get(key).is_some(), "缺少 camelCase 字段 {key}");
        }
        // snake_case 不得出现（会让前端 TraeSettings 类型映射成 undefined）
        assert!(wire.get("proxy_port").is_none());
        assert!(wire.get("auto_start_proxy").is_none());
        // snake_case 输入必须被当成未知字段而非可识别的配置
        let from_snake: TraeSettings =
            serde_json::from_value(serde_json::json!({ "proxy_port": 1234 })).unwrap();
        assert_eq!(from_snake.proxy_port, TRAE_DEFAULT_PROXY_PORT);
    }

    #[test]
    fn patch_accepts_camel_case_and_preserves_untouched_fields() {
        // 走真实文件系统会污染 ~/.buddy-switch，这里只验证「JSON 合并 + 反序列化」规则本身。
        let current = serde_json::to_value(TraeSettings::default()).unwrap();
        let mut base = current.as_object().unwrap().clone();
        for (key, value) in serde_json::json!({ "proxyPort": 19999, "notify": "none" })
            .as_object()
            .unwrap()
        {
            base.insert(key.clone(), value.clone());
        }
        let merged: TraeSettings = serde_json::from_value(serde_json::Value::Object(base)).unwrap();
        assert_eq!(merged.proxy_port, 19999);
        assert_eq!(merged.notify, "none");
        // 未提交的字段保持原值
        assert_eq!(merged.api_port, TRAE_DEFAULT_API_PORT);
        assert!(merged.auto_start_proxy);
    }

    #[test]
    fn proxy_domain_list_trims_and_lowercases() {
        let mut settings = TraeSettings::default();
        settings.proxy_domains = " Trae.CN , , api.trae.com.cn ,".into();
        assert_eq!(
            settings.proxy_domain_list(),
            vec!["trae.cn", "api.trae.com.cn"]
        );
    }

    #[test]
    fn should_intercept_matches_domain_and_subdomains_only() {
        let mut settings = TraeSettings::default();
        settings.proxy_domains = "trae.cn,trae.com.cn".into();
        // 精确域名与子域命中
        assert!(settings.should_intercept("trae.cn"));
        assert!(settings.should_intercept("api.trae.cn"));
        assert!(settings.should_intercept("a.b.trae.cn"));
        // 带端口
        assert!(settings.should_intercept("api.trae.cn:443"));
        // 大小写不敏感
        assert!(settings.should_intercept("API.TRAE.CN"));
        // 关键反例：不能因为后缀相同就误命中（nottrae.cn / eviltrae.cn）
        assert!(!settings.should_intercept("nottrae.cn"));
        assert!(!settings.should_intercept("eviltrae.cn"));
        assert!(!settings.should_intercept("example.com"));
        assert!(!settings.should_intercept(""));
    }

    #[test]
    fn empty_proxy_domains_intercepts_nothing() {
        let mut settings = TraeSettings::default();
        settings.proxy_domains = String::new();
        assert!(settings.proxy_domain_list().is_empty());
        assert!(!settings.should_intercept("api.trae.cn"));
    }
}
