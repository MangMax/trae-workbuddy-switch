//! `GET /v1/models`：按 Key 绑定 region 返回模型列表 + 来源标注（P0-4）。

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use serde_json::json;

use crate::routes::{authenticate, json_response, openai_error};
use crate::state::GatewayState;

/// 模型列表处理器。
pub async fn handler(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    let record = match authenticate(&state, &headers) {
        Ok(record) => record,
        Err(error) => return openai_error(error),
    };

    let snapshot = state.catalogs.current(record.region);
    let data: Vec<serde_json::Value> = snapshot
        .models
        .iter()
        .map(|model| {
            json!({
                "id": model.id,
                "object": "model",
                "created": 0,
                "owned_by": "workbuddy",
                "context_window": model.context_window,
                "max_tokens": model.max_tokens,
                "supports_images": model.supports_images,
                "credits": model.credits,
                "badges": model.badges,
                "free": model.free,
            })
        })
        .collect();

    json_response(
        StatusCode::OK,
        json!({
            "object": "list",
            "data": data,
            "wb_region": snapshot.region,
            "wb_source": snapshot.source,
            "wb_fetched_at": snapshot.fetched_at,
            "wb_note": snapshot.note,
        }),
    )
}
