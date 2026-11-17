use crate::{
    diagnostics::Diagnostics,
    protocol::{self, DEFAULT_MODEL, DEFAULT_REASONING_EFFORT},
    rpc::CodexRpc,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{broadcast, mpsc, Mutex};
use uuid::Uuid;

const INSTRUCTIONS: &str = "You are the model backend for WorkBuddy. Respond to the user's messages using the provided conversation context. Tools named wb_tool_* are supplied and executed by WorkBuddy; use them for all external actions. Do not use Codex built-in tools, skills, commands, filesystem access, plans, or subagents. Do not claim an action occurred unless a WorkBuddy tool result confirms it. Return normal assistant text or invoke a provided tool.";

#[derive(Debug, Clone)]
pub struct BridgeError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}
impl BridgeError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            status: 400,
            code: "invalid_request",
            message: message.into(),
        }
    }
    fn failure(message: impl Into<String>) -> Self {
        Self {
            status: 502,
            code: "codex_error",
            message: message.into(),
        }
    }
}

struct PendingTool {
    rpc_id: Value,
    call: Value,
    reported: bool,
}
struct Session {
    id: String,
    history: Vec<Value>,
    names: HashMap<String, String>,
    events: broadcast::Receiver<Value>,
    pending: HashMap<String, PendingTool>,
    turn_id: Option<String>,
    touched: Instant,
    done: bool,
}
struct SessionEntry {
    identity: Value,
    history: std::sync::RwLock<Vec<Value>>,
    state: Mutex<Session>,
}

// A streaming client may reserialize the content of the immediately previous
// assistant tool-call message. Its tool calls and complete result IDs must still
// match the live session exactly; all earlier history remains strict.
fn matches_pending_tool_results(history: &[Value], messages: &[Value], session: &Session) -> bool {
    let Some(last) = history.len().checked_sub(1) else {
        return false;
    };
    if messages.len() <= history.len() || history[..last] != messages[..last] {
        return false;
    }
    let expected = &history[last];
    let actual = &messages[last];
    if expected["role"] != "assistant"
        || actual["role"] != "assistant"
        || expected.get("name") != actual.get("name")
        || expected.get("tool_calls") != actual.get("tool_calls")
        || !expected
            .get("tool_calls")
            .and_then(Value::as_array)
            .is_some_and(|v| !v.is_empty())
    {
        return false;
    }
    let pending: std::collections::HashSet<&str> = session
        .pending
        .iter()
        .filter(|(_, value)| value.reported)
        .map(|(id, _)| id.as_str())
        .collect();
    let incoming = &messages[history.len()..];
    let ids: std::collections::HashSet<&str> = incoming
        .iter()
        .filter(|m| m["role"] == "tool")
        .filter_map(|m| m["tool_call_id"].as_str())
        .collect();
    !pending.is_empty()
        && incoming.len() == pending.len()
        && ids.len() == incoming.len()
        && ids == pending
}

// An old conversation can be reconstructed as text only after its tools have
// finished and the client has supplied a new user message. Never invent a
// response for an outstanding dynamic-tool RPC after a restart.
fn can_replay_finished_tools(messages: &[Value]) -> bool {
    if messages.last().and_then(|m| m["role"].as_str()) != Some("user") {
        return false;
    }
    let mut outstanding = std::collections::HashSet::new();
    let mut seen = std::collections::HashSet::new();
    for message in messages {
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let Some(id) = call["id"].as_str() else {
                    return false;
                };
                if !seen.insert(id) || !outstanding.insert(id) {
                    return false;
                }
            }
        }
        if message["role"] == "tool" {
            let Some(id) = message["tool_call_id"].as_str() else {
                return false;
            };
            if !outstanding.remove(id) {
                return false;
            }
        }
    }
    outstanding.is_empty()
}

pub struct Bridge {
    rpc: Arc<CodexRpc>,
    cwd: PathBuf,
    sessions: Mutex<Vec<Arc<SessionEntry>>>,
    logs: Mutex<Vec<Value>>,
    diagnostics: Option<Arc<Diagnostics>>,
    instance_id: String,
}

