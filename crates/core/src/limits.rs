//! Límites de uso de la cuenta de Claude: la ventana de 5 horas y la de 7 días.
//!
//! El dato sale del propio Claude Code, que se lo pasa al programa de la línea
//! de estado en cada actualización (`rate_limits`). `alarviterm-hook
//! statusline` lo guarda en el directorio de estado y después ejecuta la línea
//! de estado del usuario, así que la suya se sigue viendo igual. No se lee
//! ninguna credencial ni se hace ninguna petición.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Fichero del directorio de estado con los últimos límites conocidos.
pub const LIMITS_MARK: &str = ".limits";

/// Argumento con el que Claude Code llama al hook como línea de estado.
pub const STATUSLINE_ARG: &str = "statusline";

/// Una ventana de uso.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    /// Porcentaje consumido, de 0 a 100.
    pub used_percent: f64,
    /// Momento en que la ventana se reinicia, en segundos Unix.
    pub resets_at: i64,
}

impl Window {
    fn from_json(value: &Value) -> Option<Self> {
        let used_percent = value.get("used_percentage")?.as_f64()?;
        let resets_at = value.get("resets_at")?.as_f64()? as i64;
        (used_percent.is_finite() && resets_at > 0).then_some(Self {
            used_percent: used_percent.clamp(0.0, 100.0),
            resets_at,
        })
    }

    fn to_json(self) -> Value {
        json!({ "used_percentage": self.used_percent, "resets_at": self.resets_at })
    }

    /// Porcentaje que queda por gastar, redondeado.
    pub fn remaining_percent(&self) -> u32 {
        (100.0 - self.used_percent).round().clamp(0.0, 100.0) as u32
    }

    /// Segundos que faltan para el reinicio.
    pub fn seconds_left(&self, now: i64) -> i64 {
        (self.resets_at - now).max(0)
    }
}

/// Últimos límites que alguna sesión de Claude Code ha comunicado.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RateLimits {
    /// Cuándo se recibieron, en segundos Unix.
    pub updated: i64,
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}

impl RateLimits {
    /// Extrae los límites del JSON que Claude Code entrega a la línea de
    /// estado. Sin suscripción, o antes de la primera respuesta, no vienen.
    pub fn from_statusline(input: &Value, now: i64) -> Option<Self> {
        let limits = input.get("rate_limits")?;
        let window = |key: &str| limits.get(key).and_then(Window::from_json);
        let found = Self {
            updated: now,
            five_hour: window("five_hour"),
            seven_day: window("seven_day"),
        };
        (found.five_hour.is_some() || found.seven_day.is_some()).then_some(found)
    }

    pub fn from_json(text: &str) -> Option<Self> {
        let value: Value = serde_json::from_str(text).ok()?;
        Some(Self {
            updated: value.get("updated")?.as_i64()?,
            five_hour: value.get("five_hour").and_then(Window::from_json),
            seven_day: value.get("seven_day").and_then(Window::from_json),
        })
    }

    pub fn to_json(&self) -> String {
        json!({
            "updated": self.updated,
            "five_hour": self.five_hour.map(Window::to_json),
            "seven_day": self.seven_day.map(Window::to_json),
        })
        .to_string()
    }

