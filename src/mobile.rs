use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::header,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use serde_json::json;
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct MobileHub {
    code: Arc<String>,
    events: broadcast::Sender<String>,
}

impl MobileHub {
    pub fn new() -> Self {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let code = format!("{:06}", (seed % 1_000_000) as u32);
        let (events, _) = broadcast::channel(128);
        println!("Hyprly mobile pairing code: {code}");
        println!("Open http://<this-machine-ip>:8765 on your phone");
        Self {
            code: Arc::new(code),
            events,
        }
    }

    pub fn publish(&self, event: serde_json::Value) {
        let _ = self.events.send(event.to_string());
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

    pub fn transcript(&self, text: &str) {
        self.publish(json!({"type":"transcript", "text": text}));
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
            message = socket.recv() => if message.is_none() { break },
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
                .route("/ws", get(websocket))
                .with_state(hub);
            let listener = tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 8765)))
                .await
                .expect("bind mobile server on port 8765");
            axum::serve(listener, app).await.expect("mobile server");
        });
    });
}
