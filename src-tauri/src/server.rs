use crate::bridge::{sanitize_error, Bridge, BridgeError, StreamDelta};
use axum::{
    body::{to_bytes, Body, Bytes},
    extract::{Request, State},
    http::{header, HeaderValue, StatusCode},
    response::Response,
    routing::any,
    Router,
};
use serde_json::{json, Value};
use std::{
    convert::Infallible,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{
    net::TcpListener,
    sync::{mpsc, oneshot},
};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

#[derive(Clone)]
struct Api {
    bridge: Arc<Bridge>,
    token: Arc<str>,
}

pub struct ServerHandle {
    shutdown: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<()>,
    sweeper: tokio::task::JoinHandle<()>,
}
impl ServerHandle {
    pub async fn stop(mut self) {
        self.sweeper.abort();
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if tokio::time::timeout(std::time::Duration::from_secs(2), &mut self.task)
            .await
            .is_err()
        {
            self.task.abort();
            let _ = self.task.await;
        }
    }
}

pub async fn start(bridge: Arc<Bridge>, token: String, port: u16) -> Result<ServerHandle, String> {
    if token.len() < 24 {
        return Err("本机 API Key 长度不足。".into());
    }
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .map_err(|e| format!("无法监听端口 {port}：{e}"))?;
    let cleanup_bridge = bridge.clone();
    let sweeper = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
        loop {
            interval.tick().await;
            cleanup_bridge.cleanup_expired().await;
        }
    });
    let router = Router::new().fallback(any(handle)).with_state(Api {
        bridge,
        token: token.into(),
    });
    let (tx, rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await;
    });
    Ok(ServerHandle {
        shutdown: Some(tx),
        task,
        sweeper,
    })
}

