//! Control de código fuente de una carpeta de trabajo: qué ficheros han
//! cambiado respecto de la última confirmación, cómo eran antes y en qué se
//! diferencian. Se apoya en el `git` del sistema y solo escribe cuando el
//! usuario guarda un fichero.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::{Error, Result};

/// Tamaño a partir del cual un fichero no se abre para comparar.
pub const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

/// Qué le ha pasado a un fichero respecto de la última confirmación.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl Status {
    /// La letra con la que se le conoce en cualquier cliente de git.
    pub fn letter(self) -> &'static str {
        match self {
            Status::Modified => "M",
            Status::Added => "A",
            Status::Deleted => "D",
            Status::Renamed => "R",
            Status::Untracked => "U",
            Status::Conflicted => "!",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Modified => "modificado",
            Status::Added => "añadido",
            Status::Deleted => "borrado",
            Status::Renamed => "renombrado",
            Status::Untracked => "sin seguimiento",
            Status::Conflicted => "en conflicto",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Ruta relativa a la raíz del árbol de trabajo, con `/`.
    pub path: String,
    pub status: Status,
    /// Si alguna parte del cambio está ya preparada para confirmar.
    pub staged: bool,
}

/// Un árbol de trabajo con sus cambios.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub root: PathBuf,
    /// Rama actual, o la confirmación abreviada si no hay ninguna.
    pub branch: String,
    pub changes: Vec<Change>,
}

fn git(dir: &Path, args: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Lee el árbol de trabajo que contiene `dir`. `None` si no es de git.
pub fn repo(dir: &Path) -> Option<Repo> {
    let root = git(dir, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(String::from_utf8_lossy(&root).trim_end_matches('\n'));
    let status = git(&root, &["status", "--porcelain=v1", "-z", "--branch", "--untracked-files=all"])?;
    let (branch, changes) = parse_status(&status);
    Some(Repo { root, branch, changes })
}

/// Todos los árboles de trabajo a los que pertenecen unas carpetas: el de
/// cada una y los demás del mismo repositorio, cada uno con sus cambios y sin
/// repetir. Van por orden de ruta, que deja juntos los de un mismo proyecto.
pub fn discover(dirs: &[PathBuf]) -> Vec<Repo> {
    let mut roots: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let Some(root) = git(dir, &["rev-parse", "--show-toplevel"]) else {
            continue;
        };
        let root = PathBuf::from(String::from_utf8_lossy(&root).trim_end_matches('\n'));
        if roots.contains(&root) {
            continue;
        }
        // Los árboles hermanos solo se preguntan una vez por repositorio.
        let siblings = worktrees(&root);
        roots.push(root);
        for tree in siblings {
            if !roots.contains(&tree.path) && tree.path.is_dir() {
                roots.push(tree.path);
            }
        }
    }
    roots.sort();
    read_all(&roots)
}

/// Vuelve a leer los cambios de unos árboles de trabajo ya conocidos.
pub fn refresh(roots: &[PathBuf]) -> Vec<Repo> {
    read_all(roots)
}

/// Lee varios árboles de trabajo a la vez. Con decenas de ellos, hacerlo de
/// uno en uno tarda segundos: cada lectura es un `git status` aparte.
fn read_all(roots: &[PathBuf]) -> Vec<Repo> {
    const WORKERS: usize = 8;
    let next = std::sync::atomic::AtomicUsize::new(0);
    let found: std::sync::Mutex<Vec<(usize, Repo)>> = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..WORKERS.min(roots.len()) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some(root) = roots.get(index) else { break };
                if let Some(read) = repo(root) {
                    if let Ok(mut found) = found.lock() {
                        found.push((index, read));
                    }
                }
            });
        }
    });
    let mut found = found.into_inner().unwrap_or_default();
    found.sort_by_key(|(index, _)| *index);
    found.into_iter().map(|(_, repo)| repo).collect()
}

