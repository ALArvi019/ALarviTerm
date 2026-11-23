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

/// Contadores del equipo entero en un instante. Los de CPU y de red son
/// acumulados: el uso sale de comparar dos muestras.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SystemSample {
    /// Tiempo de CPU ocupado y total, en ticks, sumando todos los núcleos.
    pub cpu_busy: u64,
    pub cpu_total: u64,
    pub mem_total: u64,
    /// Memoria que una aplicación nueva podría usar sin recurrir al intercambio.
    pub mem_available: u64,
    pub swap_total: u64,
    pub swap_free: u64,
    /// Bytes recibidos y enviados por las interfaces de red físicas.
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    /// Carga media del último minuto.
    pub load: f64,
    pub cores: usize,
}

impl SystemSample {
    pub fn mem_used(&self) -> u64 {
        self.mem_total.saturating_sub(self.mem_available)
    }

    pub fn mem_percent(&self) -> f64 {
        if self.mem_total == 0 {
            return 0.0;
        }
        self.mem_used() as f64 / self.mem_total as f64 * 100.0
    }

    pub fn swap_used(&self) -> u64 {
        self.swap_total.saturating_sub(self.swap_free)
    }

    /// Porcentaje de CPU del equipo usado desde `before`, de 0 a 100.
    pub fn cpu_percent_since(&self, before: &Self) -> Option<f64> {
        let total = self.cpu_total.checked_sub(before.cpu_total).filter(|t| *t > 0)?;
        let busy = self.cpu_busy.checked_sub(before.cpu_busy)?;
        Some((busy as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
    }

    /// Bytes por segundo recibidos y enviados desde `before`.
    pub fn net_rates_since(&self, before: &Self, seconds: f64) -> Option<(f64, f64)> {
        if seconds <= 0.0 {
            return None;
        }
        // Un contador que retrocede es una interfaz que desapareció.
        let rx = self.rx_bytes.checked_sub(before.rx_bytes)?;
        let tx = self.tx_bytes.checked_sub(before.tx_bytes)?;
        Some((rx as f64 / seconds, tx as f64 / seconds))
    }
}

/// Lee los contadores del equipo en `/proc`. Son tres ficheros pequeños que
/// el núcleo genera al vuelo, sin acceso a disco.
pub fn system_sample() -> Option<SystemSample> {
    let (cpu_busy, cpu_total, cores) = parse_cpu(&std::fs::read_to_string("/proc/stat").ok()?)?;
    let memory = parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)?;
    let (rx_bytes, tx_bytes) = std::fs::read_to_string("/proc/net/dev")
        .map(|text| {
            parse_net_dev(&text, |name| Path::new("/sys/class/net").join(name).join("device").exists())
        })
        .unwrap_or_default();
    let load = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|text| text.split_whitespace().next()?.parse().ok())
        .unwrap_or(0.0);
    Some(SystemSample { cpu_busy, cpu_total, rx_bytes, tx_bytes, load, cores, ..memory })
}

/// Ticks ocupados y totales de la línea global de `/proc/stat`, y cuántos
/// núcleos lista.
pub fn parse_cpu(stat: &str) -> Option<(u64, u64, usize)> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let fields: Vec<u64> = line.split_whitespace().skip(1).filter_map(|f| f.parse().ok()).collect();
    if fields.len() < 4 {
        return None;
    }
    // user nice system idle iowait irq softirq steal. Los dos últimos campos,
    // de invitados, ya están contados dentro de user y nice.
    let total: u64 = fields.iter().take(8).sum();
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    let cores = stat
        .lines()
        .filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(u8::is_ascii_digit))
        .count();
    Some((total.saturating_sub(idle), total, cores.max(1)))
}

/// Memoria e intercambio de `/proc/meminfo`, en bytes.
pub fn parse_meminfo(text: &str) -> Option<SystemSample> {
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(':'))
            .and_then(|rest| rest.split_whitespace().next()?.parse::<u64>().ok())
            .map(|kb| kb * 1024)
    };
    Some(SystemSample {
        mem_total: field("MemTotal")?,
        mem_available: field("MemAvailable")?,
        swap_total: field("SwapTotal").unwrap_or(0),
        swap_free: field("SwapFree").unwrap_or(0),
        ..SystemSample::default()
    })
}

