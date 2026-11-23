//! Monitorización de Claude Code a partir de sus hooks.
//!
//! Claude Code avisa de lo que hace ejecutando un comando por cada evento
//! (`SessionStart`, `PreToolUse`, `PermissionRequest`, `Stop`...). ALarviTerm
//! registra el binario `alarviterm-hook` para esos eventos con un fichero de
//! ajustes propio que se pasa con `claude --settings`, de modo que la
//! configuración global del usuario no se toca y sus hooks siguen funcionando.
//!
//! Cada evento se reduce a un [`AgentState`] que se guarda en un fichero por
//! sesión. La aplicación vigila ese directorio. El estado es siempre el del
//! último evento recibido: no se deduce nada del contenido del terminal.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::Result;

/// Variable con el directorio donde `alarviterm-hook` deja el estado.
pub const STATE_DIR_ENV: &str = "ALARVITERM_STATE_DIR";

/// Variable que distingue un lanzamiento del proceso del siguiente. Al
/// reiniciar una sesión, el proceso que muere todavía emite su evento de
/// cierre: sin esta marca se tomaría por el estado del proceso nuevo.
pub const LAUNCH_ENV: &str = "ALARVITERM_LAUNCH_ID";

/// Fichero que toca el backend cuando un proceso termina, para que la
/// aplicación refresque sin esperar al sondeo periódico.
pub const BACKEND_MARK: &str = ".backend";

/// Fichero donde el backend deja el texto que el usuario copia dentro de él,
/// para que la aplicación lo lleve al portapapeles del sistema.
pub const CLIPBOARD_MARK: &str = ".clipboard";

/// Eventos de Claude Code a los que se suscribe alarviterm.
pub const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PermissionRequest",
    "Notification",
    "Stop",
    "SubagentStart",
    "SubagentStop",
    "PreCompact",
    "SessionEnd",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Recién arrancado, todavía sin ninguna petición.
    Ready,
    /// Procesando una petición. Con `tool`, ejecutando esa herramienta.
    Working,
    /// Detenido a la espera de que el usuario apruebe o responda algo.
    Permission,
    /// Turno terminado, a la espera de la siguiente instrucción.
    Waiting,
    /// Compactando el contexto.
    Compacting,
    /// Claude Code ha cerrado la conversación.
    Ended,
}

impl Phase {
    fn as_str(self) -> &'static str {
        match self {
            Phase::Ready => "ready",
            Phase::Working => "working",
            Phase::Permission => "permission",
            Phase::Waiting => "waiting",
            Phase::Compacting => "compacting",
            Phase::Ended => "ended",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "ready" => Phase::Ready,
            "working" => Phase::Working,
            "permission" => Phase::Permission,
            "waiting" => Phase::Waiting,
            "compacting" => Phase::Compacting,
            "ended" => Phase::Ended,
            _ => return None,
        })
    }

    /// El usuario tiene algo que hacer en esta sesión.
    pub fn needs_user(self) -> bool {
        matches!(self, Phase::Permission | Phase::Waiting)
    }
}

/// Lo último que Claude Code ha contado de una sesión.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentState {
    pub phase: Phase,
    /// Herramienta en ejecución o pendiente de permiso.
    pub tool: Option<String>,
    /// Texto del aviso de Claude Code, o el origen de una compactación.
    pub message: Option<String>,
    /// Desde cuándo está en esta fase.
    pub since: i64,
    /// Cuándo llegó el último evento.
    pub updated_at: i64,
    /// Identificador de la conversación, para reanudarla con exactitud.
    pub conversation: Option<String>,
    pub transcript: Option<String>,
    pub permission_mode: Option<String>,
    /// Modelo que respondió por última vez, leído de la transcripción.
    pub model: Option<String>,
    /// Lanzamiento del proceso que emitió el evento.
    pub launch: Option<String>,
    /// Subagentes en marcha. Con el turno terminado, los que quedan trabajan
    /// en segundo plano.
    pub subagents: u32,
    /// Si Claude Code dice tener un comando de shell corriendo en segundo
    /// plano. No viene de los hooks: lo rellena quien lee el estado, con lo
    /// que el propio Claude Code publica de sus sesiones.
    pub background_shell: bool,
}

