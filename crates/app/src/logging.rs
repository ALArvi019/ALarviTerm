//! Registro mínimo por la salida de error. El nivel se elige con
//! `ALARVITERM_LOG` (error, warn, info, debug, trace). Por defecto, warn.

use log::{Level, LevelFilter, Log, Metadata, Record};

struct StderrLogger;

impl Log for StderrLogger {
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
            eprintln!("ALarviTerm {tag} [{}] {}", record.target(), record.args());
        }
    }

    fn flush(&self) {}
}

pub fn init() {
    let level = std::env::var("ALARVITERM_LOG")
        .ok()
        .and_then(|v| v.parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::Warn);
    if log::set_logger(&StderrLogger).is_ok() {
        log::set_max_level(level);
    }
}