impl Bridge {
    pub fn new(rpc: Arc<CodexRpc>, cwd: PathBuf) -> Self {
        Self::with_diagnostics(rpc, cwd, None)
    }
    pub fn with_diagnostics(
        rpc: Arc<CodexRpc>,
        cwd: PathBuf,
        diagnostics: Option<Arc<Diagnostics>>,
    ) -> Self {
        let instance_id = Uuid::new_v4().simple().to_string();
        if let Some(log) = &diagnostics {
            let _ = log.record(
                "bridge_started",
                json!({"bridge":instance_id,"version":env!("CARGO_PKG_VERSION")}),
            );
        }
        Self {
            rpc,
            cwd,
            sessions: Mutex::new(Vec::new()),
            logs: Mutex::new(Vec::new()),
            diagnostics,
            instance_id,
        }
    }
    fn diagnostic(&self, event: &'static str, mut data: Value) {
        if let Some(log) = &self.diagnostics {
            data["bridge"] = json!(self.instance_id);
            if let Err(error) = log.record(event, data) {
                eprintln!("AI Router diagnostic write failed: {error}");
            }
        }
    }
    pub async fn models(&self) -> Result<Vec<Value>, BridgeError> {
        let mut list = Vec::new();
        let mut cursor = Value::Null;
        loop {
            let mut params = json!({"limit":100,"includeHidden":false});
            if !cursor.is_null() {
                params["cursor"] = cursor;
            }
            let result = self
                .rpc
                .request("model/list", params)
                .await
                .map_err(BridgeError::failure)?;
            if let Some(items) = result["data"].as_array() {
                list.extend(items.iter().cloned());
            }
            cursor = result["nextCursor"].clone();
            if cursor.is_null() {
                break;
            }
        }
        Ok(list)
    }

    pub async fn count(&self) -> usize {
        self.sessions.lock().await.len()
    }
    pub async fn logs(&self) -> Vec<Value> {
        self.logs.lock().await.clone()
    }
    pub async fn log_error(&self, error: &str) {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut logs = self.logs.lock().await;
        logs.insert(
            0,
            json!({"at":at,"type":"error","message":sanitize_error(error)}),
        );
        logs.truncate(100);
    }

