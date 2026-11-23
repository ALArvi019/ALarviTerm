//! Pestaña de código de la barra lateral: los repositorios de las carpetas
//! abiertas, con sus árboles de trabajo, y dentro de cada uno el árbol de los
//! ficheros que han cambiado. Un clic en un fichero lo abre en la comparación.

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio};
use alarviterm_core::paths::display_path;
use alarviterm_core::scm::{self, Change, Repo, Status, TreeNode};

pub struct CodeBarCallbacks {
    /// Abrir un fichero cambiado: raíz de su árbol de trabajo y el cambio.
    pub open: Box<dyn Fn(PathBuf, Change)>,
    /// Volver a leer los repositorios.
    pub refresh: Box<dyn Fn()>,
}

pub struct CodeBar {
    pub root: gtk::Box,
    callbacks: CodeBarCallbacks,
    list: gtk::ListBox,
    summary: gtk::Label,
    spinner: gtk::Spinner,
    /// Botón que pliega o despliega todo de una vez.
    fold: gtk::Button,
    repos: RefCell<Vec<Repo>>,
    /// Repositorios desplegados, por su raíz.
    expanded: RefCell<HashSet<PathBuf>>,
    /// Carpetas plegadas, como «raíz|ruta».
    collapsed: RefCell<HashSet<String>>,
    /// Fichero abierto en la comparación.
    selected: RefCell<Option<(PathBuf, String)>>,
}

impl CodeBar {
    pub fn new(callbacks: CodeBarCallbacks) -> Rc<Self> {
        let summary = gtk::Label::new(Some("Leyendo repositorios…"));
        summary.add_css_class("mt-stat-note");
        summary.set_xalign(0.0);
        summary.set_hexpand(true);
        summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let spinner = gtk::Spinner::new();
        let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh.add_css_class("flat");
        refresh.set_tooltip_text(Some("Volver a leer los cambios"));
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        header.set_margin_top(6);
        header.set_margin_bottom(2);
        header.set_margin_start(14);
        header.set_margin_end(6);
        header.append(&summary);
        let fold = gtk::Button::from_icon_name("view-list-symbolic");
        fold.add_css_class("flat");
        header.append(&spinner);
        header.append(&fold);
        header.append(&refresh);

        let list = gtk::ListBox::new();
        list.add_css_class("mt-code-files");
        list.set_selection_mode(gtk::SelectionMode::Single);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("mt-sidebar");
        root.append(&header);
        root.append(&scroller);

        let bar = Rc::new(Self {
            root,
            callbacks,
            list,
            summary,
            spinner,
            fold,
            repos: RefCell::new(Vec::new()),
            expanded: RefCell::new(HashSet::new()),
            collapsed: RefCell::new(HashSet::new()),
            selected: RefCell::new(None),
        });

        let weak = Rc::downgrade(&bar);
        refresh.connect_clicked(move |_| {
            if let Some(bar) = weak.upgrade() {
                (bar.callbacks.refresh)();
            }
        });
        let weak = Rc::downgrade(&bar);
        bar.fold.connect_clicked(move |_| {
            if let Some(bar) = weak.upgrade() {
                bar.toggle_all();
            }
        });
        // Clic derecho sobre la cabecera de un repositorio.
        let secondary = gtk::GestureClick::new();
        secondary.set_button(gdk::BUTTON_SECONDARY);
        let weak = Rc::downgrade(&bar);
        secondary.connect_pressed(move |_, _, _, y| {
            let Some(bar) = weak.upgrade() else { return };
            let Some(row) = bar.list.row_at_y(y as i32) else { return };
            let name = row.widget_name();
            let Some(index) = name.strip_prefix("r:").and_then(|i| i.parse::<usize>().ok()) else {
                return;
            };
            let root = bar.repos.borrow().get(index).map(|r| r.root.clone());
            let Some(root) = root else { return };
            let menu = gio::Menu::new();
            if scm::is_main_worktree(&root) {
                // Sin acción, la entrada sale apagada: explica por qué no hay nada que hacer.
                menu.append(Some("Árbol principal del repositorio, no se elimina"), None);
            } else {
                menu.append_item(&super::menus::item(
                    "Eliminar este worktree…",
                    "win.code-worktree-remove",
                    &root.to_string_lossy(),
                ));
            }
            super::menus::popup(&row, &menu);
        });
        bar.list.add_controller(secondary);

        let weak = Rc::downgrade(&bar);
        bar.list.connect_row_activated(move |_, row| {
            if let Some(bar) = weak.upgrade() {
                bar.activated(&row.widget_name());
            }
        });
        bar
    }

