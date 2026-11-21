mod logging;
mod ui;

use adw::prelude::*;
use gtk::glib;

const DEFAULT_APP_ID: &str = "io.github.alarvi019.ALarviTerm";

fn main() -> glib::ExitCode {
    logging::init();

    // Si ALarviTerm se lanza desde dentro de tmux, los clientes que se conectan a
    // nuestro servidor no deben creerse anidados en el del usuario.
    // Se hace antes de crear ningún hilo.
    std::env::remove_var("TMUX");
    std::env::remove_var("TMUX_PANE");

    // Identificador alternativo para instancias de desarrollo o de prueba que
    // no deben fundirse con la instancia de uso diario.
    let app_id = std::env::var("ALARVITERM_APP_ID").unwrap_or_else(|_| DEFAULT_APP_ID.to_string());
    let app = adw::Application::builder().application_id(app_id).build();

    app.connect_activate(|app| {
        if let Some(window) = app.active_window() {
            window.present();
            return;
        }
        // El icono se busca por nombre en el tema. Sin instalar no existe y
        // el escritorio pone el suyo genérico.
        gtk::Window::set_default_icon_name(DEFAULT_APP_ID);
        ui::style::install();
        match ui::window::MainWindow::build(app) {
            Ok(window) => window.present(),
            Err(e) => {
                log::error!("no se pudo iniciar: {e}");
                ui::window::present_startup_error(app, &e.to_string());
            }
        }
    });

    app.run()
}
