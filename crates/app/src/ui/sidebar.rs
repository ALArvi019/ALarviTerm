//! Barra lateral: grupos plegables con sus sesiones y el estado de cada una.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use alarviterm_core::model::Project;
use alarviterm_core::plugins::{self, Tone};
use alarviterm_core::search::{SessionFilter, StatusFilter};
use alarviterm_core::sessions::{SessionView, Snapshot};

use super::status;

/// A qué se refiere una fila de la lista.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowTarget {
    Session(String),
    Group(i64),
    Ungrouped,
}

const UNGROUPED_NAME: &str = "u";

pub fn row_target(row: &gtk::ListBoxRow) -> Option<RowTarget> {
    let name = row.widget_name();
    if name == UNGROUPED_NAME {
        Some(RowTarget::Ungrouped)
    } else if let Some(id) = name.strip_prefix("s:") {
        Some(RowTarget::Session(id.to_string()))
    } else if let Some(id) = name.strip_prefix("g:") {
        id.parse().ok().map(RowTarget::Group)
    } else {
        None
    }
}

struct SessionRow {
    row: gtk::ListBoxRow,
    dot: gtk::Box,
    detail: gtk::Label,
    /// Lo que añaden los complementos: etiqueta, cronómetro y botones.
    extras: gtk::Box,
}

/// Lo que un complemento pone en la fila de una sesión.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RowExtra {
    pub badge: Option<String>,
    pub tone: Tone,
    pub timer_since: Option<i64>,
    pub tooltip: Option<String>,
    pub buttons: Vec<RowButton>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowButton {
    pub label: String,
    pub icon: Option<String>,
    /// Parámetro de la acción `win.plugin-action`.
    pub target: String,
}

pub struct Sidebar {
    pub root: gtk::Box,
    pub list: gtk::ListBox,
    menu: gtk::PopoverMenu,
    rows: RefCell<HashMap<String, SessionRow>>,
    /// Sesiones visibles, en el orden en que aparecen.
    order: RefCell<Vec<String>>,
    search_bar: gtk::Revealer,
    search: gtk::SearchEntry,
    status: gtk::DropDown,
    search_toggle: gtk::ToggleButton,
    /// Texto de la lista vacía, que cita el atajo vigente.
    empty_hint: RefCell<String>,
    /// Cronómetros de los complementos a la vista, con su instante de inicio.
    timers: RefCell<Vec<(gtk::Label, i64)>>,
}

/// Qué hace la ventana con lo que ocurre en el buscador.
pub struct SearchCallbacks {
    /// Cambió el texto o el filtro de estado.
    pub changed: Rc<dyn Fn()>,
    /// Intro: abrir la primera sesión que coincide.
    pub accepted: Rc<dyn Fn()>,
    /// Se cerró el buscador.
    pub closed: Rc<dyn Fn()>,
}

