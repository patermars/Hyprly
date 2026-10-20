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
const GESTURE_CLICK_GAP: Duration = Duration::from_millis(750);
const ANSWER_OVERLAY_MIN_ANSWER_HEIGHT: i32 = 56;
const FLOATING_PREVIEW_TITLE: &str = "Hyprly Floating Answer Preview";

#[derive(Clone, Debug)]
struct PairingInfo {
    code: String,
    setup_url: String,
    secure_url: String,
    fingerprint: String,
}

enum UiEvent {
    Pairing(PairingInfo),
    Connection(bool),
    ScreenFinished(u32),
    UpdateAvailable { version: String, url: String },
    Message(String),
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
    overlay_child: Option<Child>,
    paired: bool,
    screen_pid: Option<u32>,
    backend_stdin: Option<std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>>,
    overlay_stdin: Option<std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>>,
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

pub fn run_screen_test_overlay() {
    let application = gtk::Application::builder()
        .application_id("io.github.patermars.HyprlyScreenTest")
        .build();
    application.connect_activate(|application| {
        let label = gtk::Label::new(Some(
            "NO SCREEN SHARE TEST\n\nThis text should stay visible here,\nbut be hidden in the screen share.",
        ));
        label.set_justify(gtk::Justification::Center);
        label.set_wrap(true);
        label.add_css_class("screen-test-label");

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.set_halign(gtk::Align::Fill);
        content.set_valign(gtk::Align::Fill);
        content.set_hexpand(true);
        content.set_vexpand(true);
        content.add_css_class("screen-test-window");
        content.append(&label);

        let window = gtk::ApplicationWindow::builder()
            .application(application)
            .title("Hyprly No Screen Share Test")
            .default_width(460)
            .default_height(220)
            .child(&content)
            .build();
        let provider = gtk::CssProvider::new();
        provider.load_from_data(
            ".screen-test-window { background-color: #171923; }\
             .screen-test-label { color: #f5f7ff; font-size: 20px; font-weight: 700; padding: 28px; }",
        );
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
        window.present();
    });
    application.run_with_args(&["hyprly-screen-test"]);
}

pub fn run_answer_overlay() {
    // Keep the application ID aligned with the verified test window class so
    // the user's existing Hyprland float/no_screen_share rule also matches it.
    let application = gtk::Application::builder()
        .application_id("io.github.patermars.HyprlyScreenTest")
        .build();
    application.connect_activate(|application| {
        let saved_overlay_size = crate::config::Config::load()
            .map(|config| config.screen.overlay_window)
            .unwrap_or_default();
        let overlay_size = Rc::new(std::cell::RefCell::new(saved_overlay_size));
        let initially_visible = std::env::var("HYPRLY_OVERLAY_VISIBLE").as_deref() != Ok("0");
        let overlay_is_visible = Rc::new(std::cell::Cell::new(initially_visible));
        let (event_tx, event_rx) = mpsc::channel::<serde_json::Value>();
        std::thread::spawn(move || {
            for line in std::io::stdin().lock().lines().map_while(|line| line.ok()) {
                if let Ok(event) = serde_json::from_str(&line) {
                    let _ = event_tx.send(event);
                }
            }
        });

        let theme_provider = gtk::CssProvider::new();
        let root = gtk::Box::new(gtk::Orientation::Vertical, 12);
        root.set_margin_top(0);
        root.set_margin_bottom(0);
        root.set_margin_start(0);
        root.set_margin_end(0);
        root.add_css_class("overlay-root");

        let pages = gtk::Stack::new();
        pages.set_hexpand(true);
        pages.set_vexpand(true);
        // The Context page contains several multi-line editors. Keep its
        // natural height from forcing the Session page to that same height.
        pages.set_vhomogeneous(false);
        let page_switcher = gtk::StackSwitcher::new();
        page_switcher.set_stack(Some(&pages));
        page_switcher.set_halign(gtk::Align::Start);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        header.add_css_class("overlay-island");
        let logo = gtk::DrawingArea::builder()
            .content_width(26)
            .content_height(26)
            .build();
        logo.set_size_request(26, 26);
        let logo_surface = create_logo_surface();
        logo.set_draw_func(move |_, context, width, height| {
            let scale = (width as f64 / logo_surface.width() as f64)
                .min(height as f64 / logo_surface.height() as f64);
            let x = (width as f64 - logo_surface.width() as f64 * scale) / 2.0;
            let y = (height as f64 - logo_surface.height() as f64 * scale) / 2.0;
            context.translate(x, y);
            context.scale(scale, scale);
            let _ = context.set_source_surface(&logo_surface, 0.0, 0.0);
            let _ = context.paint();
        });
        let connection = gtk::Label::new(Some("Waiting for Hyprly…"));
        connection.set_hexpand(true);
        connection.set_xalign(0.0);
        connection.add_css_class("overlay-status");
        let pause = gtk::Button::with_label("Pause");
        pause.add_css_class("overlay-control");
        let clear = gtk::Button::with_label("Clear");
        clear.add_css_class("overlay-control");
        let minimize = gtk::Button::from_icon_name("window-close-symbolic");
        minimize.set_tooltip_text(Some("Minimize floating window"));
        minimize.add_css_class("overlay-control");
        let copy_code = gtk::Button::with_label("Copy code");
        copy_code.add_css_class("overlay-control");
        copy_code.set_visible(false);
        header.append(&logo);
        header.append(&page_switcher);
        header.append(&connection);
        header.append(&pause);
        header.append(&clear);
        header.append(&minimize);
        root.append(&header);

        root.append(&pages);

        let session = gtk::Box::new(gtk::Orientation::Vertical, 10);
        session.set_vexpand(true);
        session.set_margin_top(0);
        session.set_margin_bottom(0);
        session.add_css_class("hud-card");
        let context_strip = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        context_strip.add_css_class("hud-context-strip");
        let context_tag = gtk::Label::new(Some("CONTEXT"));
        context_tag.add_css_class("overlay-heading");
        let vision_status = gtk::Label::new(Some("Live vision ready"));
        vision_status.set_hexpand(true);
        vision_status.set_halign(gtk::Align::End);
        vision_status.add_css_class("overlay-status");
        context_strip.append(&context_tag);
        context_strip.append(&vision_status);
        session.append(&context_strip);
        let pairing = gtk::Box::new(gtk::Orientation::Vertical, 6);
        pairing.add_css_class("hud-pairing");
        let pair_intro =
            gtk::Label::new(Some("Enter your Hyprly pairing code to start the session"));
        pair_intro.set_xalign(0.0);
        pair_intro.add_css_class("overlay-status");
        pairing.append(&pair_intro);
        let local_code = gtk::Entry::builder()
            .placeholder_text("6-digit pairing code")
            .max_length(6)
            .max_width_chars(24)
            .build();
        local_code.set_width_chars(24);
        let local_pair = gtk::Button::with_label("Connect");
        let local_pair_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        local_pair_row.append(&local_code);
        local_pair_row.append(&local_pair);
        pairing.append(&local_pair_row);
        session.append(&pairing);
        let answer = gtk::Label::new(Some("Waiting for context…"));
        answer.set_xalign(0.0);
        answer.set_yalign(0.0);
        answer.set_wrap(true);
        answer.set_selectable(true);
        answer.add_css_class("overlay-answer");
        let streaming_answer = Rc::new(std::cell::RefCell::new(String::new()));
        let answer_text = Rc::new(std::cell::RefCell::new(String::new()));
        let answer_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        answer_actions.set_halign(gtk::Align::End);
        answer_actions.append(&copy_code);
        session.append(&answer_actions);
        let answer_scroll = gtk::ScrolledWindow::builder()
            .min_content_height(ANSWER_OVERLAY_MIN_ANSWER_HEIGHT)
            .max_content_height(saved_overlay_size.max_height as i32)
            .vexpand(true)
            .child(&answer)
            .build();
        answer_scroll.add_css_class("hud-answer-flow");
        session.append(&answer_scroll);
        let transcript_strip = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        transcript_strip.set_hexpand(true);
        transcript_strip.add_css_class("hud-inner-panel");
        let transcript_heading = gtk::Label::new(Some("LIVE TRANSCRIPT"));
        transcript_heading.add_css_class("overlay-heading");
        let transcript = gtk::Label::new(Some("No audio detected yet."));
        transcript.set_xalign(0.0);
        transcript.set_hexpand(true);
        transcript.set_single_line_mode(true);
        transcript.set_ellipsize(gtk::pango::EllipsizeMode::End);
        transcript.set_selectable(true);
        transcript.set_tooltip_text(Some("No audio detected yet."));
        transcript.add_css_class("overlay-transcript");
        transcript_strip.append(&transcript_heading);
        transcript_strip.append(&transcript);
        session.append(&transcript_strip);
        let session_scroll = gtk::ScrolledWindow::builder()
            .min_content_height(1)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&session)
            .build();
        session_scroll.set_propagate_natural_height(false);
        pages.add_titled(&session_scroll, Some("session"), "Session");

        let context_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
        context_page.set_margin_top(12);
        let context_intro = gtk::Label::new(Some(
            "Add background that should guide answers. Save applies it to this Hyprly session.",
        ));
        context_intro.set_wrap(true);
        context_intro.set_xalign(0.0);
        context_page.append(&context_intro);
        let (resume_label, resume) = overlay_context_field(
            "Resume or experience",
            "Paste your resume or key experiences",
            4,
        );
        let (job_label, job) = overlay_context_field(
            "Job description",
            "Paste the job description or key requirements",
            4,
        );
        let (company_label, company) = overlay_context_field(
            "Company notes",
            "Paste company, team, product, or other key details",
            3,
        );
        context_page.append(&resume_label);
        context_page.append(&overlay_context_upload(&resume.1));
        context_page.append(&resume.0);
        context_page.append(&job_label);
        context_page.append(&overlay_context_upload(&job.1));
        context_page.append(&job.0);
        context_page.append(&company_label);
        context_page.append(&overlay_context_upload(&company.1));
        context_page.append(&company.0);
        let context_saved = gtk::Label::new(Some(""));
        let save_context = gtk::Button::with_label("Save context");
        save_context.set_halign(gtk::Align::End);
        let context_actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        context_actions.append(&context_saved);
        context_actions.append(&save_context);
        context_page.append(&context_actions);
        let context_scroll = gtk::ScrolledWindow::builder()
            .min_content_height(1)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&context_page)
            .build();
        context_scroll.set_propagate_natural_height(false);
        pages.add_titled(&context_scroll, Some("context"), "Context");

        overlay_apply_style(&theme_provider);
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &theme_provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }

