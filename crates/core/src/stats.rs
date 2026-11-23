//! Estadísticas de uso de Claude Code a partir de sus transcripciones.
//!
//! Claude Code guarda cada conversación en un fichero `.jsonl` dentro de su
//! carpeta de configuración. Cada respuesta del modelo lleva los tokens que
//! consumió, el modelo, la hora y la carpeta de trabajo. Aquí se leen esos
//! ficheros, solo para leer, y se agregan por día, por hora de la semana, por
//! modelo y por proyecto.
//!
//! Los ficheros suman gigabytes, así que se leen en varios hilos, se analizan
//! solo las líneas que interesan y el resultado de cada fichero se guarda en
//! una caché: un fichero que no ha cambiado no se vuelve a leer.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use memchr::memmem;
use serde_json::{json, Value};

/// Versión del formato de la caché. Cambiarla la invalida.
const CACHE_VERSION: u64 = 1;
/// Hueco máximo entre dos eventos de una sesión que aún cuenta como trabajo
/// seguido al estimar el tiempo activo.
const ACTIVE_GAP: i64 = 300;
/// Por encima de este tamaño una línea de usuario no se analiza entera.
const BIG_LINE: usize = 64 * 1024;

/// Una respuesta del modelo con su consumo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// Segundos Unix.
    pub ts: i64,
    /// Huella del mensaje, para no contarlo dos veces.
    pub key: u64,
    pub session: u64,
    /// Índices en las tablas de nombres del fichero.
    pub model: u32,
    pub project: u32,
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

/// Un mensaje escrito por el usuario: el comienzo de un turno.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prompt {
    pub ts: i64,
    pub key: u64,
    pub session: u64,
}

/// Lo que se extrae de un fichero de transcripción.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileData {
    pub size: u64,
    pub mtime: i64,
    pub models: Vec<String>,
    pub projects: Vec<String>,
    pub events: Vec<Event>,
    pub prompts: Vec<Prompt>,
}

