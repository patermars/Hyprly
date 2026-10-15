use anyhow::{Context, Result};
use gtk::prelude::*;
use gtk4 as gtk;
use ksni::{
    menu::{MenuItem, StandardItem},
    Tray, TrayMethods,
};
use qrcode::{Color, QrCode};
use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    rc::Rc,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

const GROQ_KEYS_URL: &str = "https://console.groq.com/keys";

#[derive(Clone, Debug)]
struct PairingInfo {
    code: String,
    setup_url: String,
    secure_url: String,
    fingerprint: String,
}

enum UiEvent {
    Pairing(PairingInfo),
    UpdateAvailable { version: String, url: String },
    Message(String),
    GroqError(String),
}

#[derive(Clone, Copy)]
enum TrayAction {
    Open,
    Connect,
    Disconnect,
    Exit,
}

struct HyprlyTray {
    actions: Sender<TrayAction>,
}

impl Tray for HyprlyTray {
    fn id(&self) -> String {
        "hyprly".into()
    }

    fn title(&self) -> String {
        "Hyprly".into()
    }

    fn icon_name(&self) -> String {
        "logo".into()
    }

    fn icon_theme_path(&self) -> String {
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets").into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let Some((width, height, source)) = decode_png(include_bytes!("../assets/logo.png")) else {
            return Vec::new();
        };
        let side = 64usize;
        let mut data = Vec::with_capacity(side * side * 4);
        for y in 0..side {
            for x in 0..side {
                let sx = x * width / side;
                let sy = y * height / side;
                let i = (sy * width + sx) * 4;
                data.extend_from_slice(&[source[i + 3], source[i], source[i + 1], source[i + 2]]);
            }
        }
        vec![ksni::Icon {
            width: side as i32,
            height: side as i32,
            data,
        }]
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        [
            ("Open Hyprly", TrayAction::Open),
            ("Connect", TrayAction::Connect),
            ("Disconnect", TrayAction::Disconnect),
            ("Exit", TrayAction::Exit),
        ]
        .into_iter()
        .map(|(label, action)| {
            let actions = self.actions.clone();
            StandardItem {
                label: label.into(),
                activate: Box::new(move |_| {
                    let _ = actions.send(action);
                }),
                ..Default::default()
            }
            .into()
        })
        .collect()
    }
}

struct AppState {
    application: gtk::Application,
    window: gtk::ApplicationWindow,
    api_key: gtk::Entry,
    backup_keys: Vec<gtk::Entry>,
    status: gtk::Label,
    code: gtk::Label,
    setup_url: gtk::LinkButton,
    secure_url: gtk::LinkButton,
    fingerprint: gtk::Label,
    qr_area: gtk::DrawingArea,
    qr_code: Rc<std::cell::RefCell<Option<QrCode>>>,
    connect_button: gtk::Button,
    disconnect_button: gtk::Button,
    update_notice: gtk::Box,
    update_label: gtk::Label,
    update_link: gtk::LinkButton,
    child: Option<Child>,
    backend_stdin: Option<std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>>,
    event_tx: Sender<UiEvent>,
    event_rx: Receiver<UiEvent>,
    tray_rx: Receiver<TrayAction>,
    mouse_rx: Receiver<crate::input::MousePress>,
    screen_trigger: Option<crate::config::MouseGesture>,
    trigger_clicks: Option<ClickSequence>,
    recording: Option<GestureRecording>,
    trigger_label: Option<gtk::Label>,
}

#[derive(Clone, Copy)]
struct ClickSequence {
    button: u16,
    clicks: u8,
    last_click: std::time::Instant,
}

struct GestureRecording {
    label: gtk::Label,
    candidate: Option<ClickSequence>,
    deadline: std::time::Instant,
    ignore_until: std::time::Instant,
    armed: bool,
}

pub fn run() {
    let application = gtk::Application::builder()
        .application_id("io.github.patermars.Hyprly")
        .build();
    application.connect_activate(build_window);
    application.run();
}