    /// Una fila pulsada: plegar un repositorio o una carpeta, o abrir un fichero.
    fn activated(&self, name: &str) {
        let mut parts = name.splitn(3, ':');
        let (Some(kind), Some(index)) = (parts.next(), parts.next().and_then(|i| i.parse::<usize>().ok())) else {
            return;
        };
        let rest = parts.next().unwrap_or("");
        let Some(root) = self.repos.borrow().get(index).map(|r| r.root.clone()) else {
            return;
        };
        match kind {
            "r" => {
                let mut expanded = self.expanded.borrow_mut();
                if !expanded.remove(&root) {
                    expanded.insert(root);
                }
                drop(expanded);
                self.fill();
            }
            "d" => {
                let key = format!("{}|{rest}", root.display());
                let mut collapsed = self.collapsed.borrow_mut();
                if !collapsed.remove(&key) {
                    collapsed.insert(key);
                }
                drop(collapsed);
                self.fill();
            }
            "f" => {
                let change = self.change_for(&root, rest);
                if let Some(change) = change {
                    (self.callbacks.open)(root, change);
                }
            }
            _ => {}
        }
    }

    /// Hay algún repositorio desplegado.
    fn any_expanded(&self) -> bool {
        let expanded = self.expanded.borrow();
        self.repos.borrow().iter().any(|r| expanded.contains(&r.root))
    }

    /// Pliega todo si hay algo desplegado. Si no, despliega los repositorios con cambios
    /// y todas sus carpetas.
    fn toggle_all(&self) {
        if self.any_expanded() {
            self.expanded.borrow_mut().clear();
        } else {
            let roots: HashSet<PathBuf> =
                self.repos.borrow().iter().filter(|r| !r.changes.is_empty()).map(|r| r.root.clone()).collect();
            *self.expanded.borrow_mut() = roots;
            self.collapsed.borrow_mut().clear();
        }
        self.fill();
    }

    /// Ajusta el botón de plegar a lo que haría al pulsarlo.
    fn update_fold(&self) {
        let open = self.any_expanded();
        self.fold.set_icon_name(if open { "go-up-symbolic" } else { "go-down-symbolic" });
        self.fold.set_tooltip_text(Some(if open { "Plegar todo" } else { "Desplegar todos los cambios" }));
        self.fold.set_sensitive(open || self.repos.borrow().iter().any(|r| !r.changes.is_empty()));
    }

    pub fn set_loading(&self, loading: bool) {
        self.spinner.set_visible(loading);
        if loading {
            self.spinner.start();
        } else {
            self.spinner.stop();
        }
    }

    /// Pone los repositorios leídos. Lo plegado y lo elegido se conservan.
    pub fn set_repos(&self, repos: Vec<Repo>) {
        *self.repos.borrow_mut() = repos;
        self.fill();
    }

