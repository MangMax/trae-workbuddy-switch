//! OpenAI 请求体 → Trae `llm_utils_chat` 请求体改写。
//!
//! 上游不是 OpenAI 兼容端点，它要求一组**固定字段**（`config_name` / `model_name` /
//! `function` / `workspace_id` …）。本模块负责把标准 OpenAI 请求「翻译」过去，
//! 并顺手修掉几个上游不接受、但 OpenAI 客户端普遍会发的形态。
//!
//! ## 三处必须改写的形态
//!
//! 1. `content` 只接受**数组**形态（`[{"type":"text","text":…}]`），字符串会被拒。
//! 2. `tools[].function.parameters` 只接受 **JSON 字符串**，不接受对象。
//! 3. `assistant.tool_calls[].function` 要改成 `function_call`（历史消息回传时）。
//!
//! ## 关于 `stream`
//!
//! 上游**只**支持流式。`stream: false` 的客户端请求由本网关在本地聚合
//! （见 [`super::sse::aggregate`]），而不是把 `stream` 关掉发给上游。

use serde_json::{json, Map, Value};

use super::{TRAE_APP_ID, TRAE_FUNCTION, TRAE_IDE_VERSION, TRAE_IDE_VERSION_CODE};

/// 模型显示名 → `(config_name, model_name)`。
///
/// 两个名字都要发给上游，且**大小写敏感**：`config_name` 用展示名，
/// `model_name` 是上游内部名（多为小写下划线 + `__dev` 后缀）。
/// 未知模型回落到默认模型而不是报错——客户端传错名字时，给一个能用的模型
/// 比直接 400 更有用。
pub fn model_config(model: &str) -> (&'static str, &'static str) {
    match model.trim().to_lowercase().as_str() {
        "deepseek-v4-flash" => ("DeepSeek-V4-Flash", "deepseek_v4_flash__dev"),
        "deepseek-v4-flash-official" => (
            "DeepSeek-V4-Flash-Official",
            "DeepSeek-V4-Flash-Official__dev",
        ),
        "deepseek-v4-pro" => ("DeepSeek-V4-Pro", "deepseek_v4_pro__dev"),
        "glm-5.2" => ("glm-5.2", "glm-5.2__dev"),
        "glm-5.3" => ("glm-5.3", "glm-5.3__dev"),
        "doubao-seed-2.1-pro" | "seed-code-pro-0430" => {
            ("Doubao-Seed-2.1-Pro", "Doubao-Seed-2.1-Pro__dev")
        }
        "doubao-seed-2.1-turbo" => ("Doubao-Seed-2.1-Turbo", "Doubao-Seed-2.1-Turbo__dev"),
        "kimi-k2.7-code" => ("kimi-k2.7-code", "kimi-k2.7-code__dev"),
        "minimax-m3" => ("minimax-m3", "minimax-m3__dev"),
        _ => ("DeepSeek-V4-Flash", "deepseek_v4_flash__dev"),
    }
}

/// 网关对外暴露的模型清单（`/v1/models`）。
pub const MODEL_NAMES: &[&str] = &[
    "doubao-seed-2.1-pro",
    "doubao-seed-2.1-turbo",
    "doubao-seed-2.0-code",
    "deepseek-v4-flash",
    "deepseek-v4-pro",
    "glm-5.2",
    "glm-5.3",
    "glm-5-turbo",
    "glm-5",
    "kimi-k2.7-code",
    "kimi-k3",
    "kimi-k2.6",
    "minimax-m3",
    "qwen-3.7-plus",
    "sagitta",
    "aquila",
];

/// `/v1/models` 的响应体。
pub fn models_response() -> Value {
    json!({
        "object": "list",
        "data": MODEL_NAMES
            .iter()
            .map(|name| json!({
                "id": name,
                "object": "model",
                "created": 1_753_600_000,
                "owned_by": "trae",
                "context_length": 131_072,
            }))
            .collect::<Vec<_>>(),
    })
}

/// 生成一个 UUID 形态的随机串（上游要求 `conversation_id` 等字段形如 UUID）。
fn uuid_like() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 从 OpenAI 请求体读取模型名；缺失或空白时用 `default_model`。
pub fn model_of(body: &Value, default_model: &str) -> String {
    body.get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_model)
        .to_string()
}

