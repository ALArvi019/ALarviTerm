//! Resumen de uso por consola, para medir el lector de transcripciones.
//!
//! `cargo run --release -p alarviterm-core --example stats -- 30`

use std::time::Instant;

use alarviterm_core::stats::{self, Library};

fn main() {
    let days: i64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(30);
    let Some(root) = stats::transcripts_dir() else {
        eprintln!("no se encuentra la carpeta de Claude Code");
        return;
    };
    let now = alarviterm_core::now_unix();
    let start = now - days * 86_400;
    let mut library = Library::default();
    let started = Instant::now();
    // Hace falta también el periodo anterior, para comparar.
    let report = library.refresh(&root, start - days * 86_400);
    let read = started.elapsed();
    let summary = library.summarize(Some(start), now, stats::system_local);
    let totals = summary.totals;
    println!(
        "{} ficheros, {} leídos ({} MB) en {:.1} s",
        report.files,
        report.read,
        report.bytes_read / (1024 * 1024),
        read.as_secs_f64()
    );
    println!(
        "entrada {} ({} % de caché), salida {}, {} respuestas, {} turnos, {} sesiones, {} activos",
        stats::compact(totals.input_total()),
        totals.cache_percent().unwrap_or(0),
        stats::compact(totals.output),
        totals.messages,
        totals.turns,
        totals.sessions,
        stats::compact_duration(totals.active_secs),
    );
    println!("{} de {} días con actividad, {} modelos, {} proyectos", summary.active_days(), summary.days.len(), summary.models.len(), summary.projects.len());
    for row in summary.models.iter().take(5) {
        println!("  {:<28} {:>12} {:>10} {:>7}", row.name, stats::compact(row.input), stats::compact(row.output), row.messages);
    }
}