fn build_window(application: &gtk::Application) {
    let (event_tx, event_rx) = mpsc::channel();
    let (tray_tx, tray_rx) = mpsc::channel();
    let (mouse_tx, mouse_rx) = mpsc::channel();
    crate::input::start(mouse_tx);
    start_tray(tray_tx, event_tx.clone());

    let api_key = gtk::Entry::builder()
        .placeholder_text("gsk_…")
        .visibility(false)
        .hexpand(true)
        .build();
    let status = gtk::Label::new(Some("Add your Groq API key to get started."));
    status.set_xalign(0.0);
    status.set_wrap(true);

    install_theme(application);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 18);
    root.add_css_class("app-root");
    root.set_margin_top(22);
    root.set_margin_bottom(22);
    root.set_margin_start(26);
    root.set_margin_end(26);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    header.add_css_class("app-header");
    let brand_text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    brand_text.set_hexpand(true);
    let wordmark = gtk::DrawingArea::builder()
        .content_width(112)
        .content_height(38)
        .build();
    wordmark.set_size_request(112, 38);
    wordmark.add_css_class("wordmark");
    let wordmark_surface = Rc::new(std::cell::RefCell::new(create_wordmark_surface(
        &load_gui_theme(),
    )));
    wordmark.set_draw_func({
        let surface = wordmark_surface.clone();
        move |_, context, width, height| {
            let surface = surface.borrow();
            let scale = (width as f64 / surface.width() as f64)
                .min(height as f64 / surface.height() as f64);
            let x = (width as f64 - surface.width() as f64 * scale) / 2.0;
            let y = (height as f64 - surface.height() as f64 * scale) / 2.0;
            context.translate(x, y);
            context.scale(scale, scale);
            let _ = context.set_source_surface(&*surface, 0.0, 0.0);
            context.source().set_filter(gtk::cairo::Filter::Best);
            let _ = context.paint();
        }
    });
    let subtitle = gtk::Label::new(Some("Private meeting audio, with answers on your phone."));
    subtitle.add_css_class("dim-label");
    subtitle.set_xalign(0.0);
    brand_text.append(&wordmark);
    brand_text.append(&subtitle);
    header.append(&brand_text);
    let header_actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    header_actions.set_halign(gtk::Align::End);
    let settings_button = gtk::Button::from_icon_name("emblem-system-symbolic");
    settings_button.set_tooltip_text(Some("Edit config.toml settings"));
    settings_button.add_css_class("flat");
    header_actions.append(&settings_button);
    let theme_picker = gtk::DropDown::from_strings(&["Midnight", "Paper", "Aurora"]);
    theme_picker.add_css_class("theme-picker");
    theme_picker.set_tooltip_text(Some("Choose appearance"));
    theme_picker.set_valign(gtk::Align::Start);
    header_actions.append(&theme_picker);
    header.append(&header_actions);
    root.append(&header);

    let update_notice = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    update_notice.add_css_class("update-notice");
    let update_label = gtk::Label::new(None);
    update_label.set_xalign(0.0);
    update_label.set_hexpand(true);
    update_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    update_label.set_max_width_chars(32);
    let update_link = gtk::LinkButton::with_label(
        "https://github.com/patermars/Hyprly/releases/latest",
        "View",
    );
    update_link.add_css_class("flat");
    update_link.set_tooltip_text(Some("Open the latest Hyprly release"));
    let dismiss_update = gtk::Button::from_icon_name("window-close-symbolic");
    dismiss_update.add_css_class("flat");
    dismiss_update.set_tooltip_text(Some("Dismiss update notice"));
    update_notice.append(&update_label);
    update_notice.append(&update_link);
    update_notice.append(&dismiss_update);
    update_notice.set_visible(false);
    root.append(&update_notice);
    dismiss_update.connect_clicked({
        let update_notice = update_notice.clone();
        move |_| update_notice.set_visible(false)
    });

    let columns = gtk::Grid::new();
    columns.set_column_spacing(18);
    columns.set_row_spacing(18);
    columns.set_column_homogeneous(false);

    let setup_card = gtk::Box::new(gtk::Orientation::Vertical, 12);
    setup_card.add_css_class("card");
    setup_card.append(&section_heading("Groq API key"));
    let key_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    key_row.append(&api_key);
    let help = gtk::LinkButton::with_label(GROQ_KEYS_URL, "?");
    help.set_tooltip_text(Some("Get an API key from Groq"));
    key_row.append(&help);
    let show_key = gtk::Button::with_label("Show");
    key_row.append(&show_key);
    setup_card.append(&key_row);
    let mut backup_keys = Vec::new();
    for index in 2..=3 {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let entry = gtk::Entry::builder()
            .placeholder_text(&format!("Backup key {index} (optional)"))
            .visibility(false)
            .hexpand(true)
            .build();
        row.append(&entry);
        let show = gtk::Button::with_label("Show");
        let visible = Rc::new(std::cell::Cell::new(false));
        show.connect_clicked({
            let entry = entry.clone();
            let visible = visible.clone();
            move |button| {
                let next = !visible.get();
                visible.set(next);
                entry.set_visibility(next);
                button.set_label(if next { "Hide" } else { "Show" });
            }
        });
        row.append(&show);
        setup_card.append(&row);
        backup_keys.push(entry);
    }
    let failover_note = gtk::Label::new(Some(
        "If a request fails or times out, Hyprly tries the next saved key.",
    ));
    failover_note.add_css_class("muted");
    failover_note.set_xalign(0.0);
    setup_card.append(&failover_note);

    let key_actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let save_key = gtk::Button::with_label("Save key");
    let test_key = gtk::Button::with_label("Test Groq API");
    key_actions.append(&save_key);
    key_actions.append(&test_key);
    setup_card.append(&key_actions);
    let key_note = gtk::Label::new(Some("Stored locally in your private Hyprly configuration."));
    key_note.add_css_class("muted");
    key_note.set_xalign(0.0);
    setup_card.append(&key_note);

    setup_card.append(&section_heading("System checks"));
    let checks = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    checks.set_homogeneous(true);
    let test_audio = gtk::Button::with_label("Test PipeWire audio");
    let test_uploads = gtk::Button::with_label("Check PDF / DOCX tools");
    checks.append(&test_audio);
    checks.append(&test_uploads);
    setup_card.append(&checks);
    columns.attach(&setup_card, 0, 0, 5, 1);

    let pairing_card = gtk::Box::new(gtk::Orientation::Vertical, 12);
    pairing_card.add_css_class("card");
    pairing_card.append(&section_heading("Phone pairing"));
    let pairing = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    pairing.add_css_class("pairing-layout");
    let pairing_text = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let code = gtk::Label::new(Some("Not connected"));
    code.add_css_class("title-2");
    code.add_css_class("pairing-code");
    code.set_xalign(0.0);
    code.set_selectable(true);
    let code_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let copy_code = gtk::Button::with_label("Copy code");
    copy_code.set_tooltip_text(Some("Copy pairing code to clipboard"));
    code_row.append(&code);
    code_row.append(&copy_code);
    let setup_url = gtk::LinkButton::with_label("https://example.com", "Setup URL");
    setup_url.set_halign(gtk::Align::Start);
    setup_url.set_hexpand(true);
    setup_url.set_sensitive(false);
    let secure_url = gtk::LinkButton::with_label("https://example.com", "Secure URL");
    secure_url.set_halign(gtk::Align::Start);
    secure_url.set_hexpand(true);
    secure_url.set_sensitive(false);
    let fingerprint = gtk::Label::new(Some(""));
    fingerprint.set_xalign(0.0);
    fingerprint.set_wrap(true);
    fingerprint.set_selectable(true);
    pairing_text.append(&code_row);
    pairing_text.append(&setup_url);
    pairing_text.append(&secure_url);
    pairing_text.append(&fingerprint);
    pairing.append(&pairing_text);

    let qr_code = Rc::new(std::cell::RefCell::new(None::<QrCode>));
    let qr_draw = qr_code.clone();
    let qr_area = gtk::DrawingArea::builder()
        .content_width(190)
        .content_height(190)
        .build();
    qr_area.set_draw_func(move |_, context, width, height| {
        context.set_source_rgb(1.0, 1.0, 1.0);
        let _ = context.paint();
        let qr = qr_draw.borrow();
        let Some(qr) = qr.as_ref() else { return };
        let width_modules = qr.width() as f64;
        let symbol_size = width_modules + 8.0;
        let pixel = (width.min(height) as f64 / symbol_size).floor().max(1.0);
        let left = (width as f64 - pixel * symbol_size) / 2.0 + 4.0 * pixel;
        let top = (height as f64 - pixel * symbol_size) / 2.0 + 4.0 * pixel;
        context.set_source_rgb(0.06, 0.07, 0.09);
        for (index, color) in qr.to_colors().iter().enumerate() {
            if *color == Color::Dark {
                let x = index % qr.width();
                let y = index / qr.width();
                context.rectangle(
                    left + x as f64 * pixel,
                    top + y as f64 * pixel,
                    pixel,
                    pixel,
                );
            }
        }
        let _ = context.fill();
    });
    pairing.append(&qr_area);
    pairing_card.append(&pairing);
    columns.attach(&pairing_card, 5, 0, 7, 1);
    root.append(&columns);

    let connection_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let connect = gtk::Button::with_label("Connect");
    connect.add_css_class("suggested-action");
    let disconnect = gtk::Button::with_label("Disconnect");
    disconnect.set_sensitive(false);
    connection_actions.append(&connect);
    connection_actions.append(&disconnect);
    connection_actions.add_css_class("connection-actions");
    root.append(&connection_actions);
    status.add_css_class("status-line");
    root.append(&status);

    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("Hyprly")
        .default_width(1050)
        .default_height(700)
        .child(&root)
        .build();
    window.set_icon_name(Some("logo"));

    let screen_trigger = crate::config::Config::load()
        .ok()
        .and_then(|config| config.screen.mouse_gesture);
    let state = Rc::new(std::cell::RefCell::new(AppState {
        application: application.clone(),
        window: window.clone(),
        api_key: api_key.clone(),
        backup_keys: backup_keys.clone(),
        status: status.clone(),
        code: code.clone(),
        setup_url: setup_url.clone(),
        secure_url: secure_url.clone(),
        fingerprint: fingerprint.clone(),
        qr_area: qr_area.clone(),
        qr_code,
        connect_button: connect.clone(),
        disconnect_button: disconnect.clone(),
        update_notice,
        update_label,
        update_link,
        child: None,
        backend_stdin: None,
        event_tx,
        event_rx,
        tray_rx,
        mouse_rx,
        screen_trigger,
        trigger_clicks: None,
        recording: None,
        trigger_label: None,
    }));
    check_for_updates(state.borrow().event_tx.clone());

    settings_button.connect_clicked({
        let state = state.clone();
        move |_| open_config_editor(&state)
    });

    copy_code.connect_clicked({
        let code = code.clone();
        let status = status.clone();
        move |_| {
            let value = code.text();
            if value != "Not connected" {
                if let Some(display) = gtk::gdk::Display::default() {
                    display.clipboard().set_text(&value);
                    status.set_text("Pairing code copied to clipboard.");
                }
            }
        }
    });

    let theme_provider = theme_provider();
    let theme_names = ["midnight", "paper", "aurora"];
    let initial_theme = load_gui_theme();
    let initial_index = theme_names
        .iter()
        .position(|name| *name == initial_theme)
        .unwrap_or(0);
    theme_picker.set_selected(initial_index as u32);
    apply_gui_theme(&theme_provider, &initial_theme);
    *wordmark_surface.borrow_mut() = create_wordmark_surface(&initial_theme);
    wordmark.queue_draw();
    theme_picker.connect_selected_notify({
        let provider = theme_provider.clone();
        let wordmark = wordmark.clone();
        let wordmark_surface = wordmark_surface.clone();
        move |picker| {
            let theme = theme_names
                .get(picker.selected() as usize)
                .copied()
                .unwrap_or("midnight");
            apply_gui_theme(&provider, theme);
            *wordmark_surface.borrow_mut() = create_wordmark_surface(theme);
            wordmark.queue_draw();
            save_gui_theme(theme);
        }
    });

    let key_visibility = Rc::new(std::cell::Cell::new(false));
    show_key.connect_clicked({
        let entry = api_key.clone();
        let key_visibility = key_visibility.clone();
        move |button| {
            let visible = !key_visibility.get();
            key_visibility.set(visible);
            entry.set_visibility(visible);
            button.set_label(if visible { "Hide" } else { "Show" });
        }
    });
    save_key.connect_clicked({
        let state = state.clone();
        move |_| save_current_key(&state)
    });
    test_key.connect_clicked({
        let state = state.clone();
        move |_| test_api_key(&state)
    });
    test_audio.connect_clicked({
        let state = state.clone();
        move |_| check_pipewire(&state)
    });
    test_uploads.connect_clicked({
        let state = state.clone();
        move |_| check_document_tools(&state)
    });
    connect.connect_clicked({
        let state = state.clone();
        move |_| start_backend(&state)
    });
    disconnect.connect_clicked({
        let state = state.clone();
        move |_| stop_backend(&state)
    });
    window.connect_close_request({
        let window = window.clone();
        move |_| {
            window.set_visible(false);
            gtk::glib::Propagation::Stop
        }
    });

    gtk::glib::timeout_add_local(Duration::from_millis(200), {
        let state = state.clone();
        move || {
            poll_events(&state);
            gtk::glib::ControlFlow::Continue
        }
    });

    window.present();
    let configured_keys = crate::config::Config::load()
        .map(|config| config.api.api_keys)
        .unwrap_or_default();
    if configured_keys.is_empty() {
        api_key.grab_focus();
    } else {
        api_key.set_text(&configured_keys[0]);
        for (entry, key) in backup_keys.iter().zip(configured_keys.iter().skip(1)) {
            entry.set_text(key);
        }
        start_backend(&state);
    }
}

