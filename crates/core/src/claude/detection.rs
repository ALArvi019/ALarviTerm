use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::paths::home_dir;
use crate::sessions::tmux::{find_in_path, is_executable};

/// Lo que se puede saber de la autenticación sin leer ninguna credencial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthHint {
    /// Existe el fichero de credenciales de Claude Code. No se abre.
    CredentialsFile,
    /// Hay una clave o token de API en el entorno.
    ApiKeyInEnvironment,
    /// No hay indicios. Puede estar autenticado por otra vía (llavero del
    /// sistema, proveedor en la nube), así que no equivale a «sin sesión».
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeInstall {
    pub executable: PathBuf,
    /// Salida de `claude --version`, si se pudo obtener.
    pub version: Option<String>,
    pub config_dir: Option<PathBuf>,
    pub auth: AuthHint,
}

/// Localiza el ejecutable sin lanzarlo. `override_path` es la ruta que el
/// usuario haya fijado en los ajustes.
pub fn find_executable(override_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = override_path {
        return is_executable(path).then(|| path.to_path_buf());
    }
    find_in_path("claude").or_else(|| {
        // Al arrancar desde el lanzador del escritorio el PATH puede no
        // incluir el directorio del instalador nativo.
        let fallback = home_dir().ok()?.join(".local/bin/claude");
        is_executable(&fallback).then_some(fallback)
    })
}

/// Detección completa. Ejecuta `claude --version`, que tarda décimas de
/// segundo: no llamar desde el hilo de la interfaz.
pub fn detect(override_path: Option<&Path>) -> Option<ClaudeInstall> {
    let executable = find_executable(override_path)?;
    let version = Command::new(&executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|v| !v.is_empty());
    let config_dir = config_dir().filter(|dir| dir.is_dir());
    let auth = auth_hint(config_dir.as_deref());
    Some(ClaudeInstall {
        executable,
        version,
        config_dir,
        auth,
    })
}

fn config_dir() -> Option<PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => home_dir().ok().map(|home| home.join(".claude")),
    }
}

fn auth_hint(config_dir: Option<&Path>) -> AuthHint {
    if config_dir.is_some_and(|dir| dir.join(".credentials.json").is_file()) {
        return AuthHint::CredentialsFile;
    }
    let in_env = ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"]
        .iter()
        .any(|var| std::env::var_os(var).is_some_and(|v| !v.is_empty()));
    if in_env {
        AuthHint::ApiKeyInEnvironment
    } else {
        AuthHint::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_must_be_executable() {
        assert_eq!(find_executable(Some(Path::new("/no/existe/claude"))), None);
        assert_eq!(
            find_executable(Some(Path::new("/bin/sh"))),
            Some(PathBuf::from("/bin/sh"))
        );
    }

    #[test]
    fn auth_hint_sees_credentials_file_without_reading_it() {
        let dir = std::env::temp_dir().join(format!("alarviterm-auth-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("directorio temporal");
        std::fs::write(dir.join(".credentials.json"), "").expect("fichero");
        assert_eq!(auth_hint(Some(&dir)), AuthHint::CredentialsFile);
        std::fs::remove_dir_all(&dir).expect("limpieza");
    }
}
