use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Query, State,
    },
    http::header,
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::SocketAddr,
    path::Path,
    sync::atomic::{AtomicU64, AtomicU8, AtomicUsize},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::AsyncWriteExt,
    process::Command,
    sync::{broadcast, watch},
};

use crate::response::AiResponse;

#[derive(Clone)]
pub struct MobileHub {
    code: Arc<String>,
    events: broadcast::Sender<String>,
    pub paused: Arc<AtomicBool>,
    commands: broadcast::Sender<MobileCommand>,
    last_answer: Arc<Mutex<Option<String>>>,
    https_url: Arc<String>,
    http_url: Arc<String>,
    ca_certificate: Arc<String>,
    server_certificate: Arc<String>,
    server_key: Arc<String>,
    connected_clients: Arc<AtomicUsize>,
    local_connected: Arc<AtomicBool>,
    connected: watch::Sender<bool>,
    answer_target: Arc<AtomicU8>,
}

#[derive(Clone, Debug)]
pub enum MobileCommand {
    Pause,
    Resume,
    ClearTranscript,
    TranscribeRecentAudio,
    UpdateContext(String),
}

impl MobileHub {
    pub fn new(answer_target: &str) -> anyhow::Result<Self> {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let code = format!("{:06}", (seed % 1_000_000) as u32);
        let (events, _) = broadcast::channel(128);
        let (commands, _) = broadcast::channel(32);
        let (connected, _) = watch::channel(false);
        let local_ip = Self::local_mobile_ip();
        let (ca_certificate, server_certificate, server_key) = tls_certificates(local_ip)?;
        let host = local_ip
            .map(|ip| ip.to_string())
            .unwrap_or_else(|| "<this-machine-ip>".into());
        let http_base = format!("http://{host}:8765");
        let https_base = format!("https://{host}:8766");
        Ok(Self {
            code: Arc::new(code),
            events,
            paused: Arc::new(AtomicBool::new(false)),
            commands,
            last_answer: Arc::new(Mutex::new(None)),
            https_url: Arc::new(https_base),
            http_url: Arc::new(http_base),
            ca_certificate: Arc::new(ca_certificate),
            server_certificate: Arc::new(server_certificate),
            server_key: Arc::new(server_key),
            connected_clients: Arc::new(AtomicUsize::new(0)),
            local_connected: Arc::new(AtomicBool::new(false)),
            connected,
            answer_target: Arc::new(AtomicU8::new(answer_target_code(answer_target))),
        })
    }

    pub fn set_answer_target(&self, target: &str) {
        self.answer_target
            .store(answer_target_code(target), Ordering::Relaxed);
    }

    pub fn pairing_info(&self) -> serde_json::Value {
        serde_json::json!({
            "event": "pairing",
            "code": self.code.as_str(),
            "setup_url": self.http_url.as_str(),
            "secure_url": format!("{}/?code={}", self.https_url, self.code),
            "fingerprint": certificate_fingerprint(&self.ca_certificate),
        })
    }

