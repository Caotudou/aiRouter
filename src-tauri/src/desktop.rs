use crate::{
    bridge::Bridge,
    diagnostics::Diagnostics,
    discovery::resolve_codex,
    protocol::{DEFAULT_MODEL, DEFAULT_REASONING_EFFORT},
    rpc::CodexRpc,
    server::{self, ServerHandle},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;

#[derive(Clone, Serialize, Deserialize)]
struct Settings {
    port: u16,
    binary: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            port: 18321,
            binary: "codex".into(),
        }
    }
}
struct Core {
    directory: PathBuf,
    settings: Settings,
    token: String,
    resolved: Option<PathBuf>,
    rpc: Option<Arc<CodexRpc>>,
    bridge: Option<Arc<Bridge>>,
    server: Option<ServerHandle>,
    account: Value,
    quota: Value,
    models: Vec<Value>,
    error: Option<String>,
    logs: Vec<Value>,
}
struct AppState(Mutex<Core>);
impl Core {
    fn new(directory: PathBuf) -> Result<Self, String> {
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let settings = std::fs::read(directory.join("settings.json"))
            .ok()
            .and_then(|v| serde_json::from_slice(&v).ok())
            .or_else(|| {
                #[cfg(windows)]
                {
                    crate::legacy_windows::legacy_directory()
                        .and_then(|p| std::fs::read(p.join("settings.json")).ok())
                        .and_then(|v| serde_json::from_slice(&v).ok())
                }
                #[cfg(not(windows))]
                {
                    None
                }
            })
            .unwrap_or_default();
        let entry = keyring::Entry::new("local.airouter.desktop", "local-api-key")
            .map_err(|e| e.to_string())?;
        let token = match entry.get_password() {
            Ok(token) => token,
            Err(keyring::Error::NoEntry) => {
                #[cfg(windows)]
                let legacy = crate::legacy_windows::read_token()
                    .map_err(|e| format!("迁移旧版本机密钥失败：{e}"))?;
                #[cfg(not(windows))]
                let legacy: Option<String> = None;
                let token = legacy.unwrap_or_else(|| {
                    format!(
                        "{}{}",
                        uuid::Uuid::new_v4().simple(),
                        uuid::Uuid::new_v4().simple()
                    )
                });
                entry
                    .set_password(&token)
                    .map_err(|e| format!("无法保存本机密钥：{e}"))?;
                token
            }
            Err(e) => return Err(format!("无法读取本机密钥：{e}")),
        };
        Ok(Self {
            directory,
            settings,
            token,
            resolved: None,
            rpc: None,
            bridge: None,
            server: None,
            account: Value::Null,
            quota: Value::Null,
            models: Vec::new(),
            error: None,
            logs: Vec::new(),
        })
    }
    fn log(&mut self, status: &str) {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.logs
            .insert(0, json!({"at":at,"type":"service","status":status}));
        self.logs.truncate(100);
    }
    async fn connect(&mut self) -> Result<(), String> {
        if let Some(rpc) = &self.rpc {
            if rpc.is_alive() {
                return Ok(());
            }
            self.stop().await;
        }
        let binary = self.settings.binary.clone();
        let path = tokio::time::timeout(
            std::time::Duration::from_secs(12),
            tokio::task::spawn_blocking(move || resolve_codex(&binary)),
        )
        .await
        .map_err(|_| "查找 Codex 超时。".to_string())?
        .map_err(|e| e.to_string())??;
        let cwd = self.directory.join("workspace");
        tokio::fs::create_dir_all(&cwd)
            .await
            .map_err(|e| e.to_string())?;
        let rpc = CodexRpc::start(&path, &cwd).await?;
        let diagnostics =
            Diagnostics::open(self.directory.join("logs").join("session-events.jsonl"))
                .map_err(|e| format!("无法创建会话诊断日志：{e}"))?;
        self.bridge = Some(Arc::new(Bridge::with_diagnostics(
            rpc.clone(),
            cwd,
            Some(Arc::new(diagnostics)),
        )));
        self.rpc = Some(rpc);
        self.resolved = Some(path);
        Ok(())
    }
    async fn refresh(&mut self) {
        let result=async {
            self.connect().await?;
            let rpc=self.rpc.as_ref().ok_or("Codex 尚未连接")?;
            let account=rpc.request("account/read",json!({"refreshToken":false})).await?;
            let models=self.bridge.as_ref().ok_or("服务未初始化")?.models().await.map_err(|e|e.message)?;
            let quota=rpc.request("account/rateLimits/read",json!({})).await.unwrap_or(Value::Null);
            self.account=match account["account"].as_object() { Some(a)=>json!({"type":a.get("type"),"planType":a.get("planType")}),None=>Value::Null };
            self.models=models.into_iter().map(|m|json!({"model":m["model"],"displayName":m["displayName"],"isDefault":m["isDefault"]})).collect();
            self.quota=quota; Ok::<(),String>(())
        }.await;
        self.error = result.err();
    }
    async fn status(&self) -> Value {
        let mut logs = self.logs.clone();
        if let Some(bridge) = &self.bridge {
            logs.extend(bridge.logs().await);
        }
        logs.sort_by_key(|v| std::cmp::Reverse(v["at"].as_u64().unwrap_or(0)));
        logs.truncate(100);
        json!({"running":self.server.is_some(),"connected":self.rpc.as_ref().is_some_and(|r|r.is_alive()),"port":self.settings.port,
          "baseUrl":format!("http://127.0.0.1:{}/v1",self.settings.port),"binary":self.settings.binary,
          "resolvedBinary":self.resolved.as_ref().map(|p|p.to_string_lossy().to_string()),
          "defaultModel":DEFAULT_MODEL,"defaultReasoningEffort":DEFAULT_REASONING_EFFORT,
          "account":self.account,"quota":self.quota,"models":self.models,"error":self.error,"logs":logs,
          "activeSessions":if let Some(b)=&self.bridge {b.count().await}else{0}})
    }
    async fn stop(&mut self) {
        if let Some(bridge) = self.bridge.take() {
            bridge.close().await;
        }
        if let Some(server) = self.server.take() {
            server.stop().await;
        }
        if let Some(rpc) = self.rpc.take() {
            let _ = rpc.stop().await;
        }
        self.log("stopped");
    }
}