impl AgentState {
    /// El turno terminó pero la sesión sigue ocupada por su cuenta: no espera
    /// al usuario, espera a que acabe un comando o un subagente.
    pub fn busy_in_background(&self) -> bool {
        matches!(self.phase, Phase::Waiting | Phase::Ready) && (self.background_shell || self.subagents > 0)
    }
}

impl AgentState {
    pub fn to_json(&self) -> String {
        json!({
            "phase": self.phase.as_str(),
            "tool": self.tool,
            "message": self.message,
            "since": self.since,
            "updated_at": self.updated_at,
            "conversation": self.conversation,
            "transcript": self.transcript,
            "permission_mode": self.permission_mode,
            "model": self.model,
            "launch": self.launch,
            "subagents": self.subagents,
        })
        .to_string()
    }

    pub fn from_json(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text).ok()?;
        Some(Self {
            phase: Phase::parse(value.get("phase")?.as_str()?)?,
            tool: text_field(&value, "tool"),
            message: text_field(&value, "message"),
            since: value.get("since")?.as_i64()?,
            updated_at: value.get("updated_at")?.as_i64()?,
            conversation: text_field(&value, "conversation"),
            transcript: text_field(&value, "transcript"),
            permission_mode: text_field(&value, "permission_mode"),
            model: text_field(&value, "model"),
            launch: text_field(&value, "launch"),
            subagents: value.get("subagents").and_then(Value::as_u64).unwrap_or(0) as u32,
            background_shell: false,
        })
    }
}