/// FNV-1a de 64 bits. Estable entre versiones, que es lo que pide la caché.
fn fnv(parts: &[&str]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for byte in part.bytes().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

/// Segundos Unix de una marca `2026-11-21T20:27:01.123Z`. Claude Code las
/// escribe siempre en UTC.
pub fn parse_timestamp(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let number = |range: std::ops::Range<usize>| text.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(5..7)?, number(8..10)?);
    let (hour, minute, second) = (number(11..13)?, number(14..16)?, number(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    // Días desde la época con el calendario gregoriano proléptico.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

fn intern(table: &mut Vec<String>, name: &str) -> u32 {
    match table.iter().position(|n| n == name) {
        Some(index) => index as u32,
        None => {
            table.push(name.to_string());
            (table.len() - 1) as u32
        }
    }
}

/// Valor de una clave de texto sin analizar la línea entera. Se queda con la
/// última aparición, que en estas líneas es la del nivel superior.
fn quick_text<'a>(line: &'a [u8], key: &str) -> Option<&'a str> {
    let pattern = format!("\"{key}\":\"");
    let start = memmem::rfind(line, pattern.as_bytes())? + pattern.len();
    let end = start + memchr::memchr(b'"', &line[start..])?;
    std::str::from_utf8(&line[start..end]).ok()
}

impl FileData {
    /// Lee una transcripción línea a línea.
    pub fn read(path: &Path) -> std::io::Result<Self> {
        let file = std::fs::File::open(path)?;
        let meta = file.metadata()?;
        let mut data = Self {
            size: meta.len(),
            mtime: mtime_of(&meta),
            ..Self::default()
        };
        let assistant = memmem::Finder::new(b"\"type\":\"assistant\"");
        let user = memmem::Finder::new(b"\"type\":\"user\"");
        let mut seen = HashSet::new();
        let mut seen_prompts = HashSet::new();
        let mut reader = BufReader::with_capacity(256 * 1024, file);
        let mut line = Vec::new();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line)? == 0 {
                break;
            }
            if assistant.find(&line).is_some() {
                data.take_assistant(&line, &mut seen);
            } else if user.find(&line).is_some() {
                data.take_user(&line, &mut seen_prompts);
            }
        }
        Ok(data)
    }

    fn take_assistant(&mut self, line: &[u8], seen: &mut HashSet<u64>) -> Option<()> {
        let value: Value = serde_json::from_slice(line).ok()?;
        if value.get("type")?.as_str()? != "assistant" {
            return None;
        }
        let message = value.get("message")?;
        let usage = message.get("usage")?;
        let count = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
        let (input, output) = (count("input_tokens"), count("output_tokens"));
        let (cache_read, cache_write) = (count("cache_read_input_tokens"), count("cache_creation_input_tokens"));
        if input + output + cache_read + cache_write == 0 {
            return None;
        }
        let model = message.get("model").and_then(Value::as_str).unwrap_or("desconocido");
        if model.starts_with('<') {
            // Mensajes que escribe el propio programa, sin llamada al modelo.
            return None;
        }
        let text = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or("");
        // Una respuesta se escribe en varias líneas, una por bloque, todas con
        // el mismo identificador y el mismo consumo. Cuenta una sola vez.
        let id = message.get("id").and_then(Value::as_str).unwrap_or("");
        let key = if id.is_empty() {
            fnv(&[text("uuid")])
        } else {
            fnv(&[id, text("requestId")])
        };
        if !seen.insert(key) {
            return None;
        }
        let event = Event {
            ts: parse_timestamp(text("timestamp"))?,
            key,
            session: fnv(&[text("sessionId")]),
            model: intern(&mut self.models, model),
            project: intern(&mut self.projects, text("cwd")),
            input,
            cache_read,
            cache_write,
            output,
        };
        self.events.push(event);
        Some(())
    }

    fn take_user(&mut self, line: &[u8], seen: &mut HashSet<u64>) -> Option<()> {
        let (prompt, session, timestamp) = if line.len() > BIG_LINE {
            // Resultados de herramientas enormes: basta con lo imprescindible.
            (
                quick_text(line, "promptId")?.to_string(),
                quick_text(line, "sessionId").unwrap_or("").to_string(),
                quick_text(line, "timestamp")?.to_string(),
            )
        } else {
            let value: Value = serde_json::from_slice(line).ok()?;
            if value.get("type")?.as_str()? != "user" || value.get("isMeta").and_then(Value::as_bool) == Some(true) {
                return None;
            }
            let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
            (text("promptId")?, text("sessionId").unwrap_or_default(), text("timestamp")?)
        };
        // Todas las líneas de un turno comparten identificador de petición.
        let key = fnv(&[&prompt]);
        if !seen.insert(key) {
            return None;
        }
        self.prompts.push(Prompt {
            ts: parse_timestamp(&timestamp)?,
            key,
            session: fnv(&[&session]),
        });
        Some(())
    }

    fn to_json(&self, path: &Path) -> Value {
        json!({
            "p": path.to_string_lossy(),
            "s": self.size,
            "m": self.mtime,
            "mo": self.models,
            "pr": self.projects,
            "e": self.events.iter().map(|e| json!([
                e.ts, e.key, e.session, e.model, e.project, e.input, e.cache_read, e.cache_write, e.output
            ])).collect::<Vec<_>>(),
            "q": self.prompts.iter().map(|p| json!([p.ts, p.key, p.session])).collect::<Vec<_>>(),
        })
    }

    fn from_json(value: &Value) -> Option<(PathBuf, Self)> {
        let names = |key: &str| -> Option<Vec<String>> {
            value.get(key)?.as_array()?.iter().map(|n| n.as_str().map(str::to_string)).collect()
        };
        let events = value
            .get("e")?
            .as_array()?
            .iter()
            .map(|row| {
                let n = |i: usize| row.get(i).and_then(Value::as_u64);
                Some(Event {
                    ts: row.get(0)?.as_i64()?,
                    key: n(1)?,
                    session: n(2)?,
                    model: n(3)? as u32,
                    project: n(4)? as u32,
                    input: n(5)?,
                    cache_read: n(6)?,
                    cache_write: n(7)?,
                    output: n(8)?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let prompts = value
            .get("q")?
            .as_array()?
            .iter()
            .map(|row| {
                Some(Prompt {
                    ts: row.get(0)?.as_i64()?,
                    key: row.get(1)?.as_u64()?,
                    session: row.get(2)?.as_u64()?,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let data = Self {
            size: value.get("s")?.as_u64()?,
            mtime: value.get("m")?.as_i64()?,
            models: names("mo")?,
            projects: names("pr")?,
            events,
            prompts,
        };
        // Una caché dañada no debe hacer que un índice se salga de su tabla.
        let fits = data
            .events
            .iter()
            .all(|e| (e.model as usize) < data.models.len() && (e.project as usize) < data.projects.len());
        fits.then(|| (PathBuf::from(value.get("p").and_then(Value::as_str).unwrap_or("")), data))
    }
}

fn mtime_of(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// Carpeta de transcripciones de Claude Code.
pub fn transcripts_dir() -> Option<PathBuf> {
    crate::limits::claude_config_dir().map(|dir| dir.join("projects"))
}

/// Ficheros `.jsonl` bajo una carpeta, con su tamaño y fecha de cambio.
fn list_transcripts(root: &Path) -> Vec<(PathBuf, u64, i64)> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            // Sin seguir enlaces: metadata del enlace, no de su destino.
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                if depth < 6 {
                    pending.push((path, depth + 1));
                }
            } else if meta.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
                found.push((path, meta.len(), mtime_of(&meta)));
            }
        }
    }
    found
}

/// Todo lo leído hasta ahora, por fichero.
#[derive(Debug, Default)]
pub struct Library {
    files: HashMap<PathBuf, FileData>,
}

/// Qué hizo una actualización de la biblioteca.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Refresh {
    /// Ficheros dentro del periodo pedido.
    pub files: usize,
    /// Los que hubo que leer porque eran nuevos o habían cambiado.
    pub read: usize,
    pub bytes_read: u64,
}

impl Library {
    /// Carga la caché. Si falta, está dañada o es de otra versión, empieza
    /// vacía y se reconstruye sola.
    pub fn load(cache: &Path) -> Self {
        let load = || -> Option<Self> {
            let value: Value = serde_json::from_slice(&std::fs::read(cache).ok()?).ok()?;
            if value.get("version")?.as_u64()? != CACHE_VERSION {
                return None;
            }
            let files = value.get("files")?.as_array()?.iter().filter_map(FileData::from_json).collect();
            Some(Self { files })
        };
        load().unwrap_or_default()
    }

    pub fn save(&self, cache: &Path) -> std::io::Result<()> {
        let files: Vec<Value> = self.files.iter().map(|(path, data)| data.to_json(path)).collect();
        let text = json!({ "version": CACHE_VERSION, "files": files }).to_string();
        let temporary = cache.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, cache)
    }

    /// Pone al día los ficheros cambiados desde `since` (segundos Unix; 0 es
    /// todo). Los que no han cambiado de tamaño ni de fecha se reutilizan y
    /// los que ya no existen se olvidan.
    pub fn refresh(&mut self, root: &Path, since: i64) -> Refresh {
        let listed = list_transcripts(root);
        let existing: HashSet<&PathBuf> = listed.iter().map(|(path, _, _)| path).collect();
        self.files.retain(|path, _| existing.contains(path));

        let mut report = Refresh::default();
        let mut stale = Vec::new();
        for (path, size, mtime) in &listed {
            if *mtime < since {
                continue;
            }
            report.files += 1;
            let fresh = self.files.get(path).is_some_and(|f| f.size == *size && f.mtime == *mtime);
            if !fresh {
                report.bytes_read += size;
                stale.push(path.clone());
            }
        }
        report.read = stale.len();

        // Los ficheros grandes primero, repartidos por turnos entre los hilos.
        stale.sort_by_key(|path| std::cmp::Reverse(std::fs::metadata(path).map_or(0, |m| m.len())));
        let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 6);
        let read: Vec<(PathBuf, FileData)> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|worker| {
                    let stale = &stale;
                    scope.spawn(move || {
                        stale
                            .iter()
                            .skip(worker)
                            .step_by(workers)
                            .filter_map(|path| FileData::read(path).ok().map(|data| (path.clone(), data)))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        });
        self.files.extend(read);
        report
    }

    #[cfg(test)]
    fn insert(&mut self, path: &str, data: FileData) {
        self.files.insert(PathBuf::from(path), data);
    }
}

/// Sumas de un periodo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Totals {
    /// Tokens de entrada sin caché.
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
    /// Respuestas del modelo.
    pub messages: u64,
    /// Mensajes del usuario.
    pub turns: u64,
    pub sessions: u64,
    /// Segundos con actividad, estimados.
    pub active_secs: u64,
}