#[tauri::command]
async fn router_status(state: State<'_, AppState>) -> Result<Value, String> {
    Ok(state.0.lock().await.status().await)
}
#[tauri::command]
async fn router_refresh(state: State<'_, AppState>) -> Result<Value, String> {
    let mut c = state.0.lock().await;
    c.refresh().await;
    Ok(c.status().await)
}
#[tauri::command]
async fn router_start(state: State<'_, AppState>) -> Result<Value, String> {
    let mut c = state.0.lock().await;
    if c.server.is_none() {
        c.refresh().await;
        if let Some(e) = &c.error {
            return Err(e.clone());
        }
        if c.account["type"] != "chatgpt" {
            return Err("请先在本机 Codex 中使用 ChatGPT 登录。".into());
        }
        c.server = Some(
            server::start(
                c.bridge.clone().ok_or("服务未初始化")?,
                c.token.clone(),
                c.settings.port,
            )
            .await?,
        );
        c.log("started");
    }
    Ok(c.status().await)
}
#[tauri::command]
async fn router_stop(state: State<'_, AppState>) -> Result<Value, String> {
    let mut c = state.0.lock().await;
    c.stop().await;
    Ok(c.status().await)
}
#[tauri::command]
async fn router_save(state: State<'_, AppState>, settings: Settings) -> Result<Value, String> {
    let mut c = state.0.lock().await;
    if c.server.is_some() {
        return Err("请先停止服务再修改设置。".into());
    }
    if settings.port < 1024 || settings.binary.trim().is_empty() {
        return Err("端口须为 1024–65535，且 Codex 路径不能为空。".into());
    }
    c.stop().await;
    c.settings = settings;
    let data = serde_json::to_vec_pretty(&c.settings).map_err(|e| e.to_string())?;
    let file = c.directory.join("settings.json");
    let temp = c.directory.join("settings.tmp");
    tokio::fs::write(&temp, data)
        .await
        .map_err(|e| e.to_string())?;
    tokio::fs::rename(temp, file)
        .await
        .map_err(|e| e.to_string())?;
    c.refresh().await;
    Ok(c.status().await)
}
#[tauri::command]
async fn router_test(state: State<'_, AppState>) -> Result<String, String> {
    let (port, token) = {
        let c = state.0.lock().await;
        if c.server.is_none() {
            return Err("请先启动服务。".into());
        }
        (c.settings.port, c.token.clone())
    };
    let result = tokio::time::timeout(std::time::Duration::from_secs(90), async move {
        let mut conn=tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,port)).await.map_err(|e|e.to_string())?;
        let body=json!({"model":DEFAULT_MODEL,"messages":[{"role":"user","content":"Reply with exactly: AI Router OK"}]}).to_string();
        let request=format!("POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
        conn.write_all(request.as_bytes()).await.map_err(|e|e.to_string())?;
        let mut data=Vec::new();conn.read_to_end(&mut data).await.map_err(|e|e.to_string())?;
        let response=String::from_utf8(data).map_err(|e|e.to_string())?;
        let (headers,body)=response.split_once("\r\n\r\n").ok_or("服务响应格式无效")?;
        let value:Value=serde_json::from_str(body).map_err(|e|e.to_string())?;
        if !headers.starts_with("HTTP/1.1 200") {return Err(value["error"]["message"].as_str().unwrap_or("连接测试失败").to_owned())}
        Ok::<Value,String>(value)
    }).await.map_err(|_|"连接测试超时。".to_string())??;
    Ok(result["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .into())
}
#[tauri::command]
async fn router_copy(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    field: String,
) -> Result<(), String> {
    let c = state.0.lock().await;
    let value = match field.as_str() {
        "url" => format!("http://127.0.0.1:{}/v1", c.settings.port),
        "key" => c.token.clone(),
        "model" => DEFAULT_MODEL.into(),
        "all" => format!(
            "Base URL: http://127.0.0.1:{}/v1\nAPI Key: {}\nModel: {}\nReasoning effort: {}",
            c.settings.port, c.token, DEFAULT_MODEL, DEFAULT_REASONING_EFFORT
        ),
        _ => return Err("未知复制字段。".into()),
    };
    app.clipboard().write_text(value).map_err(|e| e.to_string())
}
#[tauri::command]
async fn router_choose_binary(app: tauri::AppHandle) -> Result<Option<String>, String> {
    tokio::task::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("选择 Codex 程序")
            .blocking_pick_file()
            .and_then(|p| p.into_path().ok())
            .map(|p| p.to_string_lossy().to_string())
    })
    .await
    .map_err(|e| e.to_string())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let path = app.path().app_data_dir()?;
            let core = Core::new(path).map_err(std::io::Error::other)?;
            app.manage(AppState(Mutex::new(core)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            router_status,
            router_refresh,
            router_start,
            router_stop,
            router_save,
            router_test,
            router_copy,
            router_choose_binary
        ])
        .run(tauri::generate_context!())
        .expect("AI Router 启动失败");
}