fn error(status: u16, code: &str, message: &str) -> Response {
    let body =
        json!({"error":{"message":sanitize_error(message),"type":"router_error","code":code}})
            .to_string();
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR))
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body))
        .unwrap()
}
fn json_response(value: Value) -> Response {
    Response::builder()
        .status(200)
        .header(header::CONTENT_TYPE, "application/json; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(value.to_string()))
        .unwrap()
}

async fn handle(State(api): State<Api>, req: Request) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    if host != "127.0.0.1" && host != "localhost" {
        return error(403, "invalid_host", "仅允许本机访问。");
    }
    if req.headers().contains_key(header::ORIGIN) {
        return error(403, "invalid_origin", "不接受网页跨域调用。");
    }
    let expected = format!("Bearer {}", api.token);
    let supplied = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if supplied.len() != expected.len()
        || !bool::from(supplied.as_bytes().ct_eq(expected.as_bytes()))
    {
        return error(401, "invalid_api_key", "本机 API Key 无效。");
    }
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    if method == axum::http::Method::GET && path == "/health" {
        return json_response(json!({"status":"ok","codexConnected":true}));
    }
    if method == axum::http::Method::GET && (path == "/v1/models" || path == "/models") {
        return match api.bridge.models().await {
            Ok(models) => {
                let mut data =
                    vec![json!({"id":"codex-local","object":"model","owned_by":"local"})];
                data.extend(
                    models
                        .iter()
                        .map(|m| json!({"id":m["model"],"object":"model","owned_by":"openai"})),
                );
                json_response(json!({"object":"list","data":data}))
            }
            Err(e) => {
                api.bridge.log_error(&e.message).await;
                error(e.status, e.code, &e.message)
            }
        };
    }
    if method != axum::http::Method::POST
        || !["/v1/chat/completions", "/chat/completions"].contains(&path.as_str())
    {
        return error(
            404,
            "not_found",
            "未找到接口。支持 /v1/models 和 /v1/chat/completions。",
        );
    }
    if !req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/json"))
    {
        return error(415, "invalid_content_type", "请使用 application/json。");
    }
    let bytes = match to_bytes(req.into_body(), 16 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return error(413, "request_too_large", "请求内容超过 16 MB。"),
    };
    let body: Value = match serde_json::from_slice(&bytes) {
        Ok(Value::Object(v)) => Value::Object(v),
        _ => return error(400, "invalid_json", "请求必须是有效 JSON 对象。"),
    };
    let body = api.bridge.apply_defaults(body);
    if body["stream"] != true {
        let bridge = api.bridge.clone();
        let task = tokio::spawn(async move { bridge.complete(body, None).await });
        return match task.await.unwrap_or_else(|e| {
            Err(BridgeError {
                status: 502,
                code: "codex_error",
                message: e.to_string(),
            })
        }) {
            Ok(v) => json_response(v),
            Err(e) => {
                api.bridge.log_error(&e.message).await;
                error(e.status, e.code, &e.message)
            }
        };
    }
    let (output_tx, output_rx) = mpsc::channel::<Result<Bytes, Infallible>>(64);
    tokio::spawn(async move {
        let id = format!("chatcmpl-{}", Uuid::new_v4());
        let model = body["model"].as_str().unwrap_or("gpt-6.1-sol").to_owned();
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let chunk = |delta: Value, reason: Value| {
            format!(
                "data: {}\n\n",
                json!({"id":id,"object":"chat.completion.chunk","created":created,"model":model,"choices":[{"index":0,"delta":delta,"finish_reason":reason}]})
            )
        };
        if output_tx
            .send(Ok(Bytes::from(chunk(
                json!({"role":"assistant"}),
                Value::Null,
            ))))
            .await
            .is_err()
        {
            return;
        }
        let (delta_tx, mut delta_rx) = mpsc::unbounded_channel();
        let bridge = api.bridge.clone();
        let body_copy = body.clone();
        let task = tokio::spawn(async move { bridge.complete(body_copy, Some(delta_tx)).await });
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(15));
        heartbeat.tick().await;
        loop {
            tokio::select! {
                _ = output_tx.closed() => { drop(delta_rx); let _ = task.await; return; }
                _ = heartbeat.tick() => { if output_tx.send(Ok(Bytes::from_static(b": keep-alive\n\n"))).await.is_err() { drop(delta_rx); let _ = task.await; return; } }
                delta = delta_rx.recv() => match delta {
                    Some(StreamDelta::Content(delta)) => if output_tx.send(Ok(Bytes::from(chunk(json!({"content":delta}), Value::Null)))).await.is_err() { drop(delta_rx); let _ = task.await; return; },
                    Some(StreamDelta::ReasoningSummary(delta)) => if output_tx.send(Ok(Bytes::from(chunk(json!({"reasoning_content":delta}), Value::Null)))).await.is_err() { drop(delta_rx); let _ = task.await; return; },
                    None => break,
                }
            }
        }
        let result = match task.await {
            Ok(result) => result,
            Err(e) => Err(BridgeError {
                status: 502,
                code: "codex_error",
                message: e.to_string(),
            }),
        };
        match result {
            Ok(result) => {
                let choice = &result["choices"][0];
                if let Some(calls) = choice["message"]["tool_calls"].as_array() {
                    let calls: Vec<Value> = calls
                        .iter()
                        .enumerate()
                        .map(|(i, c)| {
                            let mut c = c.clone();
                            c["index"] = json!(i);
                            c
                        })
                        .collect();
                    let _ = output_tx
                        .send(Ok(Bytes::from(chunk(
                            json!({"tool_calls":calls}),
                            Value::Null,
                        ))))
                        .await;
                }
                let _ = output_tx
                    .send(Ok(Bytes::from(chunk(
                        json!({}),
                        choice["finish_reason"].clone(),
                    ))))
                    .await;
                if body.pointer("/stream_options/include_usage") == Some(&Value::Bool(true)) {
                    let _=output_tx.send(Ok(Bytes::from(format!("data: {}\n\n",json!({"id":id,"object":"chat.completion.chunk","created":created,"model":model,"choices":[],"usage":result["usage"]}))))).await;
                }
            }
            Err(e) => {
                api.bridge.log_error(&e.message).await;
                let _ = output_tx
                    .send(Ok(Bytes::from(format!(
                        "data: {}\n\n",
                        json!({"error":{"message":sanitize_error(&e.message),"type":"router_error","code":e.code}})
                    ))))
                    .await;
            }
        }
        let _ = output_tx
            .send(Ok(Bytes::from_static(b"data: [DONE]\n\n")))
            .await;
    });
    let mut response = Response::new(Body::from_stream(ReceiverStream::new(output_rx)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response
}
