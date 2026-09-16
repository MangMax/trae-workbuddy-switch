//! 网关错误类型与「上游类别 → HTTP 状态码 / 协议错误体」映射（P0-7）。

use serde_json::{json, Value};

use buddy_switch_core::modules::region::{self, Region, RegionMismatch};
use buddy_switch_core::modules::upstream::UpstreamErrorKind;

/// 网关统一错误。
#[derive(Debug, Clone)]
pub enum GatewayError {
    /// 未携带/无效/已吊销 Key → 401
    Unauthorized(String),
    /// 该 region 无可用账号 → 401
    NoCredential { region: Region },
    /// 凭据 region 不符 → 401（安全红线 F，含修复指引）
    RegionMismatch(RegionMismatch),
    /// 上游失败 → 按 kind 映射
    Upstream {
        kind: UpstreamErrorKind,
        status: u16,
        message: String,
    },
    /// 请求参数不合法 → 400
    BadRequest(String),
    /// 网关自身错误（端口占用/目录失败/内部）→ 502（不返回裸堆栈）
    Internal(String),
}

impl GatewayError {
    /// 上游类别 → HTTP 状态码（A-3.5）。
    pub fn status(&self) -> u16 {
        match self {
            GatewayError::Unauthorized(_)
            | GatewayError::NoCredential { .. }
            | GatewayError::RegionMismatch(_) => 401,
            GatewayError::Upstream { kind, .. } => match kind {
                UpstreamErrorKind::HardCredit => 402,
                UpstreamErrorKind::SoftRate => 429,
                UpstreamErrorKind::SessionDead => 401,
                UpstreamErrorKind::NotFound => 502,
                UpstreamErrorKind::Server => 502,
                UpstreamErrorKind::Client => 400,
            },
            GatewayError::BadRequest(_) => 400,
            GatewayError::Internal(_) => 502,
        }
    }

    /// 错误类别名（`type`/`code` 字段用）。
    pub fn type_name(&self) -> &'static str {
        match self {
            GatewayError::Unauthorized(_) => "unauthorized",
            GatewayError::NoCredential { .. } => "no_credential",
            GatewayError::RegionMismatch(_) => "region_mismatch",
            GatewayError::Upstream { kind, .. } => match kind {
                UpstreamErrorKind::HardCredit => "hard_credit",
                UpstreamErrorKind::SoftRate => "soft_rate",
                UpstreamErrorKind::SessionDead => "session_dead",
                UpstreamErrorKind::NotFound => "not_found",
                UpstreamErrorKind::Server => "server",
                UpstreamErrorKind::Client => "client",
            },
            GatewayError::BadRequest(_) => "bad_request",
            GatewayError::Internal(_) => "internal",
        }
    }

    /// 可读消息（不含堆栈）。
    pub fn message(&self) -> String {
        match self {
            GatewayError::Unauthorized(message) => message.clone(),
            GatewayError::NoCredential { region } => format!(
                "未找到已登录的{}账号，请在桌面端登录该版本后重试",
                region_display(*region)
            ),
            GatewayError::RegionMismatch(mismatch) => mismatch.message(),
            GatewayError::Upstream { message, .. } => message.chars().take(400).collect(),
            GatewayError::BadRequest(message) => message.clone(),
            GatewayError::Internal(message) => message.clone(),
        }
    }

    /// OpenAI 端点错误体：`{"error":{"message","type","code"}}`。
    pub fn openai_body(&self) -> Value {
        let kind = self.type_name();
        json!({ "error": { "message": self.message(), "type": kind, "code": kind } })
    }

    /// Anthropic 端点错误体：`{"type":"error","error":{"type","message"}}`。
    pub fn anthropic_body(&self) -> Value {
        json!({ "type": "error", "error": { "type": self.type_name(), "message": self.message() } })
    }
}

/// region 展示名（错误文案用）。
pub fn region_display(region: Region) -> &'static str {
    region::region_spec(region).display_name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(kind: UpstreamErrorKind) -> GatewayError {
        GatewayError::Upstream {
            kind,
            status: 0,
            message: "boom".to_string(),
        }
    }

    #[test]
    fn upstream_kind_status_mapping_matches_design() {
        assert_eq!(kind(UpstreamErrorKind::HardCredit).status(), 402);
        assert_eq!(kind(UpstreamErrorKind::SoftRate).status(), 429);
        assert_eq!(kind(UpstreamErrorKind::SessionDead).status(), 401);
        assert_eq!(kind(UpstreamErrorKind::NotFound).status(), 502);
        assert_eq!(kind(UpstreamErrorKind::Server).status(), 502);
        assert_eq!(kind(UpstreamErrorKind::Client).status(), 400);
    }

    #[test]
    fn local_error_status_mapping() {
        assert_eq!(GatewayError::Unauthorized("x".into()).status(), 401);
        assert_eq!(GatewayError::NoCredential { region: Region::Cn }.status(), 401);
        assert_eq!(GatewayError::BadRequest("x".into()).status(), 400);
        assert_eq!(GatewayError::Internal("x".into()).status(), 502);
    }

    #[test]
    fn region_mismatch_is_unauthorized_and_upstream_messages_are_bounded() {
        let mismatch = GatewayError::RegionMismatch(RegionMismatch {
            actual_domain: "www.workbuddy.ai".to_string(),
            expected_file: "workbuddy-desktop.info".to_string(),
            env_var: "WORKBUDDY_AUTH_FILE".to_string(),
            actual_region: Region::Global,
            expected_region: Region::Cn,
        });
        assert_eq!(mismatch.status(), 401);
        assert_eq!(mismatch.type_name(), "region_mismatch");
        let upstream = GatewayError::Upstream {
            kind: UpstreamErrorKind::Server,
            status: 500,
            message: "x".repeat(1000),
        };
        assert_eq!(upstream.message().chars().count(), 400);
    }

    #[test]
    fn error_bodies_have_expected_shape() {
        let err = kind(UpstreamErrorKind::HardCredit);
        let openai = err.openai_body();
        assert_eq!(openai["error"]["type"], "hard_credit");
        assert_eq!(openai["error"]["code"], "hard_credit");
        assert!(openai["error"]["message"].is_string());

        let anthropic = err.anthropic_body();
        assert_eq!(anthropic["type"], "error");
        assert_eq!(anthropic["error"]["type"], "hard_credit");
        assert!(anthropic["error"]["message"].is_string());
    }
}