/// Interpreta `git status --porcelain=v1 -z --branch`.
pub fn parse_status(output: &[u8]) -> (String, Vec<Change>) {
    let mut branch = String::new();
    let mut changes = Vec::new();
    let mut entries = output.split(|b| *b == 0).filter(|e| !e.is_empty());
    while let Some(entry) = entries.next() {
        let entry = String::from_utf8_lossy(entry);
        if let Some(head) = entry.strip_prefix("## ") {
            branch = head
                .strip_prefix("No commits yet on ")
                .unwrap_or(head)
                .split("...")
                .next()
                .unwrap_or(head)
                .to_string();
            continue;
        }
        let mut codes = entry.chars();
        let (Some(index), Some(tree)) = (codes.next(), codes.next()) else {
            continue;
        };
        let Some(path) = entry.get(3..).filter(|p| !p.is_empty()) else {
            continue;
        };
        // Un renombrado trae detrás, como otra entrada, la ruta antigua.
        if index == 'R' || index == 'C' {
            entries.next();
        }
        let status = match (index, tree) {
            ('?', _) => Status::Untracked,
            ('U', _) | (_, 'U') | ('A', 'A') | ('D', 'D') => Status::Conflicted,
            ('D', _) | (_, 'D') => Status::Deleted,
            ('A', _) => Status::Added,
            ('R', _) => Status::Renamed,
            _ => Status::Modified,
        };
        changes.push(Change {
            path: path.to_string(),
            status,
            staged: index != ' ' && index != '?',
        });
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    (branch, changes)
}

/// Un árbol de trabajo del repositorio: el principal o uno añadido con
/// `git worktree`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

/// Árboles de trabajo del repositorio al que pertenece `dir`.
pub fn worktrees(dir: &Path) -> Vec<Worktree> {
    git(dir, &["worktree", "list", "--porcelain"])
        .map(|output| parse_worktrees(&String::from_utf8_lossy(&output)))
        .unwrap_or_default()
}

pub fn parse_worktrees(output: &str) -> Vec<Worktree> {
    let mut found: Vec<Worktree> = Vec::new();
    for line in output.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            found.push(Worktree { path: PathBuf::from(path), branch: String::new() });
        } else if let Some(last) = found.last_mut() {
            if let Some(branch) = line.strip_prefix("branch ") {
                last.branch = branch.strip_prefix("refs/heads/").unwrap_or(branch).to_string();
            } else if line == "detached" {
                last.branch = "sin rama".to_string();
            }
        }
    }
    found
}

/// Si la carpeta es el árbol de trabajo principal del repositorio. Los
/// añadidos con `git worktree` se pueden retirar, el principal no.
pub fn is_main_worktree(root: &Path) -> bool {
    let dir = |flag: &str| {
        git(root, &["rev-parse", "--path-format=absolute", flag])
            .map(|out| PathBuf::from(String::from_utf8_lossy(&out).trim_end_matches('\n')))
    };
    match (dir("--git-dir"), dir("--git-common-dir")) {
        (Some(own), Some(common)) => own == common,
        _ => true,
    }
}

