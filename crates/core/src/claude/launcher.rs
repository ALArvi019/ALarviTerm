use std::path::Path;

use crate::model::{Session, SessionKind};
use crate::{Error, Result};

/// Cómo retomar una conversación al relanzar una sesión de Claude Code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    /// Conversación nueva.
    Fresh,
    /// `claude --continue`: la conversación más reciente del directorio.
    ContinueLatest,
    /// `claude --resume <id>`: una conversación concreta.
    Conversation(String),
}

/// Construye el comando que el backend debe ejecutar para la sesión.
///
/// Las sesiones de shell devuelven un comando vacío: el backend arranca el
/// shell por defecto. Las de Claude Code se envuelven en el shell interactivo
/// del usuario (`$SHELL -ic 'exec claude ...'`) para heredar el mismo entorno
/// (PATH, variables, gestores de versiones) que tendría en una terminal
/// normal, también cuando ALarviTerm se abre desde el lanzador del escritorio.
///
/// `hook_settings` es el fichero que registra los hooks de monitorización. Se
/// pasa con `--settings`, que se suma a la configuración del usuario. Si los
/// argumentos adicionales ya traen su propio `--settings` se respeta ese y la
/// sesión queda sin monitorizar.
///
/// `startup` es el comando de inicio del proyecto. Se ejecuta en ese mismo
/// shell justo antes, y si falla la sesión no llega a arrancar: el error queda
/// a la vista en el terminal.
pub fn build_argv(
    session: &Session,
    claude: Option<&Path>,
    shell: &str,
    resume: &Resume,
    hook_settings: Option<&Path>,
    startup: Option<&str>,
) -> Result<Vec<String>> {
    let startup = startup.map(str::trim).filter(|s| !s.is_empty());
    if startup.is_some_and(|s| s.contains('\0')) {
        return Err(Error::Invalid("el comando de inicio contiene un carácter nulo".into()));
    }
    let prefix = startup.map(|s| format!("{s} && ")).unwrap_or_default();
    match session.kind {
        SessionKind::Shell => match startup {
            None => Ok(Vec::new()),
            Some(_) => {
                let quoted = shlex::try_quote(shell)
                    .map_err(|_| Error::Invalid("la ruta del shell contiene un carácter nulo".into()))?;
                Ok(vec![shell.to_string(), "-ic".into(), format!("{prefix}exec {quoted}")])
            }
        },
        SessionKind::Claude => {
            let claude = claude.ok_or_else(|| {
                Error::Invalid(
                    "no se encuentra el ejecutable de Claude Code (claude) en el PATH".into(),
                )
            })?;
            let mut inner = vec![claude.to_string_lossy().into_owned()];
            if let Some(model) = session.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
                inner.push("--model".into());
                inner.push(model.to_string());
            }
            match resume {
                Resume::Fresh => {}
                Resume::ContinueLatest => inner.push("--continue".into()),
                Resume::Conversation(id) => {
                    inner.push("--resume".into());
                    inner.push(id.clone());
                }
            }
            let extra = split_args(&session.extra_args)?;
            if let Some(settings) = hook_settings.filter(|_| !has_own_settings(&extra)) {
                inner.push("--settings".into());
                inner.push(settings.to_string_lossy().into_owned());
            }
            inner.extend(extra);
            let joined = shlex::try_join(inner.iter().map(String::as_str)).map_err(|_| {
                Error::Invalid("los argumentos contienen un carácter nulo".into())
            })?;
            Ok(vec![
                shell.to_string(),
                "-ic".into(),
                format!("{prefix}exec {joined}"),
            ])
        }
    }
}

/// El usuario ya indica un fichero de ajustes en los argumentos adicionales.
pub fn has_own_settings(extra: &[String]) -> bool {
    extra
        .iter()
        .any(|arg| arg == "--settings" || arg.starts_with("--settings="))
}

