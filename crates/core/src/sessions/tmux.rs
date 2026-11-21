//! Backend tmux. Cada sesión gestionada es una sesión tmux con una única
//! ventana y un único panel, en un servidor tmux propio (socket aparte y
//! fichero de configuración propio) para no mezclarse con el tmux del usuario.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use super::backend::{AttachCommand, BackendSession, LaunchSpec, Liveness, SessionBackend};
use crate::{Error, Result};

const NAME: &str = "tmux";
const FIELD_SEPARATOR: &str = "|~|";

/// Variables que deja un Claude Code en sus procesos hijos. Si ALarviTerm se
/// arranca desde dentro de una sesión de Claude Code no deben llegar al
/// servidor tmux, o las sesiones nuevas se creerían anidadas en aquella.
const NESTED_CLAUDE_ENV: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_CHILD_NOTES_DIR",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_BRIDGE_SESSION_ID",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "CLAUDE_EFFORT",
    "AI_AGENT",
];

#[derive(Debug, Clone)]
pub struct TmuxOptions {
    /// Líneas de historial que conserva tmux por sesión.
    pub history_limit: u32,
    /// Fichero que tmux toca cuando un proceso termina o una sesión se
    /// cierra. Quien lo vigile se entera al momento, sin sondear.
    pub notify_file: Option<PathBuf>,
    /// Fichero donde tmux deja lo que se copia en su modo de copia, que es
    /// lo que ocurre al seleccionar con el ratón. Quien lo vigile lo puede
    /// llevar al portapapeles del sistema sin depender de xclip ni de wl-copy.
    pub clipboard_file: Option<PathBuf>,
}