/// Retira un árbol de trabajo añadido: borra su carpeta y su registro en el
/// repositorio. La rama se conserva. Con `force` se retira aunque tenga
/// cambios sin confirmar, que se pierden.
pub fn remove_worktree(root: &Path, force: bool) -> Result<()> {
    if is_main_worktree(root) {
        return Err(Error::Invalid("es el árbol de trabajo principal del repositorio y no se puede retirar".into()));
    }
    // Se pide desde el repositorio principal, no desde la carpeta que va a
    // desaparecer.
    let common = git(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .map(|out| PathBuf::from(String::from_utf8_lossy(&out).trim_end_matches('\n')))
        .ok_or_else(|| Error::Invalid("no se encuentra el repositorio de ese árbol de trabajo".into()))?;
    let mut command = Command::new("git");
    command.arg("-C").arg(&common).args(["worktree", "remove"]);
    if force {
        command.arg("--force");
    }
    let output = command.arg(root).stdin(Stdio::null()).output()?;
    if output.status.success() {
        Ok(())
    } else {
        let reason = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(Error::Invalid(if reason.is_empty() { "git no pudo retirar el árbol de trabajo".into() } else { reason }))
    }
}

/// Contenido del fichero en la última confirmación. `None` si entonces no
/// existía, como en un fichero nuevo.
pub fn original(root: &Path, path: &str) -> Option<String> {
    let bytes = git(root, &["show", &format!("HEAD:{path}")])?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Lee un fichero del árbol de trabajo como texto.
pub fn read_text(root: &Path, path: &str) -> Result<String> {
    let full = root.join(path);
    let size = std::fs::metadata(&full)?.len();
    if size > MAX_FILE_BYTES {
        return Err(Error::Invalid(format!("el fichero pesa {} MB, demasiado para abrirlo aquí", size / (1024 * 1024))));
    }
    let bytes = std::fs::read(&full)?;
    if bytes.contains(&0) {
        return Err(Error::Invalid("es un fichero binario".into()));
    }
    String::from_utf8(bytes).map_err(|_| Error::Invalid("el fichero no está en UTF-8".into()))
}

/// Guarda el texto en el fichero. Escribe en uno temporal y lo cambia de
/// nombre, para no dejar el fichero a medias si algo falla, y conserva sus
/// permisos.
pub fn save(root: &Path, path: &str, text: &str) -> Result<()> {
    let full = root.join(path);
    if !full.starts_with(root) || path.split('/').any(|part| part == "..") {
        return Err(Error::Invalid("la ruta sale del árbol de trabajo".into()));
    }
    let temporary = full.with_extension(format!(
        "{}.alarviterm-{}.tmp",
        full.extension().and_then(|e| e.to_str()).unwrap_or(""),
        std::process::id()
    ));
    std::fs::write(&temporary, text)?;
    if let Ok(meta) = std::fs::metadata(&full) {
        let _ = std::fs::set_permissions(&temporary, meta.permissions());
    }
    std::fs::rename(&temporary, &full).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        e.into()
    })
}

/// Qué es una línea al comparar dos versiones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Same,
    /// Quitada en la versión antigua, o añadida en la nueva.
    Changed,
}

/// Comparación por líneas de dos textos.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LineDiff {
    /// Una entrada por línea de la versión antigua.
    pub old: Vec<LineKind>,
    /// Una entrada por línea de la versión nueva.
    pub new: Vec<LineKind>,
    /// Puntos en que ambas versiones coinciden: línea antigua y línea nueva,
    /// contadas desde cero y en orden. Sirven para desplazarlas a la par.
    pub anchors: Vec<(usize, usize)>,
    pub added: usize,
    pub removed: usize,
    /// Bloques de cambios, en orden.
    pub blocks: Vec<Block>,
}

/// Un bloque de líneas seguidas que cambia entre las dos versiones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Block {
    pub kind: BlockKind,
    /// Primera línea en la versión antigua y cuántas abarca.
    pub old_start: usize,
    pub old_len: usize,
    /// Primera línea en la versión nueva y cuántas abarca. Un bloque solo
    /// quitado abarca cero: `new_start` es la línea donde estaba.
    pub new_start: usize,
    pub new_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Added,
    Removed,
    Modified,
}

impl LineDiff {
    /// Línea de la versión antigua que cae a la altura de una de la nueva.
    pub fn old_line_for(&self, new_line: usize) -> usize {
        match self.anchors.iter().rev().find(|(_, new)| *new <= new_line) {
            Some((old, new)) => {
                let next = self.anchors.iter().find(|(_, n)| *n > new_line);
                let limit = next.map_or(self.old.len(), |(o, _)| *o);
                (old + (new_line - new)).min(limit.max(*old))
            }
            None => 0,
        }
    }

    /// Línea de la versión nueva donde empieza cada bloque de cambios. Uno
    /// solo quitado cuenta con la línea donde estaba.
    pub fn hunks(&self) -> Vec<usize> {
        let mut starts: Vec<usize> = self.blocks.iter().map(|block| block.new_start).collect();
        starts.dedup();
        starts
    }