impl Totals {
    /// Toda la entrada: la nueva más la leída y la escrita en caché.
    pub fn input_total(&self) -> u64 {
        self.input + self.cache_read + self.cache_write
    }

    /// Parte de la entrada que salió de la caché, de 0 a 100.
    pub fn cache_percent(&self) -> Option<u32> {
        let total = self.input_total();
        (total > 0).then(|| (self.cache_read as f64 * 100.0 / total as f64).round() as u32)
    }
}

/// Un día del periodo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Day {
    /// Segundos Unix de un instante de ese día, para ponerle fecha.
    pub ts: i64,
    pub input: u64,
    pub output: u64,
    pub messages: u64,
    pub turns: u64,
    pub active_secs: u64,
}

/// Una fila de un desglose, por modelo o por proyecto.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Row {
    pub name: String,
    pub input: u64,
    pub output: u64,
    pub messages: u64,
}

/// Todo lo que enseña el panel para un periodo.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Summary {
    pub totals: Totals,
    /// El periodo anterior de la misma duración, para comparar. `None` si el
    /// periodo es todo el historial.
    pub previous: Option<Totals>,
    /// Un elemento por día, del más antiguo al más reciente, sin huecos.
    pub days: Vec<Day>,
    /// Respuestas por día de la semana (0 es lunes) y hora local.
    pub heat: [[u64; 24]; 7],
    /// De mayor a menor entrada.
    pub models: Vec<Row>,
    pub projects: Vec<Row>,
}

