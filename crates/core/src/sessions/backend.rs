use std::path::PathBuf;

use crate::Result;

/// Lo que hace falta para arrancar un proceso dentro del backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub backend_id: String,
    pub cwd: PathBuf,
    /// Comando y argumentos. Vacío significa «el shell por defecto del usuario».
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Estado del proceso según el backend. Es un hecho comprobado, no una
/// estimación: no dice si Claude piensa o espera, solo si el proceso existe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    /// El proceso sigue vivo.
    Alive {
        pid: Option<u32>,
        command: Option<String>,
    },
    /// El proceso terminó y el backend conserva la sesión con su salida.
    Exited { code: Option<i32> },
    /// El backend no conoce la sesión (reinicio del equipo, sesión matada).
    Missing,
}

impl Liveness {
    pub fn is_alive(&self) -> bool {
        matches!(self, Liveness::Alive { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendSession {
    pub backend_id: String,
    pub liveness: Liveness,
    /// Momento de la última salida del proceso (segundos Unix), si el backend
    /// lo conoce.
    pub last_output_at: Option<i64>,
    pub cwd: Option<PathBuf>,
}

/// Comando que un widget de terminal debe ejecutar en su PTY para ver e
/// interactuar con la sesión.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachCommand {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Contrato entre la aplicación y quien mantiene vivos los procesos. La
/// interfaz gráfica solo conoce este trait: para ver una sesión pide un
/// [`AttachCommand`] y lo lanza en un terminal, sin saber si detrás hay tmux u
/// otra cosa.
pub trait SessionBackend {
    /// Identificador estable que se guarda junto a cada sesión.
    fn name(&self) -> &'static str;

    /// Comprueba que el backend se puede usar y devuelve su versión.
    fn check(&self) -> Result<String>;

    fn create_session(&self, spec: &LaunchSpec) -> Result<()>;

    fn attach_command(&self, backend_id: &str) -> Result<AttachCommand>;

    /// Desconecta todos los clientes de la sesión sin terminarla.
    fn detach_session(&self, backend_id: &str) -> Result<()>;

    fn list_sessions(&self) -> Result<Vec<BackendSession>>;

    /// Termina la sesión y su proceso. No falla si ya no existía.
    fn kill_session(&self, backend_id: &str) -> Result<()>;

    /// Escribe texto literal en la entrada de la sesión.
    fn send_input(&self, backend_id: &str, text: &str) -> Result<()>;

    fn resize(&self, backend_id: &str, cols: u16, rows: u16) -> Result<()>;

    fn get_status(&self, backend_id: &str) -> Result<Liveness>;
}
