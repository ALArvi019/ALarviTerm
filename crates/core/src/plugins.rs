//! Complementos: programas externos que añaden a cada sesión una etiqueta, un
//! cronómetro y acciones propias. Sirven para conectar la aplicación con
//! herramientas de cada cual (un gestor de tareas, un control de horas) sin
//! meter su código aquí.
//!
//! Un complemento es una carpeta con un `plugin.json` y un ejecutable. La
//! aplicación habla con él por la entrada y la salida estándar, en JSON:
//!
//! * `<comando> status` recibe la lista de sesiones y responde qué mostrar en
//!   cada una.
//! * `<comando> action <id>` recibe la sesión elegida y responde con un
//!   mensaje y, si quiere, una dirección web que abrir.
//!
//! El contrato completo está en `docs/PLUGINS.md`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::sessions::SessionView;
use crate::{Error, Result};

/// Versión del contrato que la aplicación envía en cada petición.
pub const PROTOCOL_VERSION: u32 = 1;

const MANIFEST: &str = "plugin.json";
const DEFAULT_INTERVAL: u64 = 30;
const MIN_INTERVAL: u64 = 5;
const DEFAULT_TIMEOUT: u64 = 10;
const MAX_TIMEOUT: u64 = 60;
/// Tope de lo que se lee de la salida de un complemento.
const MAX_OUTPUT: u64 = 1024 * 1024;
/// Acciones que se aceptan por sesión, para que un fallo no llene la lista.
const MAX_ACTIONS: usize = 12;

/// Un complemento instalado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    /// Nombre de su carpeta. Lo identifica.
    pub id: String,
    /// Nombre que se enseña al usuario.
    pub name: String,
    pub dir: PathBuf,
    /// Ejecutable y argumentos fijos.
    pub command: Vec<String>,
    /// Segundos entre dos consultas de estado.
    pub interval: u64,
    /// Segundos que se espera una respuesta.
    pub timeout: u64,
}

impl Plugin {
    /// Lee el manifiesto de una carpeta de complemento.
    pub fn load(dir: &Path) -> Result<Self> {
        let id = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty() && !n.starts_with('.'))
            .ok_or_else(|| Error::Invalid("carpeta de complemento sin nombre".into()))?;
        let text = std::fs::read_to_string(dir.join(MANIFEST))?;
        let manifest: Value = serde_json::from_str(&text)
            .map_err(|e| Error::Invalid(format!("{id}: {MANIFEST} no es JSON válido: {e}")))?;
        let command: Vec<String> = match manifest.get("command") {
            Some(Value::String(one)) => vec![one.clone()],
            Some(Value::Array(parts)) => parts
                .iter()
                .map(|p| p.as_str().map(str::to_string))
                .collect::<Option<_>>()
                .ok_or_else(|| Error::Invalid(format!("{id}: `command` debe ser una lista de textos")))?,
            _ => return Err(Error::Invalid(format!("{id}: falta `command` en {MANIFEST}"))),
        };
        if command.first().is_none_or(|c| c.trim().is_empty()) {
            return Err(Error::Invalid(format!("{id}: `command` está vacío")));
        }
        let number = |key: &str| manifest.get(key).and_then(Value::as_u64);
        Ok(Self {
            name: manifest
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .unwrap_or(&id)
                .to_string(),
            id,
            dir: dir.to_path_buf(),
            command,
            interval: number("interval").unwrap_or(DEFAULT_INTERVAL).max(MIN_INTERVAL),
            timeout: number("timeout").unwrap_or(DEFAULT_TIMEOUT).clamp(1, MAX_TIMEOUT),
        })
    }

    /// Ejecutable ya resuelto: una ruta relativa se busca en la carpeta del
    /// complemento, un nombre suelto en el PATH.
    fn program(&self) -> PathBuf {
        let first = Path::new(&self.command[0]);
        if first.is_absolute() || first.components().count() == 1 {
            first.to_path_buf()
        } else {
            self.dir.join(first)
        }
    }

    /// Ejecuta el complemento con unos argumentos y una entrada, y devuelve su
    /// salida. Bloquea hasta `timeout` segundos: no llamar desde el hilo de la
    /// interfaz.
    pub fn run(&self, args: &[&str], input: &str) -> Result<String> {
        let fail = |message: String| Error::Invalid(format!("complemento {}: {message}", self.name));
        let mut child = Command::new(self.program())
            .args(&self.command[1..])
            .args(args)
            .current_dir(&self.dir)
            .env("ALARVITERM_PLUGIN_DIR", &self.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| fail(format!("no se pudo ejecutar: {e}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            // Un programa que no lee su entrada cierra la tubería: no es un error.
            let _ = stdin.write_all(input.as_bytes());
        }
        let stdout = child.stdout.take();
        let reader = std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(stdout) = stdout {
                let _ = stdout.take(MAX_OUTPUT).read_to_string(&mut text);
            }
            text
        });

        let deadline = Instant::now() + Duration::from_secs(self.timeout);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(fail(format!("no respondió en {} s", self.timeout)));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(15)),
                Err(e) => return Err(fail(e.to_string())),
            }
        }
        reader.join().map_err(|_| fail("no se pudo leer su salida".into()))
    }

    /// Pregunta qué mostrar en cada sesión.
    pub fn status(&self, sessions: &[SessionInfo]) -> Result<PluginStatus> {
        let output = self.run(&["status"], &status_request(sessions))?;
        PluginStatus::parse(&output)
            .ok_or_else(|| Error::Invalid(format!("complemento {}: respuesta de estado no válida", self.name)))
    }

    /// Ejecuta una acción sobre una sesión.
    pub fn act(&self, action: &str, session: &SessionInfo) -> Result<ActionResult> {
        let input = json!({ "version": PROTOCOL_VERSION, "session": session.to_json() }).to_string();
        let output = self.run(&["action", action], &input)?;
        ActionResult::parse(&output)
            .ok_or_else(|| Error::Invalid(format!("complemento {}: respuesta de acción no válida", self.name)))
    }
}

