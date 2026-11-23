//! Ajustes del usuario. Se guardan como un único JSON en la tabla de ajustes
//! y se leen con tolerancia: lo que falte o no se entienda toma su valor de
//! fábrica, para que una versión nueva nunca tropiece con ajustes viejos.

use serde_json::{json, Value};

use crate::persistence::Database;
use crate::search::SortOrder;
use crate::Result;

const KEY: &str = "prefs";

pub const MIN_HISTORY: u32 = 1_000;
pub const MAX_HISTORY: u32 = 1_000_000;
pub const DEFAULT_HISTORY: u32 = 10_000;
pub const DEFAULT_FONT: &str = "Monospace 11";
pub const DEFAULT_PROFILE: &str = "Predeterminado";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    Dark,
    Light,
    /// El que marque el escritorio.
    System,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::Dark, Theme::Light, Theme::System];

    pub fn label(self) -> &'static str {
        match self {
            Theme::Dark => "Oscuro",
            Theme::Light => "Claro",
            Theme::System => "El del sistema",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::System => "system",
        }
    }

    pub fn parse(value: &str) -> Self {
        Self::ALL.into_iter().find(|t| t.as_str() == value).unwrap_or_default()
    }
}

/// Esquemas de color del terminal: identificador y nombre. Los colores
/// concretos los define la interfaz.
pub const SCHEMES: &[(&str, &str)] = &[
    ("auto", "Según el tema"),
    ("alarviterm-dark", "ALarviTerm oscuro"),
    ("alarviterm-light", "ALarviTerm claro"),
    ("solarized-dark", "Solarized oscuro"),
    ("gruvbox-dark", "Gruvbox oscuro"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cursor {
    #[default]
    Block,
    Bar,
    Underline,
}

impl Cursor {
    pub const ALL: [Cursor; 3] = [Cursor::Block, Cursor::Bar, Cursor::Underline];

    pub fn label(self) -> &'static str {
        match self {
            Cursor::Block => "Bloque",
            Cursor::Bar => "Barra",
            Cursor::Underline => "Subrayado",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Cursor::Block => "block",
            Cursor::Bar => "bar",
            Cursor::Underline => "underline",
        }
    }

    pub fn parse(value: &str) -> Self {
        Self::ALL.into_iter().find(|c| c.as_str() == value).unwrap_or_default()
    }
}

/// Aspecto de un terminal. Cada sesión puede elegir uno.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    /// Descripción de fuente de Pango, por ejemplo `JetBrains Mono 11`.
    pub font: String,
    /// Identificador de [`SCHEMES`].
    pub scheme: String,
    pub cursor: Cursor,
    pub blink: bool,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: DEFAULT_PROFILE.to_string(),
            font: DEFAULT_FONT.to_string(),
            scheme: "auto".to_string(),
            cursor: Cursor::Block,
            // Un cursor parpadeante despierta el proceso varias veces por
            // segundo, así que de fábrica va apagado.
            blink: false,
        }
    }
}

impl Profile {
    fn from_json(value: &Value) -> Option<Self> {
        let default = Self::default();
        let text = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Some(Self {
            name: text("name")?,
            font: text("font").unwrap_or(default.font),
            scheme: text("scheme")
                .filter(|s| SCHEMES.iter().any(|(id, _)| id == s))
                .unwrap_or(default.scheme),
            cursor: text("cursor").map(|c| Cursor::parse(&c)).unwrap_or_default(),
            blink: value.get("blink").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "font": self.font,
            "scheme": self.scheme,
            "cursor": self.cursor.as_str(),
            "blink": self.blink,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub theme: Theme,
    pub sort: SortOrder,
    /// Líneas de historial que guarda el backend por sesión.
    pub history_limit: u32,
    /// Ruta del ejecutable de Claude Code. Vacío: se busca en el PATH.
    pub claude_path: String,
    /// Shell de las sesiones. Vacío: el de la variable `SHELL`.
    pub shell: String,
    /// Modelo que propone el formulario de sesión nueva.
    pub default_model: String,
    /// Argumentos que propone el formulario de sesión nueva para Claude Code.
    pub default_args: String,
    /// Tema de Claude Code para las sesiones de ALarviTerm. Vacío: el del usuario.
    pub claude_theme: String,
    pub footer_git: bool,
    pub footer_model: bool,
    pub footer_usage: bool,
    /// Recoger y mostrar los límites de uso de la cuenta de Claude.
    pub account_limits: bool,
    /// Mostrar bajo el cuadro de texto de Claude Code la línea de estado que
    /// el usuario tiene en sus ajustes.
    pub session_statusline: bool,
    pub notify_permission: bool,
    pub notify_turn_end: bool,
    /// Nunca está vacía.
    pub profiles: Vec<Profile>,
    /// Nombre del perfil de las sesiones que no eligen otro.
    pub default_profile: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            sort: SortOrder::Manual,
            history_limit: DEFAULT_HISTORY,
            claude_path: String::new(),
            shell: String::new(),
            default_model: String::new(),
            default_args: String::new(),
            claude_theme: String::new(),
            footer_git: true,
            footer_model: true,
            footer_usage: true,
            account_limits: true,
            session_statusline: true,
            notify_permission: true,
            notify_turn_end: true,
            profiles: vec![Profile::default()],
            default_profile: DEFAULT_PROFILE.to_string(),
        }
    }
}

impl Settings {
    /// Qué se registra como línea de estado en las sesiones.
    pub fn statusline(&self) -> crate::monitor::StatusLine {
        use crate::monitor::StatusLine;
        if !self.session_statusline {
            StatusLine::Quiet
        } else if self.account_limits {
            StatusLine::Chained
        } else {
            StatusLine::Untouched
        }
    }

