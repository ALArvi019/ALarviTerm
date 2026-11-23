//! Paneles: cada uno tiene una cabecera con pestañas y botones, y debajo el
//! terminal de la pestaña activa. Aquí solo hay widgets. Qué paneles existen
//! y qué contienen lo decide `alarviterm_core::layout`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use alarviterm_core::layout::{Node, NodeId, Orientation};

use super::dnd::{self, SessionRef};
use super::icons::{self, Icon};
use super::menus;
use super::status;

const PLACEHOLDER: &str = "placeholder";

/// Qué se muestra en el hueco de una pestaña.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildKind {
    Terminal,
    /// El backend no tiene la sesión.
    Missing,
    /// La sesión vive, pero su terminal perdió la conexión.
    Detached,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropZone {
    Center,
    Left,
    Right,
    Top,
    Bottom,
}

pub struct TabInfo {
    pub session_id: String,
    pub name: String,
    pub status_class: &'static str,
}

#[derive(Clone)]
pub struct PaneCallbacks {
    /// El panel ha recibido el foco o un clic.
    pub focused: Rc<dyn Fn(NodeId)>,
    /// Se ha soltado una sesión sobre una zona del panel.
    pub dropped: Rc<dyn Fn(NodeId, DropZone, String)>,
}

pub struct PaneView {
    pub id: NodeId,
    pub root: gtk::Box,
    tabs_box: gtk::Box,
    tabs_signature: RefCell<String>,
    maximize: gtk::Button,
    close: gtk::Button,
    stack: gtk::Stack,
    children: RefCell<HashMap<String, (ChildKind, gtk::Widget)>>,
}

impl PaneView {
    pub fn new(id: NodeId, callbacks: &PaneCallbacks) -> Rc<Self> {
        let target = id.to_string();

        let tabs_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        scroller.set_hexpand(true);
        scroller.set_child(Some(&tabs_box));

        let split_right = icons::button(
            Icon::SplitRight,
            14,
            "Dividir a la derecha (Ctrl+Mayús+D)",
            "win.pane-split-right",
            &target,
        );
        let split_down = icons::button(
            Icon::SplitDown,
            14,
            "Dividir hacia abajo (Ctrl+Mayús+E)",
            "win.pane-split-down",
            &target,
        );
        let maximize = icons::button(
            Icon::Maximize,
            14,
            "Maximizar o restaurar el panel (Ctrl+Mayús+Intro)",
            "win.pane-maximize",
            &target,
        );
        let close = icons::button(Icon::Close, 14, "Cerrar el panel", "win.pane-close", &target);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        header.add_css_class("mt-pane-header");
        header.append(&scroller);
        for button in [&split_right, &split_down, &maximize, &close] {
            header.append(button);
        }

        let placeholder = gtk::Label::new(Some(
            "Panel vacío\nElige una sesión en la barra lateral o arrástrala aquí",
        ));
        placeholder.set_justify(gtk::Justification::Center);
        placeholder.add_css_class("mt-pane-placeholder");
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::None);
        stack.set_hexpand(true);
        stack.set_vexpand(true);
        stack.add_named(&placeholder, Some(PLACEHOLDER));

