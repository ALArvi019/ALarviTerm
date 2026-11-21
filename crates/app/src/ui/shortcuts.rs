//! Editor de atajos de teclado.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gdk, glib};
use alarviterm_core::actions::{self, ActionDef, Binding, Category, KeyCombo, Keymap};

use super::keys;

const CANCEL: &str = "cancel";
const CLEAR: &str = "clear";
const ADD: &str = "add";
const REPLACE: &str = "replace";

struct Editor {
    dialog: adw::Dialog,
    page: adw::PreferencesPage,
    groups: RefCell<Vec<adw::PreferencesGroup>>,
    reset_all: gtk::Button,
    keymap: Rc<RefCell<Keymap>>,
    /// Se llama después de cada cambio, con el mapa ya actualizado.
    on_change: Box<dyn Fn()>,
}

pub fn show(
    parent: &impl IsA<gtk::Widget>,
    keymap: Rc<RefCell<Keymap>>,
    on_change: impl Fn() + 'static,
) -> adw::Dialog {
    let page = adw::PreferencesPage::new();
    page.set_description(
        "Pulsa una acción para cambiar su atajo. Los atajos que un programa de terminal \
         necesita, como Ctrl+N, pueden limitarse a cuando el foco está fuera del terminal.",
    );

    let reset_all = gtk::Button::with_label("Restablecer todo");
    let header = adw::HeaderBar::new();
    header.pack_start(&reset_all);
    let chrome = adw::ToolbarView::new();
    chrome.add_top_bar(&header);
    chrome.set_content(Some(&page));

    let dialog = adw::Dialog::new();
    dialog.set_title("Atajos de teclado");
    dialog.set_content_width(680);
    dialog.set_content_height(640);
    dialog.set_child(Some(&chrome));

    let editor = Rc::new(Editor {
        dialog: dialog.clone(),
        page,
        groups: RefCell::new(Vec::new()),
        reset_all,
        keymap,
        on_change: Box::new(on_change),
    });
    {
        let weak = Rc::downgrade(&editor);
        editor.reset_all.connect_clicked(move |_| {
            if let Some(editor) = weak.upgrade() {
                *editor.keymap.borrow_mut() = Keymap::default();
                changed(&editor);
            }
        });
    }
    fill(&editor);
    // El diálogo mantiene vivo al editor mientras exista.
    let keep = RefCell::new(Some(editor));
    dialog.connect_closed(move |_| {
        keep.borrow_mut().take();
    });
    dialog.present(Some(parent));
    dialog
}

fn changed(editor: &Rc<Editor>) {
    (editor.on_change)();
    fill(editor);
}

fn fill(editor: &Rc<Editor>) {
    for group in editor.groups.borrow_mut().drain(..) {
        editor.page.remove(&group);
    }
    let keymap = editor.keymap.borrow();
    editor.reset_all.set_sensitive(!keymap.is_all_default());
    let mut groups = Vec::new();
    for category in Category::ALL {
        let group = adw::PreferencesGroup::new();
        group.set_title(category.label());
        for def in actions::ACTIONS.iter().filter(|d| d.category == category) {
            group.add(&action_row(editor, def, &keymap));
        }
        editor.page.add(&group);
        groups.push(group);
    }
    *editor.groups.borrow_mut() = groups;
}

fn action_row(editor: &Rc<Editor>, def: &'static ActionDef, keymap: &Keymap) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(def.title);
    row.set_activatable(true);

    let labels = keymap.labels(def.id);
    let keys = gtk::Label::new(Some(if labels.is_empty() { "Sin atajo" } else { labels.as_str() }));
    keys.add_css_class(if labels.is_empty() { "dim-label" } else { "mt-kbd" });
    keys.set_valign(gtk::Align::Center);
    row.add_suffix(&keys);

    if !keymap.is_default(def.id) {
        let reset = gtk::Button::from_icon_name("edit-undo-symbolic");
        reset.add_css_class("flat");
        reset.set_valign(gtk::Align::Center);
        reset.set_tooltip_text(Some("Volver al atajo de fábrica"));
        let weak = Rc::downgrade(editor);
        reset.connect_clicked(move |_| {
            if let Some(editor) = weak.upgrade() {
                editor.keymap.borrow_mut().reset(def.id);
                changed(&editor);
            }
        });
        row.add_suffix(&reset);
    }

    let weak = Rc::downgrade(editor);
    row.connect_activated(move |_| {
        if let Some(editor) = weak.upgrade() {
            capture(&editor, def);
        }
    });
    row
}