/// Bytes recibidos y enviados en `/proc/net/dev`, sumando las interfaces que
/// `wanted` acepta. Sirve para dejar fuera la de bucle local y las virtuales,
/// que repetirían el tráfico que ya pasa por la física.
pub fn parse_net_dev(text: &str, wanted: impl Fn(&str) -> bool) -> (u64, u64) {
    text.lines()
        .filter_map(|line| {
            let (name, counters) = line.split_once(':')?;
            let name = name.trim();
            let fields: Vec<&str> = counters.split_whitespace().collect();
            let (rx, tx) = (fields.first()?.parse::<u64>().ok()?, fields.get(8)?.parse::<u64>().ok()?);
            wanted(name).then_some((rx, tx))
        })
        .fold((0, 0), |(rx, tx), (r, t)| (rx + r, tx + t))
}

/// Cantidad de bytes en pocas cifras: `512 B`, `34 KB`, `1.2 MB`, `15.6 GB`.
pub fn compact_bytes(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes.max(0.0);
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit < 2 || value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Un proceso en un instante: nombre corto, CPU acumulada y memoria residente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessSample {
    pub name: String,
    pub cpu_ticks: u64,
    pub rss_bytes: u64,
}

/// Todos los procesos del equipo, por su identificador.
pub type ProcessTable = HashMap<u32, ProcessSample>;

/// Lee `/proc` una vez. Los hilos del núcleo, sin memoria propia, se dejan fuera.
pub fn process_table() -> ProcessTable {
    let page = page_size();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return ProcessTable::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            let text = std::fs::read_to_string(entry.path().join("stat")).ok()?;
            let sample = parse_process(&text, page)?;
            (sample.rss_bytes > 0).then_some((pid, sample))
        })
        .collect()
}

fn parse_process(text: &str, page: u64) -> Option<ProcessSample> {
    let stat = parse_stat(text)?;
    let name = text.get(text.find('(')? + 1..text.rfind(')')?)?.to_string();
    Some(ProcessSample { name, cpu_ticks: stat.ticks, rss_bytes: stat.rss_pages * page })
}

/// Programas que más consumen. Los procesos de un mismo programa van juntos.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TopProcesses {
    /// Por CPU, de más a menos. Vacío si aún no hay muestra anterior.
    pub by_cpu: Vec<ProcessRow>,
    /// Por memoria residente, de más a menos.
    pub by_memory: Vec<ProcessRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessRow {
    pub name: String,
    /// Cuántos procesos de ese programa hay.
    pub count: usize,
    /// Porcentaje de un núcleo desde la muestra anterior.
    pub cpu_percent: f64,
    pub rss_bytes: u64,
}