        let hint = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        hint.add_css_class("mt-drop-hint");
        hint.set_can_target(false);
        hint.set_visible(false);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&stack));
        overlay.add_overlay(&hint);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("mt-pane");
        root.set_size_request(160, 90);
        root.append(&header);
        root.append(&overlay);

        // Foco: por teclado (entra en el terminal) o por clic en cualquier parte.
        let focus = gtk::EventControllerFocus::new();
        let on_focus = callbacks.focused.clone();
        focus.connect_enter(move |_| on_focus(id));
        root.add_controller(focus);
        let click = gtk::GestureClick::new();
        click.set_button(0);
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let on_click = callbacks.focused.clone();
        click.connect_pressed(move |_, _, _, _| on_click(id));
        root.add_controller(click);

        // Soltar sesiones: en el centro se añaden como pestaña, en un borde
        // dividen el panel hacia ese lado.
        let drop = gtk::DropTarget::new(SessionRef::static_type(), gdk::DragAction::MOVE);
        {
            let (hint, overlay) = (hint.clone(), overlay.downgrade());
            drop.connect_motion(move |_, x, y| {
                if let Some(overlay) = overlay.upgrade() {
                    let (w, h) = (overlay.width(), overlay.height());
                    place_hint(&hint, zone_at(x, y, w, h), w, h);
                }
                gdk::DragAction::MOVE
            });
        }
        {
            let hint = hint.clone();
            drop.connect_leave(move |_| hint.set_visible(false));
        }
        {
            let (hint, overlay) = (hint.clone(), overlay.downgrade());
            let on_drop = callbacks.dropped.clone();
            drop.connect_drop(move |_, value, x, y| {
                hint.set_visible(false);
                let (Ok(session), Some(overlay)) = (value.get::<SessionRef>(), overlay.upgrade())
                else {
                    return false;
                };
                on_drop(id, zone_at(x, y, overlay.width(), overlay.height()), session.0);
                true
            });
        }
        overlay.add_controller(drop);

        Rc::new(Self {
            id,
            root,
            tabs_box,
            tabs_signature: RefCell::new(String::new()),
            maximize,
            close,
            stack,
            children: RefCell::new(HashMap::new()),
        })
    }

    /// Redibuja las pestañas si algo ha cambiado.
    pub fn set_tabs(&self, tabs: &[TabInfo], active: Option<&str>) {
        let signature = tabs.iter().fold(format!("{active:?}"), |acc, t| {
            format!("{acc}|{}:{}:{}", t.session_id, t.name, t.status_class)
        });
        if *self.tabs_signature.borrow() == signature {
            return;
        }
        *self.tabs_signature.borrow_mut() = signature;

        while let Some(child) = self.tabs_box.first_child() {
            self.tabs_box.remove(&child);
        }
        if tabs.is_empty() {
            let label = gtk::Label::new(Some("Sin sesión"));
            label.add_css_class("mt-tab-empty");
            self.tabs_box.append(&label);
            return;
        }
        for tab in tabs {
            let is_active = active == Some(tab.session_id.as_str());
            self.tabs_box.append(&self.tab_widget(tab, is_active));
        }
    }

    fn tab_widget(&self, tab: &TabInfo, active: bool) -> gtk::Box {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("mt-dot");
        dot.add_css_class(tab.status_class);
        dot.set_valign(gtk::Align::Center);

        let label = gtk::Label::new(Some(&tab.name));
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(24);

        let close = icons::button(
            Icon::Close,
            12,
            "Cerrar la pestaña. La sesión sigue en marcha",
            "win.tab-close",
            &tab.session_id,
        );
        close.add_css_class("mt-tab-close");

        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 7);
        widget.add_css_class("mt-tab");
        if active {
            widget.add_css_class("active");
        }
        widget.append(&dot);
        widget.append(&label);
        widget.append(&close);

        // Se actúa al soltar y no al pulsar: pulsar y mover inicia un arrastre.
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let pane = self.id;
        let session = tab.session_id.clone();
        click.connect_released(move |gesture, presses, _, _| {
            let widget = gesture.widget();
            let Some(widget) = widget else { return };
            let (action, target) = match (gesture.current_button(), presses) {
                (gdk::BUTTON_MIDDLE, _) => ("win.tab-close", session.clone()),
                (gdk::BUTTON_PRIMARY, 2) => ("win.pane-maximize", pane.to_string()),
                (gdk::BUTTON_PRIMARY, _) => ("win.tab-select", format!("{pane}|{session}")),
                (gdk::BUTTON_SECONDARY, _) => {
                    tab_menu(&widget, &session);
                    return;
                }
                _ => return,
            };
            if let Err(e) = widget.activate_action(action, Some(&target.to_variant())) {
                log::error!("acción {action} no disponible: {e}");
            }
        });
        widget.add_controller(click);
        dnd::make_draggable(&widget, &tab.session_id);

        // Soltar otra pestaña encima la coloca a este lado o al otro, según
        // en qué mitad caiga. Así se reordenan y se pasan entre paneles.
        let drop = gtk::DropTarget::new(SessionRef::static_type(), gdk::DragAction::MOVE);
        let side = |widget: &gtk::Widget, x: f64| if x > f64::from(widget.width()) / 2.0 { "after" } else { "before" };
        let mark = move |target: &gtk::DropTarget, x: f64| {
            if let Some(widget) = target.widget() {
                let after = side(&widget, x) == "after";
                set_class(&widget, "drop-after", after);
                set_class(&widget, "drop-before", !after);
            }
            gdk::DragAction::MOVE
        };
        drop.connect_enter(move |target, x, _| mark(target, x));
        drop.connect_motion(move |target, x, _| mark(target, x));
        drop.connect_leave(|target| {
            if let Some(widget) = target.widget() {
                widget.remove_css_class("drop-after");
                widget.remove_css_class("drop-before");
            }
        });
        let anchor = tab.session_id.clone();
        drop.connect_drop(move |target, value, x, _| {
            let (Some(widget), Ok(dragged)) = (target.widget(), value.get::<SessionRef>()) else {
                return false;
            };
            widget.remove_css_class("drop-after");
            widget.remove_css_class("drop-before");
            if dragged.0 == anchor {
                return false;
            }
            let destination = format!("{}|{anchor}|{}", dragged.0, side(&widget, x));
            widget.activate_action("win.tab-move", Some(&destination.to_variant())).is_ok()
        });
        widget.add_controller(drop);
        widget
    }

    pub fn child_kind(&self, session: &str) -> Option<ChildKind> {
        self.children.borrow().get(session).map(|(kind, _)| *kind)
    }

    /// Coloca el widget como contenido de la pestaña. Si estaba en otro panel,
    /// lo trae.
    pub fn set_child(&self, session: &str, kind: ChildKind, widget: &impl IsA<gtk::Widget>) {
        self.remove_child(session);
        let widget = widget.as_ref();
        if let Some(previous) = widget.parent().and_then(|p| p.downcast::<gtk::Stack>().ok()) {
            previous.remove(widget);
        }
        self.stack.add_child(widget);
        self.children
            .borrow_mut()
            .insert(session.to_string(), (kind, widget.clone()));
    }

    pub fn remove_child(&self, session: &str) {
        let removed = self.children.borrow_mut().remove(session);
        if let Some((_, widget)) = removed {
            // Puede haberse mudado ya a otro panel.
            if widget.parent().as_ref() == Some(self.stack.upcast_ref()) {
                self.stack.remove(&widget);
            }
        }
    }

    pub fn retain_children(&self, keep: impl Fn(&str) -> bool) {
        let gone: Vec<String> = self
            .children
            .borrow()
            .keys()
            .filter(|session| !keep(session))
            .cloned()
            .collect();
        for session in gone {
            self.remove_child(&session);
        }
    }

    /// Muestra el contenido de la sesión, o el aviso de panel vacío.
    pub fn show(&self, session: Option<&str>) {
        let child = session.and_then(|s| self.children.borrow().get(s).map(|(_, w)| w.clone()));
        match child {
            Some(widget) if widget.parent().as_ref() == Some(self.stack.upcast_ref()) => {
                if self.stack.visible_child().as_ref() != Some(&widget) {
                    self.stack.set_visible_child(&widget);
                }
            }
            _ => self.stack.set_visible_child_name(PLACEHOLDER),
        }
    }

    pub fn focus_content(&self) {
        if let Some(child) = self.stack.visible_child() {
            child.grab_focus();
        }
    }

    /// `focused` solo se marca cuando hay más de un panel entre los que
    /// distinguir. `alone` oculta los botones que no tienen sentido con uno.
    pub fn set_state(&self, focused: bool, maximized: bool, alone: bool) {
        set_class(&self.root, "focused", focused);
        set_class(&self.maximize, "active", maximized);
        self.maximize.set_visible(!alone || maximized);
        self.close.set_visible(!alone);
    }
}