impl Sidebar {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("mt-sidebar");
        root.set_size_request(210, -1);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        header.set_margin_top(10);
        header.set_margin_bottom(2);
        header.set_margin_start(14);
        header.set_margin_end(6);
        let title = gtk::Label::new(Some("SESIONES"));
        title.add_css_class("mt-sidebar-title");
        title.set_hexpand(true);
        title.set_xalign(0.0);
        let add_group = gtk::Button::from_icon_name("folder-new-symbolic");
        add_group.add_css_class("flat");
        add_group.set_tooltip_text(Some("Nuevo grupo"));
        add_group.set_action_name(Some("win.new-group"));
        let search_toggle = gtk::ToggleButton::new();
        search_toggle.set_icon_name("system-search-symbolic");
        search_toggle.add_css_class("flat");
        header.append(&title);
        header.append(&search_toggle);
        header.append(&add_group);

        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Nombre, carpeta o grupo"));
        search.set_search_delay(60);
        search.set_hexpand(true);
        let labels: Vec<&str> = StatusFilter::ALL.iter().map(|f| f.label()).collect();
        let status = gtk::DropDown::from_strings(&labels);
        status.set_tooltip_text(Some("Filtrar por estado"));
        status.add_css_class("mt-filter");
        let search_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        search_box.set_margin_top(4);
        search_box.set_margin_bottom(4);
        search_box.set_margin_start(10);
        search_box.set_margin_end(8);
        search_box.append(&search);
        search_box.append(&status);
        let search_bar = gtk::Revealer::new();
        search_bar.set_transition_duration(110);
        search_bar.set_child(Some(&search_box));

        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.set_activate_on_single_click(true);
        list.set_margin_bottom(8);

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&list));

        root.append(&header);
        root.append(&search_bar);
        root.append(&scroller);

        let menu = gtk::PopoverMenu::from_model(gio::MenuModel::NONE);
        // Cuelga del contenedor y no de la lista: vaciar una lista con un
        // popover como hijo no termina nunca.
        menu.set_parent(&root);
        menu.set_has_arrow(false);
        menu.set_halign(gtk::Align::Start);
        let menu_weak = menu.downgrade();
        root.connect_destroy(move |_| {
            if let Some(menu) = menu_weak.upgrade() {
                menu.unparent();
            }
        });

        Self {
            root,
            list,
            menu,
            rows: RefCell::new(HashMap::new()),
            order: RefCell::new(Vec::new()),
            search_bar,
            search,
            status,
            search_toggle,
            empty_hint: RefCell::new(String::new()),
            timers: RefCell::new(Vec::new()),
        }
    }

    pub fn connect_search(&self, callbacks: SearchCallbacks) {
        let changed = callbacks.changed.clone();
        self.search.connect_search_changed(move |_| changed());
        let changed = callbacks.changed.clone();
        self.status.connect_selected_notify(move |_| changed());
        let accepted = callbacks.accepted.clone();
        self.search.connect_activate(move |_| accepted());

        let toggle = self.search_toggle.downgrade();
        self.search.connect_stop_search(move |_| {
            if let Some(toggle) = toggle.upgrade() {
                toggle.set_active(false);
            }
        });
        // Con el desplegable enfocado, Esc también cierra el buscador.
        let keys = gtk::EventControllerKey::new();
        let toggle = self.search_toggle.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| match (key, toggle.upgrade()) {
            (gdk::Key::Escape, Some(toggle)) => {
                toggle.set_active(false);
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        });
        self.search_bar.add_controller(keys);

        // El botón es la única fuente de verdad de si el buscador está abierto.
        let (bar, search, status) = (self.search_bar.clone(), self.search.clone(), self.status.clone());
        let closed = callbacks.closed.clone();
        self.search_toggle.connect_toggled(move |toggle| {
            let open = toggle.is_active();
            bar.set_reveal_child(open);
            if open {
                search.grab_focus();
            } else {
                // Cerrar el buscador quita el filtro: una lista filtrada sin
                // nada a la vista que lo diga confunde.
                search.set_text("");
                status.set_selected(0);
                closed();
            }
        });
    }

    pub fn set_hints(&self, search_shortcut: Option<&str>, new_session_shortcut: Option<&str>) {
        let tooltip = match search_shortcut {
            Some(keys) => format!("Buscar y filtrar sesiones ({keys})"),
            None => "Buscar y filtrar sesiones".to_string(),
        };
        self.search_toggle.set_tooltip_text(Some(&tooltip));
        *self.empty_hint.borrow_mut() = match new_session_shortcut {
            Some(keys) => format!("Aún no hay sesiones.\n{keys} crea la primera."),
            None => "Aún no hay sesiones.".to_string(),
        };
    }

    /// Abre el buscador y le da el foco. Si ya lo tiene, lo cierra.
    pub fn toggle_search(&self) {
        let focused = self.search.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN);
        if self.search_toggle.is_active() && !focused {
            self.search.grab_focus();
            // Lo escrito antes queda seleccionado, para sustituirlo al teclear.
            self.search.select_region(0, -1);
        } else {
            self.search_toggle.set_active(!self.search_toggle.is_active());
        }
    }

    /// Muestra el buscador con un filtro de estado ya elegido.
    pub fn set_status_filter(&self, filter: StatusFilter) {
        self.search_toggle.set_active(true);
        let index = StatusFilter::ALL.iter().position(|f| *f == filter).unwrap_or(0);
        self.status.set_selected(index as u32);
    }

    pub fn filter(&self) -> SessionFilter {
        if !self.search_toggle.is_active() {
            return SessionFilter::default();
        }
        SessionFilter {
            query: self.search.text().to_string(),
            status: StatusFilter::ALL
                .get(self.status.selected() as usize)
                .copied()
                .unwrap_or_default(),
        }
    }

    /// Texto que resume lo que la lista enseña. Si no cambia entre dos
    /// refrescos basta con actualizar los estados.
    pub fn signature(&self, snapshot: &Snapshot) -> String {
        let filter = self.filter();
        let mut signature = String::new();
        for group in &snapshot.groups {
            signature.push_str(&format!("g{}:{}:{};", group.id, group.name, group.collapsed));
        }
        for view in &snapshot.sessions {
            signature.push_str(&format!(
                "s{}:{}:{:?}:{:?}",
                view.session.id, view.session.name, view.session.group_id, view.session.project_id
            ));
            // Qué sesiones pasan el filtro depende también de su estado.
            let (group, project) = (group_name(snapshot, view), project_name(snapshot, view));
            signature.push_str(if filter.accepts(view, group, project) { ":+" } else { ":-" });
            signature.push(';');
        }
        for project in &snapshot.projects {
            signature.push_str(&format!("p{}:{}:{}:{};", project.id, project.name, project.color, project.icon));
        }
        if filter.is_active() {
            signature.push_str(&format!("f:{}:{}", filter.status.as_str(), filter.query));
        }
        signature
    }

    /// Reconstruye la lista entera. Solo hace falta cuando cambia la
    /// estructura (altas, bajas, renombrados, movimientos, plegados).
    pub fn rebuild(&self, snapshot: &Snapshot, selected: Option<&str>, ungrouped_collapsed: bool) {
        self.list.remove_all();
        let mut rows = HashMap::new();
        let mut order = Vec::new();
        let now = alarviterm_core::now_unix();
        let filter = self.filter();
        let filtering = filter.is_active();
        // El filtro se consulta siempre: sin nada elegido oculta las archivadas.
        let accepts =
            |view: &SessionView| filter.accepts(view, group_name(snapshot, view), project_name(snapshot, view));

        let mut add_sessions = |sessions: &mut dyn Iterator<Item = &SessionView>| {
            for view in sessions {
                let entry = session_row(view, project_of(snapshot, view), now);
                self.list.append(&entry.row);
                order.push(view.session.id.clone());
                rows.insert(view.session.id.clone(), entry);
            }
        };

        for group in &snapshot.groups {
            let members = || {
                snapshot
                    .sessions
                    .iter()
                    .filter(|v| v.session.group_id == Some(group.id) && accepts(v))
            };
            // Al filtrar se ocultan los grupos sin coincidencias y los demás
            // se muestran abiertos, estén o no plegados.
            if filtering && members().next().is_none() {
                continue;
            }
            let collapsed = group.collapsed && !filtering;
            self.list.append(&group_row(
                &format!("g:{}", group.id),
                &group.name,
                members().count(),
                collapsed,
            ));
            if !collapsed {
                add_sessions(&mut members());
            }
        }

        let ungrouped_collapsed = ungrouped_collapsed && !filtering;
        let ungrouped = || {
            snapshot
                .sessions
                .iter()
                .filter(|v| v.session.group_id.is_none() && accepts(v))
        };
        if ungrouped().next().is_some() {
            // Sin grupos no hace falta cabecera: la lista es plana.
            if snapshot.groups.is_empty() {
                add_sessions(&mut ungrouped());
            } else {
                self.list.append(&group_row(
                    UNGROUPED_NAME,
                    "Sin grupo",
                    ungrouped().count(),
                    ungrouped_collapsed,
                ));
                if !ungrouped_collapsed {
                    add_sessions(&mut ungrouped());
                }
            }
        }

        let nothing_matches = filtering && order.is_empty();
        if nothing_matches || (snapshot.sessions.is_empty() && snapshot.groups.is_empty()) {
            let text = if nothing_matches {
                "Ninguna sesión coincide.".to_string()
            } else {
                self.empty_hint.borrow().clone()
            };
            let hint = gtk::Label::new(Some(&text));
            hint.add_css_class("mt-empty-hint");
            hint.set_xalign(0.0);
            hint.set_margin_top(8);
            hint.set_margin_start(8);
            let row = gtk::ListBoxRow::new();
            row.set_selectable(false);
            row.set_activatable(false);
            row.set_child(Some(&hint));
            self.list.append(&row);
        }

        let selected_row = selected.and_then(|id| rows.get(id)).map(|r| r.row.clone());
        *self.rows.borrow_mut() = rows;
        *self.order.borrow_mut() = order;
        self.list.select_row(selected_row.as_ref());
    }

    /// Actualiza indicadores y textos sin tocar la estructura de la lista.
    pub fn update_status(&self, snapshot: &Snapshot) {
        let now = alarviterm_core::now_unix();
        let rows = self.rows.borrow();
        for view in &snapshot.sessions {
            if let Some(entry) = rows.get(&view.session.id) {
                apply_status(entry, view, now);
            }
        }
    }

    /// Pone en cada fila lo que los complementos dicen de su sesión. Hay que
    /// llamarlo también tras `rebuild`, que crea las filas vacías.
    pub fn set_extras(&self, extras: &HashMap<String, Vec<RowExtra>>) {
        let now = alarviterm_core::now_unix();
        let mut timers = Vec::new();
        for (id, entry) in self.rows.borrow().iter() {
            while let Some(child) = entry.extras.first_child() {
                entry.extras.remove(&child);
            }
            for extra in extras.get(id).map(Vec::as_slice).unwrap_or_default() {
                if let Some(text) = &extra.badge {
                    let badge = gtk::Label::new(Some(text));
                    badge.add_css_class("mt-plugin-badge");
                    if let Some(class) = tone_class(extra.tone) {
                        badge.add_css_class(class);
                    }
                    badge.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    badge.set_max_width_chars(12);
                    badge.set_tooltip_text(extra.tooltip.as_deref());
                    entry.extras.append(&badge);
                }
                for button in &extra.buttons {
                    let widget = match &button.icon {
                        Some(icon) => gtk::Button::from_icon_name(icon),
                        None => gtk::Button::with_label(&button.label),
                    };
                    widget.add_css_class("flat");
                    widget.add_css_class("mt-plugin-button");
                    widget.set_valign(gtk::Align::Center);
                    widget.set_focus_on_click(false);
                    widget.set_tooltip_text(Some(&button.label));
                    widget.set_action_name(Some("win.plugin-action"));
                    widget.set_action_target_value(Some(&button.target.to_variant()));
                    entry.extras.append(&widget);
                }
                if let Some(since) = extra.timer_since {
                    let timer = gtk::Label::new(Some(&plugins::timer_text(since, now)));
                    timer.add_css_class("mt-plugin-timer");
                    timer.set_tooltip_text(extra.tooltip.as_deref());
                    entry.extras.append(&timer);
                    timers.push((timer, since));
                }
            }
            entry.extras.set_visible(entry.extras.first_child().is_some());
        }
        *self.timers.borrow_mut() = timers;
    }

    /// Avanza los cronómetros. Devuelve si hay alguno a la vista.
    pub fn tick(&self) -> bool {
        let now = alarviterm_core::now_unix();
        let timers = self.timers.borrow();
        for (label, since) in timers.iter() {
            label.set_text(&plugins::timer_text(*since, now));
        }
        !timers.is_empty()
    }

    pub fn select(&self, id: Option<&str>) {
        let row = id.and_then(|id| self.rows.borrow().get(id).map(|r| r.row.clone()));
        self.list.select_row(row.as_ref());
    }

    pub fn visible_sessions(&self) -> Vec<String> {
        self.order.borrow().clone()
    }

    /// Fila bajo la coordenada vertical dada, relativa a la lista.
    pub fn target_at(&self, y: f64) -> Option<RowTarget> {
        self.list.row_at_y(y as i32).and_then(|row| row_target(&row))
    }

    /// Abre el menú contextual en un punto dado en coordenadas de la lista.
    pub fn show_menu(&self, model: &gio::Menu, x: f64, y: f64) {
        let (x, y) = self
            .list
            .compute_point(&self.root, &gtk::graphene::Point::new(x as f32, y as f32))
            .map(|p| (f64::from(p.x()), f64::from(p.y())))
            .unwrap_or((x, y));
        self.menu.set_menu_model(Some(model));
        self.menu
            .set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        self.menu.popup();
    }
}