impl Summary {
    pub fn active_days(&self) -> usize {
        self.days.iter().filter(|d| d.messages > 0).count()
    }
}

/// Día local (contado desde la época), día de la semana con el lunes en 0 y
/// hora de un instante.
pub type LocalTime = fn(i64) -> (i64, usize, usize);

/// Hora local del sistema.
pub fn system_local(ts: i64) -> (i64, usize, usize) {
    // SAFETY: localtime_r solo escribe en la estructura que se le pasa.
    let tm = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        let time = ts as libc::time_t;
        if libc::localtime_r(&time, &mut tm).is_null() {
            return utc_local(ts);
        }
        tm
    };
    let day = (ts + tm.tm_gmtoff as i64).div_euclid(86_400);
    (day, ((tm.tm_wday + 6) % 7) as usize, tm.tm_hour.clamp(0, 23) as usize)
}

/// Lo mismo en UTC. Sirve de reserva y para las pruebas.
pub fn utc_local(ts: i64) -> (i64, usize, usize) {
    let day = ts.div_euclid(86_400);
    // El 1 de enero de 1970 fue jueves.
    ((day), ((day + 3).rem_euclid(7)) as usize, (ts.rem_euclid(86_400) / 3600) as usize)
}

/// Si la carpeta de trabajo `cwd` es `root` o está dentro de ella.
pub fn is_under(cwd: &str, root: &Path) -> bool {
    Path::new(cwd).starts_with(root)
}

/// Segundos con actividad: la suma de los huecos cortos entre eventos
/// seguidos de una misma sesión. Es una estimación, no una medida.
fn active_seconds(stamps: &mut [(u64, i64)], mut add: impl FnMut(i64, u64)) {
    stamps.sort_unstable();
    for pair in stamps.windows(2) {
        let ((session_a, a), (session_b, b)) = (pair[0], pair[1]);
        if session_a == session_b && b - a <= ACTIVE_GAP {
            add(a, (b - a) as u64);
        }
    }
}

impl Library {
    /// Resume el periodo `[start, end)`. Con `start` en `None` cubre todo el
    /// historial, desde el primer día con datos.
    pub fn summarize(&self, start: Option<i64>, end: i64, local: LocalTime) -> Summary {
        self.summarize_where(start, end, local, &|_| true)
    }