impl Default for TmuxOptions {
    fn default() -> Self {
        Self {
            history_limit: 10_000,
            notify_file: None,
            clipboard_file: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TmuxBackend {
    binary: PathBuf,
    socket: String,
    config: PathBuf,
}

impl TmuxBackend {
    /// `socket` es el nombre para `tmux -L`. `config` es el fichero que se
    /// genera con [`TmuxBackend::write_config`].
    pub fn new(binary: impl Into<PathBuf>, socket: impl Into<String>, config: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            socket: socket.into(),
            config: config.into(),
        }
    }

    pub fn socket(&self) -> &str {
        &self.socket
    }

    /// Escribe la configuración del servidor y, si ya hay uno en marcha, se la
    /// vuelve a cargar.
    pub fn write_config(&self, options: &TmuxOptions) -> Result<()> {
        std::fs::write(&self.config, render_config(options))?;
        if self.server_running()? {
            let config = self.config.to_string_lossy().into_owned();
            self.run(&["source-file", &config])?;
        }
        Ok(())
    }

    /// Detiene el servidor y todas sus sesiones. Pensado para pruebas.
    pub fn kill_server(&self) -> Result<()> {
        let output = self.output(&["kill-server"])?;
        if output.status.success() || is_no_server(&output) {
            Ok(())
        } else {
            Err(self.failure("kill-server", &output))
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.binary);
        cmd.arg("-L").arg(&self.socket).arg("-f").arg(&self.config);
        cmd.stdin(Stdio::null());
        // TMUX apuntaría al servidor del usuario si ALarviTerm corre dentro de tmux.
        cmd.env_remove("TMUX").env_remove("TMUX_PANE");
        if std::env::var_os("CLAUDECODE").is_some() {
            for var in NESTED_CLAUDE_ENV {
                cmd.env_remove(var);
            }
        }
        cmd
    }

    fn output(&self, args: &[&str]) -> Result<Output> {
        self.command().args(args).output().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::BackendUnavailable(format!(
                    "no se encuentra el ejecutable {}",
                    self.binary.display()
                ))
            } else {
                Error::Io(e)
            }
        })
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let output = self.output(args)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        } else {
            Err(self.failure(args.first().copied().unwrap_or("tmux"), &output))
        }
    }

    fn failure(&self, what: &str, output: &Output) -> Error {
        Error::Backend {
            backend: NAME,
            message: format!(
                "{what}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        }
    }

    fn server_running(&self) -> Result<bool> {
        let output = self.output(&["list-sessions", "-F", "#{session_name}"])?;
        Ok(output.status.success())
    }
}

impl SessionBackend for TmuxBackend {
    fn name(&self) -> &'static str {
        NAME
    }

    fn check(&self) -> Result<String> {
        let output = Command::new(&self.binary)
            .arg("-V")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| {
                Error::BackendUnavailable(format!("no se puede ejecutar {}: {e}", self.binary.display()))
            })?;
        if !output.status.success() {
            return Err(Error::BackendUnavailable("tmux -V ha fallado".into()));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn create_session(&self, spec: &LaunchSpec) -> Result<()> {
        validate_id(&spec.backend_id)?;
        let cwd = spec.cwd.to_string_lossy().into_owned();
        let mut args: Vec<String> = vec![
            "new-session".into(),
            "-d".into(),
            "-s".into(),
            spec.backend_id.clone(),
            "-c".into(),
            cwd,
            // Tamaño provisional, el primer cliente que se conecte impone el suyo.
            "-x".into(),
            "160".into(),
            "-y".into(),
            "48".into(),
        ];
        for (key, value) in &spec.env {
            args.push("-e".into());
            args.push(format!("{key}={value}"));
        }
        if !spec.argv.is_empty() {
            args.push("--".into());
            args.extend(spec.argv.iter().cloned());
        }
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        self.run(&refs).map(drop)
    }

    fn attach_command(&self, backend_id: &str) -> Result<AttachCommand> {
        validate_id(backend_id)?;
        Ok(AttachCommand {
            argv: vec![
                self.binary.to_string_lossy().into_owned(),
                "-L".into(),
                self.socket.clone(),
                "-f".into(),
                self.config.to_string_lossy().into_owned(),
                "attach-session".into(),
                "-t".into(),
                format!("={backend_id}"),
            ],
            env: Vec::new(),
        })
    }

    fn detach_session(&self, backend_id: &str) -> Result<()> {
        validate_id(backend_id)?;
        self.run(&["detach-client", "-s", &format!("={backend_id}")])
            .map(drop)
    }

    fn list_sessions(&self) -> Result<Vec<BackendSession>> {
        let format = [
            "#{session_name}",
            "#{pane_dead}",
            "#{pane_dead_status}",
            "#{pane_pid}",
            "#{window_activity}",
            "#{pane_current_command}",
            "#{pane_current_path}",
        ]
        .join(FIELD_SEPARATOR);
        let output = self.output(&["list-panes", "-a", "-F", &format])?;
        if !output.status.success() {
            if is_no_server(&output) {
                return Ok(Vec::new());
            }
            return Err(self.failure("list-panes", &output));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut sessions: Vec<BackendSession> = Vec::new();
        for line in text.lines() {
            match parse_pane_line(line) {
                // Solo cuenta el primer panel de cada sesión: nunca creamos más.
                Some(session) if sessions.iter().all(|s| s.backend_id != session.backend_id) => {
                    sessions.push(session)
                }
                Some(_) => {}
                None => log::warn!("línea de tmux no reconocida: {line:?}"),
            }
        }
        Ok(sessions)
    }

    fn kill_session(&self, backend_id: &str) -> Result<()> {
        validate_id(backend_id)?;
        let output = self.output(&["kill-session", "-t", &format!("={backend_id}")])?;
        if output.status.success() || is_no_server(&output) || is_missing_session(&output) {
            Ok(())
        } else {
            Err(self.failure("kill-session", &output))
        }
    }

    fn send_input(&self, backend_id: &str, text: &str) -> Result<()> {
        validate_id(backend_id)?;
        self.run(&["send-keys", "-t", &format!("={backend_id}:"), "-l", "--", text])
            .map(drop)
    }

    fn resize(&self, backend_id: &str, cols: u16, rows: u16) -> Result<()> {
        validate_id(backend_id)?;
        self.run(&[
            "resize-window",
            "-t",
            &format!("={backend_id}:"),
            "-x",
            &cols.to_string(),
            "-y",
            &rows.to_string(),
        ])
        .map(drop)
    }

    fn get_status(&self, backend_id: &str) -> Result<Liveness> {
        validate_id(backend_id)?;
        Ok(self
            .list_sessions()?
            .into_iter()
            .find(|s| s.backend_id == backend_id)
            .map(|s| s.liveness)
            .unwrap_or(Liveness::Missing))
    }
}

/// Los identificadores se usan como destino de tmux (`-t =id`), donde `:` y
/// `.` separan ventana y panel.
fn validate_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(Error::Invalid(format!("identificador de sesión no válido: {id:?}")))
    }
}