fn text_field(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Aplica un evento de hook al estado anterior. Devuelve `None` si el evento
/// no cambia nada de lo que ALarviTerm muestra.
pub fn apply_event(previous: Option<&AgentState>, event: &Value, now: i64) -> Option<AgentState> {
    let name = event.get("hook_event_name")?.as_str()?;
    let tool = text_field(event, "tool_name");
    let previous_tool = previous.and_then(|p| p.tool.clone());

    // Los subagentes no cambian la fase: solo se lleva la cuenta de cuántos
    // hay en marcha.
    if name == "SubagentStart" || name == "SubagentStop" {
        let previous = previous?;
        let subagents = if name == "SubagentStart" {
            previous.subagents.saturating_add(1)
        } else {
            previous.subagents.saturating_sub(1)
        };
        // Un aviso de fin que llega sin nadie en la cuenta no cambia nada.
        if subagents == previous.subagents {
            return None;
        }
        return Some(AgentState { subagents, updated_at: now, ..previous.clone() });
    }
    let (phase, tool, message) = match name {
        "SessionStart" => {
            // Tras una compactación automática Claude sigue con su turno.
            let resumed_turn = text_field(event, "source").as_deref() == Some("compact")
                && previous.is_some_and(|p| {
                    p.phase == Phase::Compacting && p.message.as_deref() == Some("auto")
                });
            if resumed_turn {
                (Phase::Working, None, None)
            } else {
                (Phase::Ready, None, None)
            }
        }
        "UserPromptSubmit" | "PostToolUse" => (Phase::Working, None, None),
        "PreToolUse" => (Phase::Working, tool, None),
        "PermissionRequest" => (Phase::Permission, tool, None),
        "Notification" => {
            let message = text_field(event, "message");
            match text_field(event, "notification_type").as_deref() {
                Some("permission_prompt") => (Phase::Permission, previous_tool, message),
                Some("elicitation_dialog") => (Phase::Permission, None, message),
                Some("idle_prompt") => (Phase::Waiting, None, None),
                _ => return None,
            }
        }
        "Stop" => (Phase::Waiting, None, None),
        "PreCompact" => (Phase::Compacting, None, text_field(event, "trigger")),
        "SessionEnd" => (Phase::Ended, None, text_field(event, "reason")),
        _ => return None,
    };

    let since = match previous {
        Some(p) if p.phase == phase => p.since,
        _ => now,
    };
    let keep = |field: &str, old: fn(&AgentState) -> Option<String>| {
        text_field(event, field).or_else(|| previous.and_then(old))
    };
    Some(AgentState {
        phase,
        tool,
        message,
        since,
        updated_at: now,
        conversation: keep("session_id", |p| p.conversation.clone()),
        transcript: keep("transcript_path", |p| p.transcript.clone()),
        permission_mode: keep("permission_mode", |p| p.permission_mode.clone()),
        model: previous.and_then(|p| p.model.clone()),
        launch: previous.and_then(|p| p.launch.clone()),
        // Un arranque limpio empieza de cero. Si un evento se perdió, aquí
        // se corrige.
        subagents: match name {
            "SessionStart" | "SessionEnd" => 0,
            _ => previous.map_or(0, |p| p.subagents),
        },
        background_shell: false,
    })
}

/// Estados por sesión en disco, un fichero JSON por sesión.
#[derive(Debug, Clone)]
pub struct StateStore {
    dir: PathBuf,
}

impl StateStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn ensure(&self) -> Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        Ok(())
    }

    fn path(&self, session: &str) -> Option<PathBuf> {
        valid_session_id(session).then(|| self.dir.join(format!("{session}.json")))
    }

    fn launch_path(&self, session: &str) -> Option<PathBuf> {
        valid_session_id(session).then(|| self.dir.join(format!("{session}.launch")))
    }

    /// Último estado escrito, sea del lanzamiento que sea. Sirve para
    /// recuperar la conversación de un proceso que ya no existe.
    pub fn read_any(&self, session: &str) -> Option<AgentState> {
        let text = std::fs::read_to_string(self.path(session)?).ok()?;
        AgentState::from_json(&text)
    }

    /// Estado del proceso en marcha: solo si lo emitió el lanzamiento actual.
    pub fn read(&self, session: &str) -> Option<AgentState> {
        let state = self.read_any(session)?;
        let current = std::fs::read_to_string(self.launch_path(session)?).ok()?;
        (state.launch.as_deref() == Some(current.trim())).then_some(state)
    }

    /// Anota que la sesión arranca un proceso nuevo, identificado por
    /// `launch`. Los estados de lanzamientos anteriores dejan de contar.
    pub fn begin_launch(&self, session: &str, launch: &str) -> Result<()> {
        let Some(path) = self.launch_path(session) else {
            return Err(crate::Error::Invalid(format!(
                "identificador de sesión no válido: {session}"
            )));
        };
        self.ensure()?;
        std::fs::write(path, launch)?;
        Ok(())
    }

    /// Escritura atómica: quien lee nunca ve un fichero a medias.
    pub fn write(&self, session: &str, state: &AgentState) -> Result<()> {
        let Some(path) = self.path(session) else {
            return Err(crate::Error::Invalid(format!(
                "identificador de sesión no válido: {session}"
            )));
        };
        let tmp = self
            .dir
            .join(format!(".{session}.{}.tmp", std::process::id()));
        std::fs::write(&tmp, state.to_json())?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn clear(&self, session: &str) {
        for path in [self.path(session), self.launch_path(session)].into_iter().flatten() {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => log::warn!("no se pudo borrar {}: {e}", path.display()),
            }
        }
    }
}

/// Los identificadores acaban en un nombre de fichero: solo se aceptan
/// caracteres que no pueden salir del directorio.
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Qué hace ALarviTerm con la línea de estado de las sesiones que lanza.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusLine {
    /// No se registra nada: la del usuario funciona como siempre y no se
    /// recogen límites ni contexto.
    Untouched,
    /// Se recogen los datos y después se ejecuta la del usuario.
    Chained,
    /// Se recogen los datos y no se muestra ninguna.
    Quiet,
}

