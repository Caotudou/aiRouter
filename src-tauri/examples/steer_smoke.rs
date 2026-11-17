//! Real Codex probe: steer an active turn while a dynamic tool is pending.
use ai_router_core::rpc::CodexRpc;
use serde_json::json;
use std::{path::PathBuf, time::Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let binary = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("missing codex executable")?,
    );
    let cwd = std::env::current_dir()?;
    let rpc = CodexRpc::start(&binary, &cwd).await?;
    let mut events = rpc.subscribe();
    let thread = rpc.request("thread/start", json!({"model":"gpt-6.1-sol","modelProvider":"openai","ephemeral":true,"cwd":cwd,"environments":[],"approvalPolicy":"untrusted","sandbox":"read-only","baseInstructions":"Use the provided test tool when asked. Do not use native tools.","dynamicTools":[{"type":"function","name":"wb_tool_0","description":"Get the test value","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}]})).await?;
    let id = thread["thread"]["id"].as_str().ok_or("missing thread id")?;
    let turn = rpc.request("turn/start", json!({"threadId":id,"effort":"high","environments":[],"input":[{"type":"text","text":"Call wb_tool_0, then report its result.","text_elements":[]}]})).await?;
    let turn_id = turn["turn"]["id"].as_str().ok_or("missing turn id")?;
    let text = tokio::time::timeout(Duration::from_secs(90), async {
        let mut text = String::new();
        let mut called = false;
        loop {
            let event = events.recv().await.map_err(|e|e.to_string())?;
            if event["params"]["threadId"] != id { continue; }
            match event["method"].as_str() {
                Some("item/tool/call") => {
                    rpc.request("turn/steer", json!({"threadId":id,"expectedTurnId":turn_id,"input":[{"type":"text","text":"Also include exactly STEER_OK_4819 in your final answer.","text_elements":[]}]})).await?;
                    rpc.reply(event["id"].clone(), json!({"success":true,"contentItems":[{"type":"inputText","text":"TOOL_OK_4819"}]})).await?;
                    called = true;
                }
                Some("item/agentMessage/delta") => text.push_str(event["params"]["delta"].as_str().unwrap_or("")),
                Some("turn/completed") if event["params"]["turn"]["status"] == "completed" && called => return Ok::<_,String>(text),
                Some("turn/completed") => return Err(format!("turn failed: {}",event["params"]["turn"]["status"])),
                _ => {}
            }
        }
    }).await??;
    if !text.contains("STEER_OK_4819") || !text.contains("TOOL_OK_4819") {
        return Err("steer or tool value missing in model response".into());
    }
    println!("Real Codex accepted steering and tool result in one active turn.");
    rpc.stop().await?;
    Ok(())
}
