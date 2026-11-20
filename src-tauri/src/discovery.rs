use std::{
    env,
    path::{Path, PathBuf},
};

fn executable_name() -> &'static str {
    if cfg!(windows) {
        "codex.exe"
    } else {
        "codex"
    }
}

/// Resolve the user's explicit executable, or discover the installed Codex.
/// No account credential is read or copied by the router.
pub fn resolve_codex(configured: &str) -> Result<PathBuf, String> {
    let mut name = configured.trim().to_owned();
    if name.eq_ignore_ascii_case("codex") || name.eq_ignore_ascii_case("codex.exe") {
        if let Ok(override_path) = env::var("CODEX_BIN") {
            if !override_path.trim().is_empty() {
                name = override_path.trim().to_owned();
            }
        }
    }
    if !name.eq_ignore_ascii_case("codex") && !name.eq_ignore_ascii_case("codex.exe") {
        let path = PathBuf::from(&name);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!("Codex 程序不存在：{name}。请重新选择。"))
        };
    }
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            let candidate = directory.join(executable_name());
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    for candidate in platform_candidates() {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err("没有找到 Codex。请安装 Codex，或在连接设置中选择可执行程序。".into())
}

fn platform_candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let home = env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    if let Some(home) = home.as_deref() {
        paths.push(
            Path::new(home)
                .join(".local")
                .join("bin")
                .join(executable_name()),
        );
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        if let Some(local) = env::var_os("LOCALAPPDATA") {
            paths.push(
                Path::new(&local)
                    .join("OpenAI")
                    .join("Codex")
                    .join("bin")
                    .join("codex.exe"),
            );
        }
        if let Some(root) = env::var_os("SystemRoot") {
            let powershell = Path::new(&root)
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe");
            if let Ok(output) = std::process::Command::new(powershell)
                .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", "@((Get-AppxPackage -Name OpenAI.Codex).InstallLocation) | ConvertTo-Json -Compress"])
                .creation_flags(0x08000000).output() {
                if let Ok(json) = serde_json::from_slice::<serde_json::Value>(&output.stdout) {
                    let locations: Vec<&str> = match &json {
                        serde_json::Value::String(s) => vec![s],
                        serde_json::Value::Array(a) => a.iter().filter_map(serde_json::Value::as_str).collect(),
                        _ => vec![],
                    };
                    for location in locations { paths.push(Path::new(location).join("app").join("resources").join("codex.exe")); }
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        paths.extend(["/opt/homebrew/bin/codex", "/usr/local/bin/codex"].map(PathBuf::from));
        let mut app_dirs = vec![PathBuf::from("/Applications")];
        if let Some(home) = home.as_deref() {
            app_dirs.push(Path::new(home).join("Applications"));
        }
        for app_dir in app_dirs {
            for app in ["Codex.app", "ChatGPT.app"] {
                let resources = app_dir.join(app).join("Contents/Resources");
                paths.push(resources.join("codex-cli/CodexCLI.app/Contents/MacOS/codex"));
                paths.push(resources.join("codex"));
            }
        }
    }
    paths
}