fn is_no_server(output: &Output) -> bool {
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr.contains("no server running") || stderr.contains("error connecting to")
}

fn is_missing_session(output: &Output) -> bool {
    String::from_utf8_lossy(&output.stderr).contains("can't find session")
}

fn parse_pane_line(line: &str) -> Option<BackendSession> {
    let mut fields = line.splitn(7, FIELD_SEPARATOR);
    let name = fields.next()?;
    let dead = fields.next()?;
    let dead_status = fields.next()?;
    let pid = fields.next()?;
    let activity = fields.next()?;
    let command = fields.next()?;
    let path = fields.next()?;
    let liveness = if dead == "1" {
        Liveness::Exited {
            code: dead_status.parse().ok(),
        }
    } else {
        Liveness::Alive {
            pid: pid.parse().ok(),
            command: Some(command.to_string()).filter(|c| !c.is_empty()),
        }
    };
    Some(BackendSession {
        backend_id: name.to_string(),
        liveness,
        last_output_at: activity.parse().ok().filter(|t| *t > 0),
        cwd: Some(path).filter(|p| !p.is_empty()).map(PathBuf::from),
    })
}

fn render_config(options: &TmuxOptions) -> String {
    let mut config = base_config(options);
    match options.notify_file.as_deref().map(notify_command) {
        Some(Some(command)) => {
            for hook in ["pane-died", "session-closed"] {
                config.push_str(&format!("set-hook -g {hook} \"run-shell -b {command}\"\n"));
            }
        }
        Some(None) => log::warn!(
            "la ruta del fichero de aviso tiene caracteres que tmux interpretaría, se omite"
        ),
        None => {}
    }
    match options.clipboard_file.as_deref().map(clipboard_command) {
        Some(Some(command)) => config.push_str(&format!("set -s copy-command {command}\n")),
        Some(None) => log::warn!(
            "la ruta del fichero de copia tiene caracteres que tmux interpretaría, se omite"
        ),
        None => {}
    }
    config
}

fn is_plain_path(path: &str) -> bool {
    let unsafe_char = |c: char| matches!(c, '\'' | '"' | '\\' | '#' | '$' | '`' | '\n');
    !path.chars().any(unsafe_char)
}

/// Orden que recibe por su entrada el texto copiado y lo deja en el fichero.
/// Escribe en un temporal y lo renombra, para que nadie lea una copia a medias.
fn clipboard_command(path: &Path) -> Option<String> {
    let path = path.to_str().filter(|p| is_plain_path(p))?;
    Some(format!("\"cat > '{path}.tmp' && mv '{path}.tmp' '{path}'\""))
}

/// Comando de shell que toca el fichero de aviso, o `None` si la ruta
/// contiene algo que tmux o el shell expandirían dentro de las comillas.
fn notify_command(path: &Path) -> Option<String> {
    let path = path.to_str().filter(|p| is_plain_path(p))?;
    Some(format!("'touch \\\"{path}\\\"'"))
}

