//! Integración con Claude Code: detección de la instalación y construcción
//! del comando de arranque.

pub mod detection;
pub mod launcher;

pub use detection::{detect, AuthHint, ClaudeInstall};
pub use launcher::{build_argv, Resume};
