//! Trae 网关**端到端**测试：真实 HTTP 出站 + 真实路由 + 真实 SSE 转换。
//!
//! # 为什么必须有这一层
//!
//! 网关里有三块逻辑，单测覆盖不到、编译也不会报：
//!
//! 1. **鉴权中间件是否真的挂在路由上**——`bearer_auth` 单测全绿，但若 `router()`
//!    忘了挂 layer，端口一开就是**任何人可白嫖额度**。这类「零护栏接线」只能端到端发现。
//! 2. **换号重试是否真的换**——`pool.pick` 的单测证明「能给下一个账号」，
//!    但证明不了 `stream_chat` / `aggregate_chat` 的循环真的会在失败后再挑一次。
//! 3. **SOLO SSE → OpenAI SSE 的转换在真实 HTTP 流下是否成立**——解析器单测喂的是
//!    构造好的分片，而真实 `reqwest` 响应体的分片边界不由我们决定。
//!
//! 本机没有真实 Trae 账号（`~/.buddy-switch/trae/checkin_accounts.json` 不存在），
//! 无法联调真实上游。因此这里用**本地 mock 上游**顶替：出站地址经
//! [`buddy_switch_gateway::trae::TraeGatewayState::upstream`] 覆盖到 `127.0.0.1`，
//! 其余链路（选号 / 冷却写回 / 请求体改写 / 响应转换）全部走生产代码。
//!
//! # 环境隔离
//!
//! 与 `body_limit.rs` 同一套路：`BUDDY_SWITCH_HOME` 指向已存在的临时目录，
//! 同进程内用一把互斥锁串行化（env 是进程级全局状态）。

use std::sync::Mutex;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::response::Response;
use axum::routing::post;
use tower::ServiceExt;

use buddy_switch_gateway::trae::{router, TraeGatewayConfig, TraeGatewayState};

/// 串行化所有触碰 `BUDDY_SWITCH_HOME` 的用例。
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// 容忍锁中毒：某个用例断言失败并持锁 panic 后，其余用例仍应各自报告真实结果，
/// 而不是在取锁处集体失败（"一处失败、满屏失败"会掩盖真正的缺陷）。
fn guard() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 把 `BUDDY_SWITCH_HOME` 指向一个已存在的临时目录，并写入一份账号库。
///
/// 注意层级：`BUDDY_SWITCH_HOME` 是**主目录**，store 在其下的 `.buddy-switch/`，
/// 因此 Trae 数据落在 `<home>/.buddy-switch/trae/`。少写一层 `.buddy-switch`
/// 会静默地读到空账号库（症状是每个请求都 503「无可用账号」）。
fn isolated_home(tag: &str, accounts: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "trae-gw-e2e-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let trae_dir = dir.join(".buddy-switch").join("trae");
    std::fs::create_dir_all(&trae_dir).expect("创建隔离目录");
    std::env::set_var("BUDDY_SWITCH_HOME", &dir);

    let list: Vec<serde_json::Value> = accounts
        .iter()
        .map(|(uid, jwt)| {
            serde_json::json!({ "name": format!("账号 {uid}"), "UserID": uid, "jwt": jwt })
        })
        .collect();
    let body = serde_json::json!({ "accounts": list });
    std::fs::write(
        trae_dir.join("checkin_accounts.json"),
        serde_json::to_string_pretty(&body).unwrap(),
    )
    .expect("写入账号库");
    dir
}

/// 读取冷却文件（用于断言错误是否被写回共享状态）。
fn cooldowns(home: &std::path::Path) -> serde_json::Value {
    let text = std::fs::read_to_string(
        home.join(".buddy-switch")
            .join("trae")
            .join("account_cooldowns.json"),
    )
    .unwrap_or_else(|_| "{}".into());
    serde_json::from_str(&text).unwrap_or(serde_json::json!({}))
}

// ---------------------------------------------------------------------------
// mock 上游
// ---------------------------------------------------------------------------

/// mock 上游的行为。
#[derive(Clone)]
enum Behavior {
    /// 正常返回一段 SOLO 事件流，正文回显 `x-ide-token`（便于断言"这次用的是哪个账号"）。
    Normal,
    /// 连接期直接 401（会话失效）。
    Http401,
    /// 连接期直接 429（限流）。
    Http429,
    /// 200 + 流内 `event:error`（套餐额度用尽）。
    StreamError,
    /// 只对指定 jwt 返回 401，其余正常——用于验证换号重试。
    FailForJwt(String),
}

