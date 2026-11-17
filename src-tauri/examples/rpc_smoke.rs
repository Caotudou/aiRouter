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
    let cwd = std::env::current_dir()?;
    let rpc = CodexRpc::start(&binary, &cwd).await?;
    let account = rpc
        .request("account/read", json!({"refreshToken":false}))
        .await?;
    let models = rpc
        .request("model/list", json!({"limit":100,"includeHidden":false}))
        .await?;
    println!(
        "auth type: {}",
        account["account"]["type"].as_str().unwrap_or("none")
    );
    println!(
        "model count: {}",
        models["data"].as_array().map_or(0, Vec::len)
    );
    rpc.stop().await?;
    Ok(())
}
