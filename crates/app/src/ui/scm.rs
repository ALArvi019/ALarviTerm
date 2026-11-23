//! Comparación y edición de un fichero cambiado. Ocupa el sitio de los
//! paneles mientras está abierta y al cerrarse vuelven los terminales.
//!
//! A la izquierda va la versión de la última confirmación, solo lectura, y a
//! la derecha el fichero del árbol de trabajo, que se puede editar. Las
//! líneas quitadas y añadidas van resaltadas y las dos mitades se desplazan
//! a la par. Qué fichero se abre lo decide la pestaña de código de la barra
//! lateral.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use alarviterm_core::scm::{self, Change, LineDiff, LineKind, Status};
use gtk::{gdk, glib};
use sourceview::prelude::*;

/// Espera tras la última pulsación antes de volver a comparar.
const REDIFF_DELAY: Duration = Duration::from_millis(250);

const TAG_REMOVED: &str = "scm-removed";
const TAG_ADDED: &str = "scm-added";

/// Carpetas donde buscar lenguajes y esquemas de color cuando el sistema no
/// tiene instalado GtkSourceView: la que indique el entorno de desarrollo y
/// la que deja el instalador junto al ejecutable.
fn extra_data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(dir) = std::env::var_os("ALARVITERM_SOURCEVIEW_DATA").filter(|d| !d.is_empty()) {
        dirs.push(PathBuf::from(dir));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.parent()?.to_path_buf())) {
        dirs.push(dir.join("lib/alarviterm/gtksourceview-5"));
    }
    dirs.into_iter().filter(|d| d.is_dir()).collect()
}

/// Añade esas carpetas a los buscadores de lenguajes y de esquemas. Solo hace
/// falta una vez por proceso.
fn prepare_managers() {
    thread_local!(static DONE: Cell<bool> = const { Cell::new(false) });
    if DONE.with(|done| done.replace(true)) {
        return;
    }
    let extra = extra_data_dirs();
    if extra.is_empty() {
        return;
    }
    let languages = sourceview::LanguageManager::default();
    let mut paths: Vec<String> = languages.search_path().iter().map(|p| p.to_string()).collect();
    paths.extend(extra.iter().map(|d| d.join("language-specs").to_string_lossy().into_owned()));
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    languages.set_search_path(&refs);
    let schemes = sourceview::StyleSchemeManager::default();
    for dir in &extra {
        schemes.append_search_path(&dir.join("styles").to_string_lossy());
    }
}

/// Un visor de código con números de línea y su desplazamiento.
fn code_view(editable: bool) -> (sourceview::View, sourceview::Buffer, gtk::ScrolledWindow) {
    let buffer = sourceview::Buffer::new(None);
    buffer.set_highlight_matching_brackets(editable);
    let view = sourceview::View::with_buffer(&buffer);
    view.add_css_class("mt-code");
    view.set_monospace(true);
    view.set_show_line_numbers(true);
    view.set_editable(editable);
    view.set_cursor_visible(editable);
    view.set_highlight_current_line(editable);
    view.set_tab_width(4);
    view.set_auto_indent(true);
    view.set_left_margin(6);
    view.set_right_margin(6);
    view.set_hexpand(true);
    view.set_vexpand(true);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&view));
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);
    (view, buffer, scroller)
}

fn caption(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("mt-stat-caption");
    label.add_css_class("mt-code-caption");
    label.set_xalign(0.0);
    label
}

/// El fichero abierto en la comparación.
#[derive(Clone)]
struct Open {
    root: PathBuf,
    change: Change,
    /// Si se puede editar: existe y es texto.
    editable: bool,
}

/// Lo que la vista pide a la ventana.
pub struct CodeCallbacks {
    /// Cerrar la comparación y volver a los terminales.
    pub close: Box<dyn Fn()>,
    /// Se ha guardado un fichero: los cambios del repositorio pueden ser otros.
    pub saved: Box<dyn Fn()>,
    /// Un aviso corto para el usuario.
    pub notify: Box<dyn Fn(&str)>,
}