    fn local_mobile_ip() -> Option<std::net::IpAddr> {
        let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        // UDP connect selects the route without sending any packet.
        socket.connect("1.1.1.1:53").ok()?;
        let address = socket.local_addr().ok()?.ip();
        (!address.is_unspecified() && !address.is_loopback()).then_some(address)
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

    pub fn structured_answer(&self, response: &AiResponse) {
        self.set_last_answer(&response.answer);
        self.publish(json!({
            "type": "answer",
            "text": &response.answer,
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

    pub fn subscribe_events(&self) -> broadcast::Receiver<String> {
        self.events.subscribe()
    }

    pub fn set_local_connected(&self, connected: bool) {
        self.local_connected.store(connected, Ordering::Relaxed);
        self.connected
            .send_replace(connected || self.connected_clients.load(Ordering::Relaxed) > 0);
    }

    pub fn pair_local(&self, code: &str) -> bool {
        if code.trim() != self.code.as_str() {
            return false;
        }
        self.set_local_connected(true);
        true
    }

    pub fn local_is_connected(&self) -> bool {
        self.local_connected.load(Ordering::Relaxed)
    }

    pub fn command(&self, command: MobileCommand) {
        match command {
            MobileCommand::Pause => self.paused.store(true, Ordering::Relaxed),
            MobileCommand::Resume => self.paused.store(false, Ordering::Relaxed),
            _ => {}
        }
        let _ = self.commands.send(command);
    }

    pub fn subscribe_connection(&self) -> watch::Receiver<bool> {
        self.connected.subscribe()
    }
}

fn certificate_fingerprint(pem: &str) -> String {
    Sha256::digest(pem.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn ca_parameters() -> CertificateParams {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, "Hyprly Local HTTPS CA");
    params.distinguished_name = name;
    params
}

fn write_private_file(path: &Path, contents: &str) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn tls_certificates(
    local_ip: Option<std::net::IpAddr>,
) -> anyhow::Result<(String, String, String)> {
    let tls_dir = crate::config::Config::config_path()
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("tls");
    fs::create_dir_all(&tls_dir)?;
    fs::set_permissions(&tls_dir, fs::Permissions::from_mode(0o700))?;
    let ca_cert_path = tls_dir.join("root-ca.pem");
    let ca_key_path = tls_dir.join("root-ca-key.pem");

    let (ca_certificate, ca_key) = if ca_cert_path.exists() && ca_key_path.exists() {
        let certificate = fs::read_to_string(&ca_cert_path)?;
        let key = KeyPair::from_pem(&fs::read_to_string(&ca_key_path)?)?;
        (certificate, key)
    } else {
        let key = KeyPair::generate()?;
        let certificate = ca_parameters().self_signed(&key)?.pem();
        write_private_file(&ca_key_path, &key.serialize_pem())?;
        fs::write(&ca_cert_path, &certificate)?;
        fs::set_permissions(&ca_cert_path, fs::Permissions::from_mode(0o644))?;
        (certificate, key)
    };

    let mut names = vec!["localhost".to_string(), "127.0.0.1".to_string()];
    if let Some(ip) = local_ip {
        names.push(ip.to_string());
    }
    let mut server_params = CertificateParams::new(names)?;
    let mut server_name = DistinguishedName::new();
    server_name.push(DnType::CommonName, "Hyprly Local Server");
    server_params.distinguished_name = server_name;
    server_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let server_key = KeyPair::generate()?;
    let issuer = Issuer::new(ca_parameters(), ca_key);
    let server_certificate = server_params.signed_by(&server_key, &issuer)?.pem();
    Ok((
        ca_certificate,
        server_certificate,
        server_key.serialize_pem(),
    ))
}

async fn index() -> Response {
    static_asset_response(
        include_bytes!("../mobile/index.html"),
        "text/html; charset=utf-8",
    )
}

fn static_asset_response(contents: &'static [u8], content_type: &'static str) -> Response {
    (
        [(header::CONTENT_TYPE, content_type)],
        Bytes::from_static(contents),
    )
        .into_response()
}

async fn setup_page(State(hub): State<MobileHub>) -> Response {
    let secure_url = hub.https_url.to_string();
    let fingerprint = certificate_fingerprint(&hub.ca_certificate);
    let html = format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Secure Hyprly setup</title><style>body{{margin:0;padding:28px;background:#0b0d12;color:#f3f5fa;font:16px/1.6 system-ui,sans-serif}}main{{max-width:620px;margin:8vh auto;padding:28px;border:1px solid #303744;border-radius:20px;background:#151923}}h1{{font-size:24px}}p{{color:#b0b8c8}}a{{color:#c6f36a}}code{{display:block;overflow-wrap:anywhere;padding:12px;background:#0e121a;border-radius:10px;font-size:12px}}li{{margin:12px 0}}</style><main><p>HYPRLY · SECURE PAIRING</p><h1>Enable camera access</h1><p>Mobile browsers only allow webpage camera access over HTTPS. Install Hyprly’s local certificate once, then open the secure pairing page.</p><ol><li><a href=\"/hyprly-ca.crt\" download=\"hyprly-root-ca.crt\">Download the Hyprly CA certificate</a> on this device.</li><li>Install it as a trusted CA certificate in your device’s security settings. On Android, look under <b>Security → Encryption &amp; credentials → Install a certificate</b>. On iPhone, also enable full trust under <b>Settings → General → About → Certificate Trust Settings</b>.</li><li>Open the secure pairing link below. If the browser still shows a certificate warning, confirm the certificate is installed and trusted.</li></ol><p>Verify the downloaded certificate file against the SHA-256 fingerprint printed by Hyprly:</p><code>{fingerprint}</code><p><a href=\"{secure_url}\">Open secure Hyprly and pair</a></p></main></html>"
    );
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}

async fn ca_certificate(State(hub): State<MobileHub>) -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/x-pem-file"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=hyprly-root-ca.crt",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        hub.ca_certificate.to_string(),
    )
        .into_response()
}

async fn styles() -> Response {
    static_asset_response(
        include_bytes!("../mobile/styles.css"),
        "text/css; charset=utf-8",
    )
}

async fn rewind_10_icon() -> Response {
    static_asset_response(include_bytes!("../assets/icons/rewind-10.svg"), "image/svg+xml")
}

async fn script() -> Response {
    static_asset_response(
        include_bytes!("../mobile/app.js"),
        "application/javascript; charset=utf-8",
    )
}

async fn logo() -> Response {
    static_asset_response(include_bytes!("../assets/logo.png"), "image/png")
}

async fn logo_text() -> Response {
    static_asset_response(include_bytes!("../assets/logo_text.png"), "image/png")
}

async fn logo_text_white() -> Response {
    static_asset_response(include_bytes!("../assets/logo_text_white.png"), "image/png")
}

async fn manifest() -> Response {
    static_asset_response(
        include_bytes!("../mobile/manifest.webmanifest"),
        "application/manifest+json",
    )
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
    if hub.connected_clients.fetch_add(1, Ordering::Relaxed) == 0 {
        hub.connected.send_replace(true);
    }
    let mut events = hub.events.subscribe();
    loop {
        tokio::select! {
            event = events.recv() => {
                match event {
                    Ok(event) => {
                        if hub.answer_target.load(Ordering::Relaxed) == 1
                            && serde_json::from_str::<serde_json::Value>(&event)
                                .ok()
                                .and_then(|value| value.get("type")?.as_str().map(str::to_owned))
                                .is_some_and(|event_type| matches!(event_type.as_str(), "answer_start" | "answer_token" | "answer" | "topic_reset"))
                        {
                            continue;
                        }
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
                                Some("transcribe_recent_audio") => {
                                    let _ = hub.commands.send(MobileCommand::TranscribeRecentAudio);
                                }
                                Some("update_context") => {
                                    let resume = cmd.get("resume").and_then(|v| v.as_str()).unwrap_or("");
                                    let projects = cmd.get("projects").and_then(|v| v.as_str()).unwrap_or("");
                                    let job = cmd.get("job_description").and_then(|v| v.as_str()).unwrap_or("");
                                    let company = cmd.get("company").and_then(|v| v.as_str()).unwrap_or("");
                                    let context = [
                                        ("Resume", resume),
                                        ("Project descriptions", projects),
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
    if hub.connected_clients.fetch_sub(1, Ordering::Relaxed) == 1 {
        hub.connected
            .send_replace(hub.local_connected.load(Ordering::Relaxed));
    }
}

fn answer_target_code(target: &str) -> u8 {
    match target {
        "floating" => 1,
        "both" => 2,
        _ => 0,
    }
}

pub fn start(hub: MobileHub) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new().expect("mobile runtime");
        runtime.block_on(async move {
            let https_app = Router::new()
                .route("/", get(index))
                .route("/styles.css", get(styles))
                .route("/rewind-10.svg", get(rewind_10_icon))
                .route("/app.js", get(script))
                .route("/logo.png", get(logo))
                .route("/logo-text.png", get(logo_text))
                .route("/logo-text-white.png", get(logo_text_white))
                .route("/manifest.webmanifest", get(manifest))
                .route("/extract", post(extract_document))
                .route("/ws", get(websocket))
                .layer(DefaultBodyLimit::max(10 * 1024 * 1024))
                .with_state(hub.clone());
            let http_app = Router::new()
                .route("/", get(setup_page))
                .route("/hyprly-ca.crt", get(ca_certificate))
                .with_state(hub.clone());
            let tls_config = axum_server::tls_rustls::RustlsConfig::from_pem(
                hub.server_certificate.as_bytes().to_vec(),
                hub.server_key.as_bytes().to_vec(),
            )
            .await
            .expect("load Hyprly TLS certificate");

            let https_task = async move {
                axum_server::bind_rustls(SocketAddr::from(([0, 0, 0, 0], 8766)), tls_config)
                    .serve(https_app.into_make_service())
                    .await
                    .map_err(anyhow::Error::from)
            };
            let http_task = async move {
                let listener =
                    tokio::net::TcpListener::bind(SocketAddr::from(([0, 0, 0, 0], 8765))).await?;
                axum::serve(listener, http_app)
                    .await
                    .map_err(anyhow::Error::from)
            };
            if let Err(error) = tokio::try_join!(https_task, http_task) {
                eprintln!("Mobile web server failed: {error:#}");
            }
        });
    });
}
