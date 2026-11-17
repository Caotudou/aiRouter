use ai_router_core::{
    bridge::{Bridge, RoutingDefaults},
    diagnostics::Diagnostics,
    rpc::CodexRpc,
    server,
};
use std::{path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let binary = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .ok_or("missing codex executable")?,
    );
    let cwd = std::env::current_dir()?;
    let rpc = CodexRpc::start(&binary, &cwd).await?;
    let diagnostics = std::env::var_os("AI_ROUTER_DIAGNOSTICS_PATH")
        .map(Diagnostics::open)
        .transpose()?
        .map(Arc::new);
    let bridge = Arc::new(Bridge::with_diagnostics(rpc.clone(), cwd, diagnostics));
    let handle = server::start(
        bridge.clone(),
        "smoke-test-token-0123456789abcdef".into(),
        18329,
    )
    .await?;
    println!("READY");
    loop {
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 || line.trim() == "stop" {
            break;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
            if let (Some(model), Some(effort)) = (value["model"].as_str(), value["effort"].as_str())
            {
                bridge.set_defaults(RoutingDefaults {
                    model: model.into(),
                    reasoning_effort: effort.into(),
                });
                println!("DEFAULTS_UPDATED");
            }
        }
    }
    handle.stop().await;
    bridge.close().await;
    rpc.stop().await?;
    Ok(())
}