    /// Descarta las ventanas que ya se reiniciaron: su porcentaje ya no vale.
    pub fn live(self, now: i64) -> Self {
        let alive = |w: Option<Window>| w.filter(|w| w.resets_at > now);
        Self {
            updated: self.updated,
            five_hour: alive(self.five_hour),
            seven_day: alive(self.seven_day),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.five_hour.is_none() && self.seven_day.is_none()
    }

    /// Completa con las ventanas de `older` que esta lectura no trae. Claude
    /// Code puede enviar una sola de las dos.
    pub fn merged_with(self, older: Option<Self>, now: i64) -> Self {
        let older = older.map(|o| o.live(now)).unwrap_or_default();
        Self {
            updated: self.updated,
            five_hour: self.five_hour.or(older.five_hour),
            seven_day: self.seven_day.or(older.seven_day),
        }
    }

    /// Igual que otra lectura salvo por el momento en que llegó.
    pub fn same_data(&self, other: &Self) -> bool {
        self.five_hour == other.five_hour && self.seven_day == other.seven_day
    }
}

/// Lee los límites guardados en el directorio de estado.
pub fn read(state_dir: &Path) -> Option<RateLimits> {
    RateLimits::from_json(&std::fs::read_to_string(state_dir.join(LIMITS_MARK)).ok()?)
}

/// Guarda los límites con escritura atómica.
pub fn write(state_dir: &Path, limits: &RateLimits) -> std::io::Result<()> {
    let target = state_dir.join(LIMITS_MARK);
    let temporary = state_dir.join(format!("{LIMITS_MARK}.{}.tmp", std::process::id()));
    std::fs::write(&temporary, limits.to_json())?;
    std::fs::rename(&temporary, &target)
}

/// Tiempo restante en pocas letras: `3d 4h`, `2h 13m`, `9m`.
pub fn short_duration(seconds: i64) -> String {
    let minutes = (seconds.max(0) + 59) / 60;
    let (days, hours, mins) = (minutes / 1440, (minutes % 1440) / 60, minutes % 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

/// Tiempo restante en palabras, para textos de ayuda.
pub fn long_duration(seconds: i64) -> String {
    let minutes = (seconds.max(0) + 59) / 60;
    let (days, hours, mins) = (minutes / 1440, (minutes % 1440) / 60, minutes % 60);
    let plural = |n: i64, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    if days > 0 {
        format!("{} y {}", plural(days, "día", "días"), plural(hours, "hora", "horas"))
    } else if hours > 0 {
        format!("{} y {}", plural(hours, "hora", "horas"), plural(mins, "minuto", "minutos"))
    } else {
        plural(mins, "minuto", "minutos")
    }
}

/// Carpeta de configuración de Claude Code del usuario.
pub fn claude_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    crate::paths::home_dir().ok().map(|home| home.join(".claude"))
}

/// Comando de la línea de estado que el usuario tiene en sus ajustes
/// personales de Claude Code, si tiene alguna. Solo se lee, nunca se cambia.
///
/// Los ajustes de proyecto no se miran: Claude Code no ejecuta su línea de
/// estado hasta que el usuario confía en la carpeta, y aquí no hay forma de
/// saber si lo ha hecho.
pub fn user_statusline(config_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config_dir.join("settings.json")).ok()?;
    let settings: Value = serde_json::from_str(&text).ok()?;
    let line = settings.get("statusLine")?;
    if line.get("type").and_then(Value::as_str).unwrap_or("command") != "command" {
        return None;
    }
    let command = line.get("command")?.as_str()?.trim();
    (!command.is_empty()).then(|| command.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({
            "session_id": "abc",
            "rate_limits": {
                "five_hour": { "used_percentage": 37.4, "resets_at": 1_000_000 },
                "seven_day": { "used_percentage": 12.0, "resets_at": 1_400_000 }
            }
        })
    }

    #[test]
    fn reads_both_windows_from_the_statusline_input() {
        let limits = RateLimits::from_statusline(&sample(), 900_000).expect("límites");
        let five = limits.five_hour.expect("5h");
        assert_eq!(five.remaining_percent(), 63);
        assert_eq!(five.seconds_left(900_000), 100_000);
        assert_eq!(limits.seven_day.expect("7d").remaining_percent(), 88);
        assert_eq!(limits.updated, 900_000);
    }

    #[test]
    fn missing_or_malformed_limits_are_not_invented() {
        assert!(RateLimits::from_statusline(&json!({ "session_id": "abc" }), 1).is_none());
        assert!(RateLimits::from_statusline(&json!({ "rate_limits": {} }), 1).is_none());
        let partial = json!({ "rate_limits": {
            "five_hour": { "used_percentage": "mucho", "resets_at": 5 },
            "seven_day": { "used_percentage": 140.0, "resets_at": 50 }
        }});
        let limits = RateLimits::from_statusline(&partial, 1).expect("solo 7d");
        assert!(limits.five_hour.is_none());
        assert_eq!(limits.seven_day.expect("7d").remaining_percent(), 0);
    }

    #[test]
    fn survives_a_round_trip_and_drops_expired_windows() {
        let limits = RateLimits::from_statusline(&sample(), 900_000).expect("límites");
        let again = RateLimits::from_json(&limits.to_json()).expect("json");
        assert_eq!(again, limits);
        let later = again.live(1_200_000);
        assert!(later.five_hour.is_none());
        assert!(later.seven_day.is_some());
        assert!(again.live(2_000_000).is_empty());
    }

    #[test]
    fn a_partial_reading_keeps_the_other_window_while_it_lives() {
        let full = RateLimits::from_statusline(&sample(), 900_000).expect("límites");
        let only_five = RateLimits {
            updated: 950_000,
            five_hour: Some(Window { used_percent: 50.0, resets_at: 1_000_000 }),
            seven_day: None,
        };
        let merged = only_five.merged_with(Some(full), 950_000);
        assert_eq!(merged.five_hour.expect("5h").used_percent, 50.0);
        assert_eq!(merged.seven_day, full.seven_day);
        assert!(only_five.merged_with(Some(full), 1_500_000).seven_day.is_none());
        assert!(full.same_data(&RateLimits { updated: 1, ..full }));
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(short_duration(2 * 3600 + 13 * 60), "2h 13m");
        assert_eq!(short_duration(3 * 86_400 + 4 * 3600 + 5), "3d 4h");
        assert_eq!(short_duration(30), "1m");
        assert_eq!(short_duration(-5), "0m");
        assert_eq!(long_duration(3600 + 60), "1 hora y 1 minuto");
        assert_eq!(long_duration(2 * 86_400 + 3 * 3600), "2 días y 3 horas");
        assert_eq!(long_duration(120), "2 minutos");
    }

    #[test]
    fn finds_only_a_command_statusline_in_user_settings() {
        let dir = std::env::temp_dir().join(format!("alarviterm-limits-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("carpeta");
        assert_eq!(user_statusline(&dir), None);
        std::fs::write(
            dir.join("settings.json"),
            r#"{"statusLine": {"type": "command", "command": " bash ~/linea.sh "}}"#,
        )
        .expect("ajustes");
        assert_eq!(user_statusline(&dir).as_deref(), Some("bash ~/linea.sh"));
        std::fs::write(dir.join("settings.json"), r#"{"statusLine": {"type": "otro", "command": "x"}}"#)
            .expect("ajustes");
        assert_eq!(user_statusline(&dir), None);
        std::fs::write(dir.join("settings.json"), "no es json").expect("ajustes");
        assert_eq!(user_statusline(&dir), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
