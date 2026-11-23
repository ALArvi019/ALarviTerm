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
        let crashed = logging::mark_running();
        match ui::window::MainWindow::build(app) {
            Ok(window) => {
                window.present();
                if crashed {
                    log::warn!("la ejecución anterior no terminó con normalidad");
                    let log = logging::log_path().map(|p| alarviterm_core::paths::display_path(&p));
                    window.notice(&format!(
                        "La última vez ALarviTerm se cerró de forma inesperada. El detalle está en {}",
                        log.unwrap_or_else(|| "el registro".to_string())
                    ));
                }
            }
            Err(e) => {
                log::error!("no se pudo iniciar: {e}");
                ui::window::present_startup_error(app, &e.to_string());
            }
        }
    });

    app.connect_shutdown(|_| logging::mark_stopped());
    // Que el sistema pida terminar (cierre de sesión, kill) es un cierre normal:
    // la señal solo levanta una marca y el bucle principal sale por su pie.
    logging::watch_termination();
    {
        let app = app.clone();
        glib::timeout_add_seconds_local(1, move || {
            if logging::termination_requested() {
                app.quit();
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }
    app.run()
}
