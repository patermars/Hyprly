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
    status: gtk::Label,
    code: gtk::Label,
    setup_url: gtk::LinkButton,
    secure_url: gtk::LinkButton,
    fingerprint: gtk::Label,
    qr_area: gtk::DrawingArea,
    qr_code: Rc<std::cell::RefCell<Option<QrCode>>>,
    connect_button: gtk::Button,
    disconnect_button: gtk::Button,
    child: Option<Child>,
    event_tx: Sender<UiEvent>,
    event_rx: Receiver<UiEvent>,
    tray_rx: Receiver<TrayAction>,
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
    let theme_picker = gtk::DropDown::from_strings(&["Midnight", "Paper", "Aurora"]);
    theme_picker.add_css_class("theme-picker");
    theme_picker.set_tooltip_text(Some("Choose appearance"));
    theme_picker.set_halign(gtk::Align::End);
    theme_picker.set_valign(gtk::Align::Start);
    header.append(&theme_picker);
    root.append(&header);

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

    let key_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
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
    let checks = gtk::Box::new(gtk::Orientation::Horizontal, 8);
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

    let state = Rc::new(std::cell::RefCell::new(AppState {
        application: application.clone(),
        window: window.clone(),
        api_key: api_key.clone(),
        status: status.clone(),
        code: code.clone(),
        setup_url: setup_url.clone(),
        secure_url: secure_url.clone(),
        fingerprint: fingerprint.clone(),
        qr_area: qr_area.clone(),
        qr_code,
        connect_button: connect.clone(),
        disconnect_button: disconnect.clone(),
        child: None,
        event_tx,
        event_rx,
        tray_rx,
    }));

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
    let configured_key = crate::config::Config::load()
        .map(|config| config.api.api_key)
        .unwrap_or_default();
    if configured_key.is_empty() {
        api_key.grab_focus();
    } else {
        api_key.set_text(&configured_key);
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
      .card > label.heading {{ color: {text}; font-size: 15px; font-weight: 700; padding-bottom: 7px; border-bottom: 1px solid {border}; }}
      entry {{ color: {text}; background: {input}; border: 1px solid {border}; border-radius: 10px; min-height: 38px; }}
      entry:focus {{ border-color: {accent}; box-shadow: 0 0 0 2px alpha({accent}, .22); }}
      button, dropdown > button {{ color: {text}; background: {raised}; border: 1px solid {border}; border-radius: 10px; padding: 8px 12px; }}
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
    let key = state_ref.api_key.text();
    match crate::config::Config::save_api_key(&key) {
        Ok(()) => state_ref
            .status
            .set_text("API key saved in your private Hyprly config."),
        Err(error) => state_ref
            .status
            .set_text(&format!("Could not save API key: {error}")),
    }
}

fn start_backend(state: &Rc<std::cell::RefCell<AppState>>) {
    if state.borrow().child.is_some() {
        return;
    }
    let key = state.borrow().api_key.text().to_string();
    if key.trim().is_empty() {
        state
            .borrow()
            .status
            .set_text("Add your Groq API key first.");
        return;
    }
    if let Err(error) = crate::config::Config::save_api_key(&key) {
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
        .stdin(Stdio::null())
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
    state_ref.connect_button.set_sensitive(false);
    state_ref.disconnect_button.set_sensitive(true);
    state_ref
        .status
        .set_text("Hyprly is starting. Pair your phone using the code and QR below.");
}

fn stop_backend(state: &Rc<std::cell::RefCell<AppState>>) {
    let child = state.borrow_mut().child.take();
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
            UiEvent::Message(message) => state_ref.status.set_text(&message),
            UiEvent::GroqError(message) => state_ref.status.set_text(&message),
        }
    }
}

fn test_api_key(state: &Rc<std::cell::RefCell<AppState>>) {
    let key = state.borrow().api_key.text().to_string();
    if key.trim().is_empty() {
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
                    let response = client
                        .get("https://api.groq.com/openai/v1/models")
                        .bearer_auth(key.trim())
                        .send()
                        .await?;
                    if response.status().is_success() {
                        Ok("Groq API key works.".to_string())
                    } else {
                        let status = response.status();
                        let body = response.text().await.unwrap_or_default();
                        anyhow::bail!("Groq API returned HTTP {status}: {body}");
                    }
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