/// Contenido del fichero de ajustes que registra `alarviterm-hook` en Claude
/// Code. Los hooks se suman a los ajustes del usuario.
///
/// Con `statusline` registra además el hook como línea de estado, que es por
/// donde Claude Code comunica los límites de uso de la cuenta y el contexto
/// de la sesión. El hook ejecuta después la línea de estado del usuario, así
/// que la suya se sigue viendo, salvo que se pida que calle.
///
/// `theme` es el tema de Claude Code para estas sesiones, con el nombre que
/// tendría en sus ajustes (`dark`, `custom:mi-tema`). Sin él vale el del usuario.
pub fn hook_settings(hook_binary: &Path, statusline: StatusLine, theme: Option<&str>) -> Result<String> {
    let command = shlex::try_quote(&hook_binary.to_string_lossy())
        .map_err(|_| crate::Error::Invalid("ruta del hook con un carácter nulo".into()))?
        .into_owned();
    let mut hooks = serde_json::Map::new();
    for event in HOOK_EVENTS {
        hooks.insert(
            (*event).to_string(),
            json!([{ "hooks": [{ "type": "command", "command": command, "timeout": 5 }] }]),
        );
    }
    let mut settings = json!({ "hooks": hooks });
    if let Some(theme) = theme.map(str::trim).filter(|t| !t.is_empty()) {
        settings["theme"] = json!(theme);
    }
    let quiet = match statusline {
        StatusLine::Untouched => None,
        StatusLine::Chained => Some(""),
        StatusLine::Quiet => Some(crate::limits::QUIET_ARG),
    };
    if let Some(quiet) = quiet {
        settings["statusLine"] = json!({
            "type": "command",
            "command": format!("{command} {} {quiet}", crate::limits::STATUSLINE_ARG).trim_end(),
        });
    }
    Ok(serde_json::to_string_pretty(&settings).unwrap_or_default())
}

/// Localiza `alarviterm-hook`: la variable `ALARVITERM_HOOK_BIN`, junto al
/// ejecutable de la aplicación o en el PATH.
pub fn find_hook_binary() -> Option<PathBuf> {
    const NAME: &str = "alarviterm-hook";
    if let Some(path) = std::env::var_os("ALARVITERM_HOOK_BIN").filter(|v| !v.is_empty()) {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    let sibling = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(NAME)));
    if let Some(path) = sibling.filter(|p| p.is_file()) {
        return Some(path);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(NAME))
        .find(|p| p.is_file())
}