/// Carpeta donde el usuario instala sus complementos.
pub fn plugins_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("ALARVITERM_PLUGINS_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let config = match std::env::var_os("XDG_CONFIG_HOME").filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => crate::paths::home_dir().ok()?.join(".config"),
    };
    Some(config.join("alarviterm").join("plugins"))
}

/// Complementos válidos de una carpeta, por orden de nombre, y los avisos de
/// los que no se pudieron cargar.
pub fn discover(dir: &Path) -> (Vec<Plugin>, Vec<String>) {
    let mut folders: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.join(MANIFEST).is_file())
                .collect()
        })
        .unwrap_or_default();
    folders.sort();
    let (mut plugins, mut problems) = (Vec::new(), Vec::new());
    for folder in folders {
        match Plugin::load(&folder) {
            Ok(plugin) => plugins.push(plugin),
            Err(e) => problems.push(e.to_string()),
        }
    }
    (plugins, problems)
}

/// Lo que un complemento sabe de una sesión.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub id: String,
    pub name: String,
    pub cwd: String,
    pub kind: &'static str,
    pub group: Option<String>,
    pub project: Option<String>,
    pub alive: bool,
}

impl SessionInfo {
    pub fn from_view(view: &SessionView, group: Option<&str>, project: Option<&str>) -> Self {
        Self {
            id: view.session.id.clone(),
            name: view.session.name.clone(),
            cwd: view.session.cwd.to_string_lossy().into_owned(),
            kind: view.session.kind.as_str(),
            group: group.map(str::to_string),
            project: project.map(str::to_string),
            alive: view.liveness.is_alive(),
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "cwd": self.cwd,
            "kind": self.kind,
            "group": self.group,
            "project": self.project,
            "alive": self.alive,
        })
    }
}

/// Entrada de la orden `status`.
pub fn status_request(sessions: &[SessionInfo]) -> String {
    json!({
        "version": PROTOCOL_VERSION,
        "sessions": sessions.iter().map(SessionInfo::to_json).collect::<Vec<_>>(),
    })
    .to_string()
}

/// Una acción que el complemento ofrece para una sesión.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginAction {
    pub id: String,
    pub label: String,
    /// Nombre de icono simbólico del tema, si quiere botón con icono.
    pub icon: Option<String>,
    /// Si además del menú quiere un botón en la fila de la sesión.
    pub button: bool,
}

/// Cómo se pinta la etiqueta de una sesión.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Normal,
    Ok,
    Warn,
    Muted,
}