    /// Como [`Library::summarize`], contando solo el trabajo hecho en las
    /// carpetas que `keep` acepta. Los turnos se atribuyen a la carpeta de la
    /// sesión a la que pertenecen.
    pub fn summarize_where(
        &self,
        start: Option<i64>,
        end: i64,
        local: LocalTime,
        keep: &dyn Fn(&str) -> bool,
    ) -> Summary {
        let previous_start = start.map(|s| s - (end - s));
        let mut seen = HashSet::new();
        let mut seen_prompts = HashSet::new();
        let mut totals = Totals::default();
        let mut before = Totals::default();
        let mut days: HashMap<i64, Day> = HashMap::new();
        let mut heat = [[0u64; 24]; 7];
        let mut models: HashMap<&str, Row> = HashMap::new();
        let mut projects: HashMap<&str, Row> = HashMap::new();
        let mut sessions = HashSet::new();
        let mut sessions_before = HashSet::new();
        let mut stamps: Vec<(u64, i64)> = Vec::new();
        let mut stamps_before: Vec<(u64, i64)> = Vec::new();
        let in_range = |ts: i64| ts < end && start.is_none_or(|s| ts >= s);
        let in_previous = |ts: i64| matches!((previous_start, start), (Some(p), Some(s)) if ts >= p && ts < s);

        for file in self.files.values() {
            let kept: Vec<bool> = file.projects.iter().map(|p| keep(p)).collect();
            let kept_sessions: HashSet<u64> = file
                .events
                .iter()
                .filter(|e| kept[e.project as usize])
                .map(|e| e.session)
                .collect();
            for event in &file.events {
                if !kept[event.project as usize] {
                    continue;
                }
                let (current, earlier) = (in_range(event.ts), in_previous(event.ts));
                // Al reanudar una conversación su historial se copia a otro
                // fichero: el mismo mensaje no debe sumar dos veces.
                if !(current || earlier) || !seen.insert(event.key) {
                    continue;
                }
                let all_input = event.input + event.cache_read + event.cache_write;
                let target = if current { &mut totals } else { &mut before };
                target.input += event.input;
                target.cache_read += event.cache_read;
                target.cache_write += event.cache_write;
                target.output += event.output;
                target.messages += 1;
                if !current {
                    sessions_before.insert(event.session);
                    stamps_before.push((event.session, event.ts));
                    continue;
                }
                sessions.insert(event.session);
                stamps.push((event.session, event.ts));
                let (day, weekday, hour) = local(event.ts);
                let entry = days.entry(day).or_default();
                entry.ts = event.ts;
                entry.input += all_input;
                entry.output += event.output;
                entry.messages += 1;
                heat[weekday][hour] += 1;
                for (table, name) in [
                    (&mut models, file.models[event.model as usize].as_str()),
                    (&mut projects, file.projects[event.project as usize].as_str()),
                ] {
                    let row = table.entry(name).or_default();
                    row.input += all_input;
                    row.output += event.output;
                    row.messages += 1;
                }
            }
            for prompt in &file.prompts {
                if !kept_sessions.contains(&prompt.session) {
                    continue;
                }
                let (current, earlier) = (in_range(prompt.ts), in_previous(prompt.ts));
                if !(current || earlier) || !seen_prompts.insert(prompt.key) {
                    continue;
                }
                if current {
                    totals.turns += 1;
                    stamps.push((prompt.session, prompt.ts));
                    let entry = days.entry(local(prompt.ts).0).or_default();
                    entry.ts = prompt.ts;
                    entry.turns += 1;
                } else {
                    before.turns += 1;
                    stamps_before.push((prompt.session, prompt.ts));
                }
            }
        }

        totals.sessions = sessions.len() as u64;
        before.sessions = sessions_before.len() as u64;
        active_seconds(&mut stamps, |ts, secs| {
            totals.active_secs += secs;
            days.entry(local(ts).0).or_default().active_secs += secs;
        });
        active_seconds(&mut stamps_before, |_, secs| before.active_secs += secs);

        // Días seguidos, también los vacíos, para que la gráfica tenga escala
        // de tiempo real.
        let first = start.map(|s| local(s).0).or_else(|| days.keys().min().copied());
        let last = local(end - 1).0;
        let filled = first
            .map(|first| {
                (first..=last)
                    .map(|day| {
                        let mut entry = days.get(&day).copied().unwrap_or_default();
                        if entry.ts == 0 {
                            // Un instante cualquiera de ese día: el último menos los días que faltan.
                            entry.ts = end - 1 - (last - day) * 86_400;
                        }
                        entry
                    })
                    .collect()
            })
            .unwrap_or_default();

        let ranked = |table: HashMap<&str, Row>| {
            let mut rows: Vec<Row> = table
                .into_iter()
                .map(|(name, row)| Row { name: name.to_string(), ..row })
                .collect();
            rows.sort_by(|a, b| b.input.cmp(&a.input).then_with(|| a.name.cmp(&b.name)));
            rows
        };
        Summary {
            totals,
            previous: start.map(|_| before),
            days: filled,
            heat,
            models: ranked(models),
            projects: ranked(projects),
        }
    }
}

/// Cantidad en pocas cifras: `950`, `22,9 k`, `61,3 M`, `11,6 mil M`.
pub fn compact(value: u64) -> String {
    let scaled = |divisor: f64, suffix: &str| {
        let number = value as f64 / divisor;
        let text = if number >= 100.0 { format!("{number:.0}") } else { format!("{number:.1}") };
        format!("{} {suffix}", text.replace('.', ","))
    };
    match value {
        0..=999 => value.to_string(),
        1_000..=999_999 => scaled(1e3, "k"),
        1_000_000..=999_999_999 => scaled(1e6, "M"),
        _ => scaled(1e9, "mil M"),
    }
}