fn group_row(widget_name: &str, name: &str, count: usize, collapsed: bool) -> gtk::ListBoxRow {
    let content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let arrow = gtk::Label::new(Some(if collapsed { "▸" } else { "▾" }));
    arrow.add_css_class("mt-arrow");
    let label = gtk::Label::new(Some(&name.to_uppercase()));
    label.add_css_class("mt-group-name");
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let counter = gtk::Label::new(Some(&count.to_string()));
    counter.add_css_class("mt-group-count");
    content.append(&arrow);
    content.append(&label);
    content.append(&counter);

    let row = gtk::ListBoxRow::new();
    row.set_widget_name(widget_name);
    row.add_css_class("mt-group");
    row.set_selectable(false);
    row.set_child(Some(&content));
    row
}

fn session_row(view: &SessionView, project: Option<&Project>, now: i64) -> SessionRow {
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("mt-dot");
    dot.set_valign(gtk::Align::Center);

    let name = gtk::Label::new(Some(&view.session.name));
    name.add_css_class("mt-session-name");
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);

    let detail = gtk::Label::new(None);
    detail.add_css_class("mt-session-detail");
    detail.set_xalign(0.0);
    detail.set_ellipsize(gtk::pango::EllipsizeMode::Middle);

    name.set_hexpand(true);
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    title.append(&name);
    if let Some(project) = project {
        let chip = gtk::Label::new(Some(&super::projects::label(project)));
        chip.add_css_class("mt-project");
        if let Some(class) = super::projects::color_class(project) {
            chip.add_css_class(&class);
        }
        chip.set_ellipsize(gtk::pango::EllipsizeMode::End);
        chip.set_max_width_chars(14);
        chip.set_valign(gtk::Align::Center);
        chip.set_tooltip_text(Some(&format!("Proyecto {}", project.name)));
        title.append(&chip);
    }

    let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
    texts.set_hexpand(true);
    texts.append(&title);
    texts.append(&detail);

    let extras = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    extras.set_valign(gtk::Align::Center);
    extras.set_visible(false);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    content.append(&dot);
    content.append(&texts);
    content.append(&extras);

    let row = gtk::ListBoxRow::new();
    row.set_widget_name(&format!("s:{}", view.session.id));
    row.add_css_class("mt-session");
    if view.session.archived {
        row.add_css_class("mt-archived");
    }
    row.set_child(Some(&content));

    super::dnd::make_draggable(&row, &view.session.id);

    let entry = SessionRow { row, dot, detail, extras };
    apply_status(&entry, view, now);
    entry
}