fn create_wordmark_surface(theme: &str) -> gtk::cairo::ImageSurface {
    let image = if theme == "paper" {
        include_bytes!("../assets/logo_text.png").as_slice()
    } else {
        include_bytes!("../assets/logo_text_white.png").as_slice()
    };
    let (source_width, source_height, source) = decode_png(image).expect("wordmark PNG is valid");
    let mut argb = Vec::with_capacity(source.len());
    for rgba in source.chunks_exact(4) {
        let a = rgba[3] as u16;
        let r = (rgba[0] as u16 * a / 255) as u8;
        let g = (rgba[1] as u16 * a / 255) as u8;
        let b = (rgba[2] as u16 * a / 255) as u8;
        argb.extend_from_slice(&u32::from_be_bytes([a as u8, r, g, b]).to_ne_bytes());
    }
    gtk::cairo::ImageSurface::create_for_data(
        argb,
        gtk::cairo::Format::ARgb32,
        source_width as i32,
        source_height as i32,
        source_width as i32 * 4,
    )
    .expect("wordmark image surface can be created")
}

fn open_config_editor(state: &Rc<std::cell::RefCell<AppState>>) {
    let config = crate::config::Config::load().unwrap_or_default();
    let (window, status, current_trigger) = {
        let state_ref = state.borrow();
        (
            state_ref.window.clone(),
            state_ref.status.clone(),
            state_ref.screen_trigger.clone(),
        )
    };
    let dialog = gtk::Dialog::builder()
        .title("Hyprly Settings")
        .transient_for(&window)
        .modal(true)
        .default_width(1200)
        .default_height(680)
        .build();
    let content = dialog.content_area();
    content.set_spacing(10);
    content.set_margin_top(20);
    content.set_margin_bottom(20);
    content.set_margin_start(24);
    content.set_margin_end(24);
    let tabs = gtk::Notebook::new();
    tabs.set_hexpand(true);
    tabs.set_vexpand(true);
    let api_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    api_page.set_hexpand(true);
    api_page.set_margin_top(20);
    api_page.set_margin_bottom(20);
    api_page.set_margin_start(20);
    api_page.set_margin_end(20);
    let (model, chat_model_values) = searchable_dropdown(
        &[
            (
                "GPT OSS 20B · fast and capable".into(),
                "openai/gpt-oss-20b".into(),
            ),
            (
                "GPT OSS 120B · strongest GPT OSS".into(),
                "openai/gpt-oss-120b".into(),
            ),
            (
                "Llama 3.3 70B · general purpose".into(),
                "llama-3.3-70b-versatile".into(),
            ),
            (
                "Llama 3.1 8B · lightweight".into(),
                "llama-3.1-8b-instant".into(),
            ),
            (
                "Groq Compound · built-in tools".into(),
                "groq/compound".into(),
            ),
            (
                "Groq Compound Mini · faster built-in tools".into(),
                "groq/compound-mini".into(),
            ),
        ],
        &config.api.model,
    );
    let max_tokens = gtk::SpinButton::with_range(1.0, 32768.0, 128.0);
    max_tokens.set_value(config.api.max_tokens as f64);
    api_page.append(&section_heading("AI response"));
    api_page.append(&settings_row("Chat model", "The Groq model that writes answers. Leave the current value unless you know which model you want.", &model));
    api_page.append(&settings_row("Answer length", "The maximum size of an AI answer. Higher values allow longer replies and use more of your quota.", &max_tokens));

    let audio_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    audio_page.set_hexpand(true);
    audio_page.set_margin_top(20);
    audio_page.set_margin_bottom(20);
    audio_page.set_margin_start(20);
    audio_page.set_margin_end(20);
    let audio_enabled = gtk::Switch::new();
    audio_enabled.set_active(config.audio.enabled);
    audio_page.append(&section_heading("Audio capture"));
    audio_page.append(&settings_row("Enable audio capture", "Turn this on to capture meeting audio for transcription. Changes apply after reconnecting.", &audio_enabled));
    let sources = available_audio_sources();
    let (audio_source, audio_source_values) = searchable_dropdown(&sources, &config.audio.source);
    audio_page.append(&settings_row(
        "Audio source",
        "Leave blank to use the system’s default audio. Otherwise enter the PipeWire source name.",
        &audio_source,
    ));
    let chunk_seconds = gtk::SpinButton::with_range(2.0, 4.0, 1.0);
    chunk_seconds.set_value(config.audio.chunk_seconds as f64);
    audio_page.append(&settings_row(
        "Capture chunk",
        "How often Hyprly collects a small audio segment. Shorter segments can respond sooner.",
        &chunk_seconds,
    ));
    let min_utterance = gtk::SpinButton::with_range(1.0, 4.0, 1.0);
    min_utterance.set_value(config.audio.min_utterance_seconds as f64);
    audio_page.append(&settings_row(
        "Minimum speech",
        "Ignore speech shorter than this, which helps filter out brief background sounds.",
        &min_utterance,
    ));
    let max_utterance = gtk::SpinButton::with_range(1.0, 15.0, 1.0);
    max_utterance.set_value(config.audio.max_utterance_seconds as f64);
    audio_page.append(&settings_row(
        "Maximum speech",
        "Maximum length of one spoken segment before it is sent for transcription.",
        &max_utterance,
    ));
    let (transcription_model, transcription_model_values) = searchable_dropdown(
        &[
            (
                "Whisper Large V3 Turbo · faster, lower cost".into(),
                "whisper-large-v3-turbo".into(),
            ),
            (
                "Whisper Large V3 · prioritize accuracy".into(),
                "whisper-large-v3".into(),
            ),
        ],
        &config.audio.transcription_model,
    );
    audio_page.append(&settings_row(
        "Transcription model",
        "The Groq model that turns captured audio into text.",
        &transcription_model,
    ));
    let concurrency = gtk::SpinButton::with_range(1.0, 4.0, 1.0);
    concurrency.set_value(config.audio.max_concurrent_transcriptions as f64);
    audio_page.append(&settings_row(
        "Simultaneous transcriptions",
        "How many audio segments can be transcribed at once. Higher values may use more quota.",
        &concurrency,
    ));
    let language_options = [
        ("Auto detect · recommended".into(), "".into()),
        ("English".into(), "en".into()),
        ("Hindi".into(), "hi".into()),
        ("Spanish".into(), "es".into()),
        ("French".into(), "fr".into()),
        ("German".into(), "de".into()),
        ("Italian".into(), "it".into()),
        ("Portuguese".into(), "pt".into()),
        ("Japanese".into(), "ja".into()),
        ("Korean".into(), "ko".into()),
        ("Chinese".into(), "zh".into()),
        ("Arabic".into(), "ar".into()),
        ("Bengali".into(), "bn".into()),
        ("Dutch".into(), "nl".into()),
        ("Russian".into(), "ru".into()),
        ("Turkish".into(), "tr".into()),
        ("Urdu".into(), "ur".into()),
        ("Tamil".into(), "ta".into()),
        ("Telugu".into(), "te".into()),
        ("Marathi".into(), "mr".into()),
        ("Indonesian".into(), "id".into()),
    ];
    let (language, language_values) =
        searchable_dropdown(&language_options, &config.audio.language);
    audio_page.append(&settings_row(
        "Audio language",
        "Language spoken in the meeting, using a short language code such as en.",
        &language,
    ));
    let vad = gtk::Switch::new();
    vad.set_active(config.audio.vad_enabled);
    audio_page.append(&settings_row(
        "Detect when someone is speaking",
        "Skip quiet segments so silence is not sent for transcription.",
        &vad,
    ));
    let silence_threshold = gtk::SpinButton::with_range(0.0, 1.0, 0.01);
    silence_threshold.set_digits(2);
    silence_threshold.set_value(config.audio.silence_threshold as f64);
    audio_page.append(&settings_row(
        "Silence sensitivity",
        "Audio quieter than this level is treated as silence. Lower values detect quieter speech.",
        &silence_threshold,
    ));
    let endpoint_silence = gtk::SpinButton::with_range(800.0, 1500.0, 50.0);
    endpoint_silence.set_value(config.audio.endpoint_silence_ms as f64);
    audio_page.append(&settings_row(
        "Pause before finishing speech",
        "How long a pause (in milliseconds) ends the current spoken segment.",
        &endpoint_silence,
    ));
    let debounce = gtk::SpinButton::with_range(150.0, 700.0, 50.0);
    debounce.set_value(config.audio.debounce_ms as f64);
    audio_page.append(&settings_row("Wait before answering", "Briefly waits after speech changes before asking AI to respond, reducing repeated updates.", &debounce));
    let filler_filter = gtk::Switch::new();
    filler_filter.set_active(config.audio.filler_filter);
    audio_page.append(&settings_row(
        "Clean up spoken text",
        "Remove common filler words and repeated or unfinished phrases from transcription.",
        &filler_filter,
    ));
    let confidence = gtk::SpinButton::with_range(0.0, 1.0, 0.05);
    confidence.set_digits(2);
    confidence.set_value(config.audio.confidence_threshold as f64);
    audio_page.append(&settings_row(
        "Ignore uncertain transcripts",
        "Only accept transcriptions at or above this confidence. Set to zero to keep all results.",
        &confidence,
    ));

    let context_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    context_page.set_hexpand(true);
    context_page.set_margin_top(20);
    context_page.set_margin_bottom(20);
    context_page.set_margin_start(20);
    context_page.set_margin_end(20);
    context_page.append(&section_heading("Reference context"));
    let context_intro = gtk::Label::new(Some(
        "Add one file path per line. These files are loaded locally and included as AI context.",
    ));
    context_intro.set_wrap(true);
    context_intro.set_xalign(0.0);
    context_page.append(&context_intro);
    let context_files = gtk::TextView::new();
    context_files.set_monospace(true);
    context_files.set_wrap_mode(gtk::WrapMode::WordChar);
    context_files.set_top_margin(12);
    context_files.set_bottom_margin(12);
    context_files.set_left_margin(12);
    context_files.set_right_margin(12);
    context_files
        .buffer()
        .set_text(&config.context.files.join("\n"));
    let files_scroll = gtk::ScrolledWindow::builder()
        .min_content_height(150)
        .vexpand(true)
        .child(&context_files)
        .build();
    context_page.append(&files_scroll);
    let max_context = gtk::SpinButton::with_range(1000.0, 100000.0, 1000.0);
    max_context.set_value(config.context.max_chars as f64);
    context_page.append(&settings_row(
        "Maximum reference text",
        "Limits how much text is read from your reference files and shared with the AI.",
        &max_context,
    ));

    let screen_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    screen_page.set_hexpand(true);
    screen_page.set_margin_top(20);
    screen_page.set_margin_bottom(20);
    screen_page.set_margin_start(20);
    screen_page.set_margin_end(20);
    screen_page.append(&section_heading("Ask about the screen"));
    let screen_intro = gtk::Label::new(Some(
        "Record a mouse gesture once. Hyprly watches for it while running, captures the screen, and sends the image to Groq's vision model to read and answer it.",
    ));
    screen_intro.set_wrap(true);
    screen_intro.set_xalign(0.0);
    screen_page.append(&screen_intro);
    let vision_model = gtk::Entry::builder()
        .text(&config.screen.vision_model)
        .hexpand(true)
        .build();
    screen_page.append(&settings_row(
        "Groq vision model",
        "Default: qwen/qwen3.8-27b. Save settings to apply a different vision-capable Groq model.",
        &vision_model,
    ));
    let trigger_label = gtk::Label::new(Some(
        &current_trigger
            .as_ref()
            .map(describe_gesture)
            .unwrap_or_else(|| "No gesture recorded".to_string()),
    ));
    state.borrow_mut().trigger_label = Some(trigger_label.clone());
    trigger_label.set_xalign(0.0);
    trigger_label.set_wrap(true);
    let gesture_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let record_gesture = gtk::Button::with_label("Record mouse gesture");
    let clear_gesture = gtk::Button::with_label("Clear");
    gesture_actions.append(&record_gesture);
    gesture_actions.append(&clear_gesture);
    screen_page.append(&settings_row(
        "Saved trigger",
        "Click the button, then click the chosen mouse button 1–10 times. Pause briefly to save that pattern.",
        &trigger_label,
    ));
    screen_page.append(&gesture_actions);
    let screen_privacy_note = gtk::Label::new(Some(
        "Screen images are sent to Groq. If recording times out, allow your user to read the mouse device under /dev/input, then restart Hyprly. Only mouse button presses are used; keyboard input is ignored.",
    ));
    screen_privacy_note.set_wrap(true);
    screen_privacy_note.set_xalign(0.0);
    screen_page.append(&screen_privacy_note);
    record_gesture.connect_clicked({
        let state = state.clone();
        let trigger_label = trigger_label.clone();
        let status = status.clone();
        move |_| {
            let now = std::time::Instant::now();
            state.borrow_mut().recording = Some(GestureRecording {
                label: trigger_label.clone(),
                candidate: None,
                deadline: now + Duration::from_secs(10),
                ignore_until: now + Duration::from_millis(450),
                armed: false,
            });
            trigger_label
                .set_text("Starting recording… click your chosen button after the prompt changes.");
            status.set_text("Recording a mouse gesture for 10 seconds.");
        }
    });
    clear_gesture.connect_clicked({
        let state = state.clone();
        let trigger_label = trigger_label.clone();
        let status = status.clone();
        move |_| match crate::config::Config::save_mouse_gesture(None) {
            Ok(()) => {
                let mut state_ref = state.borrow_mut();
                state_ref.screen_trigger = None;
                state_ref.trigger_clicks = None;
                state_ref.recording = None;
                state_ref.trigger_label = Some(trigger_label.clone());
                trigger_label.set_text("No gesture recorded");
                status.set_text("Screen trigger cleared.");
            }
            Err(error) => status.set_text(&format!("Could not clear screen trigger: {error}")),
        }
    });
    tabs.append_page(&api_page, Some(&gtk::Label::new(Some("AI"))));
    tabs.append_page(&audio_page, Some(&gtk::Label::new(Some("Audio"))));
    tabs.append_page(&context_page, Some(&gtk::Label::new(Some("Context"))));
    tabs.append_page(&screen_page, Some(&gtk::Label::new(Some("Screen"))));
    let scroller = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&tabs)
        .build();
    content.append(&scroller);
    let hint = gtk::Label::new(Some(
        "Changes are written to config.toml. Restart the connection to apply backend changes.",
    ));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("muted");
    content.append(&hint);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    actions.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save settings");
    save.add_css_class("suggested-action");
    actions.append(&cancel);
    actions.append(&save);
    content.append(&actions);
    cancel.connect_clicked({
        let dialog = dialog.clone();
        move |_| dialog.response(gtk::ResponseType::Cancel)
    });
    save.connect_clicked({
        let dialog = dialog.clone();
        move |_| dialog.response(gtk::ResponseType::Accept)
    });
    dialog.connect_response({
        let dialog = dialog.clone();
        let status = status.clone();
        let state = state.clone();
        move |_, response| {
            if response == gtk::ResponseType::Accept {
                let mut updated = config.clone();
                updated.screen.mouse_gesture = state.borrow().screen_trigger.clone();
                let configured_vision_model = vision_model.text().trim().to_string();
                if !configured_vision_model.is_empty() {
                    updated.screen.vision_model = configured_vision_model;
                }
                updated.api.model = selected_dropdown_value(&model, &chat_model_values);
                updated.api.max_tokens = max_tokens.value_as_int().max(1) as u32;
                updated.audio.enabled = audio_enabled.is_active();
                updated.audio.source = selected_dropdown_value(&audio_source, &audio_source_values);
                updated.audio.chunk_seconds = chunk_seconds.value_as_int() as u32;
                updated.audio.min_utterance_seconds = min_utterance.value_as_int() as u32;
                updated.audio.max_utterance_seconds = max_utterance
                    .value_as_int()
                    .max(updated.audio.min_utterance_seconds as i32)
                    as u32;
                updated.audio.transcription_model =
                    selected_dropdown_value(&transcription_model, &transcription_model_values);
                updated.audio.max_concurrent_transcriptions = concurrency.value_as_int() as usize;
                updated.audio.language = selected_dropdown_value(&language, &language_values);
                updated.audio.vad_enabled = vad.is_active();
                updated.audio.silence_threshold = silence_threshold.value() as f32;
                updated.audio.endpoint_silence_ms = endpoint_silence.value_as_int() as u64;
                updated.audio.debounce_ms = debounce.value_as_int() as u64;
                updated.audio.filler_filter = filler_filter.is_active();
                updated.audio.confidence_threshold = confidence.value() as f32;
                let files_buffer = context_files.buffer();
                let files =
                    files_buffer.text(&files_buffer.start_iter(), &files_buffer.end_iter(), true);
                updated.context.files = files
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect();
                updated.context.max_chars = max_context.value_as_int().max(1000) as usize;
                let result = toml::to_string_pretty(&updated)
                    .map_err(anyhow::Error::from)
                    .and_then(|serialized| crate::config::Config::save_config_text(&serialized));
                match result {
                    Ok(()) => status.set_text(
                        "Settings saved. Restart the connection to apply backend changes.",
                    ),
                    Err(error) => {
                        status.set_text(&format!("Could not save settings: {error}"));
                        return;
                    }
                }
            }
            dialog.close();
        }
    });
    dialog.present();
}