/// mock 上游处理器。
async fn mock_upstream(State(behavior): State<Behavior>, headers: HeaderMap) -> Response {
    let jwt = headers
        .get("x-ide-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();

    let plain = |status: StatusCode, text: &str| {
        Response::builder()
            .status(status)
            .header("content-type", "text/plain")
            .body(Body::from(text.to_string()))
            .expect("构造 mock 响应")
    };

    match behavior {
        Behavior::Http401 => plain(StatusCode::UNAUTHORIZED, "unauthorized"),
        Behavior::Http429 => plain(StatusCode::TOO_MANY_REQUESTS, "rate limited"),
        Behavior::FailForJwt(target) if jwt == target => {
            plain(StatusCode::UNAUTHORIZED, "session dead")
        }
        Behavior::StreamError => {
            let body = "id:1\nevent:error\ndata:{\"code\":1005,\"message\":\"plan limit\"}\n\n";
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .body(Body::from(body.to_string()))
                .expect("构造 mock 响应")
        }
        _ => {
            // 真实上游的报文形状：`id:N` + `event:` + `data:`，事件之间空行分隔。
            // 正文回显 jwt，这样"最终用的是哪个账号"可以从响应内容直接断言。
            let body = format!(
                "id:1\nevent:output\ndata:{{\"response\":\"{jwt}\"}}\n\n\
                 id:2\nevent:token_usage\ndata:{{\"prompt_tokens\":11,\"completion_tokens\":22,\"total_tokens\":33}}\n\n\
                 id:3\nevent:done\ndata:{{\"finish_reason\":\"stop\"}}\n\n"
            );
            Response::builder()
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream")
                .body(Body::from(body))
                .expect("构造 mock 响应")
        }
    }
}

/// 启动 mock 上游，返回 `http://127.0.0.1:<port>`。
async fn spawn_mock(behavior: Behavior) -> String {
    let app = axum::Router::new()
        .route("/api/agent/v3/llm_utils_chat", post(mock_upstream))
        .with_state(behavior);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定 mock 上游");
    let addr = listener.local_addr().expect("取 mock 地址");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

// ---------------------------------------------------------------------------
// 请求构造与驱动
// ---------------------------------------------------------------------------

fn chat_request(key: Option<&str>, body: &str) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json");
    if let Some(key) = key {
        builder = builder.header("authorization", format!("Bearer {key}"));
    }
    builder.body(Body::from(body.to_string())).expect("构造请求")
}

/// 组装「已指向 mock 上游」的网关状态。
async fn gateway_for(behavior: Behavior) -> (TraeGatewayState, String) {
    let base = spawn_mock(behavior).await;
    let mut state = TraeGatewayState::new(TraeGatewayConfig::default());
    state.upstream = base;
    let key = state.api_key().await;
    (state, key)
}

async fn body_text(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("读取响应体");
    String::from_utf8_lossy(&bytes).to_string()
}

// ---------------------------------------------------------------------------
// 鉴权
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_needs_no_authorization() {
    let _guard = guard();
    let home = isolated_home("health", &[("uid-a", "jwt-a")]);
    let (state, _key) = gateway_for(Behavior::Normal).await;

    let response = router(state)
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("构造请求"),
        )
        .await
        .expect("路由调用");

    assert_eq!(response.status(), StatusCode::OK, "/health 必须免鉴权");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn missing_authorization_is_rejected() {
    let _guard = guard();
    let home = isolated_home("noauth", &[("uid-a", "jwt-a")]);
    let (state, _key) = gateway_for(Behavior::Normal).await;

    let response = router(state)
        .oneshot(chat_request(None, r#"{"model":"m","messages":[]}"#))
        .await
        .expect("路由调用");

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "没有 Authorization 头必须 401。若这里返回 200，说明 bearer_auth 没挂在路由上——\
         端口一开就是任何人可白嫖额度"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn wrong_api_key_is_rejected() {
    let _guard = guard();
    let home = isolated_home("wrongkey", &[("uid-a", "jwt-a")]);
    let (state, _key) = gateway_for(Behavior::Normal).await;

    let response = router(state)
        .oneshot(chat_request(Some("sk-trae-not-the-key"), r#"{"model":"m","messages":[]}"#))
        .await
        .expect("路由调用");

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn empty_configured_key_rejects_instead_of_allowing_everyone() {
    let _guard = guard();
    let home = isolated_home("emptykey", &[("uid-a", "jwt-a")]);
    let (state, _key) = gateway_for(Behavior::Normal).await;

    // 把已配置的 Key 清空——这是本网关与参考实现的**有意差异**：参考实现「留空即不鉴权」，
    // 意味着猜到端口就能白嫖；本网关宁严勿宽，空 Key 一律拒绝。
    *state.api_key.write().await = String::new();

    let response = router(state)
        .oneshot(chat_request(Some(""), r#"{"model":"m","messages":[]}"#))
        .await
        .expect("路由调用");

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "配置为空 Key 时必须拒绝，而不是放行所有人"
    );
    let _ = std::fs::remove_dir_all(&home);
}

// ---------------------------------------------------------------------------
// 流式转换
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streaming_request_is_converted_to_openai_sse() {
    let _guard = guard();
    let home = isolated_home("stream", &[("uid-a", "jwt-a")]);
    let (state, key) = gateway_for(Behavior::Normal).await;

    let response = router(state)
        .oneshot(chat_request(
            Some(&key),
            r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"stream":true}"#,
        ))
        .await
        .expect("路由调用");

    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        content_type.contains("text/event-stream"),
        "流式响应必须是 text/event-stream，实际 {content_type}"
    );

    let text = body_text(response).await;
    // SOLO 事件名绝不能泄漏给客户端。
    for leaked in ["event:output", "event:token_usage", "event:done", "id:1"] {
        assert!(!text.contains(leaked), "上游私有事件名泄漏到响应里：{leaked}\n{text}");
    }
    // 必须转换成 OpenAI 的 `data: {...}` 分片，并且正文可读。
    assert!(text.contains("data: {"), "缺少 OpenAI data 行\n{text}");
    assert!(text.contains("chat.completion.chunk"), "缺少 chunk 对象\n{text}");
    assert!(text.contains("jwt-a"), "正文应回显所用账号的 jwt\n{text}");
    // **绝不悬挂连接**：无论上游怎么结束，都必须补发 [DONE]。
    assert!(
        text.trim_end().ends_with("data: [DONE]"),
        "流式响应必须以 data: [DONE] 收尾\n{text}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn non_streaming_request_is_aggregated_locally() {
    let _guard = guard();
    let home = isolated_home("aggregate", &[("uid-a", "jwt-a")]);
    let (state, key) = gateway_for(Behavior::Normal).await;

    let response = router(state)
        .oneshot(chat_request(
            Some(&key),
            r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"stream":false}"#,
        ))
        .await
        .expect("路由调用");

    assert_eq!(response.status(), StatusCode::OK);
    let text = body_text(response).await;
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("非流式必须返回 JSON");
    assert_eq!(
        parsed["object"], "chat.completion",
        "非流式必须是 chat.completion 而不是 chunk 列表\n{text}"
    );
    assert_eq!(parsed["choices"][0]["message"]["content"], "jwt-a");
    assert_eq!(parsed["choices"][0]["finish_reason"], "stop");
    assert_eq!(parsed["usage"]["prompt_tokens"], 11);
    assert_eq!(parsed["usage"]["completion_tokens"], 22);
    assert_eq!(parsed["usage"]["total_tokens"], 33);
    let _ = std::fs::remove_dir_all(&home);
}

// ---------------------------------------------------------------------------
// 冷却写回与换号
// ---------------------------------------------------------------------------

#[tokio::test]
async fn upstream_401_marks_the_account_session_dead() {
    let _guard = guard();
    let home = isolated_home("dead", &[("uid-a", "jwt-a")]);
    let (state, key) = gateway_for(Behavior::Http401).await;

    let response = router(state)
        .oneshot(chat_request(
            Some(&key),
            r#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#,
        ))
        .await
        .expect("路由调用");

    // 唯一账号 + 上游 401 → 没有可换的账号，对客户端报 401（会话失效）。
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // 关键断言：错误必须写回**共享的**冷却文件，签到页才会显示同一个状态。
    let file = cooldowns(&home);
    let entry = &file["cooldowns"]["uid-a"];
    assert_eq!(entry["type"], "SessionDead", "冷却类型必须是 SessionDead\n{file}");
    assert_eq!(entry["until"], 9_999_999_999_i64, "会话失效必须是永久冷却\n{file}");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn upstream_429_cools_the_account_without_disabling_it() {
    let _guard = guard();
    let home = isolated_home("rate", &[("uid-a", "jwt-a")]);
    let (state, key) = gateway_for(Behavior::Http429).await;

    let response = router(state)
        .oneshot(chat_request(
            Some(&key),
            r#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#,
        ))
        .await
        .expect("路由调用");

    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let file = cooldowns(&home);
    let entry = &file["cooldowns"]["uid-a"];
    assert_eq!(entry["type"], "SoftRate", "限流必须记成 SoftRate\n{file}");
    let until = entry["until"].as_i64().unwrap_or(0);
    let now = chrono::Local::now().timestamp();
    // 限流是**临时**冷却，不得像 SessionDead 那样写成永久。
    assert!(until > now && until < 9_999_999_999, "限流冷却时长不合理: {until}\n{file}");
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn rotation_retries_with_the_next_account_and_reports_success() {
    let _guard = guard();
    // 账号 A 会被 mock 拒绝，账号 B 正常。选号顺序沿用账号库顺序，因此先试 A。
    let home = isolated_home("rotate", &[("uid-a", "jwt-a"), ("uid-b", "jwt-b")]);
    let (state, key) = gateway_for(Behavior::FailForJwt("jwt-a".into())).await;

    let response = router(state)
        .oneshot(chat_request(
            Some(&key),
            r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"stream":false}"#,
        ))
        .await
        .expect("路由调用");

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "首个账号失败后必须换号重试并成功，而不是把失败直接透给客户端"
    );
    let text = body_text(response).await;
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("聚合 JSON");
    assert_eq!(
        parsed["choices"][0]["message"]["content"], "jwt-b",
        "最终必须由第二个账号完成请求\n{text}"
    );

    // A 被标记为会话失效，B 不得被牵连。
    let file = cooldowns(&home);
    assert_eq!(file["cooldowns"]["uid-a"]["type"], "SessionDead", "{file}");
    assert!(
        file["cooldowns"].get("uid-b").is_none(),
        "成功完成的账号不得被写入冷却\n{file}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn stream_error_event_is_surfaced_and_cools_the_account() {
    let _guard = guard();
    let home = isolated_home("streamerr", &[("uid-a", "jwt-a")]);
    let (state, key) = gateway_for(Behavior::StreamError).await;

    // 非流式：流内错误在聚合阶段暴露，此时响应头还没发，因此可以整轮重试；
    // 只有一个账号 → 换不动 → 最终返回错误。
    let response = router(state)
        .oneshot(chat_request(
            Some(&key),
            r#"{"model":"m","messages":[{"role":"user","content":"hi"}],"stream":false}"#,
        ))
        .await
        .expect("路由调用");

    assert_eq!(
        response.status(),
        StatusCode::BAD_GATEWAY,
        "业务错误码 1005（套餐额度）无对应 HTTP 语义，统一报 502"
    );

    // `event:error` 的 1005 必须被识别成 PlanLimit，而不是被当成普通业务码。
    let file = cooldowns(&home);
    assert_eq!(
        file["cooldowns"]["uid-a"]["type"], "PlanLimit",
        "1005 必须归类为 PlanLimit（否则额度耗尽的账号会在 10 分钟后被反复重试）\n{file}"
    );
    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn request_is_rejected_when_no_account_is_available() {
    let _guard = guard();
    // 账号库为空：这是新装用户的真实状态。
    let home = isolated_home("noaccount", &[]);
    let (state, key) = gateway_for(Behavior::Normal).await;

    let response = router(state)
        .oneshot(chat_request(
            Some(&key),
            r#"{"model":"m","messages":[{"role":"user","content":"hi"}]}"#,
        ))
        .await
        .expect("路由调用");

    assert_eq!(
        response.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "没有可用账号时应报 503（服务暂时不可用），而不是 500 或挂起"
    );
    let text = body_text(response).await;
    assert!(
        text.contains("no_healthy_account"),
        "错误码必须是 no_healthy_account，客户端才能区分「没账号」与「上游挂了」\n{text}"
    );
    assert!(
        text.contains("账号"),
        "错误文案必须能指导用户去加账号\n{text}"
    );
    let _ = std::fs::remove_dir_all(&home);
}