/// Menú contextual de una pestaña.
fn tab_menu(tab: &gtk::Widget, session: &str) {
    let menu = gio::Menu::new();
    menu.append_item(&menus::item("Cerrar esta pestaña", "win.tab-close", session));
    let many = gio::Menu::new();
    for (label, which) in [
        ("Cerrar las demás", "others"),
        ("Cerrar las de la derecha", "right"),
        ("Cerrar las de la izquierda", "left"),
    ] {
        many.append_item(&menus::item(label, "win.tab-close-many", &format!("{which}|{session}")));
    }
    menu.append_section(None, &many);
    menus::popup(tab, &menu);
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}

fn zone_at(x: f64, y: f64, width: i32, height: i32) -> DropZone {
    let (w, h) = (f64::from(width.max(1)), f64::from(height.max(1)));
    let (fx, fy) = (x / w, y / h);
    if fx < 0.25 {
        DropZone::Left
    } else if fx > 0.75 {
        DropZone::Right
    } else if fy < 0.25 {
        DropZone::Top
    } else if fy > 0.75 {
        DropZone::Bottom
    } else {
        DropZone::Center
    }
}

/// Resalta la mitad del panel que ocuparía la sesión al soltarla.
fn place_hint(hint: &gtk::Box, zone: DropZone, width: i32, height: i32) {
    use gtk::Align::{End, Fill, Start};
    let (halign, valign, w, h) = match zone {
        DropZone::Center => (Fill, Fill, -1, -1),
        DropZone::Left => (Start, Fill, width / 2, -1),
        DropZone::Right => (End, Fill, width / 2, -1),
        DropZone::Top => (Fill, Start, -1, height / 2),
        DropZone::Bottom => (Fill, End, -1, height / 2),
    };
    hint.set_halign(halign);
    hint.set_valign(valign);
    hint.set_size_request(w, h);
    hint.set_visible(true);
}