fn settings_row(label: &str, description: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_valign(gtk::Align::Center);
    let copy = gtk::Box::new(gtk::Orientation::Vertical, 3);
    let title = gtk::Label::new(Some(label));
    title.set_xalign(0.0);
    title.set_wrap(true);
    title.set_hexpand(true);
    title.add_css_class("heading");
    let help = gtk::Label::new(Some(description));
    help.set_xalign(0.0);
    help.set_wrap(true);
    help.set_hexpand(true);
    help.add_css_class("muted");
    copy.append(&title);
    copy.append(&help);
    copy.set_hexpand(true);
    row.append(&copy);
    row.append(control);
    row
}

fn searchable_dropdown(
    options: &[(String, String)],
    current: &str,
) -> (gtk::DropDown, Vec<String>) {
    let mut choices = options.to_vec();
    if !choices.iter().any(|(_, value)| value == current) {
        let label = if current.is_empty() {
            "Auto detect".to_string()
        } else {
            format!("Current setting · {current}")
        };
        choices.push((label, current.to_string()));
    }
    let labels = gtk::StringList::new(&[]);
    let mut values = Vec::with_capacity(choices.len());
    let mut selected = 0;
    for (index, (label, value)) in choices.into_iter().enumerate() {
        labels.append(&label);
        if value == current {
            selected = index as u32;
        }
        values.push(value);
    }
    let dropdown = gtk::DropDown::new(Some(labels), None::<gtk::Expression>);
    dropdown.set_enable_search(true);
    dropdown.set_hexpand(true);
    dropdown.set_selected(selected);
    (dropdown, values)
}