pub struct CodeView {
    pub root: gtk::Box,
    callbacks: CodeCallbacks,
    open: RefCell<Option<Open>>,
    title: gtk::Label,
    counts: gtk::Label,
    save: gtk::Button,
    stack: gtk::Stack,
    notice: adw::StatusPage,
    old_view: sourceview::View,
    old_buffer: sourceview::Buffer,
    old_scroller: gtk::ScrolledWindow,
    new_view: sourceview::View,
    new_buffer: sourceview::Buffer,
    new_scroller: gtk::ScrolledWindow,
    diff: RefCell<LineDiff>,
    rediff_pending: Cell<bool>,
    /// Evita que cargar un fichero dispare los avisos de edición.
    loading: Cell<bool>,
}

impl CodeView {
    pub fn new(callbacks: CodeCallbacks) -> Rc<Self> {
        prepare_managers();

        let (old_view, old_buffer, old_scroller) = code_view(false);
        let (new_view, new_buffer, new_scroller) = code_view(true);
        // Colores con transparencia: valen para el esquema claro y el oscuro.
        old_buffer.create_tag(Some(TAG_REMOVED), &[("paragraph-background", &"rgba(240, 85, 76, 0.22)")]);
        new_buffer.create_tag(Some(TAG_ADDED), &[("paragraph-background", &"rgba(63, 185, 80, 0.22)")]);

        let old_side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        old_side.append(&caption("ÚLTIMA CONFIRMACIÓN"));
        old_side.append(&old_scroller);
        let new_side = gtk::Box::new(gtk::Orientation::Vertical, 0);
        new_side.append(&caption("ÁRBOL DE TRABAJO, EDITABLE"));
        new_side.append(&new_scroller);
        let compare = gtk::Paned::new(gtk::Orientation::Horizontal);
        compare.set_start_child(Some(&old_side));
        compare.set_end_child(Some(&new_side));
        compare.set_resize_start_child(true);
        compare.set_resize_end_child(true);
        compare.set_shrink_start_child(false);
        compare.set_shrink_end_child(false);

        let notice = adw::StatusPage::new();
        notice.set_icon_name(Some("text-x-generic-symbolic"));
        let stack = gtk::Stack::new();
        stack.add_named(&notice, Some("notice"));
        stack.add_named(&compare, Some("compare"));

        let title = gtk::Label::new(None);
        title.add_css_class("mt-code-title");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        let counts = gtk::Label::new(None);
        counts.add_css_class("mt-stat-note");
        let previous = gtk::Button::from_icon_name("go-up-symbolic");
        previous.add_css_class("flat");
        previous.set_tooltip_text(Some("Cambio anterior (Alt+Arriba)"));
        let next = gtk::Button::from_icon_name("go-down-symbolic");
        next.add_css_class("flat");
        next.set_tooltip_text(Some("Cambio siguiente (Alt+Abajo)"));
        let save = gtk::Button::with_label("Guardar");
        save.add_css_class("suggested-action");
        save.set_tooltip_text(Some("Guardar el fichero (Ctrl+S)"));
        save.set_sensitive(false);
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("flat");
        close.set_tooltip_text(Some("Cerrar y volver a los terminales (Escape)"));
        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bar.add_css_class("mt-code-bar");
        bar.append(&title);
        bar.append(&counts);
        bar.append(&previous);
        bar.append(&next);
        bar.append(&save);
        bar.append(&close);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("mt-code-view");
        root.append(&bar);
        root.append(&stack);

        let view = Rc::new(Self {
            root,
            callbacks,
            open: RefCell::new(None),
            title,
            counts,
            save,
            stack,
            notice,
            old_view,
            old_buffer,
            old_scroller,
            new_view,
            new_buffer,
            new_scroller,
            diff: RefCell::new(LineDiff::default()),
            rediff_pending: Cell::new(false),
            loading: Cell::new(false),
        });
        view.apply_scheme();
        view.connect(&previous, &next, &close);
        view
    }

