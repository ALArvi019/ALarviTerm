//! Sesiones: contrato de backend, implementación tmux y gestor.

pub mod backend;
pub mod manager;
pub mod tmux;

pub use backend::{AttachCommand, BackendSession, LaunchSpec, Liveness, SessionBackend};
pub use manager::{SessionManager, SessionView, Snapshot, SESSION_ENV};
pub use tmux::{TmuxBackend, TmuxOptions};