/// Aviso que ocupa el lugar del terminal cuando no hay a qué conectarse.
pub fn notice(kind: ChildKind, session_id: &str, is_claude: bool, backend_down: bool) -> gtk::Widget {
    let (title, description, label, action) = match kind {
        ChildKind::Detached => (
            "Terminal desconectado",
            "La sesión sigue viva en el backend, pero este terminal perdió la conexión.",
            "Reconectar",
            "win.session-reattach",
        ),
        _ if backend_down => (
            "Sesión desconectada",
            "El backend no responde, así que no se puede saber si el proceso sigue vivo.",
            "Relanzar",
            "win.session-restart",
        ),
        _ => (
            "Sesión desconectada",
            "El proceso ya no existe en el backend: se detuvo o el equipo se reinició.\n\
             La configuración de la sesión se conserva.",
            "Relanzar",
            "win.session-restart",
        ),
    };
    let target = session_id.to_variant();

    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("title-2");
    let body = gtk::Label::new(Some(description));
    body.set_wrap(true);
    body.set_justify(gtk::Justification::Center);
    body.set_max_width_chars(60);
    body.add_css_class("dim-label");

    let primary = gtk::Button::with_label(label);
    primary.add_css_class("pill");
    primary.add_css_class("suggested-action");
    primary.set_action_name(Some(action));
    primary.set_action_target_value(Some(&target));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    buttons.set_halign(gtk::Align::Center);
    buttons.set_margin_top(8);
    buttons.append(&primary);
    if kind == ChildKind::Missing && is_claude {
        let resume = gtk::Button::with_label("Continuar la última conversación");
        resume.add_css_class("pill");
        resume.set_tooltip_text(Some(
            "Lanza claude --continue: retoma la conversación más reciente de ese directorio, \
             que puede no ser la de esta sesión si varias comparten carpeta.",
        ));
        resume.set_action_name(Some("win.session-continue"));
        resume.set_action_target_value(Some(&target));
        buttons.append(&resume);
    }

    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_valign(gtk::Align::Center);
    content.set_halign(gtk::Align::Center);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.append(&heading);
    content.append(&body);
    content.append(&buttons);
    content.upcast()
}

/// Construye los widgets de un nodo del árbol. Cada división es un
/// `GtkPaned` cuya posición sigue a la proporción del modelo y la actualiza
/// cuando el usuario arrastra el separador.
pub fn build_tree(
    node: &Node,
    views: &HashMap<NodeId, Rc<PaneView>>,
    paneds: &mut Vec<gtk::Paned>,
    on_ratio: &Rc<dyn Fn(NodeId, f64)>,
) -> Option<gtk::Widget> {
    match node {
        Node::Pane(pane) => views.get(&pane.id).map(|view| view.root.clone().upcast()),
        Node::Split(split) => {
            let first = build_tree(&split.first, views, paneds, on_ratio)?;
            let second = build_tree(&split.second, views, paneds, on_ratio)?;
            let horizontal = split.orientation == Orientation::Horizontal;
            let paned = gtk::Paned::new(if horizontal {
                gtk::Orientation::Horizontal
            } else {
                gtk::Orientation::Vertical
            });
            paned.set_start_child(Some(&first));
            paned.set_end_child(Some(&second));
            paned.set_resize_start_child(true);
            paned.set_resize_end_child(true);
            paned.set_shrink_start_child(false);
            paned.set_shrink_end_child(false);
            paned.set_hexpand(true);
            paned.set_vexpand(true);

            let size = move |paned: &gtk::Paned| {
                if horizontal {
                    paned.width()
                } else {
                    paned.height()
                }
            };
            // Hasta que el panel tiene tamaño no se puede traducir la
            // proporción a píxeles. Mientras tanto se ignoran los cambios de
            // posición, que serían los valores por defecto de GTK.
            let ready = Rc::new(Cell::new(false));
            let ratio = split.ratio;
            {
                let ready = ready.clone();
                paned.add_tick_callback(move |paned, _| {
                    let total = size(paned);
                    if total <= 1 {
                        return glib::ControlFlow::Continue;
                    }
                    paned.set_position((f64::from(total) * ratio).round() as i32);
                    ready.set(true);
                    glib::ControlFlow::Break
                });
            }
            let split_id = split.id;
            let on_ratio = on_ratio.clone();
            paned.connect_position_notify(move |paned| {
                let total = size(paned);
                if ready.get() && total > 1 {
                    on_ratio(split_id, f64::from(paned.position()) / f64::from(total));
                }
            });
            paneds.push(paned.clone());
            Some(paned.upcast())
        }
    }
}

/// Clase CSS del indicador de una pestaña, a partir del estado de su sesión.
pub fn status_class(view: &alarviterm_core::sessions::SessionView) -> &'static str {
    status::describe(view).class
}
