use gtk4 as gtk;
use gtk::prelude::*;
use crate::config::UiConfig;

pub struct OverlayComponents {
    pub container: gtk::Box,
    pub input: gtk::Entry,
    pub response_view: gtk::TextView,
    pub status_label: gtk::Label,
    pub spinner: gtk::Spinner,
    pub scroll: gtk::ScrolledWindow,
}

pub fn build_overlay_content(config: &UiConfig) -> OverlayComponents {
    let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
    container.set_margin_top(16);
    container.set_margin_bottom(16);
    container.set_margin_start(16);
    container.set_margin_end(16);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let title = gtk::Label::builder()
        .label("<span weight=\"bold\" size=\"large\">Hyprly</span>")
        .use_markup(true)
        .build();
    let spinner = gtk::Spinner::new();
    spinner.set_halign(gtk::Align::End);
    spinner.set_hexpand(true);
    header.append(&title);
    header.append(&spinner);

    let scroll = gtk::ScrolledWindow::new();
    // Keep the overlay a compact horizontal panel rather than a full-height
    // sidebar. The response can grow, but remains bounded on screen.
    scroll.set_vexpand(false);
    scroll.set_min_content_height(96);
    scroll.set_max_content_height(360);

    let response_view = gtk::TextView::new();
    response_view.set_editable(false);
    response_view.set_wrap_mode(gtk::WrapMode::Word);
    
    let provider = gtk::CssProvider::new();
    let css = format!("textview {{ font: {}; }}", config.font);
    provider.load_from_string(&css);
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().unwrap(),
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    scroll.set_child(Some(&response_view));

    let input = gtk::Entry::new();
    input.set_placeholder_text(Some("Ask anything..."));
    input.set_hexpand(true);

    let status_label = gtk::Label::new(Some("Ready"));
    status_label.add_css_class("dim-label");

    let container_provider = gtk::CssProvider::new();
    let container_css = format!(
        ".hyprly-overlay {{ background-color: rgba(24, 24, 32, {}); color: #e0e0e0; border-radius: 12px; }} .hyprly-overlay entry {{ background-color: #181820; color: #e0e0e0; border: 1px solid #333; border-radius: 6px; padding: 8px; }} .hyprly-overlay textview text {{ background-color: transparent; color: #e0e0e0; }}",
        config.opacity
    );
    container_provider.load_from_string(&container_css);
    gtk::style_context_add_provider_for_display(
        &gtk::gdk::Display::default().unwrap(),
        &container_provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    container.add_css_class("hyprly-overlay");

    container.append(&header);
    container.append(&scroll);
    container.append(&input);
    container.append(&status_label);

    OverlayComponents {
        container,
        input,
        response_view,
        status_label,
        spinner,
        scroll,
    }
}
