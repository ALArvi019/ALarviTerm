//! Conversaciones de Claude Code que ya existen en disco, para traerlas a
//! ALarviTerm sin empezar de cero.
//!
//! Claude Code guarda cada conversación en
//! `<config>/projects/<carpeta codificada>/<id>.jsonl` y apunta las sesiones
//! en marcha en `<config>/sessions/<pid>.json`. Aquí solo se lee.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use memchr::memmem;
use serde_json::Value;

/// Cuánto se lee del final y del principio de una transcripción para dar con
/// su nombre. Claude Code repite el título cerca del final al ir escribiendo.
const SCAN_BYTES: u64 = 4 * 1024 * 1024;

/// Una conversación guardada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    /// Identificador que acepta `claude --resume`.
    pub id: String,
    /// Nombre puesto con `/rename`, o el que Claude Code le dio. Puede faltar.
    pub title: Option<String>,
    /// Última escritura, en segundos Unix.
    pub modified: i64,
    pub size: u64,
}

/// Carpeta donde Claude Code guarda las conversaciones de `cwd`: la ruta con
/// todo lo que no es letra ni número cambiado por un guion.
pub fn transcripts_dir(config_dir: &Path, cwd: &Path) -> PathBuf {
    let encoded: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    config_dir.join("projects").join(encoded)
}

/// Conversaciones de una carpeta de trabajo, de la más reciente a la más
/// antigua. Sin carpeta o sin permisos devuelve una lista vacía.
pub fn list(config_dir: &Path, cwd: &Path) -> Vec<Conversation> {
    let Ok(entries) = std::fs::read_dir(transcripts_dir(config_dir, cwd)) else {
        return Vec::new();
    };
    let mut found: Vec<Conversation> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                return None;
            }
            let id = path.file_stem()?.to_str()?.to_string();
            let meta = entry.metadata().ok()?;
            if !meta.is_file() || meta.len() == 0 || !valid_id(&id) {
                return None;
            }
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            Some(Conversation {
                title: read_title(&path, meta.len()),
                id,
                modified,
                size: meta.len(),
            })
        })
        .collect();
    found.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| a.id.cmp(&b.id)));
    found
}

/// Cuánto se lee del principio de una transcripción para dar con su carpeta.
const CWD_SCAN_BYTES: u64 = 512 * 1024;

/// Las `limit` conversaciones más recientes de todas las carpetas, cada una
/// con la carpeta de trabajo en la que nació, que es donde hay que reanudarla.
/// Se descartan las que no la dicen o cuya carpeta ya no existe.
pub fn recent(config_dir: &Path, limit: usize) -> Vec<(Conversation, PathBuf)> {
    let Ok(folders) = std::fs::read_dir(config_dir.join("projects")) else {
        return Vec::new();
    };
    // Primero solo fechas: leer títulos de miles de ficheros sería lento.
    let mut files: Vec<(i64, u64, String, PathBuf)> = Vec::new();
    for folder in folders.flatten() {
        let Ok(entries) = std::fs::read_dir(folder.path()) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() || meta.len() == 0 || !valid_id(&id) {
                continue;
            }
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            files.push((modified, meta.len(), id, path));
        }
    }
    files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.cmp(&b.2)));
    let mut found = Vec::new();
    for (modified, size, id, path) in files {
        if found.len() >= limit {
            break;
        }
        let mut head = Vec::new();
        if File::open(&path).and_then(|f| f.take(CWD_SCAN_BYTES).read_to_end(&mut head)).is_err() {
            continue;
        }
        let Some(cwd) = read_cwd(config_dir, &path, &head).filter(|dir| dir.is_dir()) else { continue };
        // Sin nombre, lo primero que se le pidió dice de qué va.
        let title = read_title(&path, size).or_else(|| first_prompt(&head));
        found.push((Conversation { title, id, modified, size }, cwd));
    }
    found
}

