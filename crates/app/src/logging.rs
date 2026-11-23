//! Registro de la aplicación. Va a la salida de error y a
//! `alarviterm.log` en la carpeta de datos, para poder saber qué pasó después
//! de un cierre inesperado. El nivel se elige con `ALARVITERM_LOG` (error,
//! warn, info, debug, trace). Por defecto, warn.
//!
//! Un pánico de Rust deja en el fichero su mensaje, el lugar del código y la
//! pila de llamadas. Un fallo dentro de una biblioteca de C (una violación de
//! segmento, por ejemplo) no se puede relatar igual: solo queda anotada la
//! señal, y el detalle hay que buscarlo en el registro del núcleo.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, OnceLock};

use alarviterm_core::paths::Paths;
use log::{Level, LevelFilter, Log, Metadata, Record};

/// Tamaño a partir del cual el registro se archiva al arrancar.
const MAX_LOG_BYTES: u64 = 1024 * 1024;

static LOG_FILE: OnceLock<Mutex<File>> = OnceLock::new();
/// Descriptor del registro para el manejador de señales, que no puede usar
/// cerrojos ni reservar memoria.
static FATAL_FD: AtomicI32 = AtomicI32::new(-1);

struct Logger;

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if self.enabled(record.metadata()) {
            let tag = match record.level() {
                Level::Error => "ERROR",
                Level::Warn => "AVISO",
                Level::Info => "INFO",
                Level::Debug => "DEBUG",
                Level::Trace => "TRAZA",
            };
            let line = format!("ALarviTerm {tag} [{}] {}", record.target(), record.args());
            eprintln!("{line}");
            write_line(&line);
        }
    }

    fn flush(&self) {}
}

fn data_file(name: &str) -> Option<PathBuf> {
    let paths = Paths::from_env().ok()?;
    paths.ensure().ok()?;
    Some(paths.data_dir().join(name))
}

/// Fichero del registro.
pub fn log_path() -> Option<PathBuf> {
    data_file("alarviterm.log")
}

fn write_line(line: &str) {
    if let Some(file) = LOG_FILE.get() {
        if let Ok(mut file) = file.lock() {
            let _ = writeln!(file, "{} {line}", alarviterm_core::now_unix());
        }
    }
}

pub fn init() {
    let level = std::env::var("ALARVITERM_LOG")
        .ok()
        .and_then(|v| v.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::Warn);
    if let Some(path) = log_path() {
        // Un registro que ha crecido se guarda como `.1` y se empieza otro.
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
            let _ = std::fs::rename(&path, path.with_extension("log.1"));
        }
        if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
            FATAL_FD.store(file.as_raw_fd(), Ordering::Relaxed);
            let _ = LOG_FILE.set(Mutex::new(file));
        }
    }
    if log::set_logger(&Logger).is_ok() {
        log::set_max_level(level);
    }
    write_line(&format!("--- arranque, versión {}, proceso {}", env!("CARGO_PKG_VERSION"), std::process::id()));

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        write_line(&format!("PÁNICO {info}\n{backtrace}"));
        default_hook(info);
    }));
    install_signal_notes();
}

/// Deja una línea en el registro cuando el proceso muere por una señal de
/// fallo, y después deja que la señal siga su curso.
fn install_signal_notes() {
    extern "C" fn note(signal: libc::c_int) {
        let fd = FATAL_FD.load(Ordering::Relaxed);
        if fd >= 0 {
            let text: &[u8] = match signal {
                libc::SIGSEGV => b"0 FALLO violacion de segmento (SIGSEGV) en una biblioteca nativa. Detalle: journalctl -k\n",
                libc::SIGBUS => b"0 FALLO error de bus (SIGBUS) en una biblioteca nativa. Detalle: journalctl -k\n",
                libc::SIGILL => b"0 FALLO instruccion no valida (SIGILL). Detalle: journalctl -k\n",
                _ => b"0 FALLO error aritmetico (SIGFPE). Detalle: journalctl -k\n",
            };
            // SAFETY: write es segura dentro de un manejador de señales y el
            // descriptor sigue abierto durante toda la vida del proceso.
            unsafe {
                libc::write(fd, text.as_ptr().cast(), text.len());
            }
        }
        // SAFETY: se restaura la acción por defecto y se repite la señal para
        // que el proceso termine como lo haría sin este manejador.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
    }
    for signal in [libc::SIGSEGV, libc::SIGBUS, libc::SIGILL, libc::SIGFPE] {
        // SAFETY: el manejador solo usa funciones seguras en señales.
        unsafe {
            libc::signal(signal, note as extern "C" fn(libc::c_int) as libc::sighandler_t);
        }
    }
}

/// Anota que la aplicación está en marcha. Devuelve `true` si la vez anterior
/// no llegó a cerrarse con normalidad.
pub fn mark_running() -> bool {
    let Some(marker) = data_file(".running") else {
        return false;
    };
    let crashed = std::fs::read_to_string(&marker)
        .ok()
        .and_then(|pid| pid.trim().parse::<u32>().ok())
        .is_some_and(|pid| !std::path::Path::new(&format!("/proc/{pid}")).exists());
    let _ = std::fs::write(&marker, std::process::id().to_string());
    crashed
}

/// Anota un cierre normal.
pub fn mark_stopped() {
    if let Some(marker) = data_file(".running") {
        let _ = std::fs::remove_file(marker);
    }
    write_line("--- cierre normal");
}

static TERMINATION: AtomicBool = AtomicBool::new(false);

/// Anota las señales con las que el sistema pide terminar, en vez de morir
/// con ellas.
pub fn watch_termination() {
    extern "C" fn note(_signal: libc::c_int) {
        TERMINATION.store(true, Ordering::Relaxed);
    }
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: el manejador solo escribe en un atómico.
        unsafe {
            libc::signal(signal, note as extern "C" fn(libc::c_int) as libc::sighandler_t);
        }
    }
}

pub fn termination_requested() -> bool {
    TERMINATION.load(Ordering::Relaxed)
}
