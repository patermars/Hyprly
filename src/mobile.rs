use axum::{
    body::Bytes,
    extract::{
        DefaultBodyLimit,
        ws::{Message, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::header,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use serde_json::json;
use std::{
    net::SocketAddr,
    sync::atomic::AtomicU64,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncWriteExt, process::Command, sync::broadcast};

use crate::response::AiResponse;

#[derive(Clone)]
pub struct MobileHub {
    code: Arc<String>,
    events: broadcast::Sender<String>,
    pub paused: Arc<AtomicBool>,
    commands: broadcast::Sender<MobileCommand>,
    last_answer: Arc<Mutex<Option<String>>>,
}

#[derive(Clone, Debug)]
pub enum MobileCommand {
    Pause,
    Resume,
    ClearTranscript,
    UpdateContext(String),
}

impl MobileHub {
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let code = format!("{:06}", (seed % 1_000_000) as u32);
        let (events, _) = broadcast::channel(128);
        let (commands, _) = broadcast::channel(32);
        println!("Hyprly mobile pairing code: {code}");
        println!("Open http://<this-machine-ip>:8765 on your phone");
        Self {
            code: Arc::new(code),
            events,
            paused: Arc::new(AtomicBool::new(false)),
            commands,
            last_answer: Arc::new(Mutex::new(None)),
        }
    }

    pub fn publish(&self, event: serde_json::Value) {
        let _ = self.events.send(event.to_string());
    }

    pub fn status(&self, state: &str, message: &str) {
        self.publish(json!({"type":"status","state":state,"message":message}));
    }

    pub fn answer_started(&self) {
        self.publish(json!({"type":"answer_start"}));
    }

    pub fn answer_token(&self, text: &str) {
        self.publish(json!({"type":"answer_token", "text": text}));
    }

    pub fn answer_done(&self, text: &str) {
        self.publish(json!({"type":"answer", "text": text}));
    }

    pub fn set_last_answer(&self, text: &str) {
        if let Ok(mut answer) = self.last_answer.lock() {
            *answer = Some(text.to_string());
        }
    }

    pub fn restore_last_answer(&self) -> bool {
        let answer = self
            .last_answer
            .lock()
            .ok()
            .and_then(|answer| answer.clone());
        if let Some(answer) = answer {
            self.answer_done(&answer);
            true
        } else {
            false
        }
    }

    pub fn structured_answer(&self, text: &str, response: &AiResponse) {
        self.publish(json!({
            "type": "answer",
            "text": text,
            "structured": serde_json::to_value(response).unwrap_or_default()
        }));
    }

    pub fn transcript(&self, text: &str) {
        self.publish(json!({"type":"transcript", "text": text}));
    }

    pub fn topic_changed(&self) {
        self.publish(json!({"type":"topic_reset"}));
    }

    pub fn subscribe_commands(&self) -> broadcast::Receiver<MobileCommand> {
        self.commands.subscribe()
    }
}

async fn index() -> Response {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../mobile/index.html"),
    )
        .into_response()
}

async fn styles() -> Response {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../mobile/styles.css"),
    )
        .into_response()
}

async fn script() -> Response {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        include_str!("../mobile/app.js"),
    )
        .into_response()
}

async fn manifest() -> Response {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        include_str!("../mobile/manifest.webmanifest"),
    )
        .into_response()
}

#[derive(serde::Deserialize)]
struct ExtractQuery {
    format: String,
    code: String,
}

static UPLOAD_SEQUENCE: AtomicU64 = AtomicU64::new(0);

