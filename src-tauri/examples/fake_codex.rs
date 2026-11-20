//! Deterministic app-server fixture used by scripts/test-rust-bridge.mjs.
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn emit(value: Value) {
    println!("{value}");
    io::stdout().flush().unwrap();
}
fn emit_usage(thread: &Value, input: u64, output: u64, cached: u64) {
    let total = json!({"inputTokens":input,"outputTokens":output,"cachedInputTokens":cached,"totalTokens":input+output});
    emit(json!({"method":"thread/tokenUsage/updated","params":{"threadId":thread,"turnId":"turn-1","tokenUsage":{"total":total,"last":total}}}));
}
fn main() {
    let mut thread = 0u64;
    let mut models = std::collections::HashMap::<String, String>::new();
    let mut awaiting: Option<(String, usize)> = None;
    let mut steered = false;
    let mut chain_next = false;
    let mut usage_threads = std::collections::HashSet::new();
    for line in io::stdin().lock().lines().map_while(Result::ok) {
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = message["method"].as_str().unwrap_or("");
        let id = &message["id"];
        if method.is_empty() && id.is_number() {
            if let Some((current, count)) = awaiting.as_mut() {
                *count -= 1;
                if *count == 0 {
                    let current = current.clone();
                    if chain_next {
                        chain_next = false;
                        emit(
                            json!({"id":101,"method":"item/tool/call","params":{"threadId":current,"tool":"wb_tool_0","arguments":{}}}),
                        );
                        awaiting = Some((current, 1));
                        continue;
                    }
                    let answer = if steered {
                        "tools complete; user instruction received"
                    } else {
                        "tools complete"
                    };
                    if usage_threads.contains(&current) {
                        emit_usage(&json!(current), 42, 8, 12);
                    }
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"threadId":current,"delta":answer}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"threadId":current,"turn":{"status":"completed"}}}),
                    );
                    awaiting = None;
                    steered = false;
                }
            }
            continue;
        }
        match method {
            "initialize" => emit(json!({"id":id,"result":{}})),
            "model/list" => emit(
                json!({"id":id,"result":{"data":[{"model":"gpt-6.1-sol","supportedReasoningEfforts":[{"reasoningEffort":"high"}]}],"nextCursor":null}}),
            ),
            "thread/start" => {
                thread += 1;
                models.insert(
                    format!("thread-{thread}"),
                    message["params"]["model"].as_str().unwrap_or("").to_owned(),
                );
                emit(json!({"id":id,"result":{"thread":{"id":format!("thread-{thread}")}}}));
            }
            "turn/start" => {
                let current = &message["params"]["threadId"];
                let input = message["params"]["input"].to_string();
                emit(json!({"id":id,"result":{"turn":{"id":"turn-1"}}}));
                if input.contains("usage-fixture") {
                    usage_threads.insert(current.as_str().unwrap().to_owned());
                    emit_usage(current, 20, 4, 6);
                    emit_usage(current, 20, 4, 6);
                }
                if input.contains("two-tools")
                    || input.contains("one-tool")
                    || input.contains("chain-tools")
                {
                    let count = if input.contains("two-tools") { 2 } else { 1 };
                    chain_next = input.contains("chain-tools");
                    if count == 1 {
                        emit(
                            json!({"method":"item/agentMessage/delta","params":{"threadId":current,"delta":"streamed text"}}),
                        );
                    }
                    for n in 0..count {
                        emit(
                            json!({"id":100+n,"method":"item/tool/call","params":{"threadId":current,"tool":format!("wb_tool_{n}"),"arguments":{}}}),
                        );
                    }
                    awaiting = Some((current.as_str().unwrap().into(), count));
                } else if input.contains("fail-turn") {
                    emit(
                        json!({"method":"turn/completed","params":{"threadId":current,"turn":{"status":"failed","error":{"message":"fixture failure"}}}}),
                    );
                } else if input.contains("inspect-settings") {
                    let model = models
                        .get(current.as_str().unwrap_or(""))
                        .map(String::as_str)
                        .unwrap_or("");
                    let effort = message["params"]["effort"].as_str().unwrap_or("");
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"threadId":current,"delta":format!("model={model};effort={effort}")}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"threadId":current,"turn":{"status":"completed"}}}),
                    );
                } else if !input.contains("wait-turn") {
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"threadId":current,"delta":"fixture response"}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"threadId":current,"turn":{"status":"completed"}}}),
                    );
                }
            }
            "turn/steer" => {
                if awaiting.is_some()
                    && message["params"]["expectedTurnId"] == "turn-1"
                    && message["params"]["input"].is_array()
                {
                    steered = true;
                    emit(json!({"id":id,"result":{"turnId":"turn-1"}}));
                } else {
                    emit(json!({"id":id,"error":{"code":-32602,"message":"no active test turn"}}));
                }
            }
            "turn/interrupt" | "thread/unsubscribe" => emit(json!({"id":id,"result":{}})),
            _ => {}
        }
    }
}