/// Carpeta de trabajo que apunta una transcripción y que corresponde a la
/// carpeta en la que está guardada. Una sesión puede cambiar de directorio a
/// media conversación, y esas otras rutas no sirven para reanudarla.
fn read_cwd(config_dir: &Path, path: &Path, head: &[u8]) -> Option<PathBuf> {
    let home = path.parent()?;
    let mut from = 0;
    while let Some(at) = memmem::find(&head[from..], b"\"cwd\":\"") {
        let at = from + at;
        let start = memchr::memrchr(b'\n', &head[..at]).map_or(0, |i| i + 1);
        let stop = memchr::memchr(b'\n', &head[at..]).map_or(head.len(), |i| at + i);
        let cwd = serde_json::from_slice::<Value>(&head[start..stop])
            .ok()
            .and_then(|line| line.get("cwd").and_then(Value::as_str).map(PathBuf::from))
            .filter(|cwd| transcripts_dir(config_dir, cwd) == home);
        if cwd.is_some() {
            return cwd;
        }
        from = stop.max(at + 1);
    }
    None
}

/// Largo máximo del nombre que se saca de la primera petición.
const PROMPT_CHARS: usize = 48;

/// Primera petición escrita por el usuario, recortada a una línea corta. Se
/// saltan los mensajes que pone Claude Code por su cuenta, que van entre
/// etiquetas.
fn first_prompt(head: &[u8]) -> Option<String> {
    head.split(|b| *b == b'\n').find_map(|line| {
        memmem::find(line, b"\"type\":\"user\"")?;
        let line = serde_json::from_slice::<Value>(line).ok()?;
        if line.get("type").and_then(Value::as_str) != Some("user") {
            return None;
        }
        let content = line.get("message")?.get("content")?;
        let text = match content {
            Value::String(text) => text.as_str(),
            Value::Array(parts) => parts.iter().find_map(|p| p.get("text").and_then(Value::as_str))?,
            _ => return None,
        };
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() || text.starts_with('<') {
            return None;
        }
        Some(match text.char_indices().nth(PROMPT_CHARS) {
            Some((cut, _)) => format!("{}…", text[..cut].trim_end()),
            None => text,
        })
    })
}

/// Un identificador que se puede pasar a `claude --resume` sin sorpresas.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('-')
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn read_title(path: &Path, size: u64) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut tail = Vec::new();
    file.seek(SeekFrom::Start(size.saturating_sub(SCAN_BYTES))).ok()?;
    file.by_ref().take(SCAN_BYTES).read_to_end(&mut tail).ok()?;
    let mut head = Vec::new();
    if size > SCAN_BYTES {
        file.seek(SeekFrom::Start(0)).ok()?;
        file.by_ref().take(SCAN_BYTES).read_to_end(&mut head).ok()?;
    }
    // El nombre que puso el usuario manda sobre el automático.
    [("custom-title", "customTitle"), ("ai-title", "aiTitle")]
        .into_iter()
        .find_map(|(kind, field)| {
            last_title(&tail, kind, field).or_else(|| last_title(&head, kind, field))
        })
}

/// Último título de la clase `kind` que aparece en un trozo de transcripción.
fn last_title(chunk: &[u8], kind: &str, field: &str) -> Option<String> {
    let needle = format!("\"type\":\"{kind}\"");
    let mut end = chunk.len();
    while let Some(at) = memmem::rfind(&chunk[..end], needle.as_bytes()) {
        let start = memchr::memrchr(b'\n', &chunk[..at]).map_or(0, |i| i + 1);
        let stop = memchr::memchr(b'\n', &chunk[at..]).map_or(chunk.len(), |i| at + i);
        let title = serde_json::from_slice::<Value>(&chunk[start..stop])
            .ok()
            .filter(|line| line.get("type").and_then(Value::as_str) == Some(kind))
            .and_then(|line| line.get(field).and_then(Value::as_str).map(|t| t.trim().to_string()))
            .filter(|t| !t.is_empty());
        if title.is_some() {
            return title;
        }
        // La coincidencia estaba dentro de otro mensaje, o la línea venía cortada.
        end = at;
    }
    None
}

