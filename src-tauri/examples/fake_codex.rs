//! Deterministic app-server fixture used by scripts/test-rust-bridge.mjs.
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};

fn emit(value: Value) {
    println!("{value}");
    io::stdout().flush().unwrap();
}
fn main() {
    let mut thread = 0u64;
    let mut awaiting: Option<(String, usize)> = None;
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
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"threadId":current,"delta":"tools complete"}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"threadId":current,"turn":{"status":"completed"}}}),
                    );
                    awaiting = None;
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
                emit(json!({"id":id,"result":{"thread":{"id":format!("thread-{thread}")}}}));
            }
            "turn/start" => {
                let current = &message["params"]["threadId"];
                let input = message["params"]["input"].to_string();
                emit(json!({"id":id,"result":{"turn":{"id":"turn-1"}}}));
                if input.contains("two-tools") || input.contains("one-tool") {
                    let count = if input.contains("one-tool") { 1 } else { 2 };
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
                } else if !input.contains("wait-turn") {
                    emit(
                        json!({"method":"item/agentMessage/delta","params":{"threadId":current,"delta":"fixture response"}}),
                    );
                    emit(
                        json!({"method":"turn/completed","params":{"threadId":current,"turn":{"status":"completed"}}}),
                    );
                }
            }
            "turn/interrupt" | "thread/unsubscribe" => emit(json!({"id":id,"result":{}})),
            _ => {}
        }
    }
}