/// Agrupa la tabla por programa y se queda con los `limit` primeros de cada
/// clasificación. `before` es la tabla anterior y los segundos transcurridos:
/// sin ella no se puede saber la CPU.
pub fn top_processes(before: Option<(&ProcessTable, f64)>, now: &ProcessTable, limit: usize) -> TopProcesses {
    let ticks = ticks_per_second() as f64;
    let mut programs: HashMap<&str, ProcessRow> = HashMap::new();
    for (pid, sample) in now {
        let row = programs.entry(sample.name.as_str()).or_insert_with(|| ProcessRow {
            name: sample.name.clone(),
            count: 0,
            cpu_percent: 0.0,
            rss_bytes: 0,
        });
        row.count += 1;
        row.rss_bytes += sample.rss_bytes;
        if let Some((table, seconds)) = before.filter(|(_, seconds)| *seconds > 0.0) {
            // Un proceso nuevo, o un pid reutilizado, no tiene referencia.
            let earlier = table.get(pid).filter(|p| p.name == sample.name && p.cpu_ticks <= sample.cpu_ticks);
            if let Some(earlier) = earlier {
                row.cpu_percent += (sample.cpu_ticks - earlier.cpu_ticks) as f64 / ticks / seconds * 100.0;
            }
        }
    }
    let rows: Vec<ProcessRow> = programs.into_values().collect();
    let ranked = |key: fn(&ProcessRow) -> f64, keep: fn(&ProcessRow) -> bool| {
        let mut sorted: Vec<ProcessRow> = rows.iter().filter(|r| keep(r)).cloned().collect();
        sorted.sort_by(|a, b| key(b).total_cmp(&key(a)).then_with(|| a.name.cmp(&b.name)));
        sorted.truncate(limit);
        sorted
    };
    TopProcesses {
        by_cpu: match before {
            Some(_) => ranked(|r| r.cpu_percent, |r| r.cpu_percent >= 0.05),
            None => Vec::new(),
        },
        by_memory: ranked(|r| r.rss_bytes as f64, |_| true),
    }
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

    #[test]
    fn reads_the_machine_counters() {
        let stat = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 50 0 25 400 25 0 0 0 0 0\ncpu1 50 0 25 400 25 0 0 0 0 0\nintr 5\n";
        assert_eq!(parse_cpu(stat), Some((150, 1000, 2)));
        assert_eq!(parse_cpu("intr 5"), None);

        let meminfo = "MemTotal:       16000 kB\nMemFree:         1000 kB\nMemAvailable:    4000 kB\nSwapTotal:       2000 kB\nSwapFree:        1500 kB\n";
        let memory = parse_meminfo(meminfo).expect("memoria");
        assert_eq!(memory.mem_used(), 12_000 * 1024);
        assert_eq!(memory.mem_percent(), 75.0);
        assert_eq!(memory.swap_used(), 500 * 1024);
        assert!(parse_meminfo("MemFree: 1 kB").is_none());

        let net = "Inter-|   Receive                                                |  Transmit\n face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n    lo: 900 9 0 0 0 0 0 0 900 9 0 0 0 0 0 0\n  wlp3s0: 5000 50 0 0 0 0 0 0 700 7 0 0 0 0 0 0\ndocker0: 40 1 0 0 0 0 0 0 60 1 0 0 0 0 0 0\n  eth0: 100 1 0 0 0 0 0 0 20 1 0 0 0 0 0 0\n";
        assert_eq!(parse_net_dev(net, |name| name == "wlp3s0" || name == "eth0"), (5100, 720));
        assert_eq!(parse_net_dev(net, |_| false), (0, 0));
    }

    #[test]
    fn usage_comes_from_two_samples() {
        let before = SystemSample { cpu_busy: 100, cpu_total: 1000, rx_bytes: 1000, tx_bytes: 500, ..Default::default() };
        let after = SystemSample { cpu_busy: 150, cpu_total: 1200, rx_bytes: 5000, tx_bytes: 1500, ..Default::default() };
        assert_eq!(after.cpu_percent_since(&before), Some(25.0));
        assert_eq!(after.net_rates_since(&before, 2.0), Some((2000.0, 500.0)));
        assert_eq!(before.cpu_percent_since(&after), None);
        assert_eq!(before.net_rates_since(&after, 2.0), None);
        assert_eq!(after.cpu_percent_since(&after), None);

        assert_eq!(compact_bytes(512.0), "512 B");
        assert_eq!(compact_bytes(34.4 * 1024.0), "34 KB");
        assert_eq!(compact_bytes(1.25 * 1024.0 * 1024.0), "1.2 MB");
        assert_eq!(compact_bytes(15.6 * 1024.0 * 1024.0 * 1024.0), "15.6 GB");
        assert_eq!(compact_bytes(250.0 * 1024.0 * 1024.0), "250 MB");
    }

    #[test]
    fn top_processes_are_grouped_by_program() {
        let sample = |name: &str, cpu_ticks: u64, mb: u64| ProcessSample {
            name: name.into(),
            cpu_ticks,
            rss_bytes: mb * 1024 * 1024,
        };
        let parsed = parse_process("77 (mi (raro) prog) S 1 2 3 4 5 6 7 8 9 10 30 20 0 0 0 0 0 0 0 0 250 0", 4096)
            .expect("proceso");
        assert_eq!(parsed, ProcessSample { name: "mi (raro) prog".into(), cpu_ticks: 50, rss_bytes: 250 * 4096 });

        let before: ProcessTable =
            [(1, sample("claude", 100, 300)), (2, sample("claude", 100, 200)), (3, sample("zsh", 10, 5))].into();
        let tick = ticks_per_second();
        let now: ProcessTable = [
            (1, sample("claude", 100 + tick, 300)),
            (2, sample("claude", 100 + tick / 2, 200)),
            (3, sample("zsh", 10, 5)),
            (4, sample("nuevo", 9_999, 900)),
        ]
        .into();
        let top = top_processes(Some((&before, 2.0)), &now, 2);
        assert_eq!(top.by_cpu.len(), 1, "solo quien ha gastado CPU");
        assert_eq!((top.by_cpu[0].name.as_str(), top.by_cpu[0].count), ("claude", 2));
        assert!((top.by_cpu[0].cpu_percent - 75.0).abs() < 1.5);
        let memory: Vec<(&str, u64)> = top.by_memory.iter().map(|r| (r.name.as_str(), r.rss_bytes >> 20)).collect();
        assert_eq!(memory, vec![("nuevo", 900), ("claude", 500)]);
        assert!(top_processes(None, &now, 5).by_cpu.is_empty());
        assert!(!process_table().is_empty());
    }
}
