use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Ubicaciones en disco. Todo cuelga de un único directorio de datos para que
/// las pruebas y las instancias de desarrollo puedan aislarse con
/// `ALARVITERM_DATA_DIR`.
#[derive(Debug, Clone)]
pub struct Paths {
    data_dir: PathBuf,
}

impl Paths {
    pub fn from_env() -> Result<Self> {
        if let Some(dir) = std::env::var_os("ALARVITERM_DATA_DIR").filter(|v| !v.is_empty()) {
            return Ok(Self::at(PathBuf::from(dir)));
        }
        let base = match std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => home_dir()?.join(".local/share"),
        };
        Ok(Self::at(base.join("alarviterm")))
    }

    pub fn at(data_dir: PathBuf) -> Self {
        Self { data_dir }
    }

    pub fn ensure(&self) -> Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        Ok(())
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn database(&self) -> PathBuf {
        self.data_dir.join("alarviterm.db")
    }

    pub fn tmux_config(&self) -> PathBuf {
        self.data_dir.join("tmux.conf")
    }

    /// Estado de cada sesión según los hooks de Claude Code.
    pub fn state_dir(&self) -> PathBuf {
        self.data_dir.join("state")
    }

    /// Ajustes que registran los hooks de ALarviTerm en Claude Code.
    pub fn hook_settings(&self) -> PathBuf {
        self.data_dir.join("claude-hooks.json")
    }
}

pub fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| Error::Invalid("la variable HOME no está definida".into()))
}

/// Sustituye el directorio personal por `~` para mostrar rutas compactas.
pub fn display_path(path: &Path) -> String {
    if let Ok(home) = home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return if rest.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    path.display().to_string()
}

/// Expande un `~` inicial introducido por el usuario.
pub fn expand_tilde(input: &str) -> PathBuf {
    let input = input.trim();
    if input == "~" {
        if let Ok(home) = home_dir() {
            return home;
        }
    } else if let Some(rest) = input.strip_prefix("~/") {
        if let Ok(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_and_display_roundtrip() {
        let home = home_dir().expect("HOME");
        assert_eq!(expand_tilde("~"), home);
        assert_eq!(expand_tilde("~/x/y"), home.join("x/y"));
        assert_eq!(expand_tilde("/tmp/a"), PathBuf::from("/tmp/a"));
        assert_eq!(display_path(&home.join("x/y")), "~/x/y");
        assert_eq!(display_path(&home), "~");
        assert_eq!(display_path(Path::new("/etc")), "/etc");
    }
}
