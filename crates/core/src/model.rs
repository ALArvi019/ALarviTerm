use std::path::PathBuf;

use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub position: i64,
    pub collapsed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    /// Claude Code.
    Claude,
    /// Shell interactivo del usuario.
    Shell,
}

impl SessionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionKind::Claude => "claude",
            SessionKind::Shell => "shell",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "claude" => Ok(SessionKind::Claude),
            "shell" => Ok(SessionKind::Shell),
            other => Err(Error::Invalid(format!("tipo de sesión desconocido: {other}"))),
        }
    }
}

/// Sesión gestionada, tal y como se guarda en la base de datos. El proceso
/// vivo pertenece al backend y se localiza con `backend_id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub name: String,
    pub group_id: Option<i64>,
    pub kind: SessionKind,
    pub cwd: PathBuf,
    pub model: Option<String>,
    /// Argumentos adicionales tal y como los escribió el usuario.
    pub extra_args: String,
    pub env: Vec<(String, String)>,
    pub backend: String,
    pub backend_id: String,
    pub position: i64,
    pub created_at: i64,
    pub last_opened_at: Option<i64>,
    pub project_id: Option<i64>,
    /// Archivada: sigue existiendo pero no aparece en la lista habitual.
    pub archived: bool,
    /// Perfil de terminal. Sin él se usa el predeterminado.
    pub profile: Option<String>,
}

/// Datos que aporta el usuario al crear una sesión.
#[derive(Debug, Clone)]
pub struct NewSession {
    pub name: String,
    pub group_id: Option<i64>,
    pub kind: SessionKind,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub extra_args: String,
    pub env: Vec<(String, String)>,
    pub project_id: Option<i64>,
}

/// Colores disponibles para un proyecto: identificador, nombre y valor.
pub const PROJECT_COLORS: &[(&str, &str, &str)] = &[
    ("", "Sin color", ""),
    ("blue", "Azul", "#4c8dff"),
    ("green", "Verde", "#3fb950"),
    ("amber", "Ámbar", "#d29922"),
    ("red", "Rojo", "#f0554c"),
    ("purple", "Morado", "#bc8cff"),
    ("cyan", "Cian", "#39c5cf"),
    ("pink", "Rosa", "#f778ba"),
];

/// Carpeta de trabajo con nombre, aspecto y entorno propios. Las sesiones de
/// un proyecto heredan sus variables y ejecutan su comando de inicio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub path: PathBuf,
    /// Identificador de [`PROJECT_COLORS`], o vacío.
    pub color: String,
    /// Uno o dos caracteres que lo distinguen de un vistazo, o vacío.
    pub icon: String,
    pub env: Vec<(String, String)>,
    /// Orden de shell que se ejecuta antes de lanzar cada sesión.
    pub startup: String,
    pub position: i64,
}

#[derive(Debug, Clone, Default)]
pub struct NewProject {
    pub name: String,
    pub path: PathBuf,
    pub color: String,
    pub icon: String,
    pub env: Vec<(String, String)>,
    pub startup: String,
}

impl NewProject {
    /// Limpia y valida los campos. El icono se recorta a dos caracteres.
    pub fn validated(mut self) -> Result<Self> {
        self.name = self.name.trim().to_string();
        if self.name.is_empty() {
            return Err(Error::Invalid("el proyecto necesita un nombre".into()));
        }
        if !self.path.is_dir() {
            return Err(Error::Invalid(format!(
                "el directorio {} no existe",
                self.path.display()
            )));
        }
        if !PROJECT_COLORS.iter().any(|(id, _, _)| *id == self.color) {
            self.color.clear();
        }
        self.icon = self.icon.trim().chars().take(2).collect();
        self.startup = self.startup.trim().to_string();
        Ok(self)
    }
}

/// Valor hexadecimal de un color de proyecto.
pub fn project_color(id: &str) -> Option<&'static str> {
    PROJECT_COLORS
        .iter()
        .find(|(color, _, hex)| *color == id && !hex.is_empty())
        .map(|(_, _, hex)| *hex)
}

/// Escribe las variables como las lee [`parse_env_assignments`].
pub fn format_env_assignments(env: &[(String, String)]) -> String {
    env.iter()
        .map(|(key, value)| {
            let quoted = shlex::try_quote(value).map(|q| q.into_owned()).unwrap_or_default();
            format!("{key}={quoted}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Interpreta una lista `CLAVE=valor` separada por espacios, con comillas de
/// shell para valores que contengan espacios.
pub fn parse_env_assignments(input: &str) -> Result<Vec<(String, String)>> {
    let words = shlex::split(input)
        .ok_or_else(|| Error::Invalid("comillas sin cerrar en las variables de entorno".into()))?;
    words
        .into_iter()
        .map(|word| {
            let (key, value) = word.split_once('=').ok_or_else(|| {
                Error::Invalid(format!("«{word}» no tiene la forma CLAVE=valor"))
            })?;
            let valid = !key.is_empty()
                && !key.starts_with(|c: char| c.is_ascii_digit())
                && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if !valid {
                return Err(Error::Invalid(format!(
                    "«{key}» no es un nombre de variable válido"
                )));
            }
            Ok((key.to_string(), value.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_env_assignments() {
        let env = parse_env_assignments("A=1 B='dos palabras' C=").expect("válido");
        assert_eq!(
            env,
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), "dos palabras".to_string()),
                ("C".to_string(), String::new()),
            ]
        );
        assert!(parse_env_assignments("").expect("vacío").is_empty());
    }

    #[test]
    fn rejects_bad_env_assignments() {
        assert!(parse_env_assignments("SIN_IGUAL").is_err());
        assert!(parse_env_assignments("1A=x").is_err());
        assert!(parse_env_assignments("A-B=x").is_err());
        assert!(parse_env_assignments("A='sin cerrar").is_err());
    }

    #[test]
    fn kind_roundtrip() {
        for kind in [SessionKind::Claude, SessionKind::Shell] {
            assert_eq!(SessionKind::parse(kind.as_str()).expect("válido"), kind);
        }
        assert!(SessionKind::parse("otro").is_err());
    }
}