fn selected_dropdown_value(dropdown: &gtk::DropDown, values: &[String]) -> String {
    values
        .get(dropdown.selected() as usize)
        .cloned()
        .unwrap_or_default()
}

fn available_audio_sources() -> Vec<(String, String)> {
    let mut choices = vec![("Default system audio output".to_string(), String::new())];
    let Ok(output) = Command::new("wpctl").args(["status", "-n"]).output() else {
        return choices;
    };
    if !output.status.success() {
        return choices;
    }
    let mut in_sinks = false;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.contains("Sinks:") {
            in_sinks = true;
            continue;
        }
        if line.contains("Sources:") {
            break;
        }
        if !in_sinks {
            continue;
        }
        let clean = line.replace('*', " ");
        let Some((node_id, name)) = clean.trim().split_once('.') else {
            continue;
        };
        let node_id = node_id.trim().trim_matches(|ch: char| !ch.is_ascii_digit());
        let name = name.trim();
        if !node_id.is_empty() && node_id.chars().all(|ch| ch.is_ascii_digit()) && !name.is_empty()
        {
            choices.push((format!("{name} · {node_id}"), node_id.to_string()));
        }
    }
    choices
}

fn decode_png(data: &[u8]) -> Option<(usize, usize, Vec<u8>)> {
    let mut decoder = png::Decoder::new(data);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut pixels).ok()?;
    let (width, height) = (info.width as usize, info.height as usize);
    if info.color_type != png::ColorType::Rgba {
        return None;
    }
    pixels.truncate(info.buffer_size());
    Some((width, height, pixels))
}

fn section_heading(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("heading");
    label.set_xalign(0.0);
    label
}

fn theme_provider() -> gtk::CssProvider {
    let provider = gtk::CssProvider::new();
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    provider
}

fn install_theme(_application: &gtk::Application) {
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::IconTheme::for_display(&display)
            .add_search_path(concat!(env!("CARGO_MANIFEST_DIR"), "/assets"));
    }
}