    pub fn from_json(text: &str) -> Self {
        let default = Self::default();
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return default;
        };
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(|s| s.trim().to_string());
        let flag = |key: &str, fallback: bool| value.get(key).and_then(Value::as_bool).unwrap_or(fallback);
        let mut settings = Self {
            theme: text("theme").map(|t| Theme::parse(&t)).unwrap_or(default.theme),
            sort: text("sort").map(|s| SortOrder::parse(&s)).unwrap_or(default.sort),
            history_limit: value
                .get("history_limit")
                .and_then(Value::as_u64)
                .map_or(default.history_limit, |n| n.min(u64::from(u32::MAX)) as u32),
            claude_path: text("claude_path").unwrap_or_default(),
            shell: text("shell").unwrap_or_default(),
            default_model: text("default_model").unwrap_or_default(),
            default_args: text("default_args").unwrap_or_default(),
            claude_theme: text("claude_theme").unwrap_or_default(),
            footer_git: flag("footer_git", default.footer_git),
            footer_model: flag("footer_model", default.footer_model),
            footer_usage: flag("footer_usage", default.footer_usage),
            account_limits: flag("account_limits", default.account_limits),
            session_statusline: flag("session_statusline", default.session_statusline),
            notify_permission: flag("notify_permission", default.notify_permission),
            notify_turn_end: flag("notify_turn_end", default.notify_turn_end),
            profiles: value
                .get("profiles")
                .and_then(Value::as_array)
                .map(|list| list.iter().filter_map(Profile::from_json).collect())
                .unwrap_or_default(),
            default_profile: text("default_profile").unwrap_or_default(),
        };
        settings.normalize();
        settings
    }

    pub fn to_json(&self) -> String {
        json!({
            "theme": self.theme.as_str(),
            "sort": self.sort.as_str(),
            "history_limit": self.history_limit,
            "claude_path": self.claude_path,
            "shell": self.shell,
            "default_model": self.default_model,
            "default_args": self.default_args,
            "claude_theme": self.claude_theme,
            "footer_git": self.footer_git,
            "footer_model": self.footer_model,
            "footer_usage": self.footer_usage,
            "account_limits": self.account_limits,
            "session_statusline": self.session_statusline,
            "notify_permission": self.notify_permission,
            "notify_turn_end": self.notify_turn_end,
            "profiles": self.profiles.iter().map(Profile::to_json).collect::<Vec<_>>(),
            "default_profile": self.default_profile,
        })
        .to_string()
    }

    /// Deja los ajustes en un estado coherente: historial dentro de límites,
    /// al menos un perfil, nombres únicos y un perfil predeterminado que
    /// existe.
    pub fn normalize(&mut self) {
        self.history_limit = self.history_limit.clamp(MIN_HISTORY, MAX_HISTORY);
        let mut seen: Vec<String> = Vec::new();
        self.profiles.retain(|profile| {
            let fresh = !profile.name.trim().is_empty() && !seen.contains(&profile.name);
            seen.push(profile.name.clone());
            fresh
        });
        if self.profiles.is_empty() {
            self.profiles.push(Profile::default());
        }
        if !self.profiles.iter().any(|p| p.name == self.default_profile) {
            self.default_profile = self.profiles[0].name.clone();
        }
    }