/// Modelo del último mensaje del asistente en una transcripción. Solo lee el
/// final del fichero, que puede ser muy grande.
pub fn last_model(transcript: &Path) -> Option<String> {
    const TAIL: u64 = 256 * 1024;
    let mut file = std::fs::File::open(transcript).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    text.lines().rev().find_map(|line| {
        if !line.contains("\"assistant\"") {
            return None;
        }
        let value: Value = serde_json::from_str(line).ok()?;
        let model = value.get("message")?.get("model")?.as_str()?;
        // Los mensajes sintéticos (errores, interrupciones) no son un modelo.
        (!model.is_empty() && !model.starts_with('<')).then(|| model.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str, extra: Value) -> Value {
        let mut value = json!({
            "hook_event_name": name,
            "session_id": "conv-1",
            "transcript_path": "/tmp/t.jsonl",
        });
        if let (Some(base), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
            for (k, v) in extra {
                base.insert(k.clone(), v.clone());
            }
        }
        value
    }

    fn run(events: &[Value]) -> Option<AgentState> {
        let mut state: Option<AgentState> = None;
        for (i, e) in events.iter().enumerate() {
            if let Some(next) = apply_event(state.as_ref(), e, 100 + i as i64) {
                state = Some(next);
            }
        }
        state
    }

    #[test]
    fn a_turn_goes_through_working_and_ends_waiting() {
        let start = run(&[event("SessionStart", json!({"source": "startup"}))]).expect("estado");
        assert_eq!(start.phase, Phase::Ready);
        assert_eq!(start.conversation.as_deref(), Some("conv-1"));

        let tool = run(&[
            event("SessionStart", json!({})),
            event("UserPromptSubmit", json!({"permission_mode": "default"})),
            event("PreToolUse", json!({"tool_name": "Bash"})),
        ])
        .expect("estado");
        assert_eq!(tool.phase, Phase::Working);
        assert_eq!(tool.tool.as_deref(), Some("Bash"));
        assert_eq!(tool.permission_mode.as_deref(), Some("default"));
        // La fase Working empezó con la petición, no con la herramienta.
        assert_eq!(tool.since, 101);
        assert_eq!(tool.updated_at, 102);

        let done = run(&[
            event("UserPromptSubmit", json!({})),
            event("PreToolUse", json!({"tool_name": "Bash"})),
            event("PostToolUse", json!({"tool_name": "Bash"})),
            event("Stop", json!({})),
        ])
        .expect("estado");
        assert_eq!(done.phase, Phase::Waiting);
        assert_eq!(done.tool, None);
    }

    #[test]
    fn subagents_are_counted_without_changing_the_phase() {
        let state = run(&[
            event("UserPromptSubmit", json!({})),
            event("SubagentStart", json!({})),
            event("SubagentStart", json!({})),
            event("Stop", json!({})),
            event("SubagentStop", json!({})),
        ])
        .expect("estado");
        assert_eq!((state.phase, state.subagents), (Phase::Waiting, 1));
        assert!(!state.busy_in_background() || state.subagents == 1);
        assert!(state.busy_in_background());
        let again = AgentState::from_json(&state.to_json()).expect("json");
        assert_eq!(again.subagents, 1);

        let done = run(&[
            event("UserPromptSubmit", json!({})),
            event("SubagentStart", json!({})),
            event("SubagentStop", json!({})),
            event("SubagentStop", json!({})),
            event("Stop", json!({})),
        ])
        .expect("estado");
        assert_eq!(done.subagents, 0, "nunca baja de cero");
        assert!(!done.busy_in_background());
        // Un arranque nuevo empieza de cero aunque se perdiera un evento.
        let fresh = run(&[event("SubagentStart", json!({})), event("SessionStart", json!({"source": "startup"}))]);
        assert_eq!(fresh.expect("estado").subagents, 0);
        assert!(apply_event(None, &event("SubagentStart", json!({})), 1).is_none());
    }

    #[test]
    fn permission_is_reported_and_cleared_by_the_tool_result() {
        let asking = run(&[
            event("PreToolUse", json!({"tool_name": "Write"})),
            event("PermissionRequest", json!({"tool_name": "Write"})),
            event(
                "Notification",
                json!({"notification_type": "permission_prompt", "message": "Claude needs your permission"}),
            ),
        ])
        .expect("estado");
        assert_eq!(asking.phase, Phase::Permission);
        assert_eq!(asking.tool.as_deref(), Some("Write"));
        assert_eq!(asking.message.as_deref(), Some("Claude needs your permission"));
        assert!(asking.phase.needs_user());

        let granted = apply_event(Some(&asking), &event("PostToolUse", json!({})), 200).expect("estado");
        assert_eq!(granted.phase, Phase::Working);
        assert_eq!(granted.message, None);
    }

    #[test]
    fn late_subagent_and_unknown_events_do_not_change_the_state() {
        let waiting = run(&[event("Stop", json!({}))]).expect("estado");
        assert!(apply_event(Some(&waiting), &event("SubagentStop", json!({})), 300).is_none());
        assert!(apply_event(
            Some(&waiting),
            &event("Notification", json!({"notification_type": "auth_success"})),
            300
        )
        .is_none());
        assert!(apply_event(Some(&waiting), &json!({"sin": "nombre"}), 300).is_none());

        // El aviso de inactividad confirma la espera sin reiniciar su reloj.
        let idle = apply_event(
            Some(&waiting),
            &event("Notification", json!({"notification_type": "idle_prompt"})),
            400,
        )
        .expect("estado");
        assert_eq!(idle.phase, Phase::Waiting);
        assert_eq!(idle.since, waiting.since);
        assert_eq!(idle.updated_at, 400);
    }

    #[test]
    fn automatic_compaction_returns_to_working() {
        let auto = run(&[
            event("UserPromptSubmit", json!({})),
            event("PreCompact", json!({"trigger": "auto"})),
        ])
        .expect("estado");
        assert_eq!(auto.phase, Phase::Compacting);
        let after = apply_event(Some(&auto), &event("SessionStart", json!({"source": "compact"})), 500)
            .expect("estado");
        assert_eq!(after.phase, Phase::Working);

        let manual = run(&[event("PreCompact", json!({"trigger": "manual"}))]).expect("estado");
        let after = apply_event(Some(&manual), &event("SessionStart", json!({"source": "compact"})), 500)
            .expect("estado");
        assert_eq!(after.phase, Phase::Ready);
    }

    #[test]
    fn state_survives_a_roundtrip_through_the_store() {
        let dir = std::env::temp_dir().join(format!("alarviterm-monitor-{}", std::process::id()));
        let store = StateStore::new(&dir);
        store.ensure().expect("directorio");
        let mut state = run(&[event("PreToolUse", json!({"tool_name": "Read"}))]).expect("estado");
        state.model = Some("modelo-x".into());
        state.launch = Some("uno".into());
        store.write("abc123", &state).expect("escritura");
        // Sin lanzamiento anotado no se sabe de qué proceso es el estado.
        assert_eq!(store.read("abc123"), None);
        store.begin_launch("abc123", "uno").expect("lanzamiento");
        assert_eq!(store.read("abc123"), Some(state.clone()));
        // Tras relanzar, lo que escriba el proceso anterior ya no cuenta,
        // pero su conversación sigue disponible para reanudarla.
        store.begin_launch("abc123", "dos").expect("lanzamiento");
        assert_eq!(store.read("abc123"), None);
        assert_eq!(store.read_any("abc123"), Some(state));
        assert_eq!(store.read("otra"), None);
        store.clear("abc123");
        store.clear("abc123");
        assert_eq!(store.read_any("abc123"), None);
        assert!(store.begin_launch("../fuera", "x").is_err());
        assert!(store.write("../fuera", &run(&[event("Stop", json!({}))]).expect("estado")).is_err());
        std::fs::remove_dir_all(&dir).expect("limpieza");
    }

    #[test]
    fn session_ids_cannot_escape_the_directory() {
        assert!(valid_session_id("a1b2c3"));
        assert!(valid_session_id("mi_sesion-2"));
        assert!(!valid_session_id(""));
        assert!(!valid_session_id("../x"));
        assert!(!valid_session_id("a/b"));
        assert!(!valid_session_id("a.b"));
    }

    #[test]
    fn settings_register_every_event_with_a_quoted_command() {
        let text = hook_settings(Path::new("/opt/mi app/alarviterm-hook"), StatusLine::Untouched, None).expect("ajustes");
        let value: Value = serde_json::from_str(&text).expect("json");
        let hooks = value.get("hooks").and_then(Value::as_object).expect("hooks");
        assert_eq!(hooks.len(), HOOK_EVENTS.len());
        let command = hooks["Stop"][0]["hooks"][0]["command"].as_str().expect("comando");
        assert_eq!(
            shlex::split(command).expect("se puede trocear"),
            vec!["/opt/mi app/alarviterm-hook"]
        );
        assert!(value.get("statusLine").is_none());

        let text = hook_settings(Path::new("/opt/mi app/alarviterm-hook"), StatusLine::Chained, Some(" ")).expect("ajustes");
        let value: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["statusLine"]["type"], "command");
        assert_eq!(
            shlex::split(value["statusLine"]["command"].as_str().expect("comando")).expect("se puede trocear"),
            vec!["/opt/mi app/alarviterm-hook", "statusline"]
        );

        assert!(value.get("theme").is_none());

        let text = hook_settings(Path::new("/opt/hook"), StatusLine::Quiet, Some("custom:limpio")).expect("ajustes");
        let value: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(value["theme"], "custom:limpio");
        assert_eq!(value["statusLine"]["command"], "/opt/hook statusline --quiet");
    }

    #[test]
    fn last_model_reads_the_latest_assistant_line() {
        let path = std::env::temp_dir().join(format!("alarviterm-transcript-{}.jsonl", std::process::id()));
        let lines = [
            json!({"type": "assistant", "message": {"model": "modelo-a"}}).to_string(),
            json!({"type": "user", "message": {"content": "hola"}}).to_string(),
            json!({"type": "assistant", "message": {"model": "modelo-b"}}).to_string(),
            json!({"type": "assistant", "message": {"model": "<synthetic>"}}).to_string(),
            "línea rota {".to_string(),
        ];
        std::fs::write(&path, lines.join("\n")).expect("escritura");
        assert_eq!(last_model(&path).as_deref(), Some("modelo-b"));
        std::fs::remove_file(&path).expect("limpieza");
        assert_eq!(last_model(&path), None);
    }
}
