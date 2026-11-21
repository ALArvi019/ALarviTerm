//! Paso de las pulsaciones de GDK a las combinaciones del núcleo.

use gtk::gdk;
use alarviterm_core::actions::KeyCombo;

/// Combinación que corresponde a una pulsación. `None` si solo se pulsó un
/// modificador.
pub fn combo_from_event(key: gdk::Key, state: gdk::ModifierType) -> Option<KeyCombo> {
    let name = key.to_lower().name()?;
    KeyCombo::from_press(
        name.as_str(),
        state.contains(gdk::ModifierType::CONTROL_MASK),
        state.contains(gdk::ModifierType::ALT_MASK),
        state.contains(gdk::ModifierType::SHIFT_MASK),
        state.contains(gdk::ModifierType::SUPER_MASK),
    )
}