async fn extract_document(
    Query(query): Query<ExtractQuery>,
    State(hub): State<MobileHub>,
    body: Bytes,
) -> Response {
    if query.code != hub.code.as_str() {
        return (
            axum::http::StatusCode::UNAUTHORIZED,
            axum::Json(json!({"error":"Pair with Hyprly before uploading documents."})),
        )
            .into_response();
    }
    let extracted = match query.format.as_str() {
        "pdf" => {
            if !body.starts_with(b"%PDF-") {
                Err("This file does not look like a PDF.".to_string())
            } else {
                let mut child = match Command::new("pdftotext")
                    .args(["-layout", "-", "-"])
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
                {
                    Ok(child) => child,
                    Err(_) => return (axum::http::StatusCode::SERVICE_UNAVAILABLE, axum::Json(json!({"error":"PDF support requires the pdftotext utility (Poppler)."}))).into_response(),
                };
                if let Some(mut stdin) = child.stdin.take() {
                    if stdin.write_all(&body).await.is_err() {
                        return (
                            axum::http::StatusCode::BAD_REQUEST,
                            axum::Json(json!({"error":"Could not read the uploaded PDF."})),
                        )
                            .into_response();
                    }
                }
                match child.wait_with_output().await {
                    Ok(output) if output.status.success() => {
                        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
                    }
                    _ => Err("Could not extract text from this PDF.".to_string()),
                }
            }
        }
        "docx" => {
            if !body.starts_with(b"PK\x03\x04") {
                Err("This file does not look like a DOCX document.".to_string())
            } else {
                let id = UPLOAD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir()
                    .join(format!("hyprly-upload-{}-{id}.docx", std::process::id()));
                let result = async {
                    tokio::fs::write(&path, &body).await.map_err(|_| {
                        "Could not store the uploaded document temporarily.".to_string()
                    })?;
                    let output = Command::new("unzip")
                        .args(["-p", path.to_str().unwrap_or_default(), "word/document.xml"])
                        .output()
                        .await
                        .map_err(|_| "DOCX support requires the unzip utility.".to_string())?;
                    if !output.status.success() {
                        return Err("Could not extract text from this DOCX document.".to_string());
                    }
                    String::from_utf8(output.stdout)
                        .map_err(|_| "The DOCX document contains invalid text.".to_string())
                }
                .await;
                let _ = tokio::fs::remove_file(path).await;
                result
            }
        }
        _ => Err("Choose a PDF or DOCX document.".to_string()),
    };

    match extracted {
        Ok(text) => axum::Json(json!({"text": text})).into_response(),
        Err(error) => (
            axum::http::StatusCode::BAD_REQUEST,
            axum::Json(json!({"error":error})),
        )
            .into_response(),
    }
}

async fn websocket(ws: WebSocketUpgrade, State(hub): State<MobileHub>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, hub))
}

async fn handle_socket(mut socket: WebSocket, hub: MobileHub) {
    let Some(Ok(Message::Text(first))) = socket.recv().await else {
        return;
    };
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&first) else {
        return;
    };
    if payload.get("type").and_then(|v| v.as_str()) != Some("pair")
        || payload.get("code").and_then(|v| v.as_str()) != Some(hub.code.as_str())
    {
        let _ = socket
            .send(Message::Text(
                json!({"type":"error", "message":"Invalid pairing code"})
                    .to_string()
                    .into(),
            ))
            .await;
        return;
    }

    let _ = socket
        .send(Message::Text(
            json!({"type":"connected"}).to_string().into(),
        ))
        .await;
    let mut events = hub.events.subscribe();
    loop {
        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        if socket.send(Message::Text(event.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            message = socket.recv() => {
                match message {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(cmd) = serde_json::from_str::<serde_json::Value>(&text) {
                            match cmd.get("type").and_then(|v| v.as_str()) {
                                Some("pause") => {
                                    hub.paused.store(true, Ordering::Relaxed);
                                    let _ = hub.commands.send(MobileCommand::Pause);
                                }
                                Some("resume") => {
                                    hub.paused.store(false, Ordering::Relaxed);
                                    let _ = hub.commands.send(MobileCommand::Resume);
                                }
                                Some("clear_transcript") => {
                                    let _ = hub.commands.send(MobileCommand::ClearTranscript);
                                }
                                Some("update_context") => {
                                    let resume = cmd.get("resume").and_then(|v| v.as_str()).unwrap_or("");
                                    let job = cmd.get("job_description").and_then(|v| v.as_str()).unwrap_or("");
                                    let company = cmd.get("company").and_then(|v| v.as_str()).unwrap_or("");
                                    let context = [
                                        ("Resume", resume),
                                        ("Job description", job),
                                        ("Company", company),
                                    ].into_iter()
                                     .filter(|(_, value)| !value.trim().is_empty())
                                     .map(|(label, value)| format!("### {label}\n{}", value.trim()))
                                     .collect::<Vec<_>>()
                                     .join("\n\n");
                                    let _ = hub.commands.send(MobileCommand::UpdateContext(context));
                                }
                                _ => {}
                            }
                        }
                    }
                    None => break,
                    _ => {}
                }
            }
        }
    }
}

pub fn start(hub: MobileHub) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("mobile runtime");
        runtime.block_on(async move {
            let app = Router::new()
                .route("/", get(index))
                .route("/styles.css", get(styles))
                .route("/app.js", get(script))
                .route("/manifest.webmanifest", get(manifest))
                .route("/extract", post(extract_document))
                .route("/ws", get(websocket))
                .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
                .with_state(hub);
            let listener = tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 8765)))
                .await
                .expect("bind mobile server on port 8765");
            axum::serve(listener, app).await.expect("mobile server");
        });
    });
}