/// 客户端是否要求流式响应。
pub fn wants_stream(body: &Value) -> bool {
    body.get("stream").and_then(Value::as_bool).unwrap_or(false)
}

/// 改写为 `llm_utils_chat` 请求体。
///
/// `src` 不是合法 JSON 对象时原样返回（交给上游报错，而不是在这里吞掉）。
pub fn prepare_llm_chat_body(
    src: &[u8],
    default_model: &str,
    uid: &str,
    device_id: &str,
    machine_id: &str,
) -> Vec<u8> {
    let mut body: Value = match serde_json::from_slice(src) {
        Ok(value) => value,
        Err(_) => return src.to_vec(),
    };
    let Some(object) = body.as_object_mut() else {
        return src.to_vec();
    };

    normalize_messages(object);
    normalize_tool_choice(object);
    normalize_tools(object);

    let model = object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(default_model)
        .to_string();
    let (config_name, model_name) = model_config(&model);

    // 固定字段：覆盖客户端可能传来的同名值（上游只认这一套）。
    object.insert("config_name".into(), json!(config_name));
    object.insert("model_name".into(), json!(model_name));
    // 上游只支持流式；`stream:false` 由本网关本地聚合。
    object.insert("stream".into(), json!(true));
    object.insert("function".into(), json!(TRAE_FUNCTION));
    object.insert("conversation_id".into(), json!(uuid_like()));
    object.insert("user_id".into(), json!(uid));
    object.insert("session_id".into(), json!(uuid_like()));
    object.insert("device_id".into(), json!(device_id));
    object.insert("machine_id".into(), json!(machine_id));
    object.insert("project_id".into(), json!(uuid_like()));
    object.insert("workspace_id".into(), json!("e04cdd"));
    object.insert("prompt_max_tokens".into(), json!(168_000));
    object.insert("mode".into(), json!("FunctionCall"));
    object.insert("ide_version".into(), json!(TRAE_IDE_VERSION));
    object.insert("ide_version_code".into(), json!(TRAE_IDE_VERSION_CODE));
    object.insert("app_id".into(), json!(TRAE_APP_ID));
    object.insert("package_type".into(), json!("stable_cn"));

    serde_json::to_vec(&body).unwrap_or_else(|_| src.to_vec())
}

/// 消息数组的三处改写：`content` 转数组、`tool_calls` 转 `function_call`、
/// 丢掉没有函数名的空 tool_call。
fn normalize_messages(object: &mut Map<String, Value>) {
    let Some(messages) = object.get_mut("messages").and_then(Value::as_array_mut) else {
        return;
    };
    for message in messages.iter_mut() {
        let Some(message) = message.as_object_mut() else {
            continue;
        };

        if message.get("role").and_then(Value::as_str) == Some("assistant") {
            if let Some(calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) {
                let kept: Vec<Value> = calls
                    .iter_mut()
                    .filter_map(|call| {
                        let call = call.as_object_mut()?;
                        if let Some(function) = call.remove("function") {
                            call.insert("function_call".into(), function);
                        }
                        // 没有函数名的 tool_call 是上游会拒绝的脏数据，直接丢弃。
                        let named = call
                            .get("function_call")
                            .and_then(|function| function.get("name"))
                            .and_then(Value::as_str)
                            .map(|name| !name.trim().is_empty())
                            .unwrap_or(false);
                        named.then(|| Value::Object(call.clone()))
                    })
                    .collect();
                if kept.is_empty() {
                    message.remove("tool_calls");
                } else {
                    *calls = kept;
                }
            }
        }

        // `content` 字符串 → 数组形态。
        if let Some(text) = message.get("content").and_then(Value::as_str) {
            message.insert("content".into(), json!([{ "type": "text", "text": text }]));
        }
    }
}