        pause.connect_clicked({
            let pause = pause.clone();
            let paused = Rc::new(std::cell::Cell::new(false));
            move |_| {
                let now_paused = !paused.get();
                paused.set(now_paused);
                pause.set_label(if now_paused { "Resume" } else { "Pause" });
                overlay_send(
                    serde_json::json!({"type": if now_paused { "pause" } else { "resume" }}),
                );
            }
        });
        clear.connect_clicked({
            let transcript = transcript.clone();
            move |_| {
                transcript.set_text("No audio detected yet.");
                transcript.set_tooltip_text(Some("No audio detected yet."));
                overlay_send(serde_json::json!({"type":"clear_transcript"}));
            }
        });
        copy_code.connect_clicked({
            let answer_text = answer_text.clone();
            let connection = connection.clone();
            move |_| {
                let answer = answer_text.borrow();
                let extracted = extract_answer_code(&answer);
                if !extracted.is_empty() {
                    if let Some(display) = gtk::gdk::Display::default() {
                        display.clipboard().set_text(&extracted);
                        connection.set_text("Code copied");
                    }
                }
            }
        });
        save_context.connect_clicked({
            let resume = resume.1.clone();
            let job = job.1.clone();
            let company = company.1.clone();
            let context_saved = context_saved.clone();
            move |_| {
                overlay_send(serde_json::json!({
                    "type":"update_context",
                    "resume": overlay_buffer_text(&resume),
                    "job_description": overlay_buffer_text(&job),
                    "company": overlay_buffer_text(&company),
                }));
                context_saved.set_text("Saved for this session");
            }
        });
        local_pair.connect_clicked({
            let local_code = local_code.clone();
            let connection = connection.clone();
            move |_| overlay_pair_local(&local_code, &connection)
        });
        local_code.connect_changed({
            let connection = connection.clone();
            move |entry| {
                if entry.text().len() == 6 {
                    overlay_pair_local(entry, &connection);
                }
            }
        });

        let window = gtk::ApplicationWindow::builder()
            .application(application)
            .title("Hyprly Floating Assistant")
            .default_width(saved_overlay_size.width as i32)
            .default_height(saved_overlay_size.height as i32)
            .child(&root)
            .build();
        window.add_css_class("answer-window");
        window.set_decorated(false);
        window.set_resizable(false);
        window.set_size_request(
            saved_overlay_size.min_width as i32,
            saved_overlay_size.min_height as i32,
        );
        let launcher_logo = gtk::DrawingArea::builder()
            .content_width(64)
            .content_height(64)
            .build();
        launcher_logo.set_size_request(64, 64);
        let launcher_logo_surface = create_logo_surface();
        launcher_logo.set_draw_func(move |_, context, width, height| {
            let scale = (width as f64 / launcher_logo_surface.width() as f64)
                .min(height as f64 / launcher_logo_surface.height() as f64);
            let x = (width as f64 - launcher_logo_surface.width() as f64 * scale) / 2.0;
            let y = (height as f64 - launcher_logo_surface.height() as f64 * scale) / 2.0;
            context.translate(x, y);
            context.scale(scale, scale);
            let _ = context.set_source_surface(&launcher_logo_surface, 0.0, 0.0);
            let _ = context.paint();
        });
        let launcher_button = gtk::Button::new();
        launcher_button.set_child(Some(&launcher_logo));
        launcher_button.add_css_class("overlay-launcher");
        launcher_button.set_tooltip_text(Some("Open Hyprly floating assistant"));
        let launcher = gtk::ApplicationWindow::builder()
            .application(application)
            .title("Hyprly Floating Launcher")
            .default_width(64)
            .default_height(64)
            .child(&launcher_button)
            .build();
        launcher.add_css_class("overlay-launcher-window");
        launcher.set_decorated(false);
        launcher.set_resizable(false);
        launcher.set_size_request(64, 64);
        launcher.set_opacity(0.0);
        let launcher_position = Rc::new(std::cell::Cell::new(
            saved_overlay_size
                .position_x
                .zip(saved_overlay_size.position_y),
        ));
        minimize.connect_clicked({
            let window = window.clone();
            let launcher = launcher.clone();
            let overlay_is_visible = overlay_is_visible.clone();
            let launcher_position = launcher_position.clone();
            move |_| {
                if !overlay_is_visible.replace(false) {
                    return;
                }
                let position = hyprland_window_position("Hyprly Floating Assistant")
                    .ok()
                    .or_else(|| {
                        saved_overlay_size
                            .position_x
                            .zip(saved_overlay_size.position_y)
                    });
                launcher_position.set(position);
                launcher.set_opacity(0.0);
                launcher.present();
                schedule_floating_window_placement_for("Hyprly Floating Launcher", position);
                animate_overlay_transition(&window, &launcher, saved_overlay_size, false);
            }
        });
        launcher_button.connect_clicked({
            let window = window.clone();
            let launcher = launcher.clone();
            let overlay_is_visible = overlay_is_visible.clone();
            let launcher_position = launcher_position.clone();
            move |_| {
                if overlay_is_visible.replace(true) {
                    return;
                }
                animate_overlay_transition(&window, &launcher, saved_overlay_size, true);
                schedule_floating_window_placement(launcher_position.get());
            }
        });
        gtk::glib::timeout_add_local(Duration::from_millis(100), {
            let window = window.clone();
            let launcher = launcher.clone();
            let connection = connection.clone();
            let transcript = transcript.clone();
            let answer = answer.clone();
            let answer_scroll = answer_scroll.clone();
            let streaming_answer = streaming_answer.clone();
            let answer_text = answer_text.clone();
            let copy_code = copy_code.clone();
            let pairing = pairing.clone();
            let overlay_size = overlay_size.clone();
            let overlay_is_visible = overlay_is_visible.clone();
            move || {
                while let Ok(event) = event_rx.try_recv() {
                    match event.get("event").and_then(|value| value.as_str()) {
                        Some("connection") => {
                            let connected = event
                                .get("connected")
                                .and_then(|value| value.as_bool())
                                .unwrap_or(false);
                            let local = event
                                .get("local")
                                .and_then(|value| value.as_bool())
                                .unwrap_or(false);
                            pairing.set_visible(!connected && !local);
                            connection.set_text(if local {
                                "Floating session active · phone pairing is optional"
                            } else if connected {
                                "Phone paired"
                            } else {
                                "Waiting for phone pairing"
                            });
                        }
                        _ => match event.get("type").and_then(|value| value.as_str()) {
                            Some("answer_target") => {
                                let target = event
                                    .get("target")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("webpage");
                                let should_be_visible = matches!(target, "floating" | "both");
                                if should_be_visible {
                                    let was_visible = overlay_is_visible.replace(true);
                                    launcher.set_visible(false);
                                    window.set_visible(true);
                                    window.set_opacity(1.0);
                                    window.set_size_request(
                                        overlay_size.borrow().min_width as i32,
                                        overlay_size.borrow().min_height as i32,
                                    );
                                    if !was_visible {
                                        let size = *overlay_size.borrow();
                                        schedule_floating_window_placement(
                                            size.position_x.zip(size.position_y),
                                        );
                                    }
                                } else {
                                    overlay_is_visible.set(false);
                                    window.set_visible(false);
                                    launcher.set_visible(false);
                                }
                            }
                            Some("overlay_window_size") => {
                                if let Ok(config) = crate::config::Config::load() {
                                    let updated = config.screen.overlay_window;
                                    *overlay_size.borrow_mut() = updated;
                                    answer_scroll.set_max_content_height(updated.max_height as i32);
                                    window.set_size_request(
                                        updated.min_width as i32,
                                        updated.min_height as i32,
                                    );
                                    resize_answer_overlay(
                                        &window,
                                        &answer_scroll,
                                        &answer,
                                        updated,
                                    );
                                    let position = updated.position_x.zip(updated.position_y);
                                    if overlay_is_visible.get() {
                                        schedule_floating_window_placement(position);
                                    }
                                }
                            }
                            Some("status") => {
                                if let Some(message) =
                                    event.get("message").and_then(|value| value.as_str())
                                {
                                    connection.set_text(message);
                                }
                            }
                            Some("transcript") => {
                                let text = event
                                    .get("text")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("");
                                transcript.set_text(text);
                                transcript.set_tooltip_text(Some(text));
                            }
                            Some("answer_start") => {
                                streaming_answer.borrow_mut().clear();
                                answer_text.borrow_mut().clear();
                                answer.set_text("Thinking…");
                                copy_code.set_visible(false);
                                resize_answer_overlay(
                                    &window,
                                    &answer_scroll,
                                    &answer,
                                    *overlay_size.borrow(),
                                );
                            }
                            Some("answer") => {
                                streaming_answer.borrow_mut().clear();
                                let text = event
                                    .get("text")
                                    .and_then(|value| value.as_str())
                                    .unwrap_or("");
                                *answer_text.borrow_mut() = text.to_string();
                                answer.set_markup(&answer_to_pango_markup(text));
                                copy_code.set_visible(!extract_answer_code(text).is_empty());
                                resize_answer_overlay(
                                    &window,
                                    &answer_scroll,
                                    &answer,
                                    *overlay_size.borrow(),
                                );
                            }
                            Some("answer_token") => {
                                if let Some(token) =
                                    event.get("text").and_then(|value| value.as_str())
                                {
                                    let mut accumulated = streaming_answer.borrow_mut();
                                    accumulated.push_str(token);
                                    if let Some(partial) = partial_answer_text(&accumulated) {
                                        *answer_text.borrow_mut() = partial.clone();
                                        answer.set_markup(&answer_to_pango_markup(&partial));
                                        copy_code
                                            .set_visible(!extract_answer_code(&partial).is_empty());
                                        resize_answer_overlay(
                                            &window,
                                            &answer_scroll,
                                            &answer,
                                            *overlay_size.borrow(),
                                        );
                                    } else if accumulated.is_empty() {
                                        answer.set_text("Writing answer…");
                                    }
                                }
                            }
                            Some("topic_reset") => {
                                streaming_answer.borrow_mut().clear();
                                answer_text.borrow_mut().clear();
                                answer.set_text("Waiting for context…");
                                copy_code.set_visible(false);
                                resize_answer_overlay(
                                    &window,
                                    &answer_scroll,
                                    &answer,
                                    *overlay_size.borrow(),
                                );
                            }
                            _ => {}
                        },
                    }
                }
                gtk::glib::ControlFlow::Continue
            }
        });
        if initially_visible {
            window.present();
        }
        if initially_visible {
            gtk::glib::timeout_add_local_once(Duration::from_millis(300), {
                let window = window.clone();
                move || {
                    window.set_default_size(
                        saved_overlay_size.width as i32,
                        saved_overlay_size.height as i32,
                    );
                    schedule_floating_window_placement(
                        saved_overlay_size
                            .position_x
                            .zip(saved_overlay_size.position_y),
                    );
                }
            });
        }
    });
    application.run_with_args(&["hyprly-answer-overlay"]);
}