/// Conversaciones cuyo Claude Code dice tener un comando de shell corriendo
/// en segundo plano. Es el estado `shell` de su registro de sesiones: el
/// turno terminó, pero hay un comando lanzado que sigue en marcha.
pub fn with_background_shell(config_dir: &Path) -> std::collections::HashSet<String> {
    let Ok(entries) = std::fs::read_dir(config_dir.join("sessions")) else {
        return Default::default();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                return None;
            }
            let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
            if value.get("status").and_then(Value::as_str) != Some("shell") {
                return None;
            }
            let pid = u32::try_from(value.get("pid")?.as_u64()?).ok()?;
            let id = value.get("sessionId")?.as_str()?.to_string();
            process_matches(pid, value.get("procStart").and_then(Value::as_str)).then_some(id)
        })
        .collect()
}

/// Conversaciones que algún Claude Code tiene abiertas ahora mismo, con el
/// proceso que las tiene. Reanudar una de ellas a la vez en otro sitio haría
/// que dos procesos escribieran en la misma transcripción.
pub fn open_now(config_dir: &Path) -> HashMap<String, u32> {
    let Ok(entries) = std::fs::read_dir(config_dir.join("sessions")) else {
        return HashMap::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                return None;
            }
            let value: Value = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
            let pid = u32::try_from(value.get("pid")?.as_u64()?).ok()?;
            let id = value.get("sessionId")?.as_str()?.to_string();
            process_matches(pid, value.get("procStart").and_then(Value::as_str)).then_some((id, pid))
        })
        .collect()
}