    pub async fn complete(
        &self,
        body: Value,
        deltas: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<Value, BridgeError> {
        let messages = protocol::normalize_messages(&body["messages"])
            .map_err(|e| BridgeError::invalid(e.to_string()))?;
        if body.get("n").is_some_and(|n| n != 1) {
            return Err(BridgeError::invalid("仅支持 n=1。"));
        }
        if body.get("stream").is_some_and(|v| !v.is_boolean()) {
            return Err(BridgeError::invalid("stream 必须为布尔值。"));
        }
        for key in ["response_format", "logprobs", "logit_bias", "seed", "stop"] {
            if body.get(key).is_some_and(|v| !v.is_null() && *v != false) {
                return Err(BridgeError::invalid(format!("首版不支持 {key}。")));
            }
        }
        let tools = body.get("tools").cloned().unwrap_or(json!([]));
        let choice = body
            .get("tool_choice")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| BridgeError::invalid("不支持指定工具。"))
            })
            .transpose()?;
        let (dynamic_tools, names) = protocol::dynamic_tools(&tools, choice)
            .map_err(|e| BridgeError::invalid(e.to_string()))?;
        let requested = body["model"].as_str().unwrap_or(DEFAULT_MODEL);
        let model = if requested == "codex-local" {
            DEFAULT_MODEL
        } else {
            requested
        };
        let effort = body
            .get("reasoning_effort")
            .and_then(Value::as_str)
            .or_else(|| body.pointer("/reasoning/effort").and_then(Value::as_str))
            .unwrap_or(DEFAULT_REASONING_EFFORT);
        let models = self.models().await?;
        let descriptor = models
            .iter()
            .find(|m| m["model"] == model)
            .ok_or_else(|| BridgeError::invalid(format!("当前 Codex 目录中未找到模型：{model}")))?;
        if !descriptor["supportedReasoningEfforts"]
            .as_array()
            .is_some_and(|v| v.iter().any(|e| e["reasoningEffort"] == effort))
        {
            return Err(BridgeError::invalid(format!(
                "模型不支持推理强度：{effort}"
            )));
        }
        let identity = json!({"model":model,"tools":tools,"choice":choice.unwrap_or("auto")});
        self.cleanup_expired().await;
        let selected = {
            let all = self.sessions.lock().await;
            let mut best = None;
            let mut length = 0;
            let mut candidates = Vec::new();
            let mut pending_match = None;
            let mut pending_match_count = 0usize;
            for entry in all.iter() {
                if let Ok(history) = entry.history.read() {
                    let common = history
                        .iter()
                        .zip(&messages)
                        .take_while(|(a, b)| a == b)
                        .count();
                    let differing_fields = if common < history.len() && common < messages.len() {
                        ["role", "content", "name", "tool_calls", "tool_call_id"]
                            .into_iter()
                            .filter(|field| {
                                history[common].get(*field) != messages[common].get(*field)
                            })
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    let state = entry.state.try_lock().ok();
                    let pending_matches = state.as_ref().map(|s| {
                        messages
                            .iter()
                            .filter_map(|m| m["tool_call_id"].as_str())
                            .filter(|id| s.pending.contains_key(*id))
                            .count()
                    });
                    if entry.identity == identity
                        && state
                            .as_ref()
                            .is_some_and(|s| matches_pending_tool_results(&history, &messages, s))
                    {
                        pending_match_count += 1;
                        pending_match = Some(entry.clone());
                    }
                    candidates.push(json!({"same_model":entry.identity["model"]==identity["model"],"same_tools":entry.identity["tools"]==identity["tools"],"same_choice":entry.identity["choice"]==identity["choice"],"history_count":history.len(),"common_prefix_count":common,"first_different_fields":differing_fields,"idle_seconds":state.as_ref().map(|s|s.touched.elapsed().as_secs()),"pending_tool_count":state.as_ref().map(|s|s.pending.len()),"matching_tool_result_count":pending_matches}));
                    if entry.identity == identity
                        && history.len() <= messages.len()
                        && messages.starts_with(&history)
                        && history.len() >= length
                    {
                        best = Some(entry.clone());
                        length = history.len();
                    }
                }
            }
            let match_kind = if best.is_some() {
                "exact"
            } else if pending_match_count == 1 {
                best = pending_match;
                "pending_tool"
            } else {
                "none"
            };
            self.diagnostic("session_lookup", json!({"message_count":messages.len(),"last_role":messages.last().and_then(|m|m["role"].as_str()),"tool_result_count":messages.iter().filter(|m|m["role"]=="tool").count(),"replayable":can_replay_finished_tools(&messages),"matched":best.is_some(),"match_kind":match_kind,"pending_match_count":pending_match_count,"stream":body["stream"]==true,"candidates":candidates}));
            if best.is_none() && all.len() >= 24 {
                return Err(BridgeError {
                    status: 429,
                    code: "overloaded",
                    message: "本机对话数已达上限。".into(),
                });
            }
            best
        };
        let entry = if let Some(s) = selected {
            s
        } else {
            if messages.iter().any(|m| m["role"] == "tool") && !can_replay_finished_tools(&messages)
            {
                self.diagnostic(
                    "session_not_found",
                    json!({"reason":"tool_results_without_matching_live_session"}),
                );
                return Err(BridgeError {
                    status: 409,
                    code: "tool_session_expired",
                    message: "工具会话已过期，请新建 WorkBuddy 任务。".into(),
                });
            }
            let events = self.rpc.subscribe();
            let systems = messages
                .iter()
                .filter(|m| m["role"] == "system" || m["role"] == "developer")
                .map(|m| protocol::text_content(&m["content"]))
                .collect::<Vec<_>>()
                .join("\n\n");
            let thread = self.rpc.request("thread/start", json!({"model":model,"modelProvider":"openai","ephemeral":true,"cwd":self.cwd,"environments":[],"approvalPolicy":"untrusted","sandbox":"read-only","baseInstructions":format!("{INSTRUCTIONS}\n\n{systems}"),"developerInstructions":"All computer actions must be delegated to WorkBuddy dynamic tools. Do not use built-in tools.","dynamicTools":dynamic_tools,"config":{"features.shell_tool":false,"features.multi_agent":false,"features.apps":false,"web_search":"disabled"}})).await.map_err(BridgeError::failure)?;
            let id = thread["thread"]["id"]
                .as_str()
                .ok_or_else(|| BridgeError::failure("Codex 未返回会话 ID。"))?
                .to_owned();
            let entry = Arc::new(SessionEntry {
                identity,
                history: std::sync::RwLock::new(messages.clone()),
                state: Mutex::new(Session {
                    id,
                    history: Vec::new(),
                    names,
                    events,
                    pending: HashMap::new(),
                    turn_id: None,
                    touched: Instant::now(),
                    done: false,
                }),
            });
            self.sessions.lock().await.push(entry.clone());
            self.diagnostic("session_created", json!({"history_count":messages.len(),"replayed_finished_tools":messages.iter().any(|m|m["role"]=="tool")}));
            entry
        };
        let mut s = entry.state.try_lock().map_err(|_| BridgeError {
            status: 409,
            code: "conversation_busy",
            message: "同一对话已有请求运行中。".into(),
        })?;
        let incoming = &messages[s.history.len()..];
        if !s.history.is_empty() && incoming.is_empty() {
            return Err(BridgeError {
                status: 409,
                code: "duplicate_request",
                message: "消息未包含新的输入。".into(),
            });
        }
        if !s.pending.is_empty() {
            let expected = s.pending.values().filter(|p| p.reported).count();
            let ids: std::collections::HashSet<&str> = incoming
                .iter()
                .filter_map(|m| m["tool_call_id"].as_str())
                .collect();
            if incoming.len() != expected
                || ids.len() != expected
                || incoming.iter().any(|m| {
                    m["role"] != "tool"
                        || !m["tool_call_id"]
                            .as_str()
                            .is_some_and(|id| s.pending.get(id).is_some_and(|p| p.reported))
                })
            {
                return Err(BridgeError::invalid(
                    "请完整回传上一轮所有 tool_calls 的结果。",
                ));
            }
            for m in incoming {
                let pending = s
                    .pending
                    .remove(m["tool_call_id"].as_str().unwrap())
                    .unwrap();
                let replied = self
                    .rpc
                    .reply(
                        pending.rpc_id,
                        json!({"contentItems":protocol::tool_output(&m["content"]),"success":true}),
                    )
                    .await;
                if let Err(e) = replied {
                    self.dispose(&mut s).await;
                    drop(s);
                    self.remove(&entry).await;
                    return Err(BridgeError::failure(e));
                }
            }
        } else {
            if incoming.iter().any(|m| m["role"] == "tool")
                && !(s.history.is_empty() && can_replay_finished_tools(incoming))
            {
                return Err(BridgeError::invalid("没有对应的待处理工具调用。"));
            }
            let input = protocol::user_input(incoming);
            if input.is_empty() {
                self.dispose(&mut s).await;
                drop(s);
                self.remove(&entry).await;
                return Err(BridgeError::invalid("请求没有用户输入。"));
            }
            s.done = false;
            let turn = self
                .rpc
                .request(
                    "turn/start",
                    json!({"threadId":s.id,"input":input,"environments":[],"effort":effort}),
                )
                .await;
            let turn = match turn {
                Ok(turn) => turn,
                Err(e) => {
                    self.dispose(&mut s).await;
                    drop(s);
                    self.remove(&entry).await;
                    return Err(BridgeError::failure(e));
                }
            };
            s.turn_id = turn["turn"]["id"].as_str().map(str::to_owned);
        }
        let cancelled = deltas.clone();
        let result = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(180), self.collect(&mut s, deltas)) => result,
            _ = async { if let Some(tx) = cancelled { tx.closed().await } else { std::future::pending::<()>().await } } => {
                Ok(Err(BridgeError {status:499,code:"cancelled",message:"客户端已取消请求。".into()}))
            }
        };
        let (content, calls, usage) = match result {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                self.diagnostic("session_removed", json!({"reason":e.code}));
                self.dispose(&mut s).await;
                drop(s);
                self.remove(&entry).await;
                return Err(e);
            }
            Err(_) => {
                self.diagnostic("session_removed", json!({"reason":"timeout"}));
                self.dispose(&mut s).await;
                drop(s);
                self.remove(&entry).await;
                return Err(BridgeError {
                    status: 504,
                    code: "timeout",
                    message: "Codex 响应超时。".into(),
                });
            }
        };
        let mut message = json!({"role":"assistant","content":if content.is_empty(){Value::Null}else{json!(content)}});
        if !calls.is_empty() {
            message["tool_calls"] = json!(calls);
        }
        s.history = messages;
        s.history.push(message.clone());
        if let Ok(mut history) = entry.history.write() {
            *history = s.history.clone();
        }
        s.touched = Instant::now();
        self.diagnostic("response_ready", json!({"history_count":s.history.len(),"tool_call_count":calls.len(),"pending_tool_count":s.pending.len()}));
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let mut response = json!({"id":format!("chatcmpl-{}",Uuid::new_v4()),"object":"chat.completion","created":created,"model":model,"choices":[{"index":0,"message":message,"finish_reason":if calls.is_empty(){"stop"}else{"tool_calls"}}]});
        if let Some(u) = usage {
            response["usage"] = json!({"prompt_tokens":u["inputTokens"],"completion_tokens":u["outputTokens"],"total_tokens":u["totalTokens"],"prompt_tokens_details":{"cached_tokens":u["cachedInputTokens"]}});
        }
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let mut logs = self.logs.lock().await;
        logs.insert(0,json!({"at":at,"type":"request","status":if calls.is_empty(){"completed"}else{"tool_calls"},"model":model}));
        logs.truncate(100);
        Ok(response)
    }

    async fn collect(
        &self,
        s: &mut Session,
        deltas: Option<mpsc::UnboundedSender<String>>,
    ) -> Result<(String, Vec<Value>, Option<Value>), BridgeError> {
        let mut text = String::new();
        let mut usage = None;
        let mut flush_at = None;
        loop {
            let event = if let Some(deadline) = flush_at {
                match tokio::time::timeout_at(deadline, s.events.recv()).await {
                    Ok(v) => v,
                    Err(_) => break,
                }
            } else {
                s.events.recv().await
            };
            let event =
                event.map_err(|e| BridgeError::failure(format!("Codex 事件流异常：{e}")))?;
            if event["method"] == "router/disconnected" {
                return Err(BridgeError::failure("Codex 服务进程已退出。"));
            }
            if event["params"]["threadId"] != s.id {
                continue;
            }
            match event["method"].as_str().unwrap_or("") {
                "item/agentMessage/delta" => {
                    if let Some(delta) = event["params"]["delta"].as_str() {
                        text.push_str(delta);
                        if let Some(tx) = &deltas {
                            if tx.send(delta.into()).is_err() {
                                return Err(BridgeError {
                                    status: 499,
                                    code: "cancelled",
                                    message: "客户端已取消请求。".into(),
                                });
                            }
                        }
                    }
                }
                "thread/tokenUsage/updated" => {
                    usage = Some(event["params"]["tokenUsage"]["last"].clone())
                }
                "item/tool/call" => {
                    let tool = event["params"]["tool"].as_str().unwrap_or("");
                    let name = s
                        .names
                        .get(tool)
                        .ok_or_else(|| BridgeError::failure("Codex 请求了未知工具。"))?;
                    let args = &event["params"]["arguments"];
                    let args = args
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| args.to_string());
                    let id = format!("call_{}", Uuid::new_v4().simple());
                    let call = json!({"id":id,"type":"function","function":{"name":name,"arguments":args}});
                    s.pending.insert(
                        id,
                        PendingTool {
                            rpc_id: event["id"].clone(),
                            call,
                            reported: false,
                        },
                    );
                    if flush_at.is_none() {
                        flush_at = Some(tokio::time::Instant::now() + Duration::from_millis(35));
                    }
                }
                "turn/completed" => {
                    s.done = true;
                    if event["params"]["turn"]["status"] != "completed" {
                        return Err(BridgeError::failure(
                            event["params"]["turn"]["error"]["message"]
                                .as_str()
                                .unwrap_or("Codex 任务失败或中断。"),
                        ));
                    }
                    break;
                }
                "item/started"
                    if [
                        "commandExecution",
                        "fileChange",
                        "mcpToolCall",
                        "collabAgentToolCall",
                    ]
                    .contains(&event["params"]["item"]["type"].as_str().unwrap_or("")) =>
                {
                    return Err(BridgeError::failure("Codex 尝试使用内置工具，已中断。"))
                }
                method if event.get("id").is_some() && !method.is_empty() => {
                    self.rpc.send(json!({"id":event["id"],"error":{"code":-32601,"message":"Unsupported request"}})).await.map_err(BridgeError::failure)?;
                    return Err(BridgeError::failure(format!(
                        "不支持的 Codex 请求：{method}"
                    )));
                }
                _ => {}
            }
        }
        let mut calls = Vec::new();
        for pending in s.pending.values_mut().filter(|p| !p.reported) {
            pending.reported = true;
            calls.push(pending.call.clone());
        }
        Ok((text, calls, usage))
    }
    async fn abort(&self, s: &Session) {
        if let Some(id) = &s.turn_id {
            let _ = tokio::time::timeout(
                Duration::from_secs(3),
                self.rpc
                    .request("turn/interrupt", json!({"threadId":s.id,"turnId":id})),
            )
            .await;
        }
    }
    async fn remove(&self, entry: &Arc<SessionEntry>) {
        self.sessions
            .lock()
            .await
            .retain(|s| !Arc::ptr_eq(s, entry));
    }
    pub async fn close(&self) {
        let entries = std::mem::take(&mut *self.sessions.lock().await);
        self.diagnostic("bridge_stopped", json!({"session_count":entries.len()}));
        for entry in entries {
            if let Ok(mut s) = entry.state.try_lock() {
                self.dispose(&mut s).await;
            }
        }
    }
    pub async fn cleanup_expired(&self) {
        let mut expired = Vec::new();
        {
            let mut entries = self.sessions.lock().await;
            entries.retain(|entry| {
                let stale = entry
                    .state
                    .try_lock()
                    .map(|s| s.touched.elapsed() >= Duration::from_secs(600))
                    .unwrap_or(false);
                if stale {
                    expired.push(entry.clone());
                }
                !stale
            });
        }
        for entry in expired {
            if let Ok(mut session) = entry.state.try_lock() {
                self.diagnostic("session_expired", json!({"idle_seconds":session.touched.elapsed().as_secs(),"pending_tool_count":session.pending.len()}));
                self.dispose(&mut session).await;
            }
        }
    }
    async fn dispose(&self, s: &mut Session) {
        for (_, pending) in s.pending.drain() {
            let _ = self.rpc.reply(pending.rpc_id, json!({"success":false,"contentItems":[{"type":"inputText","text":"Session cancelled."}]})).await;
        }
        if !s.done {
            self.abort(s).await;
        }
        let _ = tokio::time::timeout(
            Duration::from_secs(3),
            self.rpc
                .request("thread/unsubscribe", json!({"threadId":s.id})),
        )
        .await;
    }
}

pub fn sanitize_error(message: &str) -> String {
    static PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let regex=PATTERN.get_or_init(||regex::Regex::new(r"(?i)bearer\s+\S+|\bsk-[A-Za-z0-9_-]+|\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b").unwrap());
    regex
        .replace_all(message, "[redacted]")
        .chars()
        .take(1200)
        .collect()
}