fn overlay_context_field(
    label: &str,
    placeholder_text: &str,
    rows: i32,
) -> (gtk::Label, (gtk::ScrolledWindow, gtk::TextBuffer)) {
    let heading = gtk::Label::new(Some(label));
    heading.set_xalign(0.0);
    let view = gtk::TextView::new();
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_top_margin(6);
    view.set_bottom_margin(6);
    view.set_left_margin(8);
    view.set_right_margin(8);
    let buffer = view.buffer();
    let placeholder = gtk::Label::new(Some(placeholder_text));
    placeholder.set_halign(gtk::Align::Start);
    placeholder.set_valign(gtk::Align::Start);
    placeholder.set_margin_top(12);
    placeholder.set_margin_start(10);
    placeholder.set_can_target(false);
    placeholder.add_css_class("context-placeholder");
    let input = gtk::Overlay::new();
    input.set_child(Some(&view));
    input.add_overlay(&placeholder);
    buffer.connect_changed({
        let placeholder = placeholder.clone();
        move |buffer| {
            let empty = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), true)
                .is_empty();
            placeholder.set_visible(empty);
        }
    });
    let scroll = gtk::ScrolledWindow::builder()
        .min_content_height(rows * 28)
        .child(&input)
        .build();
    (heading, (scroll, buffer))
}

fn overlay_context_upload(buffer: &gtk::TextBuffer) -> gtk::Box {
    let upload_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    upload_row.add_css_class("context-upload-dropzone");
    let hint = gtk::Label::new(Some("Drop PDF, DOCX, or TXT here"));
    hint.set_xalign(0.0);
    hint.set_hexpand(true);
    let status = gtk::Label::new(Some(""));
    status.set_xalign(1.0);
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    status.add_css_class("context-upload-status");
    let choose = gtk::Button::with_label("Choose file…");
    upload_row.append(&hint);
    upload_row.append(&status);
    upload_row.append(&choose);

    choose.connect_clicked({
        let buffer = buffer.clone();
        let status = status.clone();
        move |_| {
            let chooser = gtk::FileChooserNative::new(
                Some("Add context documents"),
                None::<&gtk::Window>,
                gtk::FileChooserAction::Open,
                Some("Add"),
                Some("Cancel"),
            );
            chooser.set_select_multiple(true);
            let filter = gtk::FileFilter::new();
            filter.set_name(Some("PDF, DOCX, and text files"));
            filter.add_pattern("*.pdf");
            filter.add_pattern("*.docx");
            filter.add_pattern("*.txt");
            chooser.add_filter(&filter);
            chooser.connect_response({
                let buffer = buffer.clone();
                let status = status.clone();
                move |dialog, response| {
                    if response == gtk::ResponseType::Accept {
                        let files = dialog.files();
                        for index in 0..files.n_items() {
                            if let Some(file) = files
                                .item(index)
                                .and_then(|item| item.downcast::<gtk::gio::File>().ok())
                            {
                                overlay_append_context_file(&buffer, &status, &file);
                            }
                        }
                    }
                    dialog.destroy();
                }
            });
            chooser.show();
        }
    });

    let drop_target = gtk::DropTarget::new(
        gtk::gdk::FileList::static_type(),
        gtk::gdk::DragAction::COPY,
    );
    drop_target.connect_enter({
        let upload_row = upload_row.clone();
        move |_, _, _| {
            upload_row.add_css_class("drop-active");
            gtk::gdk::DragAction::COPY
        }
    });
    drop_target.connect_leave({
        let upload_row = upload_row.clone();
        move |_| upload_row.remove_css_class("drop-active")
    });
    drop_target.connect_drop({
        let buffer = buffer.clone();
        let status = status.clone();
        let upload_row = upload_row.clone();
        move |_, value, _, _| {
            upload_row.remove_css_class("drop-active");
            let Ok(files) = value.get::<gtk::gdk::FileList>() else {
                return false;
            };
            for file in files.files() {
                overlay_append_context_file(&buffer, &status, &file);
            }
            true
        }
    });
    upload_row.add_controller(drop_target);
    upload_row
}

fn overlay_append_context_file(
    buffer: &gtk::TextBuffer,
    status: &gtk::Label,
    file: &gtk::gio::File,
) {
    let Some(path) = file.path() else {
        status.set_text("Choose a local file");
        return;
    };
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("document");
    status.set_text(&format!("Reading {name}…"));
    match overlay_extract_context_file(&path) {
        Ok(text) if !text.trim().is_empty() => {
            let existing = buffer.text(&buffer.start_iter(), &buffer.end_iter(), true);
            let joined = [existing.trim(), text.trim()]
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n");
            buffer.set_text(&joined);
            status.set_text(&format!("Loaded text from {name}"));
        }
        Ok(_) => status.set_text(&format!("{name}: no readable text found")),
        Err(error) => status.set_text(&format!("{name}: {error}")),
    }
}

fn overlay_extract_context_file(path: &std::path::Path) -> Result<String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "txt" => std::fs::read_to_string(path).context("could not read text file"),
        "pdf" => {
            let output = Command::new("pdftotext")
                .args(["-layout"])
                .arg(path)
                .arg("-")
                .output()
                .context("PDF upload requires pdftotext (Poppler)")?;
            if !output.status.success() {
                anyhow::bail!("could not extract text from PDF");
            }
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        "docx" => {
            let output = Command::new("unzip")
                .args(["-p"])
                .arg(path)
                .arg("word/document.xml")
                .output()
                .context("DOCX upload requires unzip")?;
            if !output.status.success() {
                anyhow::bail!("could not extract text from DOCX");
            }
            Ok(overlay_docx_xml_text(&String::from_utf8_lossy(
                &output.stdout,
            )))
        }
        _ => anyhow::bail!("choose a PDF, DOCX, or TXT file"),
    }
}

fn overlay_docx_xml_text(xml: &str) -> String {
    let mut text = String::new();
    let mut rest = xml;
    while let Some(tag_start) = rest.find('<') {
        rest = &rest[tag_start..];
        if rest.starts_with("</w:p") {
            if !text.ends_with('\n') {
                text.push('\n');
            }
        } else if rest.starts_with("<w:t") {
            if let Some(tag_end) = rest.find('>') {
                let content = &rest[tag_end + 1..];
                if let Some(text_end) = content.find("</w:t>") {
                    text.push_str(&overlay_decode_xml_entities(&content[..text_end]));
                    rest = &content[text_end + "</w:t>".len()..];
                    continue;
                }
            }
        }
        if let Some(tag_end) = rest.find('>') {
            rest = &rest[tag_end + 1..];
        } else {
            break;
        }
    }
    text.trim().to_string()
}

