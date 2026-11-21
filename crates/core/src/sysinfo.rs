//! Datos del sistema para el pie de la ventana: estado de git de un
//! directorio y consumo del árbol de procesos de una sesión. Son consultas
//! bloqueantes, quien las use debe sacarlas del hilo de la interfaz.

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSummary {
    /// Rama actual, o el commit abreviado con la cabeza separada.
    pub branch: String,
    /// Ficheros con cambios sin confirmar, incluidos los no seguidos.
    pub changes: usize,
    pub ahead: u32,
    pub behind: u32,
}

/// Estado de git de `dir`, o `None` si no es un repositorio o no hay git.
pub fn git_summary(dir: &Path) -> Option<GitSummary> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain=v2", "--branch"])
        // Consultar no debe tomar bloqueos que estorben al git del usuario.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_git_status(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_git_status(text: &str) -> Option<GitSummary> {
    let mut head = None;
    let mut oid = None;
    let (mut ahead, mut behind, mut changes) = (0, 0, 0);
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("# branch.head ") {
            head = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("# branch.oid ") {
            oid = Some(value.to_string());
        } else if let Some(value) = line.strip_prefix("# branch.ab ") {
            for part in value.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    behind = n.parse().unwrap_or(0);
                }
            }
        } else if !line.starts_with('#') && !line.is_empty() {
            changes += 1;
        }
    }
    let branch = match head? {
        detached if detached == "(detached)" => {
            let oid = oid.unwrap_or_default();
            format!("@{}", oid.get(..7).unwrap_or(&oid))
        }
        name => name,
    };
    Some(GitSummary {
        branch,
        changes,
        ahead,
        behind,
    })
}

/// Consumo acumulado de un proceso y todos sus descendientes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub rss_bytes: u64,
    /// Tiempo de CPU consumido desde que arrancaron, en ticks del reloj.
    pub cpu_ticks: u64,
    pub processes: usize,
}

struct ProcStat {
    parent: u32,
    ticks: u64,
    rss_pages: u64,
}

/// Lee `/proc` una vez y suma el árbol que cuelga de `root`.
pub fn tree_usage(root: u32) -> Option<Usage> {
    let mut stats: HashMap<u32, ProcStat> = HashMap::new();
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        // Un proceso puede desaparecer entre el listado y la lectura.
        if let Some(stat) = std::fs::read_to_string(entry.path().join("stat"))
            .ok()
            .and_then(|text| parse_stat(&text))
        {
            stats.insert(pid, stat);
        }
    }
    sum_tree(&stats, root, page_size())
}

fn sum_tree(stats: &HashMap<u32, ProcStat>, root: u32, page: u64) -> Option<Usage> {
    stats.get(&root)?;
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, stat) in stats {
        children.entry(stat.parent).or_default().push(*pid);
    }
    let mut usage = Usage {
        rss_bytes: 0,
        cpu_ticks: 0,
        processes: 0,
    };
    let mut pending = vec![root];
    while let Some(pid) = pending.pop() {
        // Un mismo pid no puede aparecer dos veces: cada uno tiene un padre.
        if let Some(stat) = stats.get(&pid) {
            usage.rss_bytes += stat.rss_pages * page;
            usage.cpu_ticks += stat.ticks;
            usage.processes += 1;
            if let Some(kids) = children.get(&pid) {
                pending.extend(kids.iter().copied().filter(|kid| *kid != pid));
            }
        }
    }
    Some(usage)
}

/// Campos de `/proc/<pid>/stat`. El nombre va entre paréntesis y puede
/// contener espacios y paréntesis, así que se cuenta desde el último cierre.
fn parse_stat(text: &str) -> Option<ProcStat> {
    let rest = &text[text.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // Tras el nombre: estado(0) ppid(1) ... utime(11) stime(12) ... rss(21).
    Some(ProcStat {
        parent: fields.get(1)?.parse().ok()?,
        ticks: fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?,
        rss_pages: fields.get(21)?.parse().ok()?,
    })
}

pub fn ticks_per_second() -> u64 {
    // SAFETY: sysconf solo lee una constante de configuración del sistema.
    let value = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(value).ok().filter(|v| *v > 0).unwrap_or(100)
}

fn page_size() -> u64 {
    // SAFETY: sysconf solo lee una constante de configuración del sistema.
    let value = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    u64::try_from(value).ok().filter(|v| *v > 0).unwrap_or(4096)
}

/// Porcentaje de un núcleo usado entre dos muestras.
pub fn cpu_percent(before: u64, after: u64, seconds: f64) -> Option<f64> {
    if seconds <= 0.0 || after < before {
        return None;
    }
    Some((after - before) as f64 / ticks_per_second() as f64 / seconds * 100.0)
}

/// Lo que consume la propia aplicación.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfUsage {
    /// Memoria proporcional del proceso: la suya más su parte de las
    /// bibliotecas que comparte con otros programas. Es la cifra que mejor
    /// dice lo que cuesta, porque la residente cuenta entera cada biblioteca.
    pub pss_bytes: u64,
    /// Memoria residente del proceso y de sus hijos.
    pub rss_bytes: u64,
    /// Tiempo de CPU del proceso y de sus hijos, en ticks del reloj.
    pub cpu_ticks: u64,
    /// El proceso y sus hijos, que son los clientes de terminal.
    pub processes: usize,
}