/// Trocea los argumentos adicionales con las reglas de comillas del shell.
pub fn split_args(input: &str) -> Result<Vec<String>> {
    shlex::split(input)
        .ok_or_else(|| Error::Invalid("comillas sin cerrar en los argumentos adicionales".into()))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn session(kind: SessionKind, model: Option<&str>, extra: &str) -> Session {
        Session {
            id: "a".into(),
            name: "a".into(),
            group_id: None,
            kind,
            cwd: PathBuf::from("/tmp"),
            model: model.map(str::to_string),
            extra_args: extra.into(),
            env: Vec::new(),
            backend: "tmux".into(),
            backend_id: "mt-a".into(),
            position: 0,
            created_at: 0,
            last_opened_at: None,
            project_id: None,
            archived: false,
            profile: None,
        }
    }

    #[test]
    fn shell_sessions_use_backend_default() {
        let argv = build_argv(&session(SessionKind::Shell, None, ""), None, "/bin/zsh", &Resume::Fresh, None, None)
            .expect("válido");
        assert!(argv.is_empty());
    }

    #[test]
    fn claude_command_is_wrapped_and_quoted() {
        let s = session(SessionKind::Claude, Some("opus"), "--append-system-prompt 'hola mundo'");
        let argv = build_argv(&s, Some(Path::new("/opt/mi claude/claude")), "/bin/zsh", &Resume::Fresh, None, None)
            .expect("válido");
        assert_eq!(argv[0], "/bin/zsh");
        assert_eq!(argv[1], "-ic");
        // El shell debe reconstruir exactamente los argumentos originales.
        let inner = argv[2].strip_prefix("exec ").expect("prefijo exec");
        assert_eq!(
            shlex::split(inner).expect("se puede trocear"),
            vec![
                "/opt/mi claude/claude",
                "--model",
                "opus",
                "--append-system-prompt",
                "hola mundo"
            ]
        );
    }

    #[test]
    fn continue_flag_goes_before_extra_args() {
        let s = session(SessionKind::Claude, None, "--verbose");
        let argv = build_argv(&s, Some(Path::new("/bin/claude")), "/bin/sh", &Resume::ContinueLatest, None, None)
            .expect("válido");
        assert_eq!(argv[2], "exec /bin/claude --continue --verbose");
    }

    #[test]
    fn exact_resume_and_hook_settings() {
        let s = session(SessionKind::Claude, None, "--verbose");
        let argv = build_argv(
            &s,
            Some(Path::new("/bin/claude")),
            "/bin/sh",
            &Resume::Conversation("abc-1".into()),
            Some(Path::new("/datos/mi app/hooks.json")),
            None,
        )
        .expect("válido");
        assert_eq!(
            argv[2],
            "exec /bin/claude --resume abc-1 --settings '/datos/mi app/hooks.json' --verbose"
        );

        // Un --settings del usuario manda sobre el de alarviterm.
        let own = session(SessionKind::Claude, None, "--settings /mio.json");
        let argv = build_argv(
            &own,
            Some(Path::new("/bin/claude")),
            "/bin/sh",
            &Resume::Fresh,
            Some(Path::new("/datos/hooks.json")),
            None,
        )
        .expect("válido");
        assert_eq!(argv[2], "exec /bin/claude --settings /mio.json");
    }

    #[test]
    fn project_startup_runs_before_the_session() {
        let claude = session(SessionKind::Claude, Some("opus"), "");
        let argv = build_argv(
            &claude,
            Some(Path::new("/bin/claude")),
            "/bin/zsh",
            &Resume::Fresh,
            None,
            Some("  source .venv/bin/activate "),
        )
        .expect("válido");
        assert_eq!(argv[2], "source .venv/bin/activate && exec /bin/claude --model opus");

        let shell = session(SessionKind::Shell, None, "");
        let argv = build_argv(&shell, None, "/bin/zsh", &Resume::Fresh, None, Some("nvm use 20"))
            .expect("válido");
        assert_eq!(argv, vec!["/bin/zsh", "-ic", "nvm use 20 && exec /bin/zsh"]);

        // Un comando vacío equivale a no tenerlo.
        let argv = build_argv(&shell, None, "/bin/zsh", &Resume::Fresh, None, Some("   ")).expect("válido");
        assert!(argv.is_empty());
    }

    #[test]
    fn missing_claude_and_bad_quotes_are_errors() {
        let s = session(SessionKind::Claude, None, "");
        assert!(build_argv(&s, None, "/bin/sh", &Resume::Fresh, None, None).is_err());
        let bad = session(SessionKind::Claude, None, "'sin cerrar");
        assert!(build_argv(&bad, Some(Path::new("/bin/claude")), "/bin/sh", &Resume::Fresh, None, None).is_err());
    }
}
