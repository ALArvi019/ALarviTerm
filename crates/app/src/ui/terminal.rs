//! Terminal real: un widget VTE cuyo PTY ejecuta el comando de conexión que
//! entrega el backend. No hay emulación propia.

use std::path::Path;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use alarviterm_core::sessions::AttachCommand;
use alarviterm_core::settings::{Cursor, Profile};
use vte::prelude::*;

use super::style;

/// Aplica a un terminal el aspecto de un perfil. Vale tanto al crearlo como
/// al cambiar los ajustes con el terminal ya abierto.
pub fn apply_profile(terminal: &vte::Terminal, profile: &Profile) {
    let colors = style::terminal_colors(&profile.scheme);
    let palette: Vec<&gdk::RGBA> = colors.palette.iter().collect();
    terminal.set_colors(Some(&colors.foreground), Some(&colors.background), &palette);
    for class in style::SCHEME_CLASSES {
        terminal.remove_css_class(class);
    }
    terminal.add_css_class(colors.class);
    terminal.set_font(Some(&gtk::pango::FontDescription::from_string(&profile.font)));
    terminal.set_cursor_shape(match profile.cursor {
        Cursor::Block => vte::CursorShape::Block,
        Cursor::Bar => vte::CursorShape::Ibeam,
        Cursor::Underline => vte::CursorShape::Underline,
    });
    terminal.set_cursor_blink_mode(if profile.blink {
        vte::CursorBlinkMode::On
    } else {
        vte::CursorBlinkMode::Off
    });
}

/// Crea el terminal y lanza en él el comando de conexión. `on_exit` se llama
/// cuando ese comando termina (sesión cerrada o cliente desconectado).
pub fn spawn(
    attach: &AttachCommand,
    cwd: &Path,
    profile: &Profile,
    on_exit: impl Fn(&vte::Terminal) + 'static,
) -> vte::Terminal {
    let terminal = vte::Terminal::new();
    terminal.add_css_class("mt-terminal");
    terminal.set_hexpand(true);
    terminal.set_vexpand(true);

    apply_profile(&terminal, profile);
    // El historial lo guarda el backend. El del widget sería una copia.
    terminal.set_scrollback_lines(0);
    terminal.set_mouse_autohide(true);
    terminal.set_audible_bell(false);
    terminal.set_allow_hyperlink(true);

    install_keys(&terminal);
    terminal.connect_child_exited(move |terminal, _status| on_exit(terminal));

    let argv: Vec<&str> = attach.argv.iter().map(String::as_str).collect();
    let env: Vec<String> = attach.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
    let env: Vec<&str> = env.iter().map(String::as_str).collect();
    let cwd = cwd.to_string_lossy();
    terminal.spawn_async(
        vte::PtyFlags::DEFAULT,
        Some(cwd.as_ref()),
        &argv,
        &env,
        glib::SpawnFlags::DEFAULT,
        || {},
        -1,
        gio::Cancellable::NONE,
        |result| {
            if let Err(e) = result {
                log::error!("no se pudo lanzar el comando de conexión: {e}");
            }
        },
    );
    terminal
}

fn install_keys(terminal: &vte::Terminal) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak = terminal.downgrade();
    keys.connect_key_pressed(move |_, key, _code, state| {
        let Some(terminal) = weak.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = state.contains(gdk::ModifierType::ALT_MASK);
        match key {
            gdk::Key::C | gdk::Key::c if ctrl && shift => {
                terminal.copy_clipboard_format(vte::Format::Text);
                glib::Propagation::Stop
            }
            gdk::Key::V | gdk::Key::v if ctrl && shift => {
                terminal.paste_clipboard();
                glib::Propagation::Stop
            }
            // VTE envía lo mismo para Intro y Mayús+Intro. Claude Code entiende
            // ESC seguido de retorno como salto de línea sin enviar.
            gdk::Key::Return | gdk::Key::KP_Enter if shift && !ctrl && !alt => {
                terminal.feed_child(b"\x1b\r");
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    });
    terminal.add_controller(keys);
}
