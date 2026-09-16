//! 会话头族与派生标识（对照参考实现 `internal/upstream/headers.go` 的
//! `injectConversationHeaders` 与 `internal/session/ids.go`）。
//!
//! 为什么需要这一族头：上游按 `X-Conversation-Request-ID` 作为**对话轮聚合主键**。
//! 若每跳都新生成，同一轮对话在后台会被拆成一串碎片（issue #35 的症状）。因此
//! 约束是：**同一轮请求内（含换号重试、路径回退、提示词降级）必须复用同一个
//! `conversation_request_id`**，跨轮才换新。
//!
//! 头族构成：
//! | 头 | 取值 |
//! |---|---|
//! | `X-Conversation-ID` | 客户端会话 id；空则**不发** |
//! | `X-Conversation-Request-ID` | 轮主键（必有，轮内稳定） |
//! | `X-Conversation-Message-ID` / `X-Request-ID` | 单次请求 id（每条独立） |
//! | `X-Root-Request-ID` | = 轮主键 |
//! | `X-Trace-ID` | 调用方 trace 或轮主键 |
//! | `X-B3-TraceId` | 轮主键（非法时回落单次请求 id） |
//! | `X-B3-SpanId` | 单次请求 id 前 16 位 |
//! | `X-B3-Sampled` | `1` |

use std::collections::HashMap;

use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// 派生标识的命名空间前缀（与参考实现一致，保证两端在相同 uid 下得到相同 id）。
const ID_NAMESPACE: &str = "wb2a";

/// 单次请求 id：32 位十六进制（16 字节随机）。
pub fn new_hex_id() -> String {
    Uuid::new_v4().simple().to_string()
}