fn apply_gui_theme(provider: &gtk::CssProvider, theme: &str) {
    let (bg, surface, raised, border, accent, text, secondary, muted, input, dark) = match theme {
        "paper" => (
            "#eef1f3",
            "#ffffff",
            "#f4f6f8",
            "rgba(36,49,67,.15)",
            "#367a52",
            "#202a35",
            "#536273",
            "#748191",
            "#f7f8fa",
            false,
        ),
        "aurora" => (
            "#091317",
            "#0d1e22",
            "#163138",
            "rgba(120,221,207,.19)",
            "#75e2c2",
            "#eaf8f4",
            "#a8c8c2",
            "#779b95",
            "#0a1a1e",
            true,
        ),
        _ => (
            "#0b0d12",
            "#141821",
            "#1c2230",
            "rgba(160,177,207,.17)",
            "#c6f36a",
            "#f3f5fa",
            "#b0b8c8",
            "#818b9d",
            "#0e121a",
            true,
        ),
    };
    let css = format!(
        r#"
      window, .app-root {{ background: {bg}; color: {text}; }}
      .app-header {{ padding: 0 0 16px; border-bottom: 1px solid {border}; }}
      .wordmark {{ background: transparent; }}
      .card {{ background: {surface}; border: 1px solid {border}; border-radius: 18px; padding: 20px; }}
      .update-notice {{ background: {surface}; border: 1px solid {border}; border-radius: 9px; padding: 3px 8px; }}
      .update-notice label {{ color: {secondary}; font-size: 12px; }}
      .update-notice button {{ min-height: 0; padding: 3px 7px; border: none; background: transparent; }}
      .card > label.heading {{ color: {text}; font-size: 15px; font-weight: 700; padding-bottom: 7px; border-bottom: 1px solid {border}; }}
      entry {{ color: {text}; background: {input}; border: 1px solid {border}; border-radius: 10px; min-height: 38px; padding: 8px 12px; }}
      entry:focus {{ border-color: {accent}; box-shadow: 0 0 0 2px alpha({accent}, .22); }}
      button, dropdown > button {{ color: {text}; background: {raised}; border: 1px solid {border}; border-radius: 10px; padding: 12px 16px; }}
      spinbutton {{ padding: 8px 12px; }}
      spinbutton button {{ margin: 3px; padding: 5px 8px; }}
      button:hover {{ border-color: {accent}; }}
      button.suggested-action {{ color: {dark_ink}; background: {accent}; border-color: {accent}; font-weight: 700; }}
      .theme-picker {{ min-width: 125px; }}
      .muted, label.dim-label {{ color: {secondary}; }}
      .muted {{ color: {muted}; }}
      .pairing-code {{ color: {accent}; font-family: monospace; font-size: 30px; font-weight: 800; letter-spacing: 5px; padding: 10px 12px; background: {input}; border: 1px solid {border}; border-radius: 12px; }}
      .status-line {{ color: {secondary}; padding: 10px 12px; background: {raised}; border: 1px solid {border}; border-radius: 10px; }}
      .card label {{ color: {text}; }}
      .connection-actions {{ padding-top: 4px; }}
      .card button {{ min-height: 38px; }}
    "#,
        dark_ink = if dark { "#102016" } else { "#ffffff" }
    );
    provider.load_from_data(&css);
}

fn gui_theme_path() -> std::path::PathBuf {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".config"))
        })
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    config.join("hyprly").join("gui-theme")
}

fn load_gui_theme() -> String {
    match std::fs::read_to_string(gui_theme_path())
        .as_deref()
        .map(str::trim)
    {
        Ok("paper") => "paper".into(),
        Ok("aurora") => "aurora".into(),
        _ => "midnight".into(),
    }
}

fn save_gui_theme(theme: &str) {
    let path = gui_theme_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, theme);
}

fn save_current_key(state: &Rc<std::cell::RefCell<AppState>>) {
    let state_ref = state.borrow();
    let keys = std::iter::once(state_ref.api_key.text().to_string())
        .chain(
            state_ref
                .backup_keys
                .iter()
                .map(|entry| entry.text().to_string()),
        )
        .collect::<Vec<_>>();
    match crate::config::Config::save_api_keys(&keys) {
        Ok(()) => state_ref
            .status
            .set_text("API key saved in your private Hyprly config."),
        Err(error) => state_ref
            .status
            .set_text(&format!("Could not save API key: {error}")),
    }
}

#[derive(serde::Deserialize)]
struct GitHubReleaseInfo {
    tag_name: String,
    html_url: String,
}

fn check_for_updates(events: Sender<UiEvent>) {
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let update = runtime.block_on(async {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(8))
                .user_agent(concat!("Hyprly/", env!("CARGO_PKG_VERSION")))
                .build()
                .ok()?;
            let response = client
                .get("https://api.github.com/repos/patermars/Hyprly/releases/latest")
                .header("Accept", "application/vnd.github+json")
                .send()
                .await
                .ok()?;
            if !response.status().is_success() {
                return None;
            }
            let release = response.json::<GitHubReleaseInfo>().await.ok()?;
            let current_version = parse_release_version(env!("CARGO_PKG_VERSION"))?;
            let latest_version = parse_release_version(&release.tag_name)?;
            if latest_version <= current_version {
                return None;
            }
            let url = if release
                .html_url
                .starts_with("https://github.com/patermars/Hyprly/releases/")
            {
                release.html_url
            } else {
                "https://github.com/patermars/Hyprly/releases/latest".to_string()
            };
            Some(UiEvent::UpdateAvailable {
                version: release.tag_name,
                url,
            })
        });
        if let Some(update) = update {
            let _ = events.send(update);
        }
    });
}

fn parse_release_version(version: &str) -> Option<(u64, u64, u64)> {
    let version = version.trim().strip_prefix('v').unwrap_or(version.trim());
    let numeric = version.split(['-', '+']).next()?;
    let mut parts = numeric.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn start_backend(state: &Rc<std::cell::RefCell<AppState>>) {
    if state.borrow().child.is_some() {
        return;
    }
    let keys = {
        let state = state.borrow();
        std::iter::once(state.api_key.text().to_string())
            .chain(
                state
                    .backup_keys
                    .iter()
                    .map(|entry| entry.text().to_string()),
            )
            .collect::<Vec<_>>()
    };
    if keys.iter().all(|key| key.trim().is_empty()) {
        state
            .borrow()
            .status
            .set_text("Add your Groq API key first.");
        return;
    }
    if let Err(error) = crate::config::Config::save_api_keys(&keys) {
        state
            .borrow()
            .status
            .set_text(&format!("Could not save API key: {error}"));
        return;
    }

    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            state
                .borrow()
                .status
                .set_text(&format!("Could not locate Hyprly: {error}"));
            return;
        }
    };
    let mut child = match Command::new(executable)
        .arg("daemon")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            state
                .borrow()
                .status
                .set_text(&format!("Could not start Hyprly: {error}"));
            return;
        }
    };
    let backend_stdin = child
        .stdin
        .take()
        .map(|stdin| std::sync::Arc::new(std::sync::Mutex::new(stdin)));

    let event_tx = state.borrow().event_tx.clone();
    if let Some(stdout) = child.stdout.take() {
        let event_tx = event_tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().flatten() {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if value.get("event").and_then(|v| v.as_str()) != Some("pairing") {
                    continue;
                }
                let info = PairingInfo {
                    code: value
                        .get("code")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .into(),
                    setup_url: value
                        .get("setup_url")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .into(),
                    secure_url: value
                        .get("secure_url")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .into(),
                    fingerprint: value
                        .get("fingerprint")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .into(),
                };
                let _ = event_tx.send(UiEvent::Pairing(info));
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().flatten() {
                if is_groq_api_error(&line) {
                    let _ = event_tx.send(UiEvent::GroqError(friendly_api_error(&line)));
                }
            }
        });
    }

    let mut state_ref = state.borrow_mut();
    state_ref.child = Some(child);
    state_ref.backend_stdin = backend_stdin;
    state_ref.connect_button.set_sensitive(false);
    state_ref.disconnect_button.set_sensitive(true);
    state_ref
        .status
        .set_text("Hyprly is starting. Pair your phone using the code and QR below.");
}

