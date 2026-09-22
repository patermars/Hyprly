use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;
use adw::prelude::*;
use gtk4_layer_shell::{LayerShell, Edge, Layer, KeyboardMode};
use crate::config::UiConfig;
use crate::ui::components::{OverlayComponents, build_overlay_content};

pub fn setup_overlay_window(
    app: &adw::Application,
    config: &UiConfig,
) -> (adw::ApplicationWindow, OverlayComponents) {
    let window = adw::ApplicationWindow::new(app);
    
    window.init_layer_shell();
    // A stable namespace lets Hyprland target this layer with a
    // `no_screen_share` layer rule.
    window.set_namespace("hyprly");
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::None);

    match config.position.as_str() {
        "right" => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Right, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, false);
        }
        "left" => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Right, false);
        }
        "top" => {
            window.set_anchor(Edge::Top, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_anchor(Edge::Bottom, false);
        }
        "bottom" => {
            window.set_anchor(Edge::Bottom, true);
            window.set_anchor(Edge::Left, true);
            window.set_anchor(Edge::Right, true);
            window.set_anchor(Edge::Top, false);
        }
        "center" => {
            window.set_anchor(Edge::Top, false);
            window.set_anchor(Edge::Right, false);
            window.set_anchor(Edge::Bottom, false);
            window.set_anchor(Edge::Left, false);
        }
        _ => {
            // Keep unknown values safe and compact instead of accidentally
            // turning the overlay into an edge-anchored surface.
            window.set_anchor(Edge::Top, false);
            window.set_anchor(Edge::Right, false);
            window.set_anchor(Edge::Bottom, false);
            window.set_anchor(Edge::Left, false);
        }
    }

    window.set_default_size(config.width, -1);

    let components = build_overlay_content(config);
    window.set_content(Some(&components.container));

    let key_controller = gtk::EventControllerKey::new();
    let win_clone = window.clone();
    key_controller.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == gtk::gdk::Key::Escape {
            hide_overlay(&win_clone);
            return gtk::glib::Propagation::Stop;
        }
        gtk::glib::Propagation::Proceed
    });
    window.add_controller(key_controller);

    let focus_controller = gtk::EventControllerFocus::new();
    let win_focus_clone = window.clone();
    focus_controller.connect_enter(move |_| {
        win_focus_clone.set_keyboard_mode(KeyboardMode::OnDemand);
    });
    components.input.add_controller(focus_controller);

    (window, components)
}

pub fn show_overlay(window: &adw::ApplicationWindow) {
    window.present();
}

pub fn hide_overlay(window: &adw::ApplicationWindow) {
    window.set_visible(false);
    window.set_keyboard_mode(KeyboardMode::None);
}
