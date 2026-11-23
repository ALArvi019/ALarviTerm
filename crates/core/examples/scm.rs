//! Mide cuánto tarda en leerse el control de código de unas carpetas.
//! Uso: cargo run --release -p alarviterm-core --example scm -- CARPETA...
fn main() {
    let dirs: Vec<std::path::PathBuf> = std::env::args().skip(1).map(Into::into).collect();
    let start = std::time::Instant::now();
    let repos = alarviterm_core::scm::discover(&dirs);
    let changes: usize = repos.iter().map(|r| r.changes.len()).sum();
    println!("{} árboles de trabajo, {changes} cambios, {:.2} s", repos.len(), start.elapsed().as_secs_f64());
}