    fn connect(self: &Rc<Self>, previous: &gtk::Button, next: &gtk::Button, close: &gtk::Button) {
        let weak = Rc::downgrade(self);
        let with = move |f: fn(&Rc<CodeView>)| {
            let weak = weak.clone();
            move || {
                if let Some(view) = weak.upgrade() {
                    f(&view);
                }
            }
        };
        let on_save = with(|view| {
            view.save_now();
        });
        self.save.connect_clicked(move |_| on_save());
        let on_previous = with(|view| view.jump(-1));
        previous.connect_clicked(move |_| on_previous());
        let on_next = with(|view| view.jump(1));
        next.connect_clicked(move |_| on_next());
        let on_close = with(|view| view.request_close());
        close.connect_clicked(move |_| on_close());

        // Al editar: el botón de guardar y, con un respiro, la comparación.
        let weak = Rc::downgrade(self);
        self.new_buffer.connect_modified_changed(move |buffer| {
            if let Some(view) = weak.upgrade() {
                view.save.set_sensitive(buffer.is_modified());
                view.show_title();
            }
        });
        let weak = Rc::downgrade(self);
        self.new_buffer.connect_changed(move |_| {
            let Some(view) = weak.upgrade() else { return };
            if view.loading.get() || view.rediff_pending.replace(true) {
                return;
            }
            let weak = Rc::downgrade(&view);
            glib::timeout_add_local_once(REDIFF_DELAY, move || {
                if let Some(view) = weak.upgrade() {
                    view.rediff_pending.set(false);
                    view.compare();
                }
            });
        });

        // La mitad antigua sigue a la nueva al desplazarse.
        let weak = Rc::downgrade(self);
        self.new_scroller.vadjustment().connect_value_changed(move |_| {
            if let Some(view) = weak.upgrade() {
                view.follow_scroll();
            }
        });

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(view) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
            match key {
                gdk::Key::s | gdk::Key::S if control && !modifiers.contains(gdk::ModifierType::SHIFT_MASK) => {
                    view.save_now();
                }
                gdk::Key::Down if alt => view.jump(1),
                gdk::Key::Up if alt => view.jump(-1),
                gdk::Key::Escape => view.request_close(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        self.root.add_controller(keys);

        let weak = Rc::downgrade(self);
        adw::StyleManager::default().connect_dark_notify(move |_| {
            if let Some(view) = weak.upgrade() {
                view.apply_scheme();
            }
        });
    }

    fn apply_scheme(&self) {
        let dark = adw::StyleManager::default().is_dark();
        let scheme = sourceview::StyleSchemeManager::default().scheme(if dark { "Adwaita-dark" } else { "Adwaita" });
        for buffer in [&self.old_buffer, &self.new_buffer] {
            buffer.set_style_scheme(scheme.as_ref());
        }
    }

    pub fn is_modified(&self) -> bool {
        self.new_buffer.is_modified()
    }

    /// Fichero abierto: raíz de su árbol de trabajo y ruta dentro de él.
    pub fn current(&self) -> Option<(PathBuf, String)> {
        self.open.borrow().as_ref().map(|open| (open.root.clone(), open.change.path.clone()))
    }

    /// Ejecuta `action` si no hay cambios sin guardar. Si los hay, pregunta
    /// antes qué hacer con ellos. Con «Cancelar» no se ejecuta.
    pub fn guarded(self: &Rc<Self>, action: impl Fn() + 'static) {
        let Some(open) = self.open.borrow().clone().filter(|_| self.is_modified()) else {
            action();
            return;
        };
        let dialog = adw::AlertDialog::new(
            Some("Cambios sin guardar"),
            Some(&format!("{} tiene cambios que no has guardado.", open.change.path)),
        );
        dialog.add_response("cancel", "Cancelar");
        dialog.add_response("discard", "Descartar");
        dialog.add_response("save", "Guardar");
        dialog.set_close_response("cancel");
        dialog.set_default_response(Some("save"));
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            let Some(view) = weak.upgrade() else { return };
            match response {
                "save" => {
                    if view.save_now() {
                        action();
                    }
                }
                "discard" => {
                    view.new_buffer.set_modified(false);
                    action();
                }
                _ => {}
            }
        });
        dialog.present(Some(&self.root));
    }

