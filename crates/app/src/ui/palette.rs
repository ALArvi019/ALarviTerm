//! Paleta de comandos: un cuadro de búsqueda sobre todo lo que la aplicación
//! sabe hacer, más las sesiones y las disposiciones.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use alarviterm_core::search;

/// Entradas que se muestran como mucho. Más no caben y no ayudan.
const MAX_ROWS: usize = 60;

#[derive(Debug, Clone)]
pub struct Command {
    pub title: String,
    /// Familia o detalle, en pequeño a la derecha.
    pub hint: String,
    pub shortcut: Option<String>,
    /// Acción de ventana que la ejecuta, y su parámetro si lo lleva.
    pub action: String,
    pub target: Option<String>,
}

fn command_row(command: &Command) -> gtk::ListBoxRow {
    let title = gtk::Label::new(Some(&command.title));
    title.set_xalign(0.0);
    title.set_hexpand(true);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let hint = gtk::Label::new(Some(&command.hint));
    hint.add_css_class("mt-palette-hint");
    hint.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    hint.set_max_width_chars(34);

    let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    content.append(&title);
    content.append(&hint);
    if let Some(shortcut) = &command.shortcut {
        let keys = gtk::Label::new(Some(shortcut));
        keys.add_css_class("mt-kbd");
        content.append(&keys);
    }
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&content));
    row
}

/// Mantiene a la vista la fila seleccionada sin quitarle el foco al cuadro de
/// búsqueda.
fn reveal(list: &gtk::ListBox, scroller: &gtk::ScrolledWindow, row: &gtk::ListBoxRow) {
    let Some(bounds) = row.compute_bounds(list) else {
        return;
    };
    let adjustment = scroller.vadjustment();
    let (top, bottom) = (f64::from(bounds.y()), f64::from(bounds.y() + bounds.height()));
    if top < adjustment.value() {
        adjustment.set_value(top);
    } else if bottom > adjustment.value() + adjustment.page_size() {
        adjustment.set_value(bottom - adjustment.page_size());
    }
}

pub fn show(
    parent: &impl IsA<gtk::Widget>,
    commands: Vec<Command>,
    on_run: impl Fn(Command) + 'static,
) -> adw::Dialog {
    let commands = Rc::new(commands);
    let on_run = Rc::new(on_run);
    // Qué comando hay en cada fila visible.
    let visible: Rc<RefCell<Vec<usize>>> = Rc::new(RefCell::new(Vec::new()));

    let entry = gtk::SearchEntry::new();
    entry.set_placeholder_text(Some("Escribe una acción o el nombre de una sesión"));
    entry.set_search_delay(0);
    entry.add_css_class("mt-palette-entry");

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Browse);
    list.add_css_class("mt-palette-list");
    let empty = gtk::Label::new(Some("Nada coincide."));
    empty.add_css_class("mt-empty-hint");
    empty.set_margin_top(14);
    empty.set_margin_bottom(14);
    list.set_placeholder(Some(&empty));

    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_propagate_natural_height(true);
    scroller.set_max_content_height(420);
    scroller.set_child(Some(&list));

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.add_css_class("mt-palette");
    content.append(&entry);
    content.append(&scroller);

    let dialog = adw::Dialog::new();
    dialog.set_content_width(640);
    dialog.set_child(Some(&content));

    let fill = {
        let (commands, visible, list, scroller) =
            (commands.clone(), visible.clone(), list.clone(), scroller.clone());
        move |query: &str| {
            let all: Vec<usize> = (0..commands.len()).collect();
            let mut ranked = search::rank(query, all, |i| {
                format!("{} {}", commands[*i].title, commands[*i].hint)
            });
            ranked.truncate(MAX_ROWS);
            list.remove_all();
            for index in &ranked {
                list.append(&command_row(&commands[*index]));
            }
            *visible.borrow_mut() = ranked;
            list.select_row(list.row_at_index(0).as_ref());
            scroller.vadjustment().set_value(0.0);
        }
    };
    fill("");
    {
        let fill = fill.clone();
        entry.connect_search_changed(move |entry| fill(entry.text().as_str()));
    }

    let run = {
        let (commands, visible, dialog) = (commands.clone(), visible.clone(), dialog.clone());
        Rc::new(move |row: i32| {
            let Some(index) = usize::try_from(row).ok().and_then(|r| visible.borrow().get(r).copied())
            else {
                return;
            };
            let (command, on_run) = (commands[index].clone(), on_run.clone());
            dialog.close();
            // Se ejecuta con la paleta ya cerrada, para que el foco y los
            // diálogos de la acción partan de la ventana.
            glib::idle_add_local_once(move || on_run(command));
        })
    };
    {
        let (run, list) = (run.clone(), list.clone());
        entry.connect_activate(move |_| {
            if let Some(row) = list.selected_row() {
                run(row.index());
            }
        });
    }
    {
        let run = run.clone();
        list.connect_row_activated(move |_, row| run(row.index()));
    }
    {
        let dialog = dialog.clone();
        entry.connect_stop_search(move |_| {
            dialog.close();
        });
    }

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let (list, scroller, visible) = (list.clone(), scroller.clone(), visible.clone());
        keys.connect_key_pressed(move |_, key, _, _| {
            let step: i64 = match key {
                gdk::Key::Down => 1,
                gdk::Key::Up => -1,
                gdk::Key::Page_Down => 8,
                gdk::Key::Page_Up => -8,
                _ => return glib::Propagation::Proceed,
            };
            let count = visible.borrow().len() as i64;
            if count > 0 {
                let current = list.selected_row().map_or(0, |r| i64::from(r.index()));
                // Las flechas dan la vuelta, las páginas se detienen al final.
                let next = if step.abs() == 1 {
                    (current + step).rem_euclid(count)
                } else {
                    (current + step).clamp(0, count - 1)
                };
                if let Some(row) = list.row_at_index(next as i32) {
                    list.select_row(Some(&row));
                    reveal(&list, &scroller, &row);
                }
            }
            glib::Propagation::Stop
        });
    }
    entry.add_controller(keys);

    dialog.present(Some(parent));
    entry.grab_focus();
    dialog
}