    /// Línea de la versión nueva que cae a la altura de una de la antigua.
    pub fn new_line_for(&self, old_line: usize) -> usize {
        match self.anchors.iter().rev().find(|(old, _)| *old <= old_line) {
            Some((old, new)) => {
                let next = self.anchors.iter().find(|(o, _)| *o > old_line);
                let limit = next.map_or(self.new.len(), |(_, n)| *n);
                (new + (old_line - old)).min(limit.max(*new))
            }
            None => 0,
        }
    }
}

/// Compara dos textos línea a línea.
pub fn diff_lines(old: &str, new: &str) -> LineDiff {
    use similar::{DiffOp, TextDiff};

    let diff = TextDiff::from_lines(old, new);
    let mut result = LineDiff::default();
    for op in diff.ops() {
        match *op {
            DiffOp::Equal { old_index, new_index, len } => {
                result.anchors.push((old_index, new_index));
                result.old.extend(std::iter::repeat_n(LineKind::Same, len));
                result.new.extend(std::iter::repeat_n(LineKind::Same, len));
            }
            DiffOp::Delete { old_index, old_len, new_index } => {
                result.old.extend(std::iter::repeat_n(LineKind::Changed, old_len));
                result.removed += old_len;
                result.blocks.push(Block {
                    kind: BlockKind::Removed,
                    old_start: old_index,
                    old_len,
                    new_start: new_index,
                    new_len: 0,
                });
            }
            DiffOp::Insert { old_index, new_index, new_len } => {
                result.new.extend(std::iter::repeat_n(LineKind::Changed, new_len));
                result.added += new_len;
                result.blocks.push(Block {
                    kind: BlockKind::Added,
                    old_start: old_index,
                    old_len: 0,
                    new_start: new_index,
                    new_len,
                });
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                result.old.extend(std::iter::repeat_n(LineKind::Changed, old_len));
                result.new.extend(std::iter::repeat_n(LineKind::Changed, new_len));
                result.removed += old_len;
                result.added += new_len;
                result.blocks.push(Block {
                    kind: BlockKind::Modified,
                    old_start: old_index,
                    old_len,
                    new_start: new_index,
                    new_len,
                });
            }
        }
    }
    result
}

/// Un nodo del árbol de ficheros cambiados.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeNode {
    /// Una carpeta, o varias seguidas con un solo hijo, como `src/tienda`.
    Folder { name: String, children: Vec<TreeNode> },
    File { name: String, change: Change },
}

