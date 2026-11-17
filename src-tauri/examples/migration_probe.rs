#[cfg(all(feature = "desktop", windows))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let legacy = ai_router_core::legacy_windows::read_token()?.ok_or("old token not found")?;
    let home = std::env::var_os("USERPROFILE").ok_or("missing USERPROFILE")?;
    let models: serde_json::Value = serde_json::from_slice(&std::fs::read(
        std::path::PathBuf::from(home)
            .join(".workbuddy")
            .join("models.json"),
    )?)?;
    let configured = models
        .as_array()
        .and_then(|a| a.iter().find(|m| m["id"] == "gpt-6.1-sol"))
        .and_then(|m| m["apiKey"].as_str())
        .ok_or("WorkBuddy model not found")?;
    if legacy != configured {
        return Err("existing WorkBuddy key does not match migrated key".into());
    }
    println!("Existing WorkBuddy key migration verified (value not displayed).");
    Ok(())
}
#[cfg(not(all(feature = "desktop", windows)))]
fn main() {
    eprintln!("Windows desktop feature required");
}