/// Duración en pocas cifras: `12 min`, `9,2 h`, `347 h`.
pub fn compact_duration(seconds: u64) -> String {
    let hours = seconds as f64 / 3600.0;
    if seconds < 3600 {
        format!("{} min", seconds / 60)
    } else if hours < 100.0 {
        format!("{hours:.1} h").replace('.', ",")
    } else {
        format!("{hours:.0} h")
    }
}

/// Cambio respecto al periodo anterior, como `+38 %`. Sin dato anterior no
/// hay con qué comparar.
pub fn change(current: u64, previous: u64) -> Option<String> {
    if previous == 0 {
        return None;
    }
    let percent = ((current as f64 - previous as f64) / previous as f64 * 100.0).round() as i64;
    Some(format!("{}{} %", if percent >= 0 { "+" } else { "−" }, percent.abs().min(999)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assistant(id: &str, session: &str, time: &str, model: &str, cwd: &str, tokens: [u64; 4]) -> String {
        json!({
            "type": "assistant", "sessionId": session, "timestamp": time, "cwd": cwd, "requestId": format!("req-{id}"),
            "uuid": format!("u-{id}"),
            "message": { "id": id, "model": model, "content": [{"type": "text", "text": "\"type\":\"user\""}],
                "usage": { "input_tokens": tokens[0], "cache_read_input_tokens": tokens[1],
                           "cache_creation_input_tokens": tokens[2], "output_tokens": tokens[3] } }
        })
        .to_string()
    }

    fn user(prompt: &str, session: &str, time: &str) -> String {
        json!({ "type": "user", "promptId": prompt, "sessionId": session, "timestamp": time,
                "message": { "role": "user", "content": "hola" } })
        .to_string()
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alarviterm-stats-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proyecto").join("abc").join("subagents")).expect("carpeta");
        dir
    }

    const DAY: i64 = 86_400;

    #[test]
    fn timestamps_become_unix_seconds() {
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_timestamp("2000-03-01T00:00:00.000Z"), Some(951_868_800));
        assert_eq!(parse_timestamp("2026-11-21T20:27:01.123Z"), Some(1_795_292_821));
        assert_eq!(parse_timestamp("ayer"), None);
        assert_eq!(parse_timestamp("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn a_transcript_is_read_once_per_message() {
        let dir = scratch("read");
        let path = dir.join("proyecto").join("a.jsonl");
        let lines = [
            user("p1", "s1", "2026-01-05T10:00:00Z"),
            // La misma respuesta en dos líneas, una por bloque de contenido.
            assistant("m1", "s1", "2026-01-05T10:00:10Z", "claude-opus-5", "/home/u/web", [10, 1000, 200, 50]),
            assistant("m1", "s1", "2026-01-05T10:00:11Z", "claude-opus-5", "/home/u/web", [10, 1000, 200, 50]),
            // Un resultado de herramienta: mismo turno, no es un mensaje nuevo.
            user("p1", "s1", "2026-01-05T10:00:20Z"),
            assistant("m2", "s1", "2026-01-05T10:00:30Z", "claude-haiku-5", "/home/u/web", [5, 0, 0, 7]),
            assistant("m3", "s1", "2026-01-05T10:00:40Z", "<synthetic>", "/home/u/web", [1, 0, 0, 1]),
            assistant("m4", "s1", "2026-01-05T10:00:50Z", "claude-opus-5", "/home/u/web", [0, 0, 0, 0]),
            json!({"type": "user", "isMeta": true, "promptId": "p9", "sessionId": "s1", "timestamp": "2026-01-05T10:01:00Z"}).to_string(),
            "esto no es json con \"type\":\"assistant\" dentro".to_string(),
            json!({"type": "system", "note": "nada"}).to_string(),
        ];
        std::fs::write(&path, lines.join("\n")).expect("transcripción");

        let data = FileData::read(&path).expect("lectura");
        assert_eq!(data.events.len(), 2);
        assert_eq!(data.prompts.len(), 1);
        assert_eq!(data.models, vec!["claude-opus-5", "claude-haiku-5"]);
        assert_eq!(data.projects, vec!["/home/u/web"]);
        assert_eq!((data.events[0].input, data.events[0].cache_read, data.events[0].output), (10, 1000, 50));
        assert_eq!(data.size, std::fs::metadata(&path).expect("meta").len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn huge_user_lines_are_read_without_parsing() {
        let dir = scratch("big");
        let path = dir.join("proyecto").join("a.jsonl");
        let big = json!({ "type": "user", "promptId": "p7", "sessionId": "s1",
            "message": { "content": [{ "type": "tool_result", "content": "x".repeat(BIG_LINE) }] },
            "timestamp": "2026-01-05T10:00:00Z" })
        .to_string();
        std::fs::write(&path, big).expect("transcripción");
        let data = FileData::read(&path).expect("lectura");
        assert_eq!(data.prompts.len(), 1);
        assert_eq!(data.prompts[0].ts, parse_timestamp("2026-01-05T10:00:00Z").expect("hora"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_reuses_unchanged_files_and_the_cache_round_trips() {
        let dir = scratch("refresh");
        let first = dir.join("proyecto").join("a.jsonl");
        let nested = dir.join("proyecto").join("abc").join("subagents").join("b.jsonl");
        std::fs::write(&first, assistant("m1", "s1", "2026-01-05T10:00:00Z", "opus", "/p", [1, 2, 3, 4])).expect("a");
        std::fs::write(&nested, assistant("m2", "s1", "2026-01-05T11:00:00Z", "opus", "/p", [1, 0, 0, 1])).expect("b");
        std::fs::write(dir.join("proyecto").join("notas.txt"), "no cuenta").expect("otro");

        let mut library = Library::default();
        let report = library.refresh(&dir, 0);
        assert_eq!((report.files, report.read), (2, 2));
        assert_eq!(library.refresh(&dir, 0).read, 0);

        let cache = dir.join("cache.json");
        library.save(&cache).expect("guardar");
        let mut loaded = Library::load(&cache);
        assert_eq!(loaded.files, library.files);
        assert_eq!(loaded.refresh(&dir, 0).read, 0);

        // Un fichero que crece se vuelve a leer y uno borrado se olvida.
        let more = format!(
            "{}\n{}",
            assistant("m1", "s1", "2026-01-05T10:00:00Z", "opus", "/p", [1, 2, 3, 4]),
            assistant("m3", "s1", "2026-01-05T10:05:00Z", "opus", "/p", [1, 0, 0, 1])
        );
        std::fs::write(&first, more).expect("a");
        std::fs::remove_file(&nested).expect("borrar");
        let report = loaded.refresh(&dir, 0);
        assert_eq!((report.files, report.read), (1, 1));
        assert_eq!(loaded.files.len(), 1);
        assert_eq!(loaded.files[&first].events.len(), 2);

        // Con una fecha de corte futura no se lee nada.
        assert_eq!(Library::default().refresh(&dir, i64::MAX).files, 0);
        std::fs::write(&cache, "{\"version\": 0}").expect("caché vieja");
        assert!(Library::load(&cache).files.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn event(key: u64, session: u64, ts: i64, model: u32, tokens: [u64; 4]) -> Event {
        Event {
            ts,
            key,
            session,
            model,
            project: 0,
            input: tokens[0],
            cache_read: tokens[1],
            cache_write: tokens[2],
            output: tokens[3],
        }
    }

    fn library() -> Library {
        let mut library = Library::default();
        // Día 10 a las 9:00 y 9:02 (sesión 1), día 11 a las 15:00 (sesión 2).
        library.insert(
            "a",
            FileData {
                models: vec!["opus".into(), "haiku".into()],
                projects: vec!["/web".into()],
                events: vec![
                    event(1, 1, 10 * DAY + 9 * 3600, 0, [10, 80, 10, 5]),
                    event(2, 1, 10 * DAY + 9 * 3600 + 120, 1, [1, 0, 0, 2]),
                    event(3, 2, 11 * DAY + 15 * 3600, 0, [20, 0, 0, 4]),
                    // Periodo anterior.
                    event(4, 3, 7 * DAY, 0, [50, 0, 0, 1]),
                ],
                prompts: vec![
                    Prompt { ts: 10 * DAY + 9 * 3600 - 60, key: 100, session: 1 },
                    Prompt { ts: 7 * DAY - 30, key: 101, session: 3 },
                ],
                ..FileData::default()
            },
        );
        // Otro fichero que repite el mensaje 1, como al reanudar una sesión.
        library.insert(
            "b",
            FileData {
                models: vec!["opus".into()],
                projects: vec!["/api".into()],
                events: vec![event(1, 1, 10 * DAY + 9 * 3600, 0, [10, 80, 10, 5])],
                ..FileData::default()
            },
        );
        library
    }

    #[test]
    fn a_period_is_summarized_with_its_previous_one() {
        let summary = library().summarize(Some(9 * DAY), 12 * DAY, utc_local);
        let totals = summary.totals;
        assert_eq!((totals.input, totals.cache_read, totals.cache_write, totals.output), (31, 80, 10, 11));
        assert_eq!(totals.input_total(), 121);
        assert_eq!(totals.cache_percent(), Some(66));
        assert_eq!((totals.messages, totals.turns, totals.sessions), (3, 1, 2));
        // Aviso, respuesta al minuto y otra dos minutos después.
        assert_eq!(totals.active_secs, 180);

        let previous = summary.previous.expect("periodo anterior");
        assert_eq!((previous.input, previous.messages, previous.turns, previous.sessions), (50, 1, 1, 1));

        assert_eq!(summary.days.len(), 3);
        assert_eq!(summary.days.iter().map(|d| d.messages).collect::<Vec<_>>(), vec![0, 2, 1]);
        assert_eq!(summary.days[1].input, 101);
        assert_eq!(summary.days[1].active_secs, 180);
        assert_eq!(utc_local(summary.days[0].ts).0, 9);
        assert_eq!(summary.active_days(), 2);

        // El día 10 desde la época es domingo y el 11 lunes.
        assert_eq!(summary.heat[6][9], 2);
        assert_eq!(summary.heat[0][15], 1);
        assert_eq!(summary.heat.iter().flatten().sum::<u64>(), 3);

        let models: Vec<(&str, u64, u64)> = summary.models.iter().map(|r| (r.name.as_str(), r.input, r.messages)).collect();
        assert_eq!(models, vec![("opus", 120, 2), ("haiku", 1, 1)]);
        assert_eq!(summary.projects.iter().map(|r| r.messages).sum::<u64>(), 3);
    }

    #[test]
    fn a_summary_can_be_limited_to_some_folders() {
        let mut library = Library::default();
        library.insert(
            "trabajo",
            FileData {
                models: vec!["opus".into()],
                projects: vec!["/trabajo/app".into(), "/casa".into()],
                events: vec![
                    event(1, 1, 10 * DAY, 0, [10, 0, 0, 1]),
                    Event { project: 1, ..event(2, 2, 10 * DAY + 60, 0, [7, 0, 0, 1]) },
                ],
                prompts: vec![
                    Prompt { ts: 10 * DAY - 5, key: 100, session: 1 },
                    Prompt { ts: 10 * DAY + 50, key: 101, session: 2 },
                ],
                ..FileData::default()
            },
        );
        let root = Path::new("/trabajo");
        let work = library.summarize_where(Some(9 * DAY), 12 * DAY, utc_local, &|cwd| is_under(cwd, root));
        assert_eq!((work.totals.input, work.totals.messages, work.totals.turns, work.totals.sessions), (10, 1, 1, 1));
        assert_eq!(work.projects.len(), 1);
        let rest = library.summarize_where(Some(9 * DAY), 12 * DAY, utc_local, &|cwd| !is_under(cwd, root));
        assert_eq!((rest.totals.input, rest.totals.turns), (7, 1));
        assert!(!is_under("/trabajo2/app", root));
    }

    #[test]
    fn the_whole_history_starts_at_the_first_day_with_data() {
        let summary = library().summarize(None, 12 * DAY, utc_local);
        assert_eq!(summary.previous, None);
        assert_eq!(summary.totals.messages, 4);
        assert_eq!(summary.days.len(), 6);
        assert!(Library::default().summarize(None, 12 * DAY, utc_local).days.is_empty());
        assert_eq!(Library::default().summarize(Some(0), 2 * DAY, utc_local).days.len(), 2);
    }

    #[test]
    fn utc_calendar_matches_known_dates() {
        // El 1 de enero de 1970 fue jueves.
        assert_eq!(utc_local(0), (0, 3, 0));
        // El 21 de noviembre de 2026 es sábado.
        let (_, weekday, hour) = utc_local(1_795_292_821);
        assert_eq!((weekday, hour), (5, 20));
        let (day, weekday, hour) = system_local(1_795_292_821);
        assert!(day > 20_000 && weekday < 7 && hour < 24);
    }

    #[test]
    fn numbers_are_shortened_for_reading() {
        assert_eq!(compact(950), "950");
        assert_eq!(compact(22_900), "22,9 k");
        assert_eq!(compact(61_300_000), "61,3 M");
        assert_eq!(compact(447_000_000), "447 M");
        assert_eq!(compact(11_600_000_000), "11,6 mil M");
        assert_eq!(compact_duration(23 * 60), "23 min");
        assert_eq!(compact_duration(33_120), "9,2 h");
        assert_eq!(compact_duration(347 * 3600), "347 h");
        assert_eq!(change(150, 100).as_deref(), Some("+50 %"));
        assert_eq!(change(50, 100).as_deref(), Some("−50 %"));
        assert_eq!(change(5, 0), None);
    }
}