    pub fn load(db: &Database) -> Self {
        match db.setting(KEY) {
            Ok(Some(text)) => Self::from_json(&text),
            Ok(None) => Self::default(),
            Err(e) => {
                log::error!("no se pudieron leer los ajustes: {e}");
                Self::default()
            }
        }
    }

    pub fn save(&self, db: &Database) -> Result<()> {
        db.set_setting(KEY, &self.to_json())
    }

    /// Perfil que corresponde a una sesión: el suyo si existe, y si no el
    /// predeterminado.
    pub fn profile(&self, name: Option<&str>) -> &Profile {
        name.and_then(|name| self.profiles.iter().find(|p| p.name == name))
            .or_else(|| self.profiles.iter().find(|p| p.name == self.default_profile))
            .unwrap_or(&self.profiles[0])
    }

    /// Nombre libre para un perfil nuevo.
    pub fn free_profile_name(&self, base: &str) -> String {
        let taken = |name: &str| self.profiles.iter().any(|p| p.name == name);
        if !taken(base) {
            return base.to_string();
        }
        (2..)
            .map(|n| format!("{base} {n}"))
            .find(|name| !taken(name))
            .unwrap_or_else(|| base.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_survive_missing_and_broken_data() {
        assert_eq!(Settings::from_json(""), Settings::default());
        assert_eq!(Settings::from_json("{}"), Settings::default());
        assert_eq!(Settings::from_json("[]"), Settings::default());
        let odd = Settings::from_json(
            r#"{"theme": "morado", "history_limit": "mucho", "footer_git": "sí", "profiles": [{"font": "X 9"}, 7]}"#,
        );
        assert_eq!(odd, Settings::default());
    }

    #[test]
    fn round_trip_keeps_every_field() {
        let mut settings = Settings {
            theme: Theme::System,
            sort: SortOrder::Activity,
            history_limit: 20_000,
            claude_path: "/opt/claude".into(),
            shell: "/bin/bash".into(),
            default_model: "opus".into(),
            default_args: "--verbose".into(),
            claude_theme: "custom:limpio".into(),
            footer_git: false,
            footer_model: true,
            footer_usage: false,
            account_limits: false,
            session_statusline: false,
            notify_permission: true,
            notify_turn_end: false,
            profiles: vec![
                Profile::default(),
                Profile {
                    name: "Grande".into(),
                    font: "JetBrains Mono 14".into(),
                    scheme: "gruvbox-dark".into(),
                    cursor: Cursor::Bar,
                    blink: true,
                },
            ],
            default_profile: "Grande".into(),
        };
        settings.normalize();
        assert_eq!(Settings::from_json(&settings.to_json()), settings);

        let db = Database::open_in_memory().expect("abre");
        assert_eq!(Settings::load(&db), Settings::default());
        settings.save(&db).expect("guarda");
        assert_eq!(Settings::load(&db), settings);
    }

    #[test]
    fn normalize_repairs_limits_and_profiles() {
        let mut settings = Settings::from_json(
            r#"{"history_limit": 5, "default_profile": "No existe",
                "profiles": [{"name": "A", "scheme": "inventado"}, {"name": "A"}, {"name": "B", "cursor": "bar"}]}"#,
        );
        assert_eq!(settings.history_limit, MIN_HISTORY);
        assert_eq!(settings.profiles.len(), 2, "los nombres repetidos se descartan");
        assert_eq!(settings.profiles[0].scheme, "auto");
        assert_eq!(settings.profiles[1].cursor, Cursor::Bar);
        assert_eq!(settings.default_profile, "A");

        settings.history_limit = u32::MAX;
        settings.profiles.clear();
        settings.normalize();
        assert_eq!(settings.history_limit, MAX_HISTORY);
        assert_eq!(settings.profiles, vec![Profile::default()]);
        assert_eq!(settings.default_profile, DEFAULT_PROFILE);
    }

    #[test]
    fn sessions_resolve_their_profile() {
        let mut settings = Settings::default();
        settings.profiles.push(Profile {
            name: "Grande".into(),
            ..Profile::default()
        });
        assert_eq!(settings.profile(Some("Grande")).name, "Grande");
        assert_eq!(settings.profile(Some("Borrado")).name, DEFAULT_PROFILE);
        assert_eq!(settings.profile(None).name, DEFAULT_PROFILE);
        settings.default_profile = "Grande".into();
        assert_eq!(settings.profile(None).name, "Grande");
        assert_eq!(settings.free_profile_name("Grande"), "Grande 2");
        assert_eq!(settings.free_profile_name("Nuevo"), "Nuevo");
    }
}
