//! Núcleo de ALarviTerm: modelo de datos, persistencia, backends de sesión y
//! lanzamiento de Claude Code. No depende de GTK, así que se puede probar sin
//! entorno gráfico.

pub mod actions;
pub mod claude;
pub mod error;
pub mod layout;
pub mod limits;
pub mod model;
pub mod monitor;
pub mod paths;
pub mod persistence;
pub mod plugins;
pub mod scm;
pub mod search;
pub mod sessions;
pub mod stats;
pub mod settings;
pub mod sysinfo;

pub use error::{Error, Result};

/// Segundos desde la época Unix.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
