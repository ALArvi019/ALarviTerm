//! Columna derecha: las listas que los complementos aportan para la sesión a
//! la vista, por ejemplo sus tareas. Solo lectura.

use gtk::prelude::*;
use alarviterm_core::plugins::Panel;

pub struct SidePanel {
    pub root: gtk::Box,
    title: gtk::Label,
    body: gtk::Box,
}

impl SidePanel {
    pub fn new() -> Self {
        let title = gtk::Label::new(None);
        title.add_css_class("mt-sidebar-title");
        title.set_xalign(0.0);
        title.set_hexpand(true);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.add_css_class("flat");
        close.set_tooltip_text(Some("Ocultar la columna"));
        close.set_action_name(Some("win.toggle-panel"));
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        header.add_css_class("mt-panel-header");
        header.append(&title);
        header.append(&close);

        let body = gtk::Box::new(gtk::Orientation::Vertical, 14);
        body.add_css_class("mt-panel-body");
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_vexpand(true);
        scroller.set_child(Some(&body));

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("mt-panel");
        root.set_size_request(300, -1);
        root.append(&header);
        root.append(&scroller);
        root.set_visible(false);
        Self { root, title, body }
    }

    /// Pinta las listas de la sesión. `session` es su nombre, o `None` si no
    /// hay ninguna a la vista.
    pub fn set(&self, session: Option<&str>, panels: &[Panel]) {
        self.title.set_text(&session.map(str::to_uppercase).unwrap_or_else(|| "SIN SESIÓN".to_string()));
        while let Some(child) = self.body.first_child() {
            self.body.remove(&child);
        }
        if panels.is_empty() {
            let empty = gtk::Label::new(Some(match session {
                Some(_) => "Ningún complemento tiene nada que enseñar de esta sesión.",
                None => "Elige una sesión para ver sus listas.",
            }));
            empty.add_css_class("mt-panel-empty");
            empty.set_wrap(true);
            empty.set_xalign(0.0);
            self.body.append(&empty);
            return;
        }
        for panel in panels {
            let (done, countable) = panel.progress();
            let name = gtk::Label::new(Some(&panel.title));
            name.add_css_class("mt-stat-heading");
            name.set_xalign(0.0);
            name.set_hexpand(true);
            let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            head.append(&name);
            let section = gtk::Box::new(gtk::Orientation::Vertical, 7);
            section.append(&head);
            if countable > 0 {
                let count = gtk::Label::new(Some(&format!("{done}/{countable}")));
                count.add_css_class("mt-stat-note");
                head.append(&count);
                let bar = gtk::ProgressBar::new();
                bar.add_css_class("mt-meter");
                bar.add_css_class("mt-meter-wide");
                bar.set_fraction(done as f64 / countable as f64);
                section.append(&bar);
            }
            // Lo pendiente primero: es lo que se viene a mirar.
            let mut items: Vec<_> = panel.items.iter().collect();
            items.sort_by_key(|item| item.done == Some(true));
            for item in items {
                let mark = gtk::Label::new(Some(match item.done {
                    Some(true) => "✓",
                    Some(false) => "○",
                    None => "·",
                }));
                mark.add_css_class("mt-panel-mark");
                mark.set_valign(gtk::Align::Start);
                let text = gtk::Label::new(Some(&item.text));
                text.set_wrap(true);
                text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                text.set_xalign(0.0);
                text.set_hexpand(true);
                text.set_selectable(true);
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                row.add_css_class("mt-panel-item");
                if item.done == Some(true) {
                    row.add_css_class("done");
                }
                row.append(&mark);
                row.append(&text);
                section.append(&row);
            }
            self.body.append(&section);
        }
    }
}