/// Lo que un complemento quiere mostrar en una sesión.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionExtras {
    pub badge: Option<String>,
    pub tone: Tone,
    /// Segundos Unix desde los que corre un cronómetro, si hay alguno.
    pub timer_since: Option<i64>,
    pub tooltip: Option<String>,
    pub actions: Vec<PluginAction>,
}

/// Respuesta de la orden `status`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PluginStatus {
    pub sessions: HashMap<String, SessionExtras>,
    /// Problema que el complemento quiere contar, por ejemplo falta de red.
    pub error: Option<String>,
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

impl PluginStatus {
    pub fn parse(output: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(output.trim()).ok()?;
        let object = value.as_object()?;
        let mut sessions = HashMap::new();
        if let Some(entries) = object.get("sessions").and_then(Value::as_object) {
            for (id, entry) in entries {
                let actions = entry
                    .get("actions")
                    .and_then(Value::as_array)
                    .map(|list| {
                        list.iter()
                            .filter_map(|action| {
                                Some(PluginAction {
                                    id: text(action, "id").filter(|id| valid_action_id(id))?,
                                    label: text(action, "label")?,
                                    icon: text(action, "icon"),
                                    button: action.get("button").and_then(Value::as_bool).unwrap_or(false),
                                })
                            })
                            .take(MAX_ACTIONS)
                            .collect()
                    })
                    .unwrap_or_default();
                let extras = SessionExtras {
                    badge: text(entry, "badge"),
                    tone: match entry.get("tone").and_then(Value::as_str) {
                        Some("ok") => Tone::Ok,
                        Some("warn") => Tone::Warn,
                        Some("muted") => Tone::Muted,
                        _ => Tone::Normal,
                    },
                    timer_since: entry.get("timer_since").and_then(Value::as_f64).map(|t| t as i64),
                    tooltip: text(entry, "tooltip"),
                    actions,
                };
                if extras != SessionExtras::default() {
                    sessions.insert(id.clone(), extras);
                }
            }
        }
        Some(Self { sessions, error: text(&value, "error") })
    }
}

/// Los identificadores de acción viajan como argumento y dentro de un nombre
/// de acción de la ventana, así que se limitan a caracteres sin sorpresas.
pub fn valid_action_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 48
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Respuesta de la orden `action`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActionResult {
    pub ok: bool,
    pub message: Option<String>,
    /// Dirección que abrir en el navegador. Solo `http` y `https`.
    pub open_url: Option<String>,
}

impl ActionResult {
    pub fn parse(output: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(output.trim()).ok()?;
        value.as_object()?;
        Some(Self {
            ok: value.get("ok").and_then(Value::as_bool).unwrap_or(true),
            message: text(&value, "message"),
            open_url: text(&value, "open_url")
                .filter(|url| url.starts_with("https://") || url.starts_with("http://")),
        })
    }
}