/// B3 trace id 合法性：长度 16 或 32，且全为十六进制字符。
pub fn valid_trace_id(candidate: &str) -> bool {
    let length = candidate.len();
    (length == 16 || length == 32) && candidate.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// 派生稳定设备/会话标识：`sha256("wb2a:<purpose>:<uid>")` 前 18 字节的十六进制（36 字符）。
///
/// 同 uid + 同 purpose 恒定，因此上游看到的是一个稳定的设备指纹，
/// 而不是每个进程随机的新设备（后者会触发风控）。
/// uid 为空时返回空串，调用方据此**不发送**该头。
pub fn derived_device_id(purpose: &str, uid: &str) -> String {
    if uid.is_empty() {
        return String::new();
    }
    let mut hasher = Sha256::new();
    hasher.update(format!("{ID_NAMESPACE}:{purpose}:{uid}").as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(18)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 由会话键派生**进程内稳定**的轮主键：`sha256("<salt>|<key>")` 前 16 字节。
pub fn request_id_for_session(salt: &str, key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(b"|");
    hasher.update(key.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 取「最后一条 user 消息」的文本作为轮键；无 user 消息或无文本内容时返回 `None`。
///
/// 用于客户端未提供任何会话键时，仍让**同一轮对话**（最后一条用户输入相同）复用主键。
pub fn turn_key(body: &Value) -> Option<String> {
    let messages = body.get("messages")?.as_array()?;
    let last_user = messages.iter().rev().find(|message| {
        message.get("role").and_then(Value::as_str) == Some("user")
    })?;
    let content = last_user.get("content")?;
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<&str>>()
            .join(""),
        _ => return None,
    };
    if text.is_empty() {
        return None;
    }
    Some(text)
}

/// 解析本轮 `conversation_request_id`。
///
/// 优先级（对照参考实现）：
/// 1. 入站 `X-Conversation-Request-ID`（客户端显式指定，直接透传）；
/// 2. 会话键派生的稳定 id（同一会话跨轮稳定）；
/// 3. 轮键（最后一条 user 文本）派生的稳定 id；
/// 4. 全新随机 id（无任何线索时的兜底）。
pub fn resolve_conversation_request_id(
    inbound: Option<&str>,
    session_key: Option<&str>,
    turn_key: Option<&str>,
) -> String {
    if let Some(value) = inbound.map(str::trim).filter(|value| !value.is_empty()) {
        return value.to_string();
    }
    if let Some(key) = session_key.map(str::trim).filter(|value| !value.is_empty()) {
        return request_id_for_session("session", key);
    }
    if let Some(key) = turn_key.map(str::trim).filter(|value| !value.is_empty()) {
        return request_id_for_session("turn", key);
    }
    new_hex_id()
}

/// 一轮请求的会话上下文。轮内**所有**重试必须复用同一实例。
#[derive(Debug, Clone)]
pub struct ConversationContext {
    /// 客户端会话 id（可为空）。
    pub conversation_id: Option<String>,
    /// 轮主键。
    pub conversation_request_id: String,
    /// 调用方 trace id（可为空）。
    pub trace_id: Option<String>,
}

impl ConversationContext {
    /// 新建一轮上下文（内部生成轮主键）。
    pub fn new(
        conversation_id: Option<String>,
        inbound_request_id: Option<&str>,
        session_key: Option<&str>,
        turn_key: Option<&str>,
        trace_id: Option<String>,
    ) -> Self {
        Self {
            conversation_id: conversation_id.filter(|value| !value.is_empty()),
            conversation_request_id: resolve_conversation_request_id(
                inbound_request_id,
                session_key,
                turn_key,
            ),
            trace_id: trace_id.filter(|value| !value.is_empty()),
        }
    }
}

/// 构造会话头族。`message_id` 由调用方传入，便于同一次请求内多处复用同一 id。
pub fn conversation_headers(
    context: &ConversationContext,
    message_id: &str,
) -> HashMap<String, String> {
    let mut headers = HashMap::new();

    if let Some(conversation_id) = context.conversation_id.as_deref() {
        if !conversation_id.is_empty() {
            headers.insert("X-Conversation-ID".to_string(), conversation_id.to_string());
        }
    }

    headers.insert(
        "X-Conversation-Request-ID".to_string(),
        context.conversation_request_id.clone(),
    );
    headers.insert(
        "X-Conversation-Message-ID".to_string(),
        message_id.to_string(),
    );
    headers.insert("X-Request-ID".to_string(), message_id.to_string());
    headers.insert(
        "X-Root-Request-ID".to_string(),
        context.conversation_request_id.clone(),
    );
    headers.insert(
        "X-Trace-ID".to_string(),
        context
            .trace_id
            .clone()
            .unwrap_or_else(|| context.conversation_request_id.clone()),
    );

    // B3：trace id 非法（长度/字符不符）时回落单次请求 id，避免发上游无法解析的值。
    let b3_trace = if valid_trace_id(&context.conversation_request_id) {
        context.conversation_request_id.clone()
    } else {
        message_id.to_string()
    };
    headers.insert("X-B3-TraceId".to_string(), b3_trace);
    headers.insert(
        "X-B3-SpanId".to_string(),
        message_id.chars().take(16).collect(),
    );
    headers.insert("X-B3-Sampled".to_string(), "1".to_string());

    headers
}

/// 构造派生标识头（`X-Machine-ID` / `X-Session-ID`）。uid 为空时返回空表。
pub fn device_headers(uid: &str) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    if uid.is_empty() {
        return headers;
    }
    headers.insert("X-Machine-ID".to_string(), derived_device_id("machine", uid));
    headers.insert("X-Session-ID".to_string(), derived_device_id("session", uid));
    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn new_hex_id_is_32_hex_and_unique() {
        let first = new_hex_id();
        let second = new_hex_id();
        assert_eq!(first.len(), 32);
        assert!(first.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(first, second, "两次生成必须不同");
    }

    #[test]
    fn valid_trace_id_enforces_length_and_charset() {
        assert!(valid_trace_id("0123456789abcdef"));
        assert!(valid_trace_id("0123456789abcdef0123456789abcdef"));
        assert!(!valid_trace_id("0123456789abcde"), "15 位非法");
        assert!(!valid_trace_id("0123456789abcdef0"), "17 位非法");
        assert!(!valid_trace_id("0123456789abcdeg"), "非十六进制字符非法");
        assert!(!valid_trace_id(""));
    }

    #[test]
    fn derived_device_id_is_stable_per_uid_and_purpose() {
        let first = derived_device_id("machine", "u1");
        let second = derived_device_id("machine", "u1");
        assert_eq!(first, second, "同 uid 同 purpose 必须稳定");
        assert_eq!(first.len(), 36, "18 字节 → 36 位十六进制");
        assert_ne!(first, derived_device_id("session", "u1"), "purpose 必须区分");
        assert_ne!(first, derived_device_id("machine", "u2"), "uid 必须区分");
        assert_eq!(derived_device_id("machine", ""), "", "空 uid 不派生");
    }

    #[test]
    fn device_headers_skipped_for_empty_uid() {
        assert!(device_headers("").is_empty());
        let headers = device_headers("u1");
        assert_eq!(headers.len(), 2);
        assert!(headers.contains_key("X-Machine-ID"));
        assert!(headers.contains_key("X-Session-ID"));
    }

    #[test]
    fn inbound_request_id_wins_over_everything() {
        let resolved = resolve_conversation_request_id(
            Some("inbound-id"),
            Some("sess"),
            Some("turn"),
        );
        assert_eq!(resolved, "inbound-id", "入站值必须直接透传");
        // 空白入站值视为未提供
        assert_ne!(
            resolve_conversation_request_id(Some("   "), None, None),
            "   "
        );
    }

    #[test]
    fn session_key_beats_turn_key_and_is_stable() {
        let from_session = resolve_conversation_request_id(None, Some("sess"), Some("turn"));
        assert_eq!(from_session, request_id_for_session("session", "sess"));
        assert_eq!(
            from_session,
            resolve_conversation_request_id(None, Some("sess"), None),
            "会话键派生必须稳定"
        );
        assert_ne!(from_session, resolve_conversation_request_id(None, None, Some("turn")));
    }

    #[test]
    fn falls_back_to_random_when_no_hint() {
        let first = resolve_conversation_request_id(None, None, None);
        let second = resolve_conversation_request_id(None, None, None);
        assert_eq!(first.len(), 32);
        assert_ne!(first, second, "无线索时必须随机而非恒定");
    }

    #[test]
    fn turn_key_reads_last_user_message() {
        let body = json!({"messages": [
            {"role": "user", "content": "first"},
            {"role": "assistant", "content": "reply"},
            {"role": "user", "content": "second"}
        ]});
        assert_eq!(turn_key(&body).as_deref(), Some("second"));
    }

    #[test]
    fn turn_key_handles_multimodal_and_missing_cases() {
        let multimodal = json!({"messages": [{"role": "user", "content": [
            {"type": "text", "text": "a"}, {"type": "image_url", "image_url": {"url": "x"}}, {"type": "text", "text": "b"}
        ]}]});
        assert_eq!(turn_key(&multimodal).as_deref(), Some("ab"));

        assert_eq!(turn_key(&json!({"messages": []})), None);
        assert_eq!(turn_key(&json!({"messages": [{"role": "assistant", "content": "x"}]})), None);
        assert_eq!(turn_key(&json!({"messages": [{"role": "user", "content": ""}]})), None);
        assert_eq!(turn_key(&json!({})), None);
    }

    #[test]
    fn conversation_headers_include_full_family() {
        let context = ConversationContext::new(
            Some("conv-1".to_string()),
            None,
            Some("sess-key"),
            None,
            Some("trace-1".to_string()),
        );
        let headers = conversation_headers(&context, "msg-1");
        assert_eq!(headers["X-Conversation-ID"], "conv-1");
        assert_eq!(headers["X-Conversation-Request-ID"], context.conversation_request_id);
        assert_eq!(headers["X-Conversation-Message-ID"], "msg-1");
        assert_eq!(headers["X-Request-ID"], "msg-1");
        assert_eq!(headers["X-Root-Request-ID"], context.conversation_request_id);
        assert_eq!(headers["X-Trace-ID"], "trace-1");
        assert_eq!(
            headers["X-B3-TraceId"], context.conversation_request_id,
            "会话键派生的 32 位 hex 本身即合法 trace id"
        );
        assert_eq!(headers["X-B3-SpanId"], "msg-1");
        assert_eq!(headers["X-B3-Sampled"], "1");
    }

    #[test]
    fn b3_trace_falls_back_when_request_id_is_not_a_valid_trace_id() {
        // 客户端透传一个非十六进制、非 16/32 位的 id → B3 必须回落 message id，
        // 否则会把上游无法解析的值发出去。
        let context = ConversationContext::new(
            None,
            Some("req-abc-123"),
            None,
            None,
            None,
        );
        assert_eq!(context.conversation_request_id, "req-abc-123");
        let headers = conversation_headers(&context, "deadbeefdeadbeef");
        assert_eq!(headers["X-B3-TraceId"], "deadbeefdeadbeef");
        assert_eq!(
            headers["X-Conversation-Request-ID"], "req-abc-123",
            "回退只影响 B3，轮主键仍用客户端原值"
        );
    }

    #[test]
    fn conversation_id_is_omitted_when_absent() {
        let context = ConversationContext::new(None, None, None, None, None);
        let headers = conversation_headers(&context, "msg-1");
        assert!(
            !headers.contains_key("X-Conversation-ID"),
            "空会话 id 不得发送空头"
        );
        // 轮主键为随机 32 hex → 恰好是合法 trace id，故 B3 采用轮主键
        assert_eq!(headers["X-B3-TraceId"], context.conversation_request_id);
    }

    #[test]
    fn trace_id_falls_back_to_request_id() {
        let context = ConversationContext::new(None, None, Some("k"), None, None);
        let headers = conversation_headers(&context, "m");
        assert_eq!(headers["X-Trace-ID"], context.conversation_request_id);
    }

    #[test]
    fn span_id_is_truncated_to_16_chars() {
        let headers = conversation_headers(
            &ConversationContext::new(None, None, None, None, None),
            "0123456789abcdef0123456789abcdef",
        );
        assert_eq!(headers["X-B3-SpanId"], "0123456789abcdef");
    }

    #[test]
    fn same_turn_reuses_request_id_across_retries() {
        // 轮内复用：同一 ConversationContext 生成的头族，轮主键恒定不变
        let context = ConversationContext::new(None, None, Some("sess"), None, None);
        let first = conversation_headers(&context, "msg-a");
        let second = conversation_headers(&context, "msg-b");
        assert_eq!(
            first["X-Conversation-Request-ID"], second["X-Conversation-Request-ID"],
            "换号重试必须复用同一轮主键"
        );
        assert_ne!(first["X-Request-ID"], second["X-Request-ID"], "单次请求 id 必须各自独立");
    }
}
