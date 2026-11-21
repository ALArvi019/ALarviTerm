//! Receptor de hooks. Claude Code lo ejecuta en cada evento con un JSON por
//! la entrada estándar. Actualiza el fichero de estado de la sesión y termina.
//!
//! Como hook nunca escribe en la salida ni falla: la salida de un hook puede
//! acabar en el contexto de Claude y un código de error puede bloquear una
//! herramienta. Fuera de una sesión de ALarviTerm (sin las variables de
//! entorno) no hace nada.
//!
//! Con el argumento `statusline` actúa como línea de estado: guarda los
//! límites de uso que Claude Code le pasa y ejecuta la línea de estado del
//! usuario con la misma entrada, de modo que la suya se ve como siempre.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use alarviterm_core::limits::{self, RateLimits};
use alarviterm_core::monitor::{apply_event, last_model, StateStore, LAUNCH_ENV, STATE_DIR_ENV};
use alarviterm_core::sessions::SESSION_ENV;

/// Las respuestas de herramientas pueden ser enormes y aquí no interesan.
const MAX_INPUT: u64 = 8 * 1024 * 1024;

fn main() {
    if std::env::args().nth(1).as_deref() == Some(limits::STATUSLINE_ARG) {
        statusline();
    } else {
        let _ = run();
    }
}

fn read_input() -> Option<String> {
    let mut input = String::new();
    std::io::stdin().take(MAX_INPUT).read_to_string(&mut input).ok()?;
    Some(input)
}

fn run() -> Option<()> {
    let session = std::env::var(SESSION_ENV).ok()?;
    let dir = std::env::var_os(STATE_DIR_ENV).filter(|d| !d.is_empty())?;
    let store = StateStore::new(dir);

    let event: serde_json::Value = serde_json::from_str(&read_input()?).ok()?;

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

fn statusline() {
    let input = read_input().unwrap_or_default();
    let _ = save_limits(&input);
    let _ = run_user_statusline(&input);
}

/// Guarda los límites solo si cambian, para no despertar a la aplicación en
/// cada refresco de la línea de estado.
fn save_limits(input: &str) -> Option<()> {
    let dir = std::env::var_os(STATE_DIR_ENV).filter(|d| !d.is_empty())?;
    let dir = Path::new(&dir);
    let now = alarviterm_core::now_unix();
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    let previous = limits::read(dir);
    let current = RateLimits::from_statusline(&value, now)?.merged_with(previous, now);
    if previous.is_some_and(|p| p.same_data(&current)) {
        return Some(());
    }
    std::fs::create_dir_all(dir).ok()?;
    limits::write(dir, &current).ok()
}

/// Ejecuta la línea de estado del usuario con la misma entrada y deja que
/// escriba directamente en la salida. Sin ella no se muestra nada.
fn run_user_statusline(input: &str) -> Option<()> {
    let command = limits::user_statusline(&limits::claude_config_dir()?)?;
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        // Un programa que no lee su entrada cierra la tubería: no es un error.
        let _ = stdin.write_all(input.as_bytes());
    }
    child.wait().ok().map(|_| ())
}