/// Consumo de este proceso y de sus hijos.
pub fn self_usage() -> Option<SelfUsage> {
    let tree = tree_usage(std::process::id())?;
    let rollup = std::fs::read_to_string("/proc/self/smaps_rollup").ok();
    Some(SelfUsage {
        pss_bytes: rollup.as_deref().and_then(parse_pss).unwrap_or(tree.rss_bytes),
        rss_bytes: tree.rss_bytes,
        cpu_ticks: tree.cpu_ticks,
        processes: tree.processes,
    })
}

/// Memoria proporcional, en bytes, de un `smaps_rollup`.
pub fn parse_pss(rollup: &str) -> Option<u64> {
    let line = rollup.lines().find(|l| l.starts_with("Pss:"))?;
    let kilobytes: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kilobytes * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pss_is_read_from_the_rollup() {
        let text = "55d0-7ffd ---p 00000000 00:00 0 [rollup]\nRss:  152340 kB\nPss:   61024 kB\nPss_Dirty: 30000 kB\n";
        assert_eq!(parse_pss(text), Some(61024 * 1024));
        assert_eq!(parse_pss("Rss: 10 kB\n"), None);
        assert_eq!(parse_pss("Pss: mucho kB\n"), None);
    }

    #[test]
    fn own_usage_counts_at_least_this_process() {
        let usage = self_usage().expect("consumo propio");
        assert!(usage.processes >= 1);
        assert!(usage.pss_bytes > 0 && usage.rss_bytes > 0);
    }

    #[test]
    fn git_status_with_branch_and_changes() {
        let text = "# branch.oid 0123456789abcdef\n\
                    # branch.head main\n\
                    # branch.upstream origin/main\n\
                    # branch.ab +2 -1\n\
                    1 .M N... 100644 100644 100644 aaa bbb src/lib.rs\n\
                    ? nuevo.txt\n";
        assert_eq!(
            parse_git_status(text),
            Some(GitSummary {
                branch: "main".into(),
                changes: 2,
                ahead: 2,
                behind: 1,
            })
        );
    }

    #[test]
    fn git_status_detached_and_clean() {
        let text = "# branch.oid 0123456789abcdef\n# branch.head (detached)\n";
        let summary = parse_git_status(text).expect("resumen");
        assert_eq!(summary.branch, "@0123456");
        assert_eq!(summary.changes, 0);
        assert_eq!(parse_git_status(""), None);
    }

    #[test]
    fn git_summary_outside_a_repository_is_none() {
        assert_eq!(git_summary(Path::new("/proc")), None);
    }

    #[test]
    fn stat_line_with_awkward_process_name() {
        let line = "4242 (tmux: server (1)) S 77 4242 4242 0 -1 4194304 100 0 0 0 30 12 0 0 20 0 1 0 5000 1000000 250 18446744073709551615 0 0 0";
        let stat = parse_stat(line).expect("stat");
        assert_eq!(stat.parent, 77);
        assert_eq!(stat.ticks, 42);
        assert_eq!(stat.rss_pages, 250);
        assert!(parse_stat("sin formato").is_none());
    }

    #[test]
    fn tree_sums_descendants_only() {
        let stat = |parent, ticks, rss_pages| ProcStat {
            parent,
            ticks,
            rss_pages,
        };
        let stats = HashMap::from([
            (10, stat(1, 5, 100)),
            (11, stat(10, 7, 50)),
            (12, stat(11, 1, 10)),
            (20, stat(1, 1000, 1000)),
        ]);
        let usage = sum_tree(&stats, 10, 4096).expect("uso");
        assert_eq!(usage.processes, 3);
        assert_eq!(usage.cpu_ticks, 13);
        assert_eq!(usage.rss_bytes, 160 * 4096);
        assert!(sum_tree(&stats, 99, 4096).is_none());
    }

    #[test]
    fn own_process_tree_is_measurable() {
        let usage = tree_usage(std::process::id()).expect("uso propio");
        assert!(usage.processes >= 1);
        assert!(usage.rss_bytes > 0);
        assert_eq!(cpu_percent(100, 100 + ticks_per_second(), 1.0), Some(100.0));
        assert_eq!(cpu_percent(10, 5, 1.0), None);
    }
}