    fn request_close(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.guarded(move || {
            if let Some(view) = weak.upgrade() {
                (view.callbacks.close)();
            }
        });
    }

    /// Carga las dos versiones de un fichero en la comparación. Quien llama
    /// se ocupa antes de los cambios sin guardar, con [`CodeView::guarded`].
    pub fn open(&self, root: &Path, change: &Change) {
        let path = change.path.as_str();
        let old = scm::original(root, path).unwrap_or_default();
        let current = if change.status == Status::Deleted { Ok(String::new()) } else { scm::read_text(root, path) };
        let editable = current.is_ok() && change.status != Status::Deleted;

        let language = sourceview::LanguageManager::default().guess_language(Some(path), None);
        self.loading.set(true);
        for buffer in [&self.old_buffer, &self.new_buffer] {
            buffer.set_language(language.as_ref());
            buffer.set_highlight_syntax(language.is_some());
        }
        self.old_buffer.set_text(&old);
        self.new_buffer.begin_irreversible_action();
        self.new_buffer.set_text(current.as_deref().unwrap_or(""));
        self.new_buffer.end_irreversible_action();
        self.new_buffer.set_modified(false);
        self.new_buffer.place_cursor(&self.new_buffer.start_iter());
        self.loading.set(false);
        self.new_view.set_editable(editable);
        *self.open.borrow_mut() = Some(Open { root: root.to_path_buf(), change: change.clone(), editable });
        self.show_title();

        match current {
            Err(error) => {
                self.notice.set_title(&change.path);
                self.notice.set_description(Some(&format!("No se puede comparar aquí: {error}.")));
                self.counts.set_text("");
                self.stack.set_visible_child_name("notice");
            }
            Ok(_) => {
                self.stack.set_visible_child_name("compare");
                self.compare();
                // Con la comparación recién pintada, a la vista el primer cambio.
                let first = self.diff.borrow().hunks().first().copied();
                if let Some(line) = first {
                    self.go_to_line(line);
                }
                self.new_view.grab_focus();
            }
        }
    }

    /// Vuelve a leer del disco el fichero abierto, si no se está editando.
    /// `change` es su estado actual, o `None` si ya no figura como cambiado.
    pub fn reload(&self, change: Option<&Change>) {
        if self.is_modified() {
            return;
        }
        let Some(open) = self.open.borrow().clone() else { return };
        let on_disk = scm::read_text(&open.root, &open.change.path).unwrap_or_default();
        if on_disk == Self::text_of(&self.new_buffer) && change.is_none_or(|c| c.status == open.change.status) {
            return;
        }
        // Se conserva la altura a la que se estaba leyendo.
        let offset = self.new_scroller.vadjustment().value();
        self.open(&open.root, change.unwrap_or(&open.change));
        let adjustment = self.new_scroller.vadjustment();
        glib::idle_add_local_once(move || adjustment.set_value(offset));
    }

    fn show_title(&self) {
        let open = self.open.borrow();
        let Some(open) = open.as_ref() else {
            self.title.set_text("");
            return;
        };
        let repo = open.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let edited = if self.is_modified() { "  ●" } else { "" };
        let note = if open.editable { "" } else { "  (solo lectura)" };
        self.title.set_text(&format!("{repo}  ›  {}{edited}{note}", open.change.path));
    }

    fn text_of(buffer: &sourceview::Buffer) -> String {
        let (start, end) = buffer.bounds();
        buffer.text(&start, &end, false).to_string()
    }