fn stop_backend(state: &Rc<std::cell::RefCell<AppState>>) {
    let (child, _) = {
        let mut state = state.borrow_mut();
        (state.child.take(), state.backend_stdin.take())
    };
    if let Some(mut child) = child {
        let _ = child.kill();
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
    clear_pairing(state);
    let state_ref = state.borrow();
    state_ref.connect_button.set_sensitive(true);
    state_ref.disconnect_button.set_sensitive(false);
    state_ref.status.set_text("Hyprly is disconnected.");
}

fn clear_pairing(state: &Rc<std::cell::RefCell<AppState>>) {
    let state = state.borrow();
    state.code.set_text("Not connected");
    state.setup_url.set_label("Setup URL");
    state.setup_url.set_sensitive(false);
    state.secure_url.set_label("Secure URL");
    state.secure_url.set_sensitive(false);
    state.fingerprint.set_text("");
    *state.qr_code.borrow_mut() = None;
    state.qr_area.queue_draw();
}

fn poll_events(state: &Rc<std::cell::RefCell<AppState>>) {
    poll_mouse_events(state);
    let mut pending = Vec::new();
    {
        let state_ref = state.borrow();
        while let Ok(action) = state_ref.tray_rx.try_recv() {
            pending.push(action);
        }
    }
    for action in pending {
        match action {
            TrayAction::Open => state.borrow().window.present(),
            TrayAction::Connect => start_backend(state),
            TrayAction::Disconnect => stop_backend(state),
            TrayAction::Exit => {
                stop_backend(state);
                state.borrow().application.quit();
            }
        }
    }

    let exited = state
        .borrow_mut()
        .child
        .as_mut()
        .and_then(|child| child.try_wait().ok().flatten());
    if exited.is_some() {
        state.borrow_mut().child = None;
        clear_pairing(state);
        let state_ref = state.borrow();
        state_ref.connect_button.set_sensitive(true);
        state_ref.disconnect_button.set_sensitive(false);
        state_ref.status.set_text("Hyprly has stopped.");
    }

    let mut events = Vec::new();
    {
        let state_ref = state.borrow();
        while let Ok(event) = state_ref.event_rx.try_recv() {
            events.push(event);
        }
    }
    for event in events {
        let state_ref = state.borrow();
        match event {
            UiEvent::Pairing(info) => {
                state_ref.code.set_text(&info.code);
                state_ref.setup_url.set_uri(&info.setup_url);
                state_ref
                    .setup_url
                    .set_label(&format!("Setup: {}", info.setup_url));
                state_ref.setup_url.set_sensitive(true);
                state_ref.secure_url.set_uri(&info.secure_url);
                state_ref
                    .secure_url
                    .set_label(&format!("Secure: {}", info.secure_url));
                state_ref.secure_url.set_sensitive(true);
                state_ref
                    .fingerprint
                    .set_text(&format!("Certificate fingerprint: {}", info.fingerprint));
                *state_ref.qr_code.borrow_mut() = QrCode::new(info.secure_url.as_bytes()).ok();
                state_ref.qr_area.queue_draw();
            }
            UiEvent::UpdateAvailable { version, url } => {
                state_ref
                    .update_label
                    .set_text(&format!("Update available: {version}"));
                state_ref.update_link.set_uri(&url);
                state_ref.update_notice.set_visible(true);
            }
            UiEvent::Message(message) => state_ref.status.set_text(&message),
            UiEvent::GroqError(message) => state_ref.status.set_text(&message),
        }
    }
}

fn poll_mouse_events(state: &Rc<std::cell::RefCell<AppState>>) {
    let mut presses = Vec::new();
    {
        let state_ref = state.borrow();
        while let Ok(press) = state_ref.mouse_rx.try_recv() {
            presses.push(press);
        }
    }

    for press in presses {
        let mut should_launch = false;
        {
            let mut state_ref = state.borrow_mut();
            if let Some(recording) = state_ref.recording.as_mut() {
                if press.at < recording.ignore_until || press.at > recording.deadline {
                    continue;
                }
                let next = next_click_sequence(recording.candidate, press);
                recording.candidate = Some(next);
                recording.label.set_text(&format!(
                    "Detected {} ({} click{}). Continue the pattern, then pause briefly to save.",
                    mouse_button_name(next.button),
                    next.clicks,
                    if next.clicks == 1 { "" } else { "s" }
                ));
            } else if let Some((button, clicks)) = state_ref
                .screen_trigger
                .as_ref()
                .map(|trigger| (trigger.button, trigger.clicks))
            {
                let next = next_click_sequence(state_ref.trigger_clicks, press);
                state_ref.trigger_clicks = Some(next);
                if next.button == button && next.clicks == clicks {
                    state_ref.trigger_clicks = None;
                    should_launch = true;
                }
            }
        }
        if should_launch {
            launch_screen_question(state);
        }
    }

    let now = std::time::Instant::now();
    let mut finished = None;
    let mut timed_out = None;
    {
        let mut state_ref = state.borrow_mut();
        let mut clear_recording = false;
        if let Some(recording) = state_ref.recording.as_mut() {
            if !recording.armed && now >= recording.ignore_until {
                recording.armed = true;
                recording
                    .label
                    .set_text("Now click your chosen mouse button 1–10 times, then pause briefly.");
            }
            if let Some(candidate) = recording.candidate {
                if now.duration_since(candidate.last_click) >= Duration::from_millis(750) {
                    finished = Some(crate::config::MouseGesture {
                        button: candidate.button,
                        clicks: candidate.clicks,
                    });
                    clear_recording = true;
                }
            }
            if finished.is_none() && now >= recording.deadline {
                timed_out = Some(recording.label.clone());
                clear_recording = true;
            }
        }
        if clear_recording {
            state_ref.recording = None;
        }
        if let Some(candidate) = state_ref.trigger_clicks {
            if now.duration_since(candidate.last_click) > Duration::from_millis(750) {
                state_ref.trigger_clicks = None;
            }
        }
    }

    if let Some(gesture) = finished {
        match crate::config::Config::save_mouse_gesture(Some(&gesture)) {
            Ok(()) => {
                let description = describe_gesture(&gesture);
                let mut state_ref = state.borrow_mut();
                state_ref.screen_trigger = Some(gesture);
                state_ref.trigger_clicks = None;
                if let Some(label) = state_ref.trigger_label.as_ref() {
                    label.set_text(&description);
                }
                state_ref.status.set_text(&format!(
                    "Screen gesture saved: {description}. Hyprly is listening while running."
                ));
            }
            Err(error) => state
                .borrow()
                .status
                .set_text(&format!("Could not save screen gesture: {error}")),
        }
    }
    if let Some(label) = timed_out {
        label.set_text("No mouse clicks detected. Check /dev/input access and try again.");
        state
            .borrow()
            .status
            .set_text("Gesture recording timed out. Check access to /dev/input/event devices.");
    }
}

fn next_click_sequence(
    previous: Option<ClickSequence>,
    press: crate::input::MousePress,
) -> ClickSequence {
    match previous {
        Some(previous)
            if previous.button == press.button
                && press.at.duration_since(previous.last_click) <= Duration::from_millis(750) =>
        {
            ClickSequence {
                button: previous.button,
                clicks: previous.clicks.saturating_add(1).min(10),
                last_click: press.at,
            }
        }
        _ => ClickSequence {
            button: press.button,
            clicks: 1,
            last_click: press.at,
        },
    }
}

fn launch_screen_question(state: &Rc<std::cell::RefCell<AppState>>) {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            state
                .borrow()
                .status
                .set_text(&format!("Could not locate Hyprly: {error}"));
            return;
        }
    };
    let backend_stdin = state.borrow().backend_stdin.clone();
    let Some(backend_stdin) = backend_stdin else {
        state
            .borrow()
            .status
            .set_text("Start Hyprly and pair the webpage before asking about the screen.");
        return;
    };
    if !send_backend_event(&backend_stdin, serde_json::json!({"type":"screen_started"})) {
        state
            .borrow()
            .status
            .set_text("Could not reach the paired webpage. Reconnect Hyprly and try again.");
        return;
    }
    match Command::new(executable)
        .arg("ask-screen")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => {
            state
                .borrow()
                .status
                .set_text("Screen question started. The answer will appear on the paired webpage.");
            let events = state.borrow().event_tx.clone();
            std::thread::spawn(move || match child.wait_with_output() {
                Ok(output) if output.status.success() => {
                    let answer = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    if answer.is_empty() {
                        let message = "Groq returned an empty answer.";
                        let _ = send_backend_event(
                            &backend_stdin,
                            serde_json::json!({"type":"screen_error","message":message}),
                        );
                        let _ = events.send(UiEvent::Message(message.to_string()));
                    } else if !send_backend_event(
                        &backend_stdin,
                        serde_json::json!({"type":"screen_answer","text":answer}),
                    ) {
                        let _ = events.send(UiEvent::Message(
                            "Could not send the screen answer to the paired webpage.".into(),
                        ));
                    }
                }
                Ok(output) => {
                    let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
                    let message = if detail.is_empty() {
                        "Screen question failed."
                    } else {
                        &detail
                    };
                    let _ = send_backend_event(
                        &backend_stdin,
                        serde_json::json!({"type":"screen_error","message":message}),
                    );
                    let _ = events.send(UiEvent::Message(format!(
                        "Screen question failed: {}",
                        message
                    )));
                }
                Err(error) => {
                    let message = format!("Could not finish screen question: {error}");
                    let _ = send_backend_event(
                        &backend_stdin,
                        serde_json::json!({"type":"screen_error","message":message}),
                    );
                    let _ = events.send(UiEvent::Message(format!("{message}")));
                }
            });
        }
        Err(error) => {
            let message = format!("Could not start screen question: {error}");
            let _ = send_backend_event(
                &backend_stdin,
                serde_json::json!({"type":"screen_error","message":message}),
            );
            state.borrow().status.set_text(&message);
        }
    }
}

