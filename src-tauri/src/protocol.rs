use serde_json::{json, Value};
use std::collections::HashMap;
use thiserror::Error;

pub const DEFAULT_MODEL: &str = "gpt-6.1-sol";
pub const DEFAULT_REASONING_EFFORT: &str = "high";

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("{0}")]
    InvalidRequest(&'static str),
    #[error("{0}")]
    Unsupported(&'static str),
}

/// Canonical representation used to match a client's history with a live turn.
pub fn normalize_messages(messages: &Value) -> Result<Vec<Value>, ProtocolError> {
    let messages = messages
        .as_array()
        .filter(|m| !m.is_empty())
        .ok_or(ProtocolError::InvalidRequest("messages 必须是非空数组。"))?;
    messages.iter().map(|m| {
        let role = m.get("role").and_then(Value::as_str).filter(|r| matches!(*r, "system" | "developer" | "user" | "assistant" | "tool"))
            .ok_or(ProtocolError::InvalidRequest("不支持的消息角色。"))?;
        let content = m.get("content").cloned().unwrap_or(Value::Null);
        if !(content.is_string() || content.is_null() || content.is_array()) {
            return Err(ProtocolError::InvalidRequest("content 必须为文本或内容数组。"));
        }
        if let Some(parts) = content.as_array() {
            for part in parts {
                let valid = match part.get("type").and_then(Value::as_str) {
                    Some("text") => part.get("text").is_some_and(Value::is_string),
                    Some("image_url") => part.pointer("/image_url/url").is_some_and(Value::is_string),
                    _ => false,
                };
                if !valid { return Err(ProtocolError::Unsupported("首版只支持文本和 image_url 图片内容。")); }
            }
        }
        let mut out = json!({"role":role,"content":content});
        if let Some(name) = m.get("name") { out["name"] = name.clone(); }
        if let Some(calls) = m.get("tool_calls") {
            let calls = calls.as_array().ok_or(ProtocolError::InvalidRequest("tool_calls 格式无效。"))?;
            let mut normalized = Vec::with_capacity(calls.len());
            for call in calls {
                let id = call.get("id").and_then(Value::as_str).filter(|v| !v.is_empty());
                let name = call.pointer("/function/name").and_then(Value::as_str);
                let args = call.pointer("/function/arguments").and_then(Value::as_str);
                if call.get("type").and_then(Value::as_str) != Some("function") || id.is_none() || name.is_none() || args.is_none() {
                    return Err(ProtocolError::InvalidRequest("tool_calls 格式无效。"));
                }
                normalized.push(json!({"id":id.unwrap(),"type":"function","function":{"name":name.unwrap(),"arguments":args.unwrap()}}));
            }
            out["tool_calls"] = Value::Array(normalized);
            if out["content"] == "" { out["content"] = Value::Null; }
        }
        if role == "tool" {
            out["tool_call_id"] = m.get("tool_call_id").filter(|v| v.is_string()).cloned()
                .ok_or(ProtocolError::InvalidRequest("工具结果缺少 tool_call_id。"))?;
        }
        Ok(out)
    }).collect()
}

/// Translate OpenAI function definitions to Codex dynamic-tool definitions.
pub fn dynamic_tools(
    tools: &Value,
    choice: Option<&str>,
) -> Result<(Vec<Value>, HashMap<String, String>), ProtocolError> {
    let tools = tools
        .as_array()
        .ok_or(ProtocolError::InvalidRequest("tools 必须为数组。"))?;
    if !matches!(choice, None | Some("auto") | Some("none")) {
        return Err(ProtocolError::Unsupported(
            "首版支持 tool_choice=auto/none，暂不支持强制指定工具。",
        ));
    }
    let mut result = Vec::new();
    let mut names = HashMap::new();
    for (index, tool) in tools.iter().enumerate() {
        let name = tool
            .pointer("/function/name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        let schema = tool
            .pointer("/function/parameters")
            .filter(|s| s.is_object());
        if tool.get("type").and_then(Value::as_str) != Some("function")
            || name.is_none()
            || schema.is_none()
        {
            return Err(ProtocolError::InvalidRequest(
                "首版仅支持具有 parameters 的 function 工具。",
            ));
        }
        let name = name.unwrap();
        let codex_name = format!("wb_tool_{index}");
        let description = tool
            .pointer("/function/description")
            .and_then(Value::as_str)
            .unwrap_or("");
        names.insert(codex_name.clone(), name.to_owned());
        if choice != Some("none") {
            result.push(json!({"type":"function","name":codex_name,"description":format!("{name}: {description}"),"inputSchema":schema.unwrap()}));
        }
    }
    Ok((result, names))
}

pub fn text_content(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_owned();
    }
    content
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter(|part| part["type"] == "text")
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Build the user inputs accepted by `turn/start`.
pub fn user_input(messages: &[Value]) -> Vec<Value> {
    let mut input = Vec::new();
    for m in messages {
        let role = m["role"].as_str().unwrap_or_default();
        if role == "system" || role == "developer" {
            continue;
        }
        let content = &m["content"];
        let text = text_content(content);
        if !text.is_empty() {
            let text = if role == "user" {
                text
            } else {
                format!("[{role}] {text}")
            };
            input.push(json!({"type":"text","text":text,"text_elements":[]}));
        }
        if let Some(parts) = content.as_array() {
            for part in parts.iter().filter(|p| p["type"] == "image_url") {
                input.push(json!({"type":"image","url":part["image_url"]["url"],"detail":part["image_url"].get("detail").and_then(Value::as_str).unwrap_or("auto")}));
            }
        }
        if let Some(calls) = m.get("tool_calls") {
            input.push(json!({"type":"text","text":format!("Previous assistant tool calls: {calls}"),"text_elements":[]}));
        }
    }
    input
}

pub fn tool_output(content: &Value) -> Vec<Value> {
    match content {
        Value::String(s) => vec![json!({"type":"inputText","text":s})],
        Value::Array(parts) => parts
            .iter()
            .map(|p| {
                if p["type"] == "text" {
                    json!({"type":"inputText","text":p["text"]})
                } else {
                    json!({"type":"inputImage","imageUrl":p["image_url"]["url"]})
                }
            })
            .collect(),
        _ => vec![json!({"type":"inputText","text":""})],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalize_tool_call_for_streamed_empty_content() {
        let m = normalize_messages(&json!([{ "role":"assistant","content":"","tool_calls":[{"id":"call_1","type":"function","function":{"name":"read","arguments":"{}"}}] }])).unwrap();
        assert!(m[0]["content"].is_null());
        assert_eq!(m[0]["tool_calls"][0]["id"], "call_1");
    }
    #[test]
    fn dynamic_tools_keep_client_names_and_schema() {
        let (spec, names) = dynamic_tools(&json!([{"type":"function","function":{"name":"read_file","description":"read","parameters":{"type":"object"}}}]), None).unwrap();
        assert_eq!(names["wb_tool_0"], "read_file");
        assert_eq!(spec[0]["inputSchema"]["type"], "object");
        assert!(dynamic_tools(&json!([]), Some("required")).is_err());
    }
    #[test]
    fn text_and_image_are_mapped_to_codex_items() {
        let messages = normalize_messages(&json!([{"role":"user","content":[{"type":"text","text":"hi"},{"type":"image_url","image_url":{"url":"data:image/png;base64,abc"}}]}])).unwrap();
        let input = user_input(&messages);
        assert_eq!(input[0]["text"], "hi");
        assert_eq!(input[1]["type"], "image");
        assert_eq!(input[1]["detail"], "auto");
    }
    #[test]
    fn invalid_media_and_unpaired_tool_results_are_rejected() {
        assert!(
            normalize_messages(&json!([{"role":"user","content":[{"type":"audio"}]}])).is_err()
        );
        assert!(normalize_messages(&json!([{"role":"tool","content":"x"}])).is_err());
    }
}
