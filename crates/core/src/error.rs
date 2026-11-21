use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("base de datos: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("entrada/salida: {0}")]
    Io(#[from] std::io::Error),
    #[error("backend {backend}: {message}")]
    Backend {
        backend: &'static str,
        message: String,
    },
    #[error("backend no disponible: {0}")]
    BackendUnavailable(String),
    #[error("no encontrado: {0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;
