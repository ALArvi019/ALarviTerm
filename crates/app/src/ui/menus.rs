//! Menús contextuales sobre filas y pestañas que se redibujan.

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

/// Abre un menú contextual junto a `near`.
///
/// El menú no se cuelga de `near`, sino del contenedor más alto de la
/// ventana que lo contiene. Las filas y las pestañas se destruyen y se
/// vuelven a crear en cada redibujo, a veces con el menú abierto, y un menú
/// colgado de un widget que desaparece tumba la aplicación.
pub fn popup(near: &impl IsA<gtk::Widget>, menu: &gio::Menu) {
    let near = near.as_ref();
    let mut anchor: Option<gtk::Widget> = None;
    let mut current = near.parent();
    while let Some(widget) = current {
        if widget.is::<gtk::Box>() {
            anchor = Some(widget.clone());
        }
        current = widget.parent();
    }
    let Some(anchor) = anchor else { return };
    let popover = gtk::PopoverMenu::from_model(Some(menu));
    popover.set_parent(&anchor);
    popover.set_has_arrow(false);
    popover.set_position(gtk::PositionType::Bottom);
    if let Some(bounds) = near.compute_bounds(&anchor) {
        popover.set_pointing_to(Some(&gdk::Rectangle::new(
            bounds.x() as i32,
            bounds.y() as i32,
            bounds.width().max(1.0) as i32,
            bounds.height().max(1.0) as i32,
        )));
    }
    popover.connect_closed(|popover| {
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    });
    popover.popup();
}

/// Entrada de menú que lanza una acción con un texto como destino.
pub fn item(label: &str, action: &str, target: &str) -> gio::MenuItem {
    let item = gio::MenuItem::new(Some(label), None);
    item.set_action_and_target_value(Some(action), Some(&target.to_variant()));
    item
}