/// El proceso sigue vivo y es el mismo que escribió el fichero: un pid puede
/// reutilizarse, así que se compara también su instante de arranque.
fn process_matches(pid: u32, proc_start: Option<&str>) -> bool {
    let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let Some(expected) = proc_start else {
        return true;
    };
    // Tras el nombre entre paréntesis, el arranque es el campo 22 de la línea.
    stat.rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().nth(19))
        .is_none_or(|started| started == expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("alarviterm-conv-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("carpeta");
        dir
    }

    #[test]
    fn encodes_the_working_directory_like_claude_code() {
        let dir = transcripts_dir(Path::new("/c"), Path::new("/home/u/mi.app/sub_dir"));
        assert_eq!(dir, Path::new("/c/projects/-home-u-mi-app-sub-dir"));
    }

    #[test]
    fn lists_conversations_with_their_latest_name() {
        let config = scratch("list");
        let cwd = Path::new("/home/u/app");
        let dir = transcripts_dir(&config, cwd);
        std::fs::create_dir_all(&dir).expect("carpeta");
        std::fs::write(
            dir.join("aaa-1.jsonl"),
            concat!(
                "{\"type\":\"ai-title\",\"aiTitle\":\"Automático\",\"sessionId\":\"aaa-1\"}\n",
                "{\"type\":\"custom-title\",\"customTitle\":\"VIEJO\",\"sessionId\":\"aaa-1\"}\n",
                "{\"type\":\"user\",\"message\":{\"content\":\"mira \\\"type\\\":\\\"custom-title\\\" raro\"}}\n",
                "{\"type\":\"custom-title\",\"customTitle\":\"JEFE\",\"sessionId\":\"aaa-1\"}\n",
                "{\"type\":\"assistant\",\"message\":{\"content\":\"hecho\"}}\n",
            ),
        )
        .expect("transcripción");
        std::fs::write(
            dir.join("bbb-2.jsonl"),
            "{\"type\":\"ai-title\",\"aiTitle\":\"Solo automático\"}\n{\"type\":\"user\"}\n",
        )
        .expect("transcripción");
        std::fs::write(dir.join("ccc-3.jsonl"), "{\"type\":\"user\"}\n").expect("transcripción");
        std::fs::write(dir.join("vacia.jsonl"), "").expect("vacía");
        std::fs::write(dir.join("notas.txt"), "x").expect("otro");

        let mut found = list(&config, cwd);
        found.sort_by(|a, b| a.id.cmp(&b.id));
        let names: Vec<_> = found.iter().map(|c| (c.id.as_str(), c.title.as_deref())).collect();
        assert_eq!(
            names,
            vec![("aaa-1", Some("JEFE")), ("bbb-2", Some("Solo automático")), ("ccc-3", None)]
        );
        assert!(list(&config, Path::new("/no/existe")).is_empty());
        let _ = std::fs::remove_dir_all(&config);
    }

    #[test]
    fn lists_recent_conversations_of_every_folder_with_their_directory() {
        let config = scratch("recent");
        let (one, two) = (scratch("recent-one"), scratch("recent-two"));
        for (cwd, id, body) in [
            (&one, "aaa-1", format!("{{\"type\":\"custom-title\",\"customTitle\":\"UNO\"}}\n{{\"type\":\"user\",\"cwd\":\"/otra/parte\"}}\n{{\"type\":\"user\",\"cwd\":\"{}\"}}\n", one.display())),
            (&two, "bbb-2", format!("{{\"type\":\"user\",\"message\":{{\"content\":\"<aviso>x</aviso>\"}}}}\n{{\"type\":\"user\",\"cwd\":\"{}\",\"message\":{{\"content\":[{{\"type\":\"text\",\"text\":\"arregla  el\\nlogin y además revisa todas las pruebas de la tienda entera\"}}]}}}}\n", two.display())),
            (&two, "ccc-3", "{\"type\":\"user\"}\n".to_string()),
        ] {
            let dir = transcripts_dir(&config, cwd);
            std::fs::create_dir_all(&dir).expect("carpeta");
            std::fs::write(dir.join(format!("{id}.jsonl")), body).expect("transcripción");
        }
        let mut found = recent(&config, 10);
        found.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        let seen: Vec<_> = found.iter().map(|(c, dir)| (c.id.as_str(), c.title.as_deref(), dir.clone())).collect();
        assert_eq!(seen, vec![("aaa-1", Some("UNO"), one.clone()), ("bbb-2", Some("arregla el login y además revisa todas las prueb…"), two.clone())]);
        assert_eq!(recent(&config, 1).len(), 1);
        // Una carpeta que ya no existe no se ofrece.
        std::fs::remove_dir_all(&two).expect("borra");
        assert_eq!(recent(&config, 10).len(), 1);
        for dir in [&config, &one] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn only_safe_identifiers_are_accepted() {
        assert!(valid_id("0c5f3fec-42d6-486d-a108-46b0cefa5871"));
        assert!(!valid_id("--dangerous"));
        assert!(!valid_id("a b"));
        assert!(!valid_id(""));
    }

    #[test]
    fn reports_only_conversations_of_living_processes() {
        let config = scratch("open");
        let sessions = config.join("sessions");
        std::fs::create_dir_all(&sessions).expect("carpeta");
        let me = std::process::id();
        std::fs::write(
            sessions.join(format!("{me}.json")),
            format!("{{\"pid\":{me},\"sessionId\":\"viva\"}}"),
        )
        .expect("viva");
        std::fs::write(
            sessions.join("otra.json"),
            format!("{{\"pid\":{me},\"sessionId\":\"reciclada\",\"procStart\":\"1\"}}"),
        )
        .expect("reciclada");
        std::fs::write(sessions.join("4000000000.json"), "{\"pid\":4000000000,\"sessionId\":\"muerta\"}")
            .expect("muerta");
        std::fs::write(sessions.join("rota.json"), "no es json").expect("rota");

        let open = open_now(&config);
        assert_eq!(open.get("viva"), Some(&me));
        assert_eq!(open.len(), 1);

        assert!(with_background_shell(&config).is_empty());
        std::fs::write(
            sessions.join("ocupada.json"),
            format!("{{\"pid\":{me},\"sessionId\":\"con-comando\",\"status\":\"shell\"}}"),
        )
        .expect("ocupada");
        std::fs::write(
            sessions.join("4000000001.json"),
            "{\"pid\":4000000001,\"sessionId\":\"muerta-con-comando\",\"status\":\"shell\"}",
        )
        .expect("muerta");
        let busy = with_background_shell(&config);
        assert!(busy.contains("con-comando"));
        assert_eq!(busy.len(), 1);
        let _ = std::fs::remove_dir_all(&config);
    }
}