fn send_backend_event(
    stdin: &std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>,
    event: serde_json::Value,
) -> bool {
    use std::io::Write;
    let Ok(mut stdin) = stdin.lock() else {
        return false;
    };
    serde_json::to_writer(&mut *stdin, &event).is_ok()
        && stdin.write_all(b"\n").is_ok()
        && stdin.flush().is_ok()
}

fn mouse_button_name(button: u16) -> String {
    match button {
        0x110 => "left mouse button".to_string(),
        0x111 => "right mouse button".to_string(),
        0x112 => "middle mouse button".to_string(),
        code => format!("mouse button {}", code - 0x10f),
    }
}

fn describe_gesture(gesture: &crate::config::MouseGesture) -> String {
    format!("{} × {}", gesture.clicks, mouse_button_name(gesture.button))
}

fn test_api_key(state: &Rc<std::cell::RefCell<AppState>>) {
    let keys = {
        let state = state.borrow();
        std::iter::once(state.api_key.text().to_string())
            .chain(
                state
                    .backup_keys
                    .iter()
                    .map(|entry| entry.text().to_string()),
            )
            .filter(|key| !key.trim().is_empty())
            .collect::<Vec<_>>()
    };
    if keys.is_empty() {
        state
            .borrow()
            .status
            .set_text("Enter a Groq API key first.");
        return;
    }
    state.borrow().status.set_text("Checking the Groq API key…");
    let sender = state.borrow().event_tx.clone();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("Could not start API check")
            .and_then(|runtime| {
                runtime.block_on(async move {
                    let client = reqwest::Client::builder()
                        .timeout(Duration::from_secs(12))
                        .build()?;
                    let mut failures = Vec::new();
                    for (index, key) in keys.iter().enumerate() {
                        match client
                            .get("https://api.groq.com/openai/v1/models")
                            .bearer_auth(key.trim())
                            .send()
                            .await
                        {
                            Ok(response) if response.status().is_success() => {
                                return Ok(format!("Groq key {} works.", index + 1))
                            }
                            Ok(response) => failures.push(format!(
                                "Key {}: HTTP {}",
                                index + 1,
                                response.status()
                            )),
                            Err(error) => failures.push(format!("Key {}: {error}", index + 1)),
                        }
                    }
                    anyhow::bail!("All Groq keys failed. {}", failures.join("; "));
                })
            });
        let message = match result {
            Ok(message) => message,
            Err(error) => friendly_api_error(&format!("{error:#}")),
        };
        let _ = sender.send(UiEvent::Message(message));
    });
}

fn check_pipewire(state: &Rc<std::cell::RefCell<AppState>>) {
    state.borrow().status.set_text("Testing PipeWire capture…");
    let sender = state.borrow().event_tx.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<String> {
            let status = Command::new("wpctl")
                .args(["status", "-n"])
                .output()
                .context("WirePlumber command wpctl was not found")?;
            if !status.status.success() {
                anyhow::bail!("wpctl could not query the PipeWire session");
            }
            let text = String::from_utf8_lossy(&status.stdout);
            let sink = selected_sink(&text);
            let mut command = Command::new("pw-record");
            command.args([
                "--rate",
                "16000",
                "--channels",
                "1",
                "--format",
                "s16",
                "--raw",
                "--sample-count",
                "1600",
            ]);
            if let Some(sink) = sink {
                command.args(["--target", &sink]);
            }
            command.arg("-");
            let recording = command
                .output()
                .context("PipeWire command pw-record was not found")?;
            if !recording.status.success() || recording.stdout.is_empty() {
                anyhow::bail!("PipeWire is running, but audio capture failed. Check your default output device.");
            }
            Ok("PipeWire and audio capture are working.".to_string())
        })();
        let message = result.unwrap_or_else(|error| format!("PipeWire check failed: {error:#}"));
        let _ = sender.send(UiEvent::Message(message));
    });
}

fn selected_sink(status: &str) -> Option<String> {
    let mut in_sinks = false;
    for line in status.lines() {
        if line.contains("Sinks:") {
            in_sinks = true;
            continue;
        }
        if line.contains("Sources:") {
            break;
        }
        if in_sinks && line.contains('*') {
            let after_star = line.split_once('*')?.1.trim_start();
            let id = after_star.split_once('.')?.0.trim();
            if id.parse::<u32>().is_ok() {
                return Some(id.to_string());
            }
        }
    }
    None
}

fn check_document_tools(state: &Rc<std::cell::RefCell<AppState>>) {
    state
        .borrow()
        .status
        .set_text("Checking optional document tools…");
    let sender = state.borrow().event_tx.clone();
    std::thread::spawn(move || {
        let has_pdf = Command::new("pdftotext").arg("-v").output().is_ok();
        let has_docx = Command::new("unzip").arg("-v").output().is_ok();
        let message = match (has_pdf, has_docx) {
            (true, true) => "PDF and DOCX uploads are enabled (Poppler and unzip found).".into(),
            (true, false) => "PDF uploads work; install unzip to enable DOCX uploads.".into(),
            (false, true) => "DOCX uploads work; install Poppler to enable PDF uploads.".into(),
            (false, false) => "Install Poppler for PDF uploads and unzip for DOCX uploads.".into(),
        };
        let _ = sender.send(UiEvent::Message(message));
    });
}

fn is_groq_api_error(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "api request failed",
        "transcription request failed",
        "error sending request",
        "timed out",
        "timeout",
        "invalid_api_key",
        "rate_limit",
        "quota",
        "exceeded",
        "expired",
        "usage limit",
        "status 401",
        "status 403",
        "status 404",
        "status 429",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn friendly_api_error(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    let summary = if lower.contains("401") || lower.contains("invalid_api_key") {
        "Groq rejected this API key. Check that it is valid and active."
    } else if lower.contains("429") || lower.contains("rate_limit") {
        "Groq rate limit reached. Wait a little before trying again."
    } else if lower.contains("quota")
        || lower.contains("usage limit")
        || lower.contains("exceeded")
        || lower.contains("billing")
    {
        "Groq reports that your usage limit has been reached."
    } else if lower.contains("timeout") || lower.contains("timed out") {
        "The request to Groq timed out. Check your connection and try again."
    } else if lower.contains("403") {
        "Groq denied this request. Check your account and API key permissions."
    } else if lower.contains("404") {
        "Groq could not find the requested model or endpoint."
    } else if lower.contains("connect") || lower.contains("dns") {
        "Could not reach Groq. Check your internet connection."
    } else {
        "Groq API error. Check the key, account limits, and network connection."
    };
    summary.into()
}

fn start_tray(actions: Sender<TrayAction>, events: Sender<UiEvent>) {
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                let _ = events.send(UiEvent::Message(format!(
                    "Could not start tray service: {error}"
                )));
                return;
            }
        };
        runtime.block_on(async move {
            match (HyprlyTray { actions }).spawn().await {
                Ok(_handle) => std::future::pending::<()>().await,
                Err(error) => {
                    let _ = events.send(UiEvent::Message(format!(
                        "System tray is unavailable. The window still works. ({error})"
                    )));
                }
            }
        });
    });
}
