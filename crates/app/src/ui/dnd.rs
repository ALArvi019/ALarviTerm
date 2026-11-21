//! Arrastre de sesiones entre la barra lateral, las pestañas y los paneles.

use gtk::prelude::*;
use gtk::{gdk, glib};

/// Lo que viaja en un arrastre: el identificador de una sesión. Es un tipo
/// propio y no un texto para que el terminal no lo acepte como texto pegado.
#[derive(Clone, Debug, PartialEq, Eq, glib::Boxed)]
#[boxed_type(name = "ALarviTermSessionRef")]
pub struct SessionRef(pub String);

/// Hace que el widget se pueda arrastrar como referencia a la sesión.
pub fn make_draggable(widget: &impl IsA<gtk::Widget>, session_id: &str) {
    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::MOVE);
    let id = session_id.to_string();
    source.connect_prepare(move |_, _, _| {
        Some(gdk::ContentProvider::for_value(&SessionRef(id.clone()).to_value()))
    });
    let weak = widget.as_ref().downgrade();
    source.connect_drag_begin(move |source, _| {
        if let Some(widget) = weak.upgrade() {
            source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&widget))), 8, 8);
        }
    });
    widget.add_controller(source);
}
