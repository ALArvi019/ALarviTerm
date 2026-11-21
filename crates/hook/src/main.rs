//! Receptor de hooks. Claude Code lo ejecuta en cada evento con un JSON por
//! la entrada estándar. Actualiza el fichero de estado de la sesión y termina.
//!
//! Nunca escribe en la salida ni falla: la salida de un hook puede acabar en
//! el contexto de Claude y un código de error puede bloquear una herramienta.
//! Fuera de una sesión de ALarviTerm (sin las variables de entorno) no hace nada.

use std::io::Read;
use std::path::Path;

use alarviterm_core::monitor::{apply_event, last_model, StateStore, LAUNCH_ENV, STATE_DIR_ENV};
use alarviterm_core::sessions::SESSION_ENV;

/// Las respuestas de herramientas pueden ser enormes y aquí no interesan.
const MAX_INPUT: u64 = 8 * 1024 * 1024;

fn main() {
    let _ = run();
}

fn run() -> Option<()> {
    let session = std::env::var(SESSION_ENV).ok()?;
    let dir = std::env::var_os(STATE_DIR_ENV).filter(|d| !d.is_empty())?;
    let store = StateStore::new(dir);

    let mut input = String::new();
    std::io::stdin().take(MAX_INPUT).read_to_string(&mut input).ok()?;
    let event: serde_json::Value = serde_json::from_str(&input).ok()?;

    // Solo cuenta como anterior lo que escribió este mismo proceso.
    let launch = std::env::var(LAUNCH_ENV).ok().filter(|l| !l.is_empty());
    let previous = store.read_any(&session).filter(|p| p.launch == launch);
    let mut state = apply_event(previous.as_ref(), &event, alarviterm_core::now_unix())?;
    state.launch = launch;
    // Al acabar un turno la transcripción ya tiene la respuesta y su modelo.
    if event.get("hook_event_name").and_then(|v| v.as_str()) == Some("Stop") {
        if let Some(model) = state.transcript.as_deref().and_then(|t| last_model(Path::new(t))) {
            state.model = Some(model);
        }
    }
    store.ensure().ok()?;
    store.write(&session, &state).ok()
}