    /// Sustituye solo los repositorios que se han vuelto a leer.
    pub fn update_repos(&self, fresh: Vec<Repo>) {
        let mut changed = false;
        {
            let mut repos = self.repos.borrow_mut();
            for repo in fresh {
                if let Some(known) = repos.iter_mut().find(|known| known.root == repo.root) {
                    if *known != repo {
                        *known = repo;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.fill();
        }
    }

    /// Raíces de los repositorios desplegados, que son los que hay a la vista.
    pub fn expanded_roots(&self) -> Vec<PathBuf> {
        let expanded = self.expanded.borrow();
        self.repos.borrow().iter().filter(|r| expanded.contains(&r.root)).map(|r| r.root.clone()).collect()
    }

    /// Despliega un repositorio, por ejemplo el de la sesión a la vista.
    pub fn expand(&self, root: &Path) {
        if self.expanded.borrow_mut().insert(root.to_path_buf()) {
            self.fill();
        }
    }

    /// Marca el fichero abierto en la comparación, o ninguno.
    pub fn select(&self, selected: Option<(PathBuf, String)>) {
        *self.selected.borrow_mut() = selected;
        self.fill();
    }

    /// El repositorio que contiene una carpeta: el de raíz más larga.
    pub fn repo_of(&self, dir: &Path) -> Option<PathBuf> {
        self.repos
            .borrow()
            .iter()
            .filter(|repo| dir.starts_with(&repo.root))
            .max_by_key(|repo| repo.root.as_os_str().len())
            .map(|repo| repo.root.clone())
    }

    pub fn change_for(&self, root: &Path, path: &str) -> Option<Change> {
        self.repos
            .borrow()
            .iter()
            .find(|repo| repo.root == root)
            .and_then(|repo| repo.changes.iter().find(|c| c.path == path).cloned())
    }

    /// Rama de un repositorio y cuántos ficheros tiene con cambios.
    pub fn changes_in(&self, root: &Path) -> Option<(String, usize)> {
        self.repos
            .borrow()
            .iter()
            .find(|repo| repo.root == root)
            .map(|repo| (repo.branch.clone(), repo.changes.len()))
    }

    pub fn first_change(&self, root: &Path) -> Option<Change> {
        self.repos.borrow().iter().find(|repo| repo.root == root).and_then(|repo| repo.changes.first().cloned())
    }

    /// Cuántos ficheros cambiados hay entre todos los repositorios.
    pub fn total_changes(&self) -> usize {
        self.repos.borrow().iter().map(|repo| repo.changes.len()).sum()
    }

    fn fill(&self) {
        self.update_fold();
        self.list.remove_all();
        let repos = self.repos.borrow();
        let total: usize = repos.iter().map(|repo| repo.changes.len()).sum();
        self.summary.set_text(&match (repos.len(), total) {
            (0, _) => "Ninguna carpeta abierta es de git".to_string(),
            (1, 0) => "1 repositorio, sin cambios".to_string(),
            (n, 0) => format!("{n} repositorios, sin cambios"),
            (1, c) => format!("1 repositorio, {c} cambios"),
            (n, c) => format!("{n} repositorios, {c} cambios"),
        });
        let expanded = self.expanded.borrow();
        let collapsed = self.collapsed.borrow();
        let selected = self.selected.borrow();
        let mut chosen = None;
        for (index, repo) in repos.iter().enumerate() {
            let open = expanded.contains(&repo.root);
            let arrow = arrow_icon(open);
            arrow.add_css_class("mt-arrow");
            let name = gtk::Label::new(Some(
                &repo.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            ));
            name.add_css_class("mt-code-repo");
            name.set_xalign(0.0);
            name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            let branch = gtk::Label::new(Some(&repo.branch));
            branch.add_css_class("mt-code-branch");
            branch.set_xalign(0.0);
            branch.set_hexpand(true);
            branch.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            content.append(&arrow);
            content.append(&name);
            content.append(&branch);
            if !repo.changes.is_empty() {
                let count = gtk::Label::new(Some(&repo.changes.len().to_string()));
                count.add_css_class("mt-code-count");
                content.append(&count);
            }
            let row = gtk::ListBoxRow::new();
            row.add_css_class("mt-code-repo-row");
            row.set_widget_name(&format!("r:{index}"));
            row.set_selectable(false);
            row.set_tooltip_text(Some(&format!("{}\nRama {}", display_path(&repo.root), repo.branch)));
            row.set_child(Some(&content));
            self.list.append(&row);
            if !open {
                continue;
            }
            if repo.changes.is_empty() {
                let empty = gtk::Label::new(Some("Sin cambios"));
                empty.add_css_class("mt-stat-note");
                empty.set_xalign(0.0);
                empty.set_margin_start(28);
                let row = gtk::ListBoxRow::new();
                row.set_selectable(false);
                row.set_activatable(false);
                row.set_child(Some(&empty));
                self.list.append(&row);
                continue;
            }
            let open_here = selected.as_ref().filter(|(root, _)| root == &repo.root).map(|(_, path)| path.as_str());
            let prefix = format!("{}|", repo.root.display());
            self.add_nodes(index, &scm::tree(&repo.changes), "", 1, &prefix, &collapsed, open_here, &mut chosen);
        }
        self.list.select_row(chosen.as_ref());
    }

    #[allow(clippy::too_many_arguments)]
    fn add_nodes(
        &self,
        repo: usize,
        nodes: &[TreeNode],
        parent: &str,
        depth: i32,
        prefix: &str,
        collapsed: &HashSet<String>,
        open: Option<&str>,
        chosen: &mut Option<gtk::ListBoxRow>,
    ) {
        for node in nodes {
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            let row = gtk::ListBoxRow::new();
            match node {
                TreeNode::Folder { name, children } => {
                    let path = format!("{parent}{name}");
                    let folded = collapsed.contains(&format!("{prefix}{path}"));
                    let arrow = arrow_icon(!folded);
                    arrow.add_css_class("mt-arrow");
                    let label = gtk::Label::new(Some(name));
                    label.add_css_class("mt-code-folder");
                    label.set_xalign(0.0);
                    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                    content.set_margin_start(depth * 12);
                    content.append(&arrow);
                    content.append(&label);
                    row.set_widget_name(&format!("d:{repo}:{path}"));
                    row.set_selectable(false);
                    row.set_child(Some(&content));
                    self.list.append(&row);
                    if !folded {
                        self.add_nodes(repo, children, &format!("{path}/"), depth + 1, prefix, collapsed, open, chosen);
                    }
                }
                TreeNode::File { name, change } => {
                    let label = gtk::Label::new(Some(name));
                    label.set_xalign(0.0);
                    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                    // Lo nuevo va sobre verde y lo que se borra sobre rojo.
                    match change.status {
                        Status::Deleted => label.add_css_class("mt-code-gone"),
                        Status::Added | Status::Untracked => label.add_css_class("mt-code-new"),
                        _ => {}
                    }
                    let holder = gtk::Box::new(gtk::Orientation::Horizontal, 0);
                    holder.set_hexpand(true);
                    holder.append(&label);
                    let letter = gtk::Label::new(Some(change.status.letter()));
                    letter.add_css_class("mt-code-status");
                    letter.add_css_class(status_class(change.status));
                    // Hueco de la flecha de las carpetas, para que los nombres casen.
                    content.set_margin_start(depth * 12 + 16);
                    content.append(&file_icon(name));
                    content.append(&holder);
                    content.append(&letter);
                    row.set_widget_name(&format!("f:{repo}:{}", change.path));
                    let staged = if change.staged { ", preparado para confirmar" } else { "" };
                    row.set_tooltip_text(Some(&format!("{}: {}{staged}", change.path, change.status.label())));
                    row.set_child(Some(&content));
                    self.list.append(&row);
                    if open == Some(change.path.as_str()) {
                        *chosen = Some(row);
                    }
                }
            }
        }
    }
}

fn status_class(status: Status) -> &'static str {
    match status {
        Status::Modified | Status::Renamed => "s-modified",
        Status::Added | Status::Untracked => "s-added",
        Status::Deleted => "s-deleted",
        Status::Conflicted => "s-conflict",
    }
}

/// Icono del tipo de fichero, el que el tema de iconos del escritorio tenga
/// para su extensión.
fn file_icon(name: &str) -> gtk::Image {
    let (kind, _) = gio::content_type_guess(Some(name), None::<&[u8]>);
    let image = gtk::Image::from_gicon(&gio::content_type_get_icon(&kind));
    image.set_pixel_size(14);
    image
}

/// Flecha de plegar y desplegar.
fn arrow_icon(open: bool) -> gtk::Image {
    let image = gtk::Image::from_icon_name(if open { "pan-down-symbolic" } else { "pan-end-symbolic" });
    image.set_pixel_size(16);
    image
}
