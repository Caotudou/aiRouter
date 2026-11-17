use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::Path,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{broadcast, oneshot, Mutex},
};

type Pending = HashMap<u64, oneshot::Sender<Result<Value, String>>>;

/// Newline-delimited JSON-RPC transport for an owned Codex app-server process.
pub struct CodexRpc {
    child: Mutex<Child>,
    input: Mutex<ChildStdin>,
    pending: Arc<Mutex<Pending>>,
    events: broadcast::Sender<Value>,
    next_id: AtomicU64,
    alive: Arc<AtomicBool>,
}

impl CodexRpc {
    pub async fn start(binary: &Path, cwd: &Path) -> Result<Arc<Self>, String> {
        let mut command = Command::new(binary);
        command
            .args(["app-server", "--listen", "stdio://", "-c", "mcp_servers={}"])
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command
            .spawn()
            .map_err(|e| format!("无法启动 Codex：{e}"))?;
        let input = child.stdin.take().ok_or("Codex stdin 不可用")?;
        let output = child.stdout.take().ok_or("Codex stdout 不可用")?;
        let pending: Arc<Mutex<Pending>> = Arc::new(Mutex::new(HashMap::new()));
        let (events, _) = broadcast::channel(1024);
        let alive = Arc::new(AtomicBool::new(true));
        let reader_alive = alive.clone();
        let client = Arc::new(Self {
            child: Mutex::new(child),
            input: Mutex::new(input),
            pending: pending.clone(),
            events: events.clone(),
            next_id: AtomicU64::new(0),
            alive,
        });
        tokio::spawn(async move {
            let mut lines = BufReader::new(output).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if message.get("method").is_some() {
                    let _ = events.send(message);
                    continue;
                }
                if let Some(id) = message.get("id").and_then(Value::as_u64) {
                    if let Some(waiter) = pending.lock().await.remove(&id) {
                        let result = if let Some(error) = message.get("error") {
                            Err(error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("Codex RPC 错误")
                                .to_owned())
                        } else {
                            Ok(message.get("result").cloned().unwrap_or(Value::Null))
                        };
                        let _ = waiter.send(result);
                    }
                }
            }
            reader_alive.store(false, Ordering::Release);
            for (_, waiter) in pending.lock().await.drain() {
                let _ = waiter.send(Err("Codex 服务进程已退出。".into()));
            }
            let _ = events.send(json!({"method":"router/disconnected"}));
        });
        client.request("initialize", json!({"clientInfo":{"name":"ai_router","title":"AI Router","version":"0.2.0"},"capabilities":{"experimentalApi":true}})).await?;
        client.send(json!({"method":"initialized"})).await?;
        Ok(client)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    pub async fn send(&self, message: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        self.input
            .lock()
            .await
            .write_all(&bytes)
            .await
            .map_err(|e| format!("Codex 写入失败：{e}"))
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        if let Err(error) = self
            .send(json!({"id":id,"method":method,"params":params}))
            .await
        {
            self.pending.lock().await.remove(&id);
            return Err(error);
        }
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err("Codex 连接已关闭。".into()),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(format!("Codex 请求超时：{method}"))
            }
        }
    }

    pub async fn reply(&self, id: Value, result: Value) -> Result<(), String> {
        self.send(json!({"id":id,"result":result})).await
    }

    pub async fn stop(&self) -> Result<(), String> {
        self.input
            .lock()
            .await
            .shutdown()
            .await
            .map_err(|e| e.to_string())?;
        let mut child = self.child.lock().await;
        if tokio::time::timeout(Duration::from_millis(1500), child.wait())
            .await
            .is_err()
        {
            child.kill().await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