/// Ordena los cambios en carpetas. Las cadenas de carpetas con una sola
/// subcarpeta se funden en un nodo, para no desplegar niveles vacíos.
pub fn tree(changes: &[Change]) -> Vec<TreeNode> {
    #[derive(Default)]
    struct Dir {
        dirs: BTreeMap<String, Dir>,
        files: Vec<(String, Change)>,
    }
    let mut top = Dir::default();
    for change in changes {
        let mut parts: Vec<&str> = change.path.split('/').collect();
        let Some(name) = parts.pop() else { continue };
        let mut dir = &mut top;
        for part in parts {
            dir = dir.dirs.entry(part.to_string()).or_default();
        }
        dir.files.push((name.to_string(), change.clone()));
    }
    fn build(dir: Dir) -> Vec<TreeNode> {
        let mut nodes = Vec::new();
        for (name, mut child) in dir.dirs {
            let mut name = name;
            while child.files.is_empty() && child.dirs.len() == 1 {
                let (inner, grandchild) = child.dirs.into_iter().next().expect("un hijo");
                name = format!("{name}/{inner}");
                child = grandchild;
            }
            nodes.push(TreeNode::Folder { name, children: build(child) });
        }
        let mut files = dir.files;
        files.sort_by(|a, b| a.0.cmp(&b.0));
        nodes.extend(files.into_iter().map(|(name, change)| TreeNode::File { name, change }));
        nodes
    }
    build(top)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_status_is_read_with_renames_and_branch() {
        let output = b"## feat/pagos...origin/feat/pagos [ahead 2]\0 M src/app.py\0M  src/staged.py\0?? notas con espacio.txt\0R  nuevo.rs\0viejo.rs\0 D borrado.txt\0UU conflicto.c\0A  src/alta.py\0";
        let (branch, changes) = parse_status(output);
        assert_eq!(branch, "feat/pagos");
        let seen: Vec<(&str, &str, bool)> =
            changes.iter().map(|c| (c.path.as_str(), c.status.letter(), c.staged)).collect();
        assert_eq!(
            seen,
            vec![
                ("borrado.txt", "D", false),
                ("conflicto.c", "!", true),
                ("notas con espacio.txt", "U", false),
                ("nuevo.rs", "R", true),
                ("src/alta.py", "A", true),
                ("src/app.py", "M", false),
                ("src/staged.py", "M", true),
            ]
        );
        assert_eq!(parse_status(b"## No commits yet on main\0").0, "main");
        assert!(parse_status(b"").1.is_empty());
    }

    #[test]
    fn worktrees_are_listed_with_their_branch() {
        let found = parse_worktrees(
            "worktree /repo\nHEAD abc\nbranch refs/heads/main\n\nworktree /repo-wt\nHEAD def\nbranch refs/heads/fix/uno\n\nworktree /suelto\nHEAD 123\ndetached\n",
        );
        let seen: Vec<(&str, &str)> =
            found.iter().map(|w| (w.path.to_str().expect("ruta"), w.branch.as_str())).collect();
        assert_eq!(seen, vec![("/repo", "main"), ("/repo-wt", "fix/uno"), ("/suelto", "sin rama")]);
    }

    #[test]
    fn lines_are_compared_and_kept_side_by_side() {
        let old = "uno\ndos\ntres\ncuatro\ncinco\n";
        let new = "uno\ndos cambiado\ntres\nnueva\ncuatro\n";
        let diff = diff_lines(old, new);
        use LineKind::{Changed, Same};
        assert_eq!(diff.old, vec![Same, Changed, Same, Same, Changed]);
        assert_eq!(diff.new, vec![Same, Changed, Same, Changed, Same]);
        assert_eq!((diff.added, diff.removed), (2, 2));
        assert_eq!(diff.hunks(), vec![1, 3, 5]);
        let kinds: Vec<BlockKind> = diff.blocks.iter().map(|b| b.kind).collect();
        assert_eq!(kinds, vec![BlockKind::Modified, BlockKind::Added, BlockKind::Removed]);
        // «cinco» se quitó al final: su bloque cae tras la última línea nueva.
        assert_eq!((diff.blocks[2].old_start, diff.blocks[2].new_start, diff.blocks[2].new_len), (4, 5, 0));
        assert_eq!(diff.new_line_for(3), 4);
        assert_eq!(diff.new_line_for(0), 0);
        // «cuatro» está en la línea 4 de la nueva y en la 3 de la antigua.
        assert_eq!(diff.old_line_for(4), 3);
        assert_eq!(diff.old_line_for(0), 0);
        assert_eq!(diff.old_line_for(2), 2);

        let same = diff_lines("a\nb\n", "a\nb\n");
        assert_eq!((same.added, same.removed), (0, 0));
        assert!(same.hunks().is_empty());
        let created = diff_lines("", "a\nb\n");
        assert_eq!((created.added, created.new.len(), created.old.len()), (2, 2, 0));
    }

    #[test]
    fn changes_are_arranged_in_folders_with_chains_merged() {
        let change = |path: &str| Change { path: path.into(), status: Status::Modified, staged: false };
        let nodes = tree(&[
            change("README.md"),
            change("src/tienda/agent/estado.py"),
            change("src/tienda/server/app.py"),
            change("src/tienda/server/rutas.py"),
            change("tests/unidad/servidor/test_app.py"),
        ]);
        fn show(nodes: &[TreeNode], depth: usize, out: &mut Vec<String>) {
            for node in nodes {
                match node {
                    TreeNode::Folder { name, children } => {
                        out.push(format!("{}{name}/", "  ".repeat(depth)));
                        show(children, depth + 1, out);
                    }
                    TreeNode::File { name, .. } => out.push(format!("{}{name}", "  ".repeat(depth))),
                }
            }
        }
        let mut lines = Vec::new();
        show(&nodes, 0, &mut lines);
        assert_eq!(
            lines,
            vec![
                "src/tienda/",
                "  agent/",
                "    estado.py",
                "  server/",
                "    app.py",
                "    rutas.py",
                "tests/unidad/servidor/",
                "  test_app.py",
                "README.md",
            ]
        );
    }

    #[test]
    fn a_real_repository_reports_its_changes_and_saves_files() {
        let dir = std::env::temp_dir().join(format!("alarviterm-scm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).expect("carpeta");
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(["-c", "user.name=prueba", "-c", "user.email=prueba@example.com", "-c", "commit.gpgsign=false"])
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("git")
                .success();
            assert!(ok, "git {args:?}");
        };
        assert!(repo(&dir).is_none());
        run(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("src/a.txt"), "uno\ndos\n").expect("fichero");
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "base"]);

        std::fs::write(dir.join("src/a.txt"), "uno\ndos\ntres\n").expect("fichero");
        std::fs::write(dir.join("nuevo.txt"), "hola\n").expect("fichero");
        let found = repo(&dir.join("src")).expect("repositorio");
        assert_eq!(found.branch, "main");
        assert_eq!(found.root.canonicalize().expect("ruta"), dir.canonicalize().expect("ruta"));
        let seen: Vec<(&str, &str)> = found.changes.iter().map(|c| (c.path.as_str(), c.status.letter())).collect();
        assert_eq!(seen, vec![("nuevo.txt", "U"), ("src/a.txt", "M")]);

        assert_eq!(original(&found.root, "src/a.txt").as_deref(), Some("uno\ndos\n"));
        assert_eq!(original(&found.root, "nuevo.txt"), None);
        assert_eq!(read_text(&found.root, "src/a.txt").expect("texto"), "uno\ndos\ntres\n");

        save(&found.root, "src/a.txt", "uno\ndos\n").expect("guarda");
        assert_eq!(repo(&dir).expect("repositorio").changes.len(), 1, "al volver al original deja de ser un cambio");
        assert!(save(&found.root, "../fuera.txt", "x").is_err());
        std::fs::write(dir.join("binario.bin"), [0u8, 1, 2]).expect("fichero");
        assert!(read_text(&found.root, "binario.bin").is_err());
        assert_eq!(worktrees(&dir).len(), 1);

        // Un segundo árbol de trabajo aparece al descubrir desde cualquier carpeta.
        let second = dir.with_extension("wt");
        let _ = std::fs::remove_dir_all(&second);
        run(&["worktree", "add", "-q", "-b", "otra", second.to_str().expect("ruta")]);
        let found = discover(&[dir.join("src"), dir.clone(), PathBuf::from("/no/existe")]);
        let branches: Vec<&str> = found.iter().map(|r| r.branch.as_str()).collect();
        assert_eq!(branches, vec!["main", "otra"]);
        assert_eq!(found[0].changes.len(), 2, "el principal conserva sus cambios");
        assert!(found[1].changes.is_empty());
        // Retirar: el principal nunca, el añadido solo a la fuerza si tiene cambios.
        assert!(is_main_worktree(&found[0].root));
        assert!(!is_main_worktree(&found[1].root));
        assert!(remove_worktree(&found[0].root, true).is_err());
        let extra = dir.with_extension("wt2");
        let _ = std::fs::remove_dir_all(&extra);
        run(&["worktree", "add", "-q", "-b", "tercera", extra.to_str().expect("ruta")]);
        std::fs::write(extra.join("suelto.txt"), "sin confirmar\n").expect("fichero");
        assert!(remove_worktree(&extra, false).is_err(), "con cambios pide forzar");
        assert!(extra.is_dir());
        remove_worktree(&extra, true).expect("retira");
        assert!(!extra.exists());
        assert_eq!(worktrees(&dir).len(), 2);
        let again = refresh(&[found[1].root.clone(), PathBuf::from("/no/existe"), found[0].root.clone()]);
        assert_eq!(again.iter().map(|r| r.branch.as_str()).collect::<Vec<_>>(), vec!["otra", "main"]);
        let _ = std::fs::remove_dir_all(&second);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