fn base_config(options: &TmuxOptions) -> String {
    format!(
        "# Generado por alarviterm. Se reescribe en cada arranque, no editar.\n\
         # Sin prefijo: tmux solo mantiene vivos los procesos, las teclas son de la aplicación.\n\
         set -g prefix None\n\
         set -g prefix2 None\n\
         set -g status off\n\
         set -g mouse on\n\
         set -s escape-time 0\n\
         set -g focus-events on\n\
         set -g default-terminal \"tmux-256color\"\n\
         set -as terminal-features \",xterm-256color:RGB\"\n\
         set -g history-limit {history}\n\
         # El panel se conserva al terminar el proceso: se ve la salida final y el código de salida.\n\
         set -g remain-on-exit on\n\
         set -g window-size latest\n\
         set -g allow-rename off\n\
         set -g set-titles off\n\
         set -s set-clipboard on\n\
         set -g allow-passthrough on\n\
         set -s exit-unattached off\n\
         set -g destroy-unattached off\n",
        history = options.history_limit
    )
}

/// Busca tmux en el PATH.
pub fn find_tmux() -> Option<PathBuf> {
    find_in_path("tmux")
}

pub(crate) fn find_in_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_alive_pane() {
        let line = "mt-abc|~|0|~||~|4242|~|1700000000|~|claude|~|/home/u/mi proyecto";
        let session = parse_pane_line(line).expect("válida");
        assert_eq!(session.backend_id, "mt-abc");
        assert_eq!(
            session.liveness,
            Liveness::Alive {
                pid: Some(4242),
                command: Some("claude".into())
            }
        );
        assert_eq!(session.last_output_at, Some(1_700_000_000));
        assert_eq!(session.cwd, Some(PathBuf::from("/home/u/mi proyecto")));
    }

    #[test]
    fn parses_dead_pane_with_status() {
        let line = "mt-x|~|1|~|3|~|99|~|1700000000|~||~|/tmp";
        let session = parse_pane_line(line).expect("válida");
        assert_eq!(session.liveness, Liveness::Exited { code: Some(3) });
    }

    #[test]
    fn rejects_truncated_line() {
        assert!(parse_pane_line("mt-x|~|0").is_none());
    }

    #[test]
    fn validates_ids() {
        assert!(validate_id("mt-0a1b_2").is_ok());
        for bad in ["", "a:b", "a.b", "a b", "=x"] {
            assert!(validate_id(bad).is_err(), "{bad:?} debería rechazarse");
        }
    }

    #[test]
    fn config_disables_prefix_and_sets_history() {
        let config = render_config(&TmuxOptions {
            history_limit: 1234,
            notify_file: Some(PathBuf::from("/datos/mi app/state/.backend")),
            clipboard_file: Some(PathBuf::from("/datos/mi app/state/.clipboard")),
        });
        assert!(config.contains(
            "set -s copy-command \"cat > '/datos/mi app/state/.clipboard.tmp' && mv '/datos/mi app/state/.clipboard.tmp' '/datos/mi app/state/.clipboard'\"\n"
        ));
        assert!(config.contains(
            "set-hook -g pane-died \"run-shell -b 'touch \\\"/datos/mi app/state/.backend\\\"'\"\n"
        ));
        assert!(config.contains("set-hook -g session-closed"));
        let odd = render_config(&TmuxOptions {
            history_limit: 1,
            notify_file: Some(PathBuf::from("/datos/#{x}/.backend")),
            clipboard_file: Some(PathBuf::from("/datos/$HOME/.clipboard")),
        });
        assert!(!odd.contains("set-hook"));
        assert!(!odd.contains("copy-command"));
        assert!(config.contains("set -g prefix None"));
        assert!(config.contains("set -g history-limit 1234"));
        assert!(config.contains("remain-on-exit on"));
    }
}
