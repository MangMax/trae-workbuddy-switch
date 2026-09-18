//! OAuth 客户端凭证外置配置（`trae/conf/oauth_client.json`）。
//!
//! ## 为什么需要它
//!
//! `client_id` 是**上游会变**的公开标识（本轮刚从 `en1oxy7wnw8j9n` 换成
//! `ono9krqynydwx5`，旧值会让授权页停在 billing status 后不回跳）。
//! 把它硬编码在二进制里意味着「上游一改就得发版」。外置后改文件 + 重启即可。
//!
//! ## 与参考实现的差异（有意）
//!
//! 参考 `oauth.rs:36-84` 外置的是**完整 `exchange_url`**；本模块外置的是 **path**。
//! 理由：新协议下 host 由**回调回传**（`${host}/trae/api/v3/oauth/ExchangeToken`），
//! 外置整条 URL 会与回调 host 打架 —— 一个是「配置说打 A 域」、一个是「回调说打 B 域」，
//! 二者冲突时的行为无从定义。外置 path 则与 host 正交，不会产生这种二义。
//!
//! ## 生效时机
//!
//! 进程内 `OnceLock` 缓存 ⇒ **改文件后需重启应用**。这是刻意的取舍：
//! 每次登录都读一次盘虽然可行，但「配置在会话中途被改」会让同一次登录的
//! 授权 URL 与交换请求用上不同的 client_id，属于极难定位的偶发缺陷。

use std::sync::OnceLock;

use crate::modules::trae::paths;
use crate::modules::trae::{TRAE_EXCHANGE_TOKEN_PATH, TRAE_OAUTH_CLIENT_ID};

/// OAuth 客户端凭证（`client_id` / `client_secret` / 交换路径）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthClientConfig {
    /// 公开 ClientID（默认 [`TRAE_OAUTH_CLIENT_ID`]）。
    pub client_id: String,
    /// 旧协议的 `ClientSecret`（默认 `"-"`，仅兜底链末位用到）。
    pub client_secret: String,
    /// 交换端点 **path**（默认 [`TRAE_EXCHANGE_TOKEN_PATH`]；host 由回调回传）。
    pub exchange_path: String,
}

impl Default for OAuthClientConfig {
    fn default() -> Self {
        Self {
            client_id: TRAE_OAUTH_CLIENT_ID.to_string(),
            client_secret: DEFAULT_CLIENT_SECRET.to_string(),
            exchange_path: TRAE_EXCHANGE_TOKEN_PATH.to_string(),
        }
    }
}

/// 旧协议兜底链使用的默认 `ClientSecret`（与参考 `oauth.rs:23` 逐字一致）。
pub const DEFAULT_CLIENT_SECRET: &str = "-";

/// 外置文件的 JSON 形状（字段全部可缺省 ⇒ 回落默认值）。
#[derive(serde::Deserialize)]
struct OAuthClientFile {
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    exchange_path: Option<String>,
}

/// 取非空字符串，否则回落默认值。
fn pick(value: Option<String>, fallback: &str) -> String {
    value
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

/// 解析外置配置文本；字段缺省 / 空串 / 整份损坏一律回落默认值。
///
/// **不报错**：这是「上游凭证的兜底来源」，读不到就应该用内置默认值继续工作，
/// 而不是让整个 OAuth 登录因为一个配置文件写坏了而不可用。
fn from_json_text(text: &str) -> OAuthClientConfig {
    let default = OAuthClientConfig::default();
    let Ok(parsed) = serde_json::from_str::<OAuthClientFile>(text) else {
        return default;
    };
    OAuthClientConfig {
        client_id: pick(parsed.client_id, &default.client_id),
        client_secret: pick(parsed.client_secret, &default.client_secret),
        exchange_path: pick(parsed.exchange_path, &default.exchange_path),
    }
}

/// 从指定路径读配置（文件缺失 / 不可读 ⇒ 默认值）。
fn from_file(path: &std::path::Path) -> OAuthClientConfig {
    match std::fs::read_to_string(path) {
        Ok(text) => from_json_text(&text),
        Err(_) => OAuthClientConfig::default(),
    }
}

/// 读取外置 OAuth 客户端配置（全局一次；缺失/损坏回退内置默认）。
pub fn oauth_client() -> &'static OAuthClientConfig {
    static CONFIG: OnceLock<OAuthClientConfig> = OnceLock::new();
    CONFIG.get_or_init(|| from_file(&paths::oauth_client_config_file()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ★ 文件缺失 ⇒ 回落内置默认（登录不该因为少一个可选配置文件而不可用）。
    #[test]
    fn oauth_client_config_falls_back_when_file_missing() {
        let missing = std::env::temp_dir().join("definitely-not-here-oauth-client.json");
        assert_eq!(from_file(&missing), OAuthClientConfig::default());

        // 默认值必须与抓包固化常量逐字一致。
        let default = OAuthClientConfig::default();
        assert_eq!(default.client_id, "ono9krqynydwx5");
        assert_eq!(default.client_secret, "-");
        assert_eq!(default.exchange_path, "/trae/api/v3/oauth/ExchangeToken");
    }

    /// ★ 外置字段可覆盖；部分缺省时只覆盖写了的那个。
    #[test]
    fn oauth_client_config_reads_overrides() {
        let full = from_json_text(
            r#"{"client_id":"custom-id","client_secret":"s3cret","exchange_path":"/custom/Exchange"}"#,
        );
        assert_eq!(full.client_id, "custom-id");
        assert_eq!(full.client_secret, "s3cret");
        assert_eq!(full.exchange_path, "/custom/Exchange");

        // 只写 client_id：其余回落默认。
        let partial = from_json_text(r#"{"client_id":"only-id"}"#);
        assert_eq!(partial.client_id, "only-id");
        assert_eq!(partial.client_secret, "-");
        assert_eq!(partial.exchange_path, "/trae/api/v3/oauth/ExchangeToken");

        // 空串与纯空白不算有效覆盖。
        let blank = from_json_text(r#"{"client_id":"   ","client_secret":""}"#);
        assert_eq!(blank.client_id, "ono9krqynydwx5");
        assert_eq!(blank.client_secret, "-");

        // 整份损坏 ⇒ 全部回落默认，绝不 panic。
        assert_eq!(
            from_json_text("{ this is not json"),
            OAuthClientConfig::default()
        );
        assert_eq!(from_json_text("[]"), OAuthClientConfig::default());
    }

    /// 默认交换路径必须与 `mod.rs` 的契约常量同源（避免两处漂移）。
    #[test]
    fn default_exchange_path_matches_contract_constant() {
        assert_eq!(
            OAuthClientConfig::default().exchange_path,
            crate::modules::trae::TRAE_EXCHANGE_TOKEN_PATH
        );
        assert_eq!(
            OAuthClientConfig::default().client_id,
            crate::modules::trae::TRAE_OAUTH_CLIENT_ID
        );
    }
}