fn overlay_decode_xml_entities(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

fn overlay_buffer_text(buffer: &gtk::TextBuffer) -> String {
    buffer
        .text(&buffer.start_iter(), &buffer.end_iter(), true)
        .to_string()
}

fn overlay_apply_style(provider: &gtk::CssProvider) {
    let base_css = ".overlay-root { background: rgba(11, 14, 20, 0.94); color: #f1f5f9; padding: 12px; } .overlay-island { background: rgba(18, 21, 30, 0.88); border: 1px solid rgba(255,255,255,0.14); border-radius: 24px; padding: 8px 12px; } .hud-card { background: linear-gradient(to bottom, rgba(45,52,70,0.38), rgba(13,16,23,0.90)); border: 1px solid rgba(255,255,255,0.12); border-radius: 22px; padding: 14px; } .hud-context-strip, .hud-controls { background: rgba(255,255,255,0.04); border: 1px solid rgba(255,255,255,0.08); border-radius: 14px; padding: 8px; } .overlay-title, .overlay-heading { color: #7dd3fc; } .overlay-status { color: #a1a1aa; font-size: 10px; } .hud-inner-panel, .hud-answer-card { background: rgba(255,255,255,0.045); border: 1px solid rgba(255,255,255,0.08); border-radius: 16px; padding: 10px; } .hud-answer-card { border-left: 4px solid #38bdf8; } .overlay-transcript, .overlay-answer { color: #f1f5f9; } .overlay-answer { font-size: 15px; padding: 8px; } .overlay-control, .overlay-pair-code { border-radius: 12px; padding: 6px 12px; }";
    let translucent_surface = ".overlay-root { background: transparent; } .hud-card { background: linear-gradient(to bottom, rgba(82,86,94,0.30), rgba(48,51,57,0.50)); }";
    let css = format!(
        "{base_css} {translucent_surface} window.answer-window, window.overlay-launcher-window {{ background: transparent; }} button.overlay-launcher {{ min-width: 64px; min-height: 64px; padding: 0; border: 1px solid rgba(125,211,252,0.48); border-radius: 999px; background: rgba(18,21,30,0.96); box-shadow: 0 4px 18px rgba(0,0,0,0.36); }} button.overlay-launcher:hover {{ background: rgba(30,37,51,0.98); }} .overlay-root {{ background: transparent; border: none; border-radius: 0; padding: 0; }} .hud-card {{ padding: 2px; border: none; }} .hud-inner-panel {{ padding: 4px 10px; }} .hud-answer-flow {{ background: transparent; border: none; box-shadow: none; padding: 0; }} .overlay-answer {{ background: transparent; padding: 2px 0; font-size: 15px; }} .context-placeholder {{ opacity: 0.62; }} .hud-pairing {{ background: rgba(255,255,255,0.035); border: 1px solid rgba(255,255,255,0.09); border-radius: 12px; padding: 8px; }} .context-upload-dropzone {{ background: rgba(255,255,255,0.035); border: 1px dashed rgba(255,255,255,0.18); border-radius: 10px; padding: 6px; }} .context-upload-dropzone.drop-active {{ background: rgba(56,189,248,0.14); border-color: rgba(56,189,248,0.75); }} .context-upload-status {{ font-size: 10px; opacity: 0.75; }} .hud-prompt-pill {{ background: rgba(37,99,235,0.72); color: #ffffff; border: 1px solid rgba(125,211,252,0.35); border-radius: 999px; padding: 5px 10px; font-size: 11px; font-weight: 600; }} .overlay-title {{ min-width: 26px; }}"
    );
    provider.load_from_data(&css);
}

fn answer_to_pango_markup(text: &str) -> String {
    let mut output = String::new();
    let mut remaining = text;
    while let Some(fence_start) = remaining.find("```") {
        output.push_str(&inline_code_markup(&remaining[..fence_start]));
        let after_fence = &remaining[fence_start + 3..];
        if let Some(fence_end) = after_fence.find("```") {
            let fenced = &after_fence[..fence_end];
            let code = fenced
                .find('\n')
                .map(|newline| &fenced[newline + 1..])
                .unwrap_or(fenced)
                .trim_matches('\n');
            output.push_str(
                "<span font_family=\"monospace\" foreground=\"#e2e8f0\" background=\"#263244\">",
            );
            output.push_str(&escape_pango_text(code));
            output.push_str("</span>");
            remaining = &after_fence[fence_end + 3..];
        } else {
            output.push_str(
                "<span font_family=\"monospace\" foreground=\"#e2e8f0\" background=\"#263244\">",
            );
            let code = after_fence
                .find('\n')
                .map(|newline| &after_fence[newline + 1..])
                .unwrap_or(after_fence)
                .trim_matches('\n');
            output.push_str(&escape_pango_text(code));
            output.push_str("</span>");
            remaining = "";
        }
    }
    output.push_str(&inline_code_markup(remaining));
    output
}

fn extract_answer_code(text: &str) -> String {
    let mut snippets = Vec::new();
    let mut remaining = text;
    while let Some(fence_start) = remaining.find("```") {
        let after_fence = &remaining[fence_start + 3..];
        let (fenced, tail) = if let Some(fence_end) = after_fence.find("```") {
            (&after_fence[..fence_end], &after_fence[fence_end + 3..])
        } else {
            (after_fence, "")
        };
        let code = fenced
            .find('\n')
            .map(|newline| &fenced[newline + 1..])
            .unwrap_or(fenced)
            .trim_matches('\n');
        if !code.trim().is_empty() {
            snippets.push(code.to_string());
        }
        remaining = tail;
    }

    if snippets.is_empty() {
        let mut remaining = text;
        while let Some(code_start) = remaining.find('`') {
            let after_tick = &remaining[code_start + 1..];
            if after_tick.starts_with("``") {
                remaining = &after_tick[2..];
                continue;
            }
            let Some(code_end) = after_tick.find('`') else {
                break;
            };
            let code = &after_tick[..code_end];
            if !code.trim().is_empty() {
                snippets.push(code.to_string());
            }
            remaining = &after_tick[code_end + 1..];
        }
    }
    snippets.join("\n\n")
}

fn resize_answer_overlay(
    window: &gtk::ApplicationWindow,
    answer_scroll: &gtk::ScrolledWindow,
    answer: &gtk::Label,
    size: crate::config::OverlayWindowConfig,
) {
    let width = size.width.clamp(size.min_width, size.max_width) as i32;
    let available_width = (answer_scroll.allocated_width() - 16).max(width - 48);
    let (_, natural_height, _, _) = answer.measure(gtk::Orientation::Vertical, available_width);
    let answer_ceiling = size
        .max_height
        .saturating_sub(size.height)
        .saturating_add(ANSWER_OVERLAY_MIN_ANSWER_HEIGHT as u32) as i32;
    let answer_height = natural_height.saturating_add(8).clamp(
        ANSWER_OVERLAY_MIN_ANSWER_HEIGHT,
        answer_ceiling.max(ANSWER_OVERLAY_MIN_ANSWER_HEIGHT),
    );
    answer_scroll.set_min_content_height(answer_height);
    let window_height = (size.height as i32 + answer_height - ANSWER_OVERLAY_MIN_ANSWER_HEIGHT)
        .clamp(size.min_height as i32, size.max_height as i32);
    window.set_default_size(width, window_height);
}

fn inline_code_markup(text: &str) -> String {
    let mut output = String::new();
    let mut remaining = text;
    while let Some(code_start) = remaining.find('`') {
        output.push_str(&escape_pango_text(&remaining[..code_start]));
        let after_tick = &remaining[code_start + 1..];
        if let Some(code_end) = after_tick.find('`') {
            output.push_str(
                "<span font_family=\"monospace\" foreground=\"#e2e8f0\" background=\"#263244\">",
            );
            output.push_str(&escape_pango_text(&after_tick[..code_end]));
            output.push_str("</span>");
            remaining = &after_tick[code_end + 1..];
        } else {
            output.push_str(&escape_pango_text(remaining));
            return output;
        }
    }
    output.push_str(&escape_pango_text(remaining));
    output
}

fn escape_pango_text(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn partial_answer_text(stream: &str) -> Option<String> {
    let marker = "\"answer\"";
    let start = stream.find(marker)? + marker.len();
    let colon = stream[start..].find(':')? + start + 1;
    let quote = stream[colon..].find('"')? + colon;
    let mut escaped = false;
    let mut end = stream.len();
    for (offset, character) in stream[quote + 1..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '"' => {
                end = quote + 1 + offset;
                break;
            }
            _ => {}
        }
    }
    let encoded = format!("\"{}\"", &stream[quote + 1..end]);
    serde_json::from_str(&encoded).ok()
}

fn overlay_send(event: serde_json::Value) {
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    if serde_json::to_writer(&mut output, &event).is_ok() {
        let _ = output.write_all(b"\n");
        let _ = output.flush();
    }
}

fn overlay_pair_local(code: &gtk::Entry, status: &gtk::Label) {
    overlay_send(serde_json::json!({
        "type":"pair_local",
        "code":code.text().as_str(),
    }));
    status.set_text("Checking pairing code…");
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
    let status = gtk::Label::new(Some(
        "Hyprly is disconnected. Add an API key if needed, then press Connect to start.",
    ));
    status.set_xalign(0.0);
    status.set_wrap(true);

    install_theme(application);
    let root = gtk::Box::new(gtk::Orientation::Vertical, 18);
    root.add_css_class("app-root");
    root.set_margin_top(18);
    root.set_margin_bottom(18);
    root.set_margin_start(24);
    root.set_margin_end(24);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    header.add_css_class("app-header");
    let brand_text = gtk::Box::new(gtk::Orientation::Horizontal, 18);
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
    let divider = gtk::Separator::new(gtk::Orientation::Vertical);
    divider.set_margin_top(4);
    divider.set_margin_bottom(4);
    brand_text.append(&divider);
    brand_text.append(&subtitle);
    header.append(&brand_text);
    let header_actions = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    header_actions.set_halign(gtk::Align::End);
    let settings_button = gtk::Button::new();
    settings_button.set_child(Some(&asset_icon("settings", 20)));
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
    columns.set_hexpand(true);

    let setup_card = gtk::Box::new(gtk::Orientation::Vertical, 12);
    setup_card.add_css_class("card");
    setup_card.set_size_request(320, -1);
    setup_card.append(&section_heading_icon("Groq API key", "key-round"));
    let key_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    key_row.append(&api_key);
    let help = gtk::LinkButton::with_label(GROQ_KEYS_URL, "?");
    help.set_tooltip_text(Some("Get an API key from Groq"));
    key_row.append(&help);
    let show_key = gtk::Button::with_label("Show");
    key_row.append(&show_key);
    setup_card.append(&key_row);
    let mut backup_keys = Vec::new();
    for index in 2..=crate::config::MAX_API_KEYS {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let entry = gtk::Entry::builder()
            .placeholder_text(format!("Backup key {index} (optional)"))
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
    set_button_icon(&save_key, "save");
    let test_key = gtk::Button::with_label("Test Groq API");
    set_button_icon(&test_key, "play");
    key_actions.append(&save_key);
    key_actions.append(&test_key);
    setup_card.append(&key_actions);
    let key_note = gtk::Label::new(Some("Stored locally in your private Hyprly configuration."));
    key_note.add_css_class("muted");
    key_note.set_xalign(0.0);
    setup_card.append(&key_note);

    setup_card.append(&section_heading_icon("System checks", "activity"));
    let checks = gtk::Box::new(gtk::Orientation::Vertical, 8);
    checks.set_homogeneous(true);
    let test_audio = gtk::Button::with_label("Test PipeWire audio");
    set_button_icon(&test_audio, "mic");
    let test_uploads = gtk::Button::with_label("Check PDF / DOCX tools");
    set_button_icon(&test_uploads, "file-text");
    checks.append(&test_audio);
    checks.append(&test_uploads);
    setup_card.append(&checks);
    columns.attach(&setup_card, 0, 0, 2, 1);

    let pairing_card = gtk::Box::new(gtk::Orientation::Vertical, 12);
    pairing_card.add_css_class("card");
    pairing_card.set_hexpand(true);
    pairing_card.append(&section_heading_icon("Phone pairing", "smartphone"));
    let pairing = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    pairing.add_css_class("pairing-layout");
    let pairing_text = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let code_caption = gtk::Label::new(Some("Pairing code"));
    code_caption.add_css_class("field-heading");
    code_caption.set_xalign(0.0);
    let code = gtk::Label::new(Some("Not connected"));
    code.add_css_class("title-2");
    code.add_css_class("pairing-code");
    code.set_xalign(0.0);
    code.set_selectable(true);
    let code_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let copy_code = gtk::Button::with_label("Copy code");
    set_button_icon(&copy_code, "copy");
    copy_code.set_tooltip_text(Some("Copy pairing code to clipboard"));
    code_row.append(&code);
    code_row.append(&copy_code);
    let setup_url = gtk::LinkButton::with_label("https://example.com", "Waiting for Hyprly…");
    setup_url.set_halign(gtk::Align::Start);
    setup_url.set_hexpand(true);
    setup_url.set_sensitive(false);
    setup_url.add_css_class("pairing-link");
    let setup_caption = gtk::Label::new(Some("Setup URL (for mobile browser)"));
    setup_caption.add_css_class("field-heading");
    setup_caption.set_xalign(0.0);
    let setup_copy = gtk::Button::with_label("Copy");
    set_button_icon(&setup_copy, "copy");
    let setup_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    setup_row.add_css_class("url-row");
    let setup_link_icon = asset_icon("link", 17);
    setup_row.append(&setup_link_icon);
    setup_row.append(&setup_url);
    setup_row.append(&setup_copy);
    let secure_url = gtk::LinkButton::with_label("https://example.com", "Waiting for Hyprly…");
    secure_url.set_halign(gtk::Align::Start);
    secure_url.set_hexpand(true);
    secure_url.set_sensitive(false);
    secure_url.add_css_class("pairing-link");
    let secure_caption = gtk::Label::new(Some("Secure URL (for mobile browser)"));
    secure_caption.add_css_class("field-heading");
    secure_caption.set_xalign(0.0);
    let secure_copy = gtk::Button::with_label("Copy");
    set_button_icon(&secure_copy, "copy");
    let secure_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    secure_row.add_css_class("url-row");
    let secure_link_icon = asset_icon("link", 17);
    secure_row.append(&secure_link_icon);
    secure_row.append(&secure_url);
    secure_row.append(&secure_copy);
    let fingerprint = gtk::Label::new(Some(""));
    fingerprint.set_xalign(0.0);
    fingerprint.set_wrap(true);
    fingerprint.set_selectable(true);
    fingerprint.add_css_class("fingerprint");
    let fingerprint_caption = gtk::Label::new(Some("Certificate fingerprint"));
    fingerprint_caption.add_css_class("field-heading");
    fingerprint_caption.set_xalign(0.0);
    pairing_text.append(&code_caption);
    pairing_text.append(&code_row);
    pairing_text.append(&setup_caption);
    pairing_text.append(&setup_row);
    pairing_text.append(&secure_caption);
    pairing_text.append(&secure_row);
    pairing_text.append(&fingerprint_caption);
    pairing_text.append(&fingerprint);
    pairing.append(&pairing_text);

    let qr_code = Rc::new(std::cell::RefCell::new(None::<QrCode>));
    let qr_draw = qr_code.clone();
    let qr_area = gtk::DrawingArea::builder()
        .content_width(190)
        .content_height(190)
        .build();
    qr_area.add_css_class("qr-preview");
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
    columns.attach(&pairing_card, 2, 0, 10, 1);
    root.append(&columns);

    let connect = gtk::Button::with_label("Connect");
    connect.add_css_class("suggested-action");
    let disconnect = gtk::Button::with_label("Disconnect");
    disconnect.set_sensitive(false);
    let answer_target = gtk::Box::new(gtk::Orientation::Horizontal, 7);
    answer_target.add_css_class("answer-target-selector");
    let webpage_target = gtk::ToggleButton::with_label("Webpage");
    webpage_target.add_css_class("target-first");
    let floating_target = gtk::ToggleButton::with_label("Floating window");
    floating_target.add_css_class("target-middle");
    floating_target.set_group(Some(&webpage_target));
    let both_target = gtk::ToggleButton::with_label("Both");
    both_target.add_css_class("target-last");
    both_target.set_group(Some(&webpage_target));
    let saved_answer_target = crate::config::Config::load()
        .map(|config| config.screen.answer_target)
        .unwrap_or_else(|_| "webpage".to_string());
    match saved_answer_target.as_str() {
        "floating" => floating_target.set_active(true),
        "both" => both_target.set_active(true),
        _ => webpage_target.set_active(true),
    }
    answer_target.set_tooltip_text(Some("Choose where Hyprly displays answers."));
    answer_target.append(&webpage_target);
    answer_target.append(&floating_target);
    answer_target.append(&both_target);
    let output_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    output_row.add_css_class("connection-toolbar");
    output_row.set_hexpand(true);
    let answer_target_label = gtk::Label::new(Some("Answer window"));
    answer_target_label.add_css_class("answer-target-label");
    output_row.append(&answer_target_label);
    output_row.append(&answer_target);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    output_row.append(&spacer);
    output_row.append(&connect);
    output_row.append(&disconnect);
    root.append(&output_row);
    status.add_css_class("status-line");
    root.append(&status);

    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("Hyprly")
        .default_width(1180)
        .default_height(760)
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
        overlay_child: None,
        paired: false,
        screen_pid: None,
        backend_stdin: None,
        overlay_stdin: None,
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
    for (button, link, label) in [
        (setup_copy, setup_url, "Setup URL"),
        (secure_copy, secure_url, "Secure URL"),
    ] {
        button.connect_clicked({
            let status = status.clone();
            move |_| {
                if let Some(display) = gtk::gdk::Display::default() {
                    display.clipboard().set_text(&link.uri());
                    status.set_text(&format!("{label} copied to clipboard."));
                }
            }
        });
    }

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
        let status = status.clone();
        move |picker| {
            let theme = theme_names
                .get(picker.selected() as usize)
                .copied()
                .unwrap_or("midnight");
            apply_gui_theme(&provider, theme);
            *wordmark_surface.borrow_mut() = create_wordmark_surface(theme);
            wordmark.queue_draw();
            if let Err(error) = save_gui_theme(theme) {
                status.set_text(&format!("Could not save theme preference: {error}"));
            }
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
    connect_answer_target(&webpage_target, "webpage", &state);
    connect_answer_target(&floating_target, "floating", &state);
    connect_answer_target(&both_target, "both", &state);
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
    }
}

fn hyprland_clients() -> Result<Vec<serde_json::Value>> {
    let output = Command::new("hyprctl")
        .args(["-j", "clients"])
        .output()
        .context("could not run `hyprctl -j clients`")?;
    if !output.status.success() {
        anyhow::bail!(
            "`hyprctl -j clients` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout)
        .context("could not parse the Hyprland client list")
}

fn hyprland_window_info(title: &str) -> Result<serde_json::Value> {
    let clients = hyprland_clients()?;
    clients
        .iter()
        .find(|window| window.get("title").and_then(|value| value.as_str()) == Some(title))
        .or_else(|| {
            clients.iter().find(|window| {
                window.get("initialTitle").and_then(|value| value.as_str()) == Some(title)
            })
        })
        .or_else(|| {
            clients.iter().find(|window| {
                ["title", "initialTitle"].iter().any(|field| {
                    window
                        .get(*field)
                        .and_then(|value| value.as_str())
                        .is_some_and(|window_title| window_title.contains(title))
                })
            })
        })
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Hyprland has no window matching `{title}`"))
}

fn hyprland_window_position(title: &str) -> Result<(i32, i32)> {
    let window = hyprland_window_info(title)?;
    hyprland_position_from_info(&window, title)
}

fn hyprland_position_from_info(
    window: &serde_json::Value,
    description: &str,
) -> Result<(i32, i32)> {
    let at = window
        .get("at")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            anyhow::anyhow!("Hyprland did not report a position for `{description}`")
        })?;
    let coordinate = |value: &serde_json::Value| {
        value
            .as_i64()
            .map(|value| value as i32)
            .or_else(|| value.as_f64().map(|value| value.round() as i32))
    };
    let x = at
        .first()
        .and_then(coordinate)
        .ok_or_else(|| {
            anyhow::anyhow!("Hyprland returned an invalid x position for `{description}`")
        })?;
    let y = at
        .get(1)
        .and_then(coordinate)
        .ok_or_else(|| {
            anyhow::anyhow!("Hyprland returned an invalid y position for `{description}`")
        })?;
    Ok((x, y))
}

fn animate_overlay_transition(
    window: &gtk::ApplicationWindow,
    launcher: &gtk::ApplicationWindow,
    size: crate::config::OverlayWindowConfig,
    showing: bool,
) {
    const ICON_SIZE: i32 = 64;
    const DURATION: Duration = Duration::from_millis(240);

    let (start_width, start_height, end_width, end_height) = if showing {
        window.set_size_request(ICON_SIZE, ICON_SIZE);
        window.set_default_size(ICON_SIZE, ICON_SIZE);
        window.set_opacity(0.0);
        window.present();
        launcher.set_opacity(1.0);
        (
            ICON_SIZE,
            ICON_SIZE,
            size.width as i32,
            size.height as i32,
        )
    } else {
        (
            size.width as i32,
            size.height as i32,
            ICON_SIZE,
            ICON_SIZE,
        )
    };
    let started = std::time::Instant::now();
    let window = window.clone();
    let launcher = launcher.clone();
    gtk::glib::timeout_add_local(Duration::from_millis(16), move || {
        let progress = (started.elapsed().as_secs_f64() / DURATION.as_secs_f64()).min(1.0);
        let eased = progress * progress * (3.0 - 2.0 * progress);
        let width = (start_width as f64 + (end_width - start_width) as f64 * eased).round() as i32;
        let height =
            (start_height as f64 + (end_height - start_height) as f64 * eased).round() as i32;
        window.set_size_request(width, height);
        window.set_default_size(width, height);
        window.set_opacity(if showing { progress } else { 1.0 - progress });
        launcher.set_opacity(if showing { 1.0 - progress } else { progress });
        if progress >= 1.0 {
            if showing {
                launcher.set_visible(false);
                window.set_size_request(size.min_width as i32, size.min_height as i32);
                window.set_default_size(size.width as i32, size.height as i32);
                window.set_opacity(1.0);
            } else {
                window.set_visible(false);
                window.set_size_request(ICON_SIZE, ICON_SIZE);
                window.set_default_size(ICON_SIZE, ICON_SIZE);
                window.set_opacity(1.0);
                launcher.set_opacity(1.0);
            }
            gtk::glib::ControlFlow::Break
        } else {
            gtk::glib::ControlFlow::Continue
        }
    });
}

fn schedule_floating_window_placement(position: Option<(i32, i32)>) {
    schedule_floating_window_placement_for("Hyprly Floating Assistant", position);
}

fn schedule_floating_window_placement_for(title: &'static str, position: Option<(i32, i32)>) {
    gtk::glib::timeout_add_local_once(Duration::from_millis(300), move || {
        if let Err(error) = place_floating_window_for(title, position) {
            eprintln!("Hyprly: could not apply floating answer window position: {error}");
        }
    });
}

fn place_floating_window_for(title: &str, position: Option<(i32, i32)>) -> Result<()> {
    let window = hyprland_window_info(title)?;
    let address = window
        .get("address")
        .and_then(|value| value.as_str())
        .ok_or_else(|| anyhow::anyhow!("Hyprland did not report the answer window address"))?;
    let selector = format!("address:{address}");
    let uses_lua_dispatch = hyprland_uses_lua_dispatch()?;

    // Pixel movement and centering only work for floating windows. Set this
    // explicitly so positioning does not depend on a user window rule.
    if uses_lua_dispatch {
        hyprland_lua_dispatch(&format!(
            "hl.dsp.window.float({{ action = \"set\", window = \"{selector}\" }})"
        ))?;
    } else {
        hyprland_dispatch("setfloating", Some(&selector))?;
    }

    // Both the assistant and its launcher share this verified Hyprland class.
    // Apply the border override by class so the round launcher stays borderless.
    if uses_lua_dispatch {
        hyprland_lua_dispatch(&format!(
            "hl.dsp.window.set_prop({{ prop = \"border_size\", value = \"0\", window = \"class:io.github.patermars.HyprlyScreenTest\" }})"
        ))?;
    } else {
        hyprland_setprop(
            "class:io.github.patermars.HyprlyScreenTest",
            "bordersize",
            "0",
        )?;
    }

    if let Some(expected) = position {
        if uses_lua_dispatch {
            hyprland_lua_dispatch(&format!(
                "hl.dsp.window.move({{ window = \"{selector}\", x = {}, y = {}, relative = false }})",
                expected.0, expected.1
            ))?;
        } else {
            hyprland_dispatch(
                "movewindowpixel",
                Some(&format!("exact {} {},{selector}", expected.0, expected.1)),
            )?;
        }
        let actual = hyprland_window_position(title)?;
        if actual != expected {
            anyhow::bail!(
                "Hyprland reports the answer window at ({}, {}), expected ({}, {})",
                actual.0,
                actual.1,
                expected.0,
                expected.1
            );
        }
    } else {
        if uses_lua_dispatch {
            hyprland_lua_dispatch(&format!(
                "hl.dsp.window.center({{ window = \"{selector}\" }})"
            ))?;
        } else {
            // The legacy centerwindow dispatcher acts on the active window.
            // Focus this exact window first to avoid centering another app.
            hyprland_dispatch("focuswindow", Some(&selector))?;
            hyprland_dispatch("centerwindow", None)?;
        }
    }
    Ok(())
}

fn hyprland_uses_lua_dispatch() -> Result<bool> {
    let output = Command::new("hyprctl")
        .arg("version")
        .output()
        .context("could not read the Hyprland version")?;
    if !output.status.success() {
        anyhow::bail!(
            "`hyprctl version` failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let version_text = String::from_utf8_lossy(&output.stdout);
    let (major, minor) = version_text
        .split_whitespace()
        .find_map(parse_hyprland_major_minor)
        .ok_or_else(|| anyhow::anyhow!("could not parse the Hyprland version: {version_text}"))?;
    Ok(major > 0 || minor >= 55)
}

fn parse_hyprland_major_minor(token: &str) -> Option<(u64, u64)> {
    let start = token.find(|character: char| character.is_ascii_digit())?;
    let version = &token[start..];
    let mut parts = version.split(|character: char| !character.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    Some((major, minor))
}

fn hyprland_lua_dispatch(expression: &str) -> Result<()> {
    let output = Command::new("hyprctl")
        // `hyprctl dispatch` wraps this expression in `hl.dispatch(...)`.
        .args(["dispatch", expression])
        .output()
        .context("could not run a Lua dispatcher through `hyprctl`")?;
    if !output.status.success() {
        anyhow::bail!(
            "Hyprland rejected Lua dispatcher `{expression}`: {}{}",
            String::from_utf8_lossy(&output.stderr).trim(),
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
    Ok(())
}

fn hyprland_dispatch(dispatcher: &str, argument: Option<&str>) -> Result<()> {
    let mut command = Command::new("hyprctl");
    command.args(["dispatch", dispatcher]);
    if let Some(argument) = argument {
        command.arg(argument);
    }
    let output = command
        .output()
        .with_context(|| format!("could not run `hyprctl dispatch {dispatcher}`"))?;
    if !output.status.success() {
        anyhow::bail!(
            "Hyprland rejected `{dispatcher}`: {}{}",
            String::from_utf8_lossy(&output.stderr).trim(),
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
    Ok(())
}

fn hyprland_setprop(window: &str, property: &str, value: &str) -> Result<()> {
    let output = Command::new("hyprctl")
        .args(["setprop", window, property, value])
        .output()
        .context("could not run a window property update through `hyprctl`")?;
    if !output.status.success() {
        anyhow::bail!(
            "Hyprland rejected window property `{property}`: {}{}",
            String::from_utf8_lossy(&output.stderr).trim(),
            String::from_utf8_lossy(&output.stdout).trim()
        );
    }
    Ok(())
}

fn create_wordmark_surface(theme: &str) -> gtk::cairo::ImageSurface {
    let image = if theme == "paper" {
        include_bytes!("../assets/logo_text.png").as_slice()
    } else {
        include_bytes!("../assets/logo_text_white.png").as_slice()
    };
    let (source_width, source_height, source) = decode_png(image).expect("wordmark PNG is valid");
    let mut argb = Vec::with_capacity(source.len());
    for rgba in source.as_chunks::<4>().0 {
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

fn create_logo_surface() -> gtk::cairo::ImageSurface {
    let (width, height, source) =
        decode_png(include_bytes!("../assets/logo.png")).expect("Hyprly logo PNG is valid");
    let mut argb = Vec::with_capacity(source.len());
    for rgba in source.as_chunks::<4>().0 {
        let alpha = rgba[3] as u16;
        let red = rgba[0] as u16 * alpha / 255;
        let green = rgba[1] as u16 * alpha / 255;
        let blue = rgba[2] as u16 * alpha / 255;
        argb.extend_from_slice(
            &u32::from_be_bytes([alpha as u8, red as u8, green as u8, blue as u8]).to_ne_bytes(),
        );
    }
    gtk::cairo::ImageSurface::create_for_data(
        argb,
        gtk::cairo::Format::ARgb32,
        width as i32,
        height as i32,
        width as i32 * 4,
    )
    .expect("logo image surface can be created")
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

    let floating_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    floating_page.set_hexpand(true);
    floating_page.set_margin_top(20);
    floating_page.set_margin_bottom(20);
    floating_page.set_margin_start(20);
    floating_page.set_margin_end(20);
    floating_page.append(&section_heading("Floating answer window"));
    let floating_intro = gtk::Label::new(Some(
        "Open the sample, move it and drag its edges with the mouse, then click Save settings. The real answer window uses the saved size and position. You can also choose to center it when it opens. The answer window can grow up to twice the saved height.",
    ));
    floating_intro.set_wrap(true);
    floating_intro.set_xalign(0.0);
    floating_page.append(&floating_intro);

    let saved_size = config.screen.overlay_window;
    let preview_window = Rc::new(std::cell::RefCell::new(None::<gtk::Window>));
    let center_on_launch = gtk::CheckButton::with_label("Center answer window when it opens");
    // Reload config to ensure we have the latest position values
    let current_position = crate::config::Config::load()
        .ok()
        .and_then(|c| c.screen.overlay_window.position_x.zip(c.screen.overlay_window.position_y))
        .or_else(|| saved_size.position_x.zip(saved_size.position_y));
    center_on_launch.set_active(current_position.is_none());
    center_on_launch.set_tooltip_text(Some(
        "Turn this off to use the position saved from the draggable sample window.",
    ));
    floating_page.append(&center_on_launch);
    let preview_button = gtk::Button::with_label("Open draggable sample window");
    preview_button.set_halign(gtk::Align::Start);
    floating_page.append(&preview_button);
    tabs.append_page(&floating_page, Some(&gtk::Label::new(Some("Floating"))));

    preview_button.connect_clicked({
        let dialog = dialog.clone();
        let preview_window = preview_window.clone();
        let center_on_launch = center_on_launch.clone();
        move |_| {
            center_on_launch.set_active(false);
            if let Some(preview) = preview_window.borrow().as_ref() {
                preview.present();
                return;
            }
            let preview = gtk::Window::builder()
                .title(FLOATING_PREVIEW_TITLE)
                .transient_for(&dialog)
                .default_width(saved_size.width as i32)
                .default_height(saved_size.height as i32)
                .build();
            preview.add_css_class("answer-window");
            preview.set_decorated(true);
            preview.set_resizable(true);
            let preview_theme = gtk::CssProvider::new();
            overlay_apply_style(&preview_theme);
            if let Some(display) = gtk::gdk::Display::default() {
                gtk::style_context_add_provider_for_display(
                    &display,
                    &preview_theme,
                    gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
                );
            }

            let preview_root = gtk::Box::new(gtk::Orientation::Vertical, 12);
            preview_root.set_margin_top(16);
            preview_root.set_margin_start(18);
            preview_root.set_margin_end(18);
            preview_root.add_css_class("overlay-root");

            let header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            header.add_css_class("overlay-island");
            let header_title = gtk::Label::new(Some("HYPRLY · SESSION PREVIEW"));
            header_title.set_hexpand(true);
            header_title.set_xalign(0.0);
            header_title.add_css_class("overlay-heading");
            let header_status = gtk::Label::new(Some("Floating answer sample"));
            header_status.add_css_class("overlay-status");
            header.append(&header_title);
            header.append(&header_status);
            preview_root.append(&header);

            let sample = gtk::Box::new(gtk::Orientation::Vertical, 10);
            sample.set_vexpand(true);
            sample.set_margin_top(12);
            sample.add_css_class("hud-card");
            let context_strip = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            context_strip.add_css_class("hud-context-strip");
            let context_title = gtk::Label::new(Some("SAMPLE ANSWER"));
            context_title.add_css_class("overlay-heading");
            let context_status = gtk::Label::new(Some("Preview content"));
            context_status.set_hexpand(true);
            context_status.set_halign(gtk::Align::End);
            context_status.add_css_class("overlay-status");
            context_strip.append(&context_title);
            context_strip.append(&context_status);
            sample.append(&context_strip);

            let sample_answer = gtk::Label::new(None);
            sample_answer.set_xalign(0.0);
            sample_answer.set_yalign(0.0);
            sample_answer.set_wrap(true);
            sample_answer.set_selectable(true);
            sample_answer.add_css_class("overlay-answer");
            sample_answer.set_markup(&answer_to_pango_markup(
                "A concise explanation can stay compact. Longer answers use more vertical space.\n\n```rust\nfn greet(name: &str) -> String {\n    format!(\"Hello, {name}!\")\n}\n\nfn main() {\n    println!(\"{}\", greet(\"Hyprly\"));\n}\n```",
            ));
            let answer_scroll = gtk::ScrolledWindow::builder()
                .min_content_height(ANSWER_OVERLAY_MIN_ANSWER_HEIGHT)
                .max_content_height(saved_size.max_height as i32)
                .vexpand(true)
                .child(&sample_answer)
                .build();
            answer_scroll.add_css_class("hud-answer-flow");
            sample.append(&answer_scroll);

            let transcript = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            transcript.add_css_class("hud-inner-panel");
            let transcript_title = gtk::Label::new(Some("LIVE TRANSCRIPT"));
            transcript_title.add_css_class("overlay-heading");
            let transcript_text = gtk::Label::new(Some("Resize this sample with the mouse."));
            transcript_text.set_hexpand(true);
            transcript_text.set_xalign(0.0);
            transcript_text.set_ellipsize(gtk::pango::EllipsizeMode::End);
            transcript.append(&transcript_title);
            transcript.append(&transcript_text);
            sample.append(&transcript);

            let sample_scroll = gtk::ScrolledWindow::builder()
                .min_content_height(1)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&sample)
                .build();
            sample_scroll.set_propagate_natural_height(false);
            preview_root.append(&sample_scroll);
            preview.set_child(Some(&preview_root));
            preview.connect_close_request({
                let preview_window = preview_window.clone();
                move |window| {
                    preview_window.borrow_mut().take();
                    let _ = window;
                    gtk::glib::Propagation::Proceed
                }
            });
            *preview_window.borrow_mut() = Some(preview.clone());
            preview.present();
        }
    });
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
        let preview_window = preview_window.clone();
        let center_on_launch = center_on_launch.clone();
        move |_, response| {
            if response == gtk::ResponseType::Accept {
                let mut updated = config.clone();
                updated.screen.mouse_gesture = state.borrow().screen_trigger.clone();
                updated.screen.answer_target = crate::config::Config::load()
                    .map(|current| current.screen.answer_target)
                    .unwrap_or_else(|_| updated.screen.answer_target.clone());
                let measured = preview_window
                    .borrow()
                    .as_ref()
                    .map(|preview| (preview.width(), preview.height()))
                    .filter(|(width, height)| *width > 0 && *height > 0)
                    .unwrap_or((
                        config.screen.overlay_window.width as i32,
                        config.screen.overlay_window.height as i32,
                    ));
                let saved_width = measured.0.clamp(120, 3000) as u32;
                let saved_height = measured.1.clamp(100, 3000) as u32;
                let preview_is_open = preview_window.borrow().is_some();
                let saved_position = if center_on_launch.is_active() {
                    None
                } else if preview_is_open {
                    match hyprland_window_position(FLOATING_PREVIEW_TITLE) {
                        Ok(position) => Some(position),
                        Err(error) => {
                            status.set_text(&format!(
                                "Could not save the preview position: {error}"
                            ));
                            return;
                        }
                    }
                } else {
                    // Reload config to get the current position values rather than
                    // using the snapshot from when the dialog opened
                    crate::config::Config::load()
                        .ok()
                        .and_then(|current| {
                            current
                                .screen
                                .overlay_window
                                .position_x
                                .zip(current.screen.overlay_window.position_y)
                        })
                        .or_else(|| {
                            config
                                .screen
                                .overlay_window
                                .position_x
                                .zip(config.screen.overlay_window.position_y)
                        })
                };
                let overlay_window = crate::config::OverlayWindowConfig {
                    min_width: saved_width,
                    width: saved_width,
                    max_width: saved_width,
                    min_height: config.screen.overlay_window.min_height.min(saved_height),
                    height: saved_height,
                    max_height: saved_height.saturating_mul(2).max(saved_height),
                    position_x: saved_position.map(|(x, _)| x),
                    position_y: saved_position.map(|(_, y)| y),
                };
                updated.screen.overlay_window = overlay_window;
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
                let position_message = saved_position
                    .map(|(x, y)| format!(" Position saved at ({x}, {y})."))
                    .unwrap_or_else(|| " The window will be centered when it opens.".to_string());
                let result = toml::to_string_pretty(&updated)
                    .map_err(anyhow::Error::from)
                    .and_then(|serialized| crate::config::Config::save_config_text(&serialized));
                match result {
                    Ok(()) => {
                        let overlay_stdin = state.borrow().overlay_stdin.clone();
                        if let Some(overlay_stdin) = overlay_stdin {
                            let _ = send_backend_event(
                                &overlay_stdin,
                                serde_json::json!({"type":"overlay_window_size"}),
                            );
                            status.set_text(&format!(
                                "Saved floating window size: {saved_width} × {saved_height} logical px.{position_message} Applied immediately."
                            ));
                        } else {
                            status.set_text(&format!(
                                "Saved floating window size: {saved_width} × {saved_height} logical px.{position_message} It applies when the window next opens."
                            ));
                        }
                    }
                    Err(error) => {
                        status.set_text(&format!("Could not save settings: {error}"));
                        return;
                    }
                }
            }
            let preview_to_close = { preview_window.borrow_mut().take() };
            if let Some(preview) = preview_to_close {
                preview.close();
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
    let Ok(output) = Command::new(crate::pipewire::WPCTL)
        .args(crate::pipewire::STATUS_ARGS)
        .output()
    else {
        return choices;
    };
    if !output.status.success() {
        return choices;
    }
    choices.extend(
        crate::pipewire::parse_sinks(&String::from_utf8_lossy(&output.stdout))
            .into_iter()
            .map(|sink| (format!("{} · {}", sink.name, sink.id), sink.id)),
    );
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

fn section_heading_icon(text: &str, icon_name: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("icon-heading");
    let icon = asset_icon(icon_name, 20);
    row.append(&icon);
    let label = section_heading(text);
    row.append(&label);
    row
}

fn set_button_icon(button: &gtk::Button, icon_name: &str) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let icon = asset_icon(icon_name, 17);
    row.append(&icon);
    row.append(&gtk::Label::new(Some(&button.label().unwrap_or_default())));
    button.set_child(Some(&row));
}

fn asset_icon(name: &str, pixel_size: i32) -> gtk::Image {
    let filename = format!("hyprly-{name}.svg");
    let source_tree = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("assets/icons")
        .join(&filename);
    let installed = std::path::Path::new("/usr/share/hyprly/icons").join(filename);
    let path = if source_tree.exists() {
        source_tree
    } else {
        installed
    };
    let image = gtk::Image::from_file(path);
    image.set_pixel_size(pixel_size);
    image
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
      .app-header {{ min-height: 52px; padding: 0 0 11px; border-bottom: 1px solid {border}; }}
      .app-header separator {{ background: {border}; min-width: 1px; }}
      .app-header label.dim-label {{ font-size: 14px; }}
      .wordmark {{ background: transparent; }}
      .card {{ background: {surface}; border: 1px solid {border}; border-radius: 16px; padding: 20px; }}
      .card .icon-heading {{ padding-bottom: 9px; border-bottom: 1px solid {border}; }}
      .card .icon-heading label {{ color: {text}; font-size: 15px; font-weight: 700; }}
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
      button.suggested-action {{ color: {connect_ink}; background: {connect_bg}; border-color: {connect_border}; font-weight: 700; }}
      .theme-picker {{ min-width: 125px; }}
      .muted, label.dim-label {{ color: {secondary}; }}
      .muted {{ color: {muted}; }}
      .pairing-code {{ color: {accent}; font-family: monospace; font-size: 30px; font-weight: 800; letter-spacing: 5px; padding: 10px 12px; background: {input}; border: 1px solid {border}; border-radius: 12px; }}
      .field-heading {{ color: {secondary}; font-size: 12px; font-weight: 700; }}
      .url-row {{ background: {input}; border: 1px solid {border}; border-radius: 10px; padding: 2px 4px 2px 10px; }}
      .url-row button {{ min-height: 34px; padding: 7px 10px; }}
      .pairing-link {{ color: {text}; font-size: 12px; text-decoration: none; }}
      .fingerprint {{ color: {muted}; background: {input}; border-radius: 9px; padding: 10px 12px; font-family: monospace; font-size: 10px; }}
      .qr-preview {{ background: #ffffff; border: 1px solid {border}; border-radius: 12px; padding: 10px; }}
      .connection-toolbar {{ background: {surface}; border: 1px solid {border}; border-radius: 13px; padding: 9px 12px; }}
      .connection-toolbar > label {{ color: {secondary}; font-weight: 700; }}
      .answer-target-label {{ color: {secondary}; font-weight: 700; }}
      .answer-target-selector {{ background: {input}; border: 1px solid {border}; border-radius: 10px; padding: 3px; }}
      .answer-target-selector button.toggle {{ color: {text}; background: transparent; border: 1px solid transparent; border-radius: 8px; padding: 9px 14px; }}
      .answer-target-selector button.toggle:hover {{ background: {raised}; }}
      .answer-target-selector button.toggle:checked {{ color: {dark_ink}; background: {accent}; border: 1px solid {accent}; box-shadow: none; font-weight: 750; }}
      .status-line {{ color: {secondary}; padding: 10px 12px; background: {raised}; border: 1px solid {border}; border-radius: 10px; }}
      .card label {{ color: {text}; }}
      .connection-actions {{ padding-top: 4px; }}
      .card button {{ min-height: 38px; }}
    "#,
        dark_ink = if dark { "#102016" } else { "#ffffff" },
        connect_bg = if theme == "paper" { accent } else { raised },
        connect_ink = if theme == "paper" {
            if dark { "#102016" } else { "#ffffff" }
        } else {
            text
        },
        connect_border = if theme == "paper" { accent } else { border }
    );
    provider.load_from_data(&css);
}

fn gui_theme_path() -> std::path::PathBuf {
    crate::config::Config::config_path().with_file_name("gui-theme")
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

fn save_gui_theme(theme: &str) -> std::io::Result<()> {
    let path = gui_theme_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, theme)
}

fn entered_api_keys(state: &AppState) -> Vec<String> {
    std::iter::once(state.api_key.text().to_string())
        .chain(
            state
                .backup_keys
                .iter()
                .map(|entry| entry.text().to_string()),
        )
        .collect()
}

fn save_current_key(state: &Rc<std::cell::RefCell<AppState>>) {
    let state_ref = state.borrow();
    let keys = entered_api_keys(&state_ref);
    match crate::config::Config::save_api_keys(&keys) {
        Ok(()) => state_ref
            .status
            .set_text("API key saved in your private Hyprly config."),
        Err(error) => state_ref
            .status
            .set_text(&format!("Could not save API key: {error}")),
    }
}

fn connect_answer_target(
    button: &gtk::ToggleButton,
    target: &'static str,
    state: &Rc<std::cell::RefCell<AppState>>,
) {
    button.connect_toggled({
        let state = state.clone();
        move |button| {
            if !button.is_active() {
                return;
            }
            let (status, backend_stdin, overlay_stdin, running) = {
                let state = state.borrow();
                (
                    state.status.clone(),
                    state.backend_stdin.clone(),
                    state.overlay_stdin.clone(),
                    state.child.is_some(),
                )
            };
            match crate::config::Config::save_answer_target(target) {
                Ok(()) if running => {
                    let backend_updated = backend_stdin.is_some_and(|stdin| {
                        send_backend_event(
                            &stdin,
                            serde_json::json!({"type":"answer_target","target":target}),
                        )
                    });
                    let overlay_updated = overlay_stdin.is_some_and(|stdin| {
                        send_backend_event(
                            &stdin,
                            serde_json::json!({"type":"answer_target","target":target}),
                        )
                    });
                    status.set_text(if backend_updated && overlay_updated {
                        "Answer destination saved and applied."
                    } else {
                        "Answer destination saved. Reconnect Hyprly if the window does not switch."
                    });
                }
                Ok(()) => status.set_text("Answer destination saved."),
                Err(error) => {
                    status.set_text(&format!("Could not save answer destination: {error}"))
                }
            }
        }
    });
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
        entered_api_keys(&state)
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
    let answer_target = crate::config::Config::load()
        .map(|config| config.screen.answer_target)
        .unwrap_or_else(|_| "webpage".to_string());
    let use_overlay = matches!(answer_target.as_str(), "floating" | "both");
    let mut command = Command::new(executable);
    command
        .arg("daemon")
        .env("HYPRLY_BACKEND_MODE", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match command.spawn() {
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
    let (overlay_child, overlay_stdin) = match spawn_answer_overlay(
        backend_stdin.clone(),
        use_overlay,
    ) {
        Ok(overlay) => (Some(overlay.0), overlay.1),
        Err(error) => {
            let _ = child.kill();
            state.borrow().status.set_text(&format!(
                "Could not start the private answer window: {error}"
            ));
            return;
        }
    };

    let event_tx = state.borrow().event_tx.clone();
    if let Some(stdout) = child.stdout.take() {
        let event_tx = event_tx.clone();
        let overlay_events = overlay_stdin.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(|line| line.ok()) {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                let event = value.get("event").and_then(|v| v.as_str());
                if let Some(overlay_stdin) = &overlay_events {
                    let _ = send_backend_event(overlay_stdin, value.clone());
                }
                if event == Some("connection") {
                    let _ = event_tx.send(UiEvent::Connection(
                        value
                            .get("connected")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                    ));
                    continue;
                }
                if event != Some("pairing") {
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
            for line in BufReader::new(stderr).lines().map_while(|line| line.ok()) {
                if is_groq_api_error(&line) {
                    let _ = event_tx.send(UiEvent::Message(friendly_api_error(&line)));
                }
            }
        });
    }

    let mut state_ref = state.borrow_mut();
    state_ref.child = Some(child);
    state_ref.overlay_child = overlay_child;
    state_ref.paired = false;
    state_ref.backend_stdin = backend_stdin;
    state_ref.overlay_stdin = overlay_stdin;
    state_ref.connect_button.set_sensitive(false);
    state_ref.disconnect_button.set_sensitive(true);
    state_ref
        .status
        .set_text("Hyprly is starting. Pair your phone using the code and QR below.");
}

fn stop_backend(state: &Rc<std::cell::RefCell<AppState>>) {
    stop_screen_process(&mut state.borrow_mut());
    let (child, overlay_child) = {
        let mut state = state.borrow_mut();
        state.backend_stdin.take();
        state.overlay_stdin.take();
        (state.child.take(), state.overlay_child.take())
    };
    reap_child(child);
    reap_child(overlay_child);
    clear_pairing(state);
    state.borrow_mut().paired = false;
    let state_ref = state.borrow();
    state_ref.connect_button.set_sensitive(true);
    state_ref.disconnect_button.set_sensitive(false);
    state_ref.status.set_text("Hyprly is disconnected.");
}

fn spawn_answer_overlay(
    backend_stdin: Option<std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>>,
    visible: bool,
) -> Result<(
    Child,
    Option<std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>>,
)> {
    let backend_stdin = backend_stdin.context("Backend input is unavailable")?;
    let executable = std::env::current_exe().context("Could not locate Hyprly")?;
    let mut child = Command::new(executable)
        .arg("answer-overlay")
        .env("HYPRLY_OVERLAY_VISIBLE", if visible { "1" } else { "0" })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Could not launch the GTK answer overlay")?;
    let Some(stdin) = child.stdin.take() else {
        let _ = child.kill();
        anyhow::bail!("The answer overlay did not provide an input pipe");
    };
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(|line| line.ok()) {
                if let Ok(event) = serde_json::from_str::<serde_json::Value>(&line) {
                    let _ = send_backend_event(&backend_stdin, event);
                }
            }
        });
    }
    Ok((
        child,
        Some(std::sync::Arc::new(std::sync::Mutex::new(stdin))),
    ))
}

fn reap_child(child: Option<Child>) {
    if let Some(mut child) = child {
        let _ = child.kill();
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
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
        stop_screen_process(&mut state.borrow_mut());
        let overlay = {
            let mut state_ref = state.borrow_mut();
            state_ref.child = None;
            state_ref.backend_stdin.take();
            state_ref.overlay_stdin.take();
            state_ref.overlay_child.take()
        };
        reap_child(overlay);
        clear_pairing(state);
        state.borrow_mut().paired = false;
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
        let mut state_ref = state.borrow_mut();
        match event {
            UiEvent::Connection(connected) => {
                state_ref.paired = connected;
                if !connected {
                    state_ref.trigger_clicks = None;
                    stop_screen_process(&mut state_ref);
                }
            }
            UiEvent::ScreenFinished(pid) => {
                if state_ref.screen_pid == Some(pid) {
                    state_ref.screen_pid = None;
                }
            }
            UiEvent::Pairing(info) => {
                state_ref.code.set_text(&info.code);
                state_ref.setup_url.set_uri(&info.setup_url);
                state_ref.setup_url.set_label(&info.setup_url);
                state_ref.setup_url.set_sensitive(true);
                state_ref.secure_url.set_uri(&info.secure_url);
                state_ref.secure_url.set_label(&info.secure_url);
                state_ref.secure_url.set_sensitive(true);
                state_ref
                    .fingerprint
                    .set_text(&info.fingerprint);
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
                recording.label.set_text(&format!(
                    "Now click your chosen mouse button 1–{} times, then pause briefly.",
                    crate::config::MAX_GESTURE_CLICKS
                ));
            }
            if let Some(candidate) = recording.candidate {
                if now.duration_since(candidate.last_click) >= GESTURE_CLICK_GAP {
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
            if now.duration_since(candidate.last_click) > GESTURE_CLICK_GAP {
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
                && press.at.duration_since(previous.last_click) <= GESTURE_CLICK_GAP =>
        {
            ClickSequence {
                button: previous.button,
                clicks: previous
                    .clicks
                    .saturating_add(1)
                    .min(crate::config::MAX_GESTURE_CLICKS),
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
    if !state.borrow().paired {
        state
            .borrow()
            .status
            .set_text("Pair the webpage before asking about the screen.");
        return;
    }
    if state.borrow().screen_pid.is_some() {
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
            let pid = child.id();
            state.borrow_mut().screen_pid = Some(pid);
            state.borrow().status.set_text(
                "Screen question started. The answer will appear here and on the paired webpage.",
            );
            let events = state.borrow().event_tx.clone();
            std::thread::spawn(move || {
                match child.wait_with_output() {
                    Ok(output) if output.status.success() => {
                        let answer = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if answer.is_empty() {
                            let message = "Groq returned an empty answer.";
                            send_screen_error(&backend_stdin, message);
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
                        send_screen_error(&backend_stdin, message);
                        let _ = events.send(UiEvent::Message(format!(
                            "Screen question failed: {}",
                            message
                        )));
                    }
                    Err(error) => {
                        let message = format!("Could not finish screen question: {error}");
                        send_screen_error(&backend_stdin, &message);
                        let _ = events.send(UiEvent::Message(message.to_string()));
                    }
                }
                let _ = events.send(UiEvent::ScreenFinished(pid));
            });
        }
        Err(error) => {
            let message = format!("Could not start screen question: {error}");
            send_screen_error(&backend_stdin, &message);
            state.borrow().status.set_text(&message);
        }
    }
}

fn stop_screen_process(state: &mut AppState) {
    if let Some(pid) = state.screen_pid.take() {
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
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

fn send_screen_error(
    stdin: &std::sync::Arc<std::sync::Mutex<std::process::ChildStdin>>,
    message: &str,
) {
    let _ = send_backend_event(
        stdin,
        serde_json::json!({"type":"screen_error","message":message}),
    );
}

fn mouse_button_name(button: u16) -> String {
    match button {
        crate::config::MIN_MOUSE_BUTTON => "left mouse button".to_string(),
        button if button == crate::config::MIN_MOUSE_BUTTON + 1 => "right mouse button".to_string(),
        button if button == crate::config::MIN_MOUSE_BUTTON + 2 => {
            "middle mouse button".to_string()
        }
        code => format!(
            "mouse button {}",
            code - (crate::config::MIN_MOUSE_BUTTON - 1)
        ),
    }
}

fn describe_gesture(gesture: &crate::config::MouseGesture) -> String {
    format!("{} × {}", gesture.clicks, mouse_button_name(gesture.button))
}

fn test_api_key(state: &Rc<std::cell::RefCell<AppState>>) {
    let keys = {
        let state = state.borrow();
        entered_api_keys(&state)
            .into_iter()
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
                                let models: serde_json::Value = response
                                    .json()
                                    .await
                                    .context("Groq returned an invalid models response")?;
                                let model_count = models
                                    .get("data")
                                    .and_then(serde_json::Value::as_array)
                                    .map_or(0, Vec::len);
                                return Ok(format!(
                                    "Groq API key {} works. {model_count} models available.",
                                    index + 1
                                ));
                            }
                            Ok(response) => {
                                let status = response.status();
                                let body = response.text().await.unwrap_or_default();
                                let detail = serde_json::from_str::<serde_json::Value>(&body)
                                    .ok()
                                    .and_then(|value| {
                                        value
                                            .get("error")
                                            .and_then(|error| error.get("message").or(Some(error)))
                                            .and_then(serde_json::Value::as_str)
                                            .map(str::to_owned)
                                    })
                                    .filter(|detail| !detail.trim().is_empty())
                                    .unwrap_or_else(|| body.trim().chars().take(240).collect());
                                failures.push(if detail.is_empty() {
                                    format!("Key {}: HTTP {status}", index + 1)
                                } else {
                                    format!("Key {}: HTTP {status}: {detail}", index + 1)
                                });
                            }
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
            let status = Command::new(crate::pipewire::WPCTL)
                .args(crate::pipewire::STATUS_ARGS)
                .output()
                .context("WirePlumber command wpctl was not found")?;
            if !status.status.success() {
                anyhow::bail!("wpctl could not query the PipeWire session");
            }
            let text = String::from_utf8_lossy(&status.stdout);
            let sink = crate::pipewire::default_sink(&text);
            let mut command = Command::new("pw-record");
            command
                .arg("--rate")
                .arg(crate::audio::AUDIO_SAMPLE_RATE.to_string())
                .arg("--channels")
                .arg(crate::audio::AUDIO_CHANNELS.to_string())
                .arg("--format")
                .arg(crate::audio::AUDIO_PCM_FORMAT)
                .arg("--raw")
                .arg("--sample-count")
                .arg(crate::audio::AUDIO_FRAME_SAMPLES.to_string());
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
    let details = message
        .strip_prefix("All Groq keys failed.")
        .unwrap_or(message)
        .trim();
    if details.is_empty() {
        summary.into()
    } else {
        format!("{summary} {details}")
    }
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