fn apply_status(entry: &SessionRow, view: &SessionView, now: i64) {
    let info = status::describe(view);
    for class in status::STATUS_CLASSES {
        entry.dot.remove_css_class(class);
    }
    entry.dot.add_css_class(info.class);

    let mut detail = info.label.clone();
    if let Some(agent) = &view.agent {
        // Con eventos de Claude Code interesa desde cuándo está así.
        if now - agent.since >= 60 {
            detail.push_str(", ");
            detail.push_str(&status::relative_time(now, agent.since));
        }
    } else if view.liveness.is_alive() {
        if let Some(at) = view.last_output_at {
            detail.push_str(", salida ");
            detail.push_str(&status::relative_time(now, at));
        }
    }
    entry.detail.set_text(&detail);

    entry.dot.set_tooltip_text(Some(&info.tooltip));
}

fn tone_class(tone: Tone) -> Option<&'static str> {
    match tone {
        Tone::Normal => None,
        Tone::Ok => Some("t-ok"),
        Tone::Warn => Some("t-warn"),
        Tone::Muted => Some("t-muted"),
    }
}

fn project_of<'a>(snapshot: &'a Snapshot, view: &SessionView) -> Option<&'a Project> {
    let id = view.session.project_id?;
    snapshot.projects.iter().find(|p| p.id == id)
}

fn project_name<'a>(snapshot: &'a Snapshot, view: &SessionView) -> Option<&'a str> {
    project_of(snapshot, view).map(|p| p.name.as_str())
}

fn group_name<'a>(snapshot: &'a Snapshot, view: &SessionView) -> Option<&'a str> {
    let id = view.session.group_id?;
    snapshot.groups.iter().find(|g| g.id == id).map(|g| g.name.as_str())
}

/// Entrada de menú con una acción de ventana y su parámetro de texto.
pub fn menu_item(label: &str, action: &str, target: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some(action), Some(&target.to_variant()));
    item
}

pub fn plain_item(label: &str, action: &str) -> gio::MenuItem {
    gio::MenuItem::new(Some(label), Some(action))
}

