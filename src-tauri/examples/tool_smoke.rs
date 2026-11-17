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
    let thread = rpc.request("thread/start", json!({
        "model":"gpt-6.1-sol", "modelProvider":"openai", "ephemeral":true,
        "cwd":cwd, "environments":[], "approvalPolicy":"untrusted", "sandbox":"read-only",
        "baseInstructions":"You must use the provided test tool when asked. Do not use native tools.",
        "dynamicTools":[{"type":"function","name":"wb_tool_0","description":"Get the secret test value; call this to learn it.","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}]
    })).await?;
    let id = thread["thread"]["id"].as_str().ok_or("missing thread id")?;
    rpc.request("turn/start", json!({"threadId":id,"effort":"high","environments":[],
        "input":[{"type":"text","text":"Call wb_tool_0 exactly once and report the value it returns.","text_elements":[]}]})).await?;
    let text = tokio::time::timeout(Duration::from_secs(90), async {
        let mut text = String::new();
        let mut called = false;
        loop {
            let event = events.recv().await.map_err(|e| e.to_string())?;
            if event["params"]["threadId"] != id { continue; }
            match event["method"].as_str() {
                Some("item/tool/call") if event["params"]["tool"] == "wb_tool_0" => {
                    called = true;
                    rpc.reply(event["id"].clone(), json!({"success":true,"contentItems":[{"type":"inputText","text":"RUST_TOOL_7392"}]})).await?;
                },
                Some("item/agentMessage/delta") => text.push_str(event["params"]["delta"].as_str().unwrap_or("")),
                Some("turn/completed") if event["params"]["turn"]["status"] == "completed" && called => return Ok::<_, String>(text),
                Some("turn/completed") => return Err(format!("turn failed or tool was not called: {}", event["params"]["turn"]["status"])),
                _ => {}
            }
        }
    }).await??;
    println!("tool response: {text}");
    if !text.contains("RUST_TOOL_7392") {
        return Err("tool response not used".into());
    }
    rpc.stop().await?;
    Ok(())
}