/// Pide la combinación nueva para una acción.
fn capture(editor: &Rc<Editor>, def: &'static ActionDef) {
    let combo_label = gtk::Label::new(Some("Pulsa la combinación"));
    combo_label.add_css_class("title-2");
    let note = gtk::Label::new(None);
    note.set_wrap(true);
    note.set_justify(gtk::Justification::Center);
    note.add_css_class("dim-label");
    let in_terminal = gtk::CheckButton::with_label("Usar también con el foco en un terminal");
    in_terminal.set_halign(gtk::Align::Center);
    in_terminal.set_sensitive(false);
    // El foco se queda en el diálogo, para que Espacio no marque la casilla.
    in_terminal.set_focusable(false);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.append(&combo_label);
    content.append(&note);
    content.append(&in_terminal);

    let has_keys = !editor.keymap.borrow().bindings(def.id).is_empty();
    let dialog = adw::AlertDialog::new(Some(def.title), None);
    dialog.set_extra_child(Some(&content));
    dialog.add_response(CANCEL, "Cancelar");
    if has_keys {
        dialog.add_response(CLEAR, "Quitar sus atajos");
        dialog.add_response(ADD, "Añadir");
    }
    dialog.add_response(REPLACE, if has_keys { "Sustituir" } else { "Guardar" });
    dialog.set_close_response(CANCEL);
    dialog.set_default_response(Some(REPLACE));
    dialog.set_response_appearance(REPLACE, adw::ResponseAppearance::Suggested);
    dialog.set_response_enabled(REPLACE, false);
    if has_keys {
        dialog.set_response_enabled(ADD, false);
    }

    let captured: Rc<RefCell<Option<KeyCombo>>> = Rc::new(RefCell::new(None));

    let apply: Rc<dyn Fn(&str)> = {
        let (weak, captured, in_terminal) = (Rc::downgrade(editor), captured.clone(), in_terminal.clone());
        Rc::new(move |response: &str| {
            let Some(editor) = weak.upgrade() else { return };
            let new = captured.borrow().clone().map(|combo| Binding {
                combo,
                in_terminal: in_terminal.is_active(),
            });
            let bindings = match (response, new) {
                (CLEAR, _) => Vec::new(),
                (REPLACE, Some(binding)) => vec![binding],
                (ADD, Some(binding)) => {
                    let mut all = editor.keymap.borrow().bindings(def.id).to_vec();
                    all.retain(|b| b.combo != binding.combo);
                    all.push(binding);
                    all
                }
                _ => return,
            };
            editor.keymap.borrow_mut().set(def.id, bindings);
            changed(&editor);
        })
    };
    {
        let apply = apply.clone();
        dialog.connect_response(None, move |_, response| apply(response));
    }

    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let apply = apply.clone();
        let (captured, combo_label, note, in_terminal, dialog) = (
            captured.clone(),
            combo_label.clone(),
            note.clone(),
            in_terminal.clone(),
            dialog.clone(),
        );
        let keymap = editor.keymap.clone();
        controller.connect_key_pressed(move |_, key, _, state| {
            let Some(combo) = keys::combo_from_event(key, state) else {
                return glib::Propagation::Stop;
            };
            let bare = !(combo.ctrl || combo.alt || combo.shift || combo.logo);
            // Esc cancela, e Intro confirma lo ya capturado.
            if bare && key == gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            if bare && combo.key == "Return" {
                if captured.borrow().is_some() {
                    apply(REPLACE);
                    dialog.close();
                }
                return glib::Propagation::Stop;
            }
            if !combo.is_bindable() {
                note.set_text("Un atajo necesita Ctrl, Alt o Súper, o ser una tecla de función.");
                return glib::Propagation::Stop;
            }
            combo_label.set_text(&combo.label());
            let owner = keymap.borrow().owner(&combo, def.id);
            let mut text = match owner {
                Some(other) => format!("Ahora lo usa «{}», que lo perderá.", other.title),
                None => String::new(),
            };
            if !combo.safe_in_terminal() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(
                    "Los programas de terminal suelen usar esta combinación. \
                     Si la activas dentro del terminal, dejarán de recibirla.",
                );
            }
            note.set_text(&text);
            in_terminal.set_sensitive(true);
            in_terminal.set_active(combo.safe_in_terminal());
            dialog.set_response_enabled(REPLACE, true);
            if dialog.has_response(ADD) {
                dialog.set_response_enabled(ADD, true);
            }
            *captured.borrow_mut() = Some(combo);
            glib::Propagation::Stop
        });
    }
    dialog.add_controller(controller);

    dialog.present(Some(&editor.dialog));
}