/// `tool_choice` 归一化：上游只认 `auto` / `required` / 具体函数名 / 缺省。
fn normalize_tool_choice(object: &mut Map<String, Value>) {
    let Some(choice) = object.remove("tool_choice") else {
        return;
    };
    let suppress = |object: &mut Map<String, Value>| {
        object.remove("tools");
        object.remove("functions");
    };
    match choice {
        Value::String(text) => {
            if text.trim().eq_ignore_ascii_case("none") {
                suppress(object);
            } else {
                object.insert("tool_choice".into(), Value::String(text));
            }
        }
        Value::Object(map) => {
            let kind = map
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_lowercase)
                .unwrap_or_default();
            match kind.as_str() {
                "none" => suppress(object),
                "auto" | "required" => {
                    object.insert("tool_choice".into(), Value::String(kind));
                }
                "function" => {
                    let name = map
                        .get("function")
                        .and_then(|function| function.get("name"))
                        .and_then(Value::as_str)
                        .or_else(|| map.get("name").and_then(Value::as_str))
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .unwrap_or("auto")
                        .to_string();
                    object.insert("tool_choice".into(), Value::String(name));
                }
                _ => {}
            }
        }
        _ => {}
    }
}

/// `tools` 归一化：`parameters` 对象 → JSON 字符串；无函数名的条目丢弃。
fn normalize_tools(object: &mut Map<String, Value>) {
    let Some(raw) = object.get_mut("tools") else {
        return;
    };
    let Some(list) = raw.as_array_mut() else {
        return;
    };
    if list.is_empty() {
        object.remove("tools");
        return;
    }
    let mut kept = Vec::with_capacity(list.len());
    for item in list.iter_mut() {
        let Some(tool) = item.as_object_mut() else {
            continue;
        };
        let Some(function) = tool.get_mut("function").and_then(Value::as_object_mut) else {
            continue;
        };
        if let Some(parameters) = function.get("parameters") {
            if parameters.is_object() {
                if let Ok(text) = serde_json::to_string(parameters) {
                    function.insert("parameters".into(), Value::String(text));
                }
            }
        }
        kept.push(item.clone());
    }
    if kept.is_empty() {
        object.remove("tools");
    } else {
        *raw = Value::Array(kept);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(body: Value) -> Value {
        let bytes = serde_json::to_vec(&body).unwrap();
        let out = prepare_llm_chat_body(&bytes, "deepseek-v4-flash", "u1", "dev1", "mach1");
        serde_json::from_slice(&out).expect("output must stay valid JSON")
    }

    #[test]
    fn model_config_is_case_insensitive_and_falls_back() {
        assert_eq!(model_config("DeepSeek-V4-Flash"), ("DeepSeek-V4-Flash", "deepseek_v4_flash__dev"));
        assert_eq!(model_config("  GLM-5.3 "), ("glm-5.3", "glm-5.3__dev"));
        assert_eq!(model_config("seed-code-pro-0430"), ("Doubao-Seed-2.1-Pro", "Doubao-Seed-2.1-Pro__dev"));
        // 未知模型回落默认，而不是报错。
        assert_eq!(model_config("no-such-model"), ("DeepSeek-V4-Flash", "deepseek_v4_flash__dev"));
        assert_eq!(model_config(""), ("DeepSeek-V4-Flash", "deepseek_v4_flash__dev"));
    }

    #[test]
    fn string_content_is_converted_to_text_array() {
        let out = prepared(json!({
            "model": "glm-5.3",
            "messages": [{"role": "user", "content": "你好"}],
        }));
        assert_eq!(
            out["messages"][0]["content"],
            json!([{"type": "text", "text": "你好"}])
        );
    }

    #[test]
    fn assistant_tool_calls_become_function_call_and_unnamed_are_dropped() {
        let out = prepared(json!({
            "model": "glm-5.3",
            "messages": [{
                "role": "assistant",
                "content": "",
                "tool_calls": [
                    {"id": "1", "function": {"name": "read_file", "arguments": "{}"}},
                    {"id": "2", "function": {"name": "  ", "arguments": "{}"}}
                ],
            }],
        }));
        let calls = out["messages"][0]["tool_calls"].as_array().unwrap();
        assert_eq!(calls.len(), 1, "无名 tool_call 必须被丢弃");
        assert_eq!(calls[0]["function_call"]["name"], "read_file");
        assert!(calls[0].get("function").is_none(), "不得同时保留 function");
    }

    #[test]
    fn tool_parameters_object_is_stringified() {
        let out = prepared(json!({
            "model": "glm-5.3",
            "messages": [],
            "tools": [{
                "type": "function",
                "function": {"name": "f", "parameters": {"type": "object", "properties": {}}}
            }],
        }));
        let parameters = &out["tools"][0]["function"]["parameters"];
        assert!(parameters.is_string(), "parameters 必须是 JSON 字符串");
        let parsed: Value = serde_json::from_str(parameters.as_str().unwrap()).unwrap();
        assert_eq!(parsed["type"], "object");
    }

    #[test]
    fn tool_choice_none_suppresses_tools() {
        let out = prepared(json!({
            "model": "glm-5.3",
            "messages": [],
            "tool_choice": "none",
            "tools": [{"type": "function", "function": {"name": "f"}}],
        }));
        assert!(out.get("tools").is_none());
        assert!(out.get("tool_choice").is_none());
    }

    #[test]
    fn tool_choice_function_maps_to_a_plain_name() {
        let out = prepared(json!({
            "model": "glm-5.3",
            "messages": [],
            "tool_choice": {"type": "function", "function": {"name": "read_file"}},
        }));
        assert_eq!(out["tool_choice"], "read_file");
    }

    #[test]
    fn required_fields_are_always_injected_and_stream_is_forced() {
        let out = prepared(json!({
            "model": "glm-5.3",
            "messages": [],
            "stream": false,
            "max_tokens": 1,
        }));
        // 上游只支持流式：客户端说 false，这里仍必须发 true。
        assert_eq!(out["stream"], true);
        assert_eq!(out["function"], TRAE_FUNCTION);
        assert_eq!(out["app_id"], TRAE_APP_ID);
        assert_eq!(out["ide_version"], TRAE_IDE_VERSION);
        assert_eq!(out["ide_version_code"], TRAE_IDE_VERSION_CODE);
        assert_eq!(out["workspace_id"], "e04cdd");
        assert_eq!(out["mode"], "FunctionCall");
        assert_eq!(out["user_id"], "u1");
        assert_eq!(out["device_id"], "dev1");
        assert_eq!(out["machine_id"], "mach1");
        assert_eq!(out["config_name"], "glm-5.3");
        assert_eq!(out["model_name"], "glm-5.3__dev");
        // 生成型字段必须存在且形如 UUID。
        for key in ["conversation_id", "session_id", "project_id"] {
            let value = out[key].as_str().expect(key);
            assert!(uuid::Uuid::parse_str(value).is_ok(), "{key}={value}");
        }
    }

    #[test]
    fn missing_model_uses_the_default() {
        let out = prepared(json!({"messages": []}));
        assert_eq!(out["config_name"], "DeepSeek-V4-Flash");
        assert_eq!(out["model_name"], "deepseek_v4_flash__dev");
    }

    #[test]
    fn invalid_json_is_passed_through_untouched() {
        let raw = b"not json at all";
        assert_eq!(prepare_llm_chat_body(raw, "m", "u", "d", "m"), raw.to_vec());
        // 合法 JSON 但不是对象，同样原样返回。
        let array = b"[1,2,3]";
        assert_eq!(prepare_llm_chat_body(array, "m", "u", "d", "m"), array.to_vec());
    }

    #[test]
    fn model_and_stream_readers_are_defensive() {
        assert_eq!(model_of(&json!({"model": "x"}), "d"), "x");
        assert_eq!(model_of(&json!({"model": "  "}), "d"), "d");
        assert_eq!(model_of(&json!({}), "d"), "d");
        assert!(!wants_stream(&json!({})));
        assert!(wants_stream(&json!({"stream": true})));
    }

    #[test]
    fn models_response_lists_every_name_as_openai_objects() {
        let response = models_response();
        let data = response["data"].as_array().unwrap();
        assert_eq!(data.len(), MODEL_NAMES.len());
        assert_eq!(data[0]["object"], "model");
        assert_eq!(response["object"], "list");
    }
}
