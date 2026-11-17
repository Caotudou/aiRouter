//! Inspect the current Codex model catalog without making a model request.
use ai_router_core::rpc::CodexRpc;
use serde_json::json;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let binary = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("missing codex executable")?,
    );
    let rpc = CodexRpc::start(&binary, &std::env::current_dir()?).await?;
    let mut cursor = serde_json::Value::Null;
    loop {
        let result = rpc
            .request(
                "model/list",
                json!({"limit":100,"includeHidden":false,"cursor":cursor}),
            )
            .await?;
        for model in result["data"].as_array().into_iter().flatten() {
            println!(
                "{}: {}",
                model["model"].as_str().unwrap_or("?"),
                model["supportedReasoningEfforts"]
                    .as_array()
                    .map(|items| items
                        .iter()
                        .filter_map(|item| item["reasoningEffort"].as_str())
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            );
        }
        cursor = result["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    rpc.stop().await?;
    Ok(())
}