/// Cronómetro `hh:mm:ss` desde un instante.
pub fn timer_text(since: i64, now: i64) -> String {
    let seconds = (now - since).max(0);
    format!("{:02}:{:02}:{:02}", seconds / 3600, (seconds / 60) % 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alarviterm-plugins-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("carpeta");
        dir
    }

    fn install(root: &Path, id: &str, manifest: &str, script: &str) -> PathBuf {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).expect("carpeta");
        std::fs::write(dir.join(MANIFEST), manifest).expect("manifiesto");
        let exe = dir.join("run.sh");
        std::fs::write(&exe, script).expect("script");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("permisos");
        dir
    }

    fn session(id: &str, name: &str) -> SessionInfo {
        SessionInfo {
            id: id.into(),
            name: name.into(),
            cwd: "/tmp".into(),
            kind: "shell",
            group: None,
            project: Some("Web".into()),
            alive: true,
        }
    }

    #[test]
    fn discovers_valid_plugins_and_reports_broken_ones() {
        let root = scratch("discover");
        install(&root, "b-bueno", r#"{"name": "Bueno", "command": ["./run.sh", "x"], "interval": 1}"#, "");
        install(&root, "a-roto", "esto no es json", "");
        install(&root, "c-sin-comando", r#"{"name": "Nada"}"#, "");
        std::fs::create_dir_all(root.join("sin-manifiesto")).expect("carpeta");

        let (plugins, problems) = discover(&root);
        assert_eq!(plugins.len(), 1);
        let plugin = &plugins[0];
        assert_eq!((plugin.id.as_str(), plugin.name.as_str()), ("b-bueno", "Bueno"));
        assert_eq!(plugin.command, vec!["./run.sh", "x"]);
        assert_eq!(plugin.interval, MIN_INTERVAL);
        assert_eq!(plugin.timeout, DEFAULT_TIMEOUT);
        assert_eq!(plugin.program(), root.join("b-bueno").join("./run.sh"));
        assert_eq!(problems.len(), 2);
        assert!(discover(&root.join("no-existe")).0.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn status_round_trip_through_a_real_process() {
        let root = scratch("status");
        // Devuelve una etiqueta con el nombre de la primera sesión recibida.
        let script = r#"#!/bin/sh
input=$(cat)
case "$1" in
  status)
    name=$(printf '%s' "$input" | sed 's/.*"name":"\([^"]*\)".*/\1/')
    printf '{"sessions":{"s1":{"badge":"%s","tone":"ok","timer_since":100,"actions":[{"id":"go","label":"Ir","button":true},{"id":"mal id","label":"No"},{"id":"sin-etiqueta"}]}}}' "$name"
    ;;
  action)
    printf '{"ok":true,"message":"hecho %s","open_url":"https://example.org/x"}' "$2"
    ;;
esac
"#;
        let dir = install(&root, "eco", r#"{"command": "./run.sh"}"#, script);
        let plugin = Plugin::load(&dir).expect("complemento");
        assert_eq!(plugin.name, "eco");

        let status = plugin.status(&[session("s1", "Pagos")]).expect("estado");
        let extras = &status.sessions["s1"];
        assert_eq!(extras.badge.as_deref(), Some("Pagos"));
        assert_eq!(extras.tone, Tone::Ok);
        assert_eq!(extras.timer_since, Some(100));
        assert_eq!(extras.actions.len(), 1);
        assert!(extras.actions[0].button);

        let result = plugin.act("go", &session("s1", "Pagos")).expect("acción");
        assert!(result.ok);
        assert_eq!(result.message.as_deref(), Some("hecho go"));
        assert_eq!(result.open_url.as_deref(), Some("https://example.org/x"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_slow_or_silent_plugin_is_an_error_not_a_hang() {
        let root = scratch("slow");
        let dir = install(&root, "lento", r#"{"command": "./run.sh", "timeout": 1}"#, "#!/bin/sh\nexec sleep 30\n");
        let plugin = Plugin::load(&dir).expect("complemento");
        let started = Instant::now();
        assert!(plugin.status(&[]).is_err());
        assert!(started.elapsed() < Duration::from_secs(5));

        let dir = install(&root, "mudo", r#"{"command": "./run.sh"}"#, "#!/bin/sh\nexit 0\n");
        assert!(Plugin::load(&dir).expect("complemento").status(&[]).is_err());
        let dir = install(&root, "fantasma", r#"{"command": "./no-existe"}"#, "");
        assert!(Plugin::load(&dir).expect("complemento").status(&[]).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn responses_are_read_defensively() {
        assert!(PluginStatus::parse("[]").is_none());
        assert!(PluginStatus::parse("").is_none());
        let status = PluginStatus::parse(r#"{"error": " sin red ", "sessions": {"a": {}, "b": {"badge": 7}}}"#)
            .expect("estado");
        assert_eq!(status.error.as_deref(), Some("sin red"));
        assert!(status.sessions.is_empty());

        let result = ActionResult::parse(r#"{"ok": false, "open_url": "file:///etc/passwd"}"#).expect("resultado");
        assert!(!result.ok);
        assert_eq!(result.open_url, None);
        assert!(ActionResult::parse("nada").is_none());
        assert!(valid_action_id("play_2-b"));
        assert!(!valid_action_id("a b"));
        assert!(!valid_action_id(""));
    }

    #[test]
    fn timer_counts_hours_minutes_and_seconds() {
        assert_eq!(timer_text(0, 3 * 3600 + 4 * 60 + 5), "03:04:05");
        assert_eq!(timer_text(100, 50), "00:00:00");
        assert_eq!(timer_text(0, 100 * 3600), "100:00:00");
    }
}