    /// Compara las dos mitades y resalta lo que cambia.
    fn compare(&self) {
        let diff = scm::diff_lines(&Self::text_of(&self.old_buffer), &Self::text_of(&self.new_buffer));
        for (buffer, tag, kinds) in
            [(&self.old_buffer, TAG_REMOVED, &diff.old), (&self.new_buffer, TAG_ADDED, &diff.new)]
        {
            let (start, end) = buffer.bounds();
            buffer.remove_tag_by_name(tag, &start, &end);
            // Los tramos seguidos se marcan de una vez.
            let mut line = 0;
            while line < kinds.len() {
                if kinds[line] != LineKind::Changed {
                    line += 1;
                    continue;
                }
                let first = line;
                while line < kinds.len() && kinds[line] == LineKind::Changed {
                    line += 1;
                }
                let (Some(from), to) = (buffer.iter_at_line(first as i32), buffer.iter_at_line(line as i32)) else {
                    continue;
                };
                buffer.apply_tag_by_name(tag, &from, &to.unwrap_or_else(|| buffer.end_iter()));
            }
        }
        self.counts.set_text(&match (diff.added, diff.removed) {
            (0, 0) => "sin diferencias".to_string(),
            (added, removed) => format!("+{added}  −{removed}"),
        });
        *self.diff.borrow_mut() = diff;
        self.follow_scroll();
    }

    /// Coloca la mitad antigua a la altura de lo que enseña la nueva.
    fn follow_scroll(&self) {
        let offset = self.new_scroller.vadjustment().value();
        let Some(top) = self.new_view.iter_at_location(0, offset as i32) else {
            return;
        };
        // Cuánto de la primera línea visible queda ya por encima.
        let (line_y, _) = self.new_view.line_yrange(&top);
        let inside = offset - f64::from(line_y);
        let old_line = self.diff.borrow().old_line_for(top.line().max(0) as usize);
        let target = self
            .old_buffer
            .iter_at_line(old_line as i32)
            .unwrap_or_else(|| self.old_buffer.end_iter());
        let (old_y, _) = self.old_view.line_yrange(&target);
        let adjustment = self.old_scroller.vadjustment();
        let limit = (adjustment.upper() - adjustment.page_size()).max(0.0);
        adjustment.set_value((f64::from(old_y) + inside).clamp(0.0, limit));
    }

    fn go_to_line(&self, line: usize) {
        let Some(mut iter) = self.new_buffer.iter_at_line(line as i32) else {
            return;
        };
        self.new_buffer.place_cursor(&iter);
        self.new_view.scroll_to_iter(&mut iter, 0.0, true, 0.0, 0.2);
    }

    /// Salta al bloque de cambios siguiente o anterior al cursor.
    fn jump(&self, step: i32) {
        let hunks = self.diff.borrow().hunks();
        if hunks.is_empty() {
            return;
        }
        let cursor = self.new_buffer.iter_at_mark(&self.new_buffer.get_insert()).line().max(0) as usize;
        let target = if step > 0 {
            hunks.iter().find(|line| **line > cursor).or(hunks.first())
        } else {
            hunks.iter().rev().find(|line| **line < cursor).or(hunks.last())
        };
        if let Some(line) = target {
            self.go_to_line(*line);
            self.new_view.grab_focus();
        }
    }

    /// Guarda el fichero abierto. Devuelve si quedó guardado.
    fn save_now(&self) -> bool {
        let Some(open) = self.open.borrow().clone() else {
            return false;
        };
        if !open.editable {
            (self.callbacks.notify)("Este fichero no se puede editar aquí");
            return false;
        }
        if !self.is_modified() {
            return true;
        }
        match scm::save(&open.root, &open.change.path, &Self::text_of(&self.new_buffer)) {
            Ok(()) => {
                self.new_buffer.set_modified(false);
                (self.callbacks.notify)(&format!("{} guardado", open.change.path));
                (self.callbacks.saved)();
                true
            }
            Err(error) => {
                (self.callbacks.notify)(&format!("No se pudo guardar: {error}"));
                false
            }
        }
    }
}
