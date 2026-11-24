//! Registro central de acciones y mapa de teclas.
//!
//! Todo lo que la interfaz puede hacer desde el teclado o la paleta de
//! comandos está declarado aquí, con su atajo de fábrica. El mapa de teclas
//! combina esos atajos con los cambios del usuario, que se guardan como JSON.
//! No depende de GTK: las teclas se nombran como lo hace GDK («n», «Return»,
//! «Page_Down») pero aquí son solo texto.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{json, Map, Value};

/// Familia a la que pertenece una acción, para agruparlas al mostrarlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Sessions,
    Panes,
    View,
    App,
}

impl Category {
    pub const ALL: [Category; 4] = [Category::Sessions, Category::Panes, Category::View, Category::App];

    pub fn label(self) -> &'static str {
        match self {
            Category::Sessions => "Sesiones",
            Category::Panes => "Paneles",
            Category::View => "Vista",
            Category::App => "Aplicación",
        }
    }
}

/// Atajo de fábrica: combinación y si vale también con el foco en un terminal.
type DefaultKey = (&'static str, bool);

#[derive(Debug, Clone, Copy)]
pub struct ActionDef {
    /// Nombre de la acción de ventana que la ejecuta.
    pub id: &'static str,
    pub title: &'static str,
    pub category: Category,
    pub default_keys: &'static [DefaultKey],
    /// Las que solo tienen sentido como atajo no salen en la paleta.
    pub in_palette: bool,
}

const fn action(
    id: &'static str,
    title: &'static str,
    category: Category,
    default_keys: &'static [DefaultKey],
) -> ActionDef {
    ActionDef {
        id,
        title,
        category,
        default_keys,
        in_palette: true,
    }
}

const fn hidden(id: &'static str, title: &'static str, default_keys: &'static [DefaultKey]) -> ActionDef {
    ActionDef {
        id,
        title,
        category: Category::Sessions,
        default_keys,
        in_palette: false,
    }
}

/// Todas las acciones, en el orden en que se muestran.
///
/// Ctrl+N y Ctrl+B pertenecen al shell y a Claude Code. Por eso su variante
/// sin Mayús solo actúa cuando el foco no está en un terminal.
pub const ACTIONS: &[ActionDef] = &[
    action(
        "new-session",
        "Nueva sesión",
        Category::Sessions,
        &[("Ctrl+Shift+N", true), ("Ctrl+N", false)],
    ),
    action("new-terminal", "Nuevo terminal en la carpeta actual", Category::Sessions, &[("Ctrl+Shift+T", true)]),
    action(
        "resume-conversation",
        "Continuar una conversación de Claude Code",
        Category::Sessions,
        &[("Ctrl+Shift+H", true)],
    ),
    action(
        "new-terminal-beside",
        "Nuevo terminal en un panel al lado",
        Category::Sessions,
        &[("Ctrl+Shift+O", true)],
    ),
    action("new-group", "Nuevo grupo", Category::Sessions, &[]),
    action("next-session", "Sesión siguiente", Category::Sessions, &[("Ctrl+Tab", true)]),
    action("previous-session", "Sesión anterior", Category::Sessions, &[("Ctrl+Shift+Tab", true)]),
    action("next-attention", "Ir a la siguiente sesión que te espera", Category::Sessions, &[("Ctrl+Shift+A", true)]),
    action("current-rename", "Renombrar la sesión actual", Category::Sessions, &[("Ctrl+Shift+R", true)]),
    action("current-restart", "Reiniciar la sesión actual", Category::Sessions, &[]),
    action(
        "current-continue",
        "Reiniciar la sesión actual continuando la conversación",
        Category::Sessions,
        &[],
    ),
    action("current-stop", "Detener la sesión actual", Category::Sessions, &[]),
    action("current-archive", "Archivar la sesión actual", Category::Sessions, &[]),
    action("current-delete", "Eliminar la sesión actual", Category::Sessions, &[]),
    action("projects", "Proyectos", Category::Sessions, &[]),
    hidden("goto-session-1", "Ir a la sesión 1", &[("Ctrl+1", true)]),
    hidden("goto-session-2", "Ir a la sesión 2", &[("Ctrl+2", true)]),
    hidden("goto-session-3", "Ir a la sesión 3", &[("Ctrl+3", true)]),
    hidden("goto-session-4", "Ir a la sesión 4", &[("Ctrl+4", true)]),
    hidden("goto-session-5", "Ir a la sesión 5", &[("Ctrl+5", true)]),
    hidden("goto-session-6", "Ir a la sesión 6", &[("Ctrl+6", true)]),
    hidden("goto-session-7", "Ir a la sesión 7", &[("Ctrl+7", true)]),
    hidden("goto-session-8", "Ir a la sesión 8", &[("Ctrl+8", true)]),
    hidden("goto-session-9", "Ir a la sesión 9", &[("Ctrl+9", true)]),
    action("split-right", "Dividir el panel a la derecha", Category::Panes, &[("Ctrl+Shift+D", true)]),
    action("split-down", "Dividir el panel hacia abajo", Category::Panes, &[("Ctrl+Shift+E", true)]),
    action("close-current", "Cerrar la pestaña o el panel", Category::Panes, &[("Ctrl+Shift+W", true)]),
    action("toggle-maximize", "Maximizar o restaurar el panel", Category::Panes, &[("Ctrl+Shift+Return", true)]),
    action("focus-left", "Foco al panel de la izquierda", Category::Panes, &[("Ctrl+Shift+Left", true)]),
    action("focus-right", "Foco al panel de la derecha", Category::Panes, &[("Ctrl+Shift+Right", true)]),
    action("focus-up", "Foco al panel de arriba", Category::Panes, &[("Ctrl+Shift+Up", true)]),
    action("focus-down", "Foco al panel de abajo", Category::Panes, &[("Ctrl+Shift+Down", true)]),
    action("next-tab", "Pestaña siguiente del panel", Category::Panes, &[("Ctrl+Page_Down", true)]),
    action("previous-tab", "Pestaña anterior del panel", Category::Panes, &[("Ctrl+Page_Up", true)]),
    action("layout-save", "Guardar la disposición actual", Category::Panes, &[]),
    action(
        "toggle-sidebar",
        "Mostrar u ocultar la barra lateral",
        Category::View,
        &[("Ctrl+Shift+B", true), ("Ctrl+B", false)],
    ),
    action("search-sessions", "Buscar sesiones en la barra lateral", Category::View, &[("Ctrl+Shift+F", true)]),
    action("focus-terminal", "Devolver el foco al terminal", Category::View, &[]),
    action("command-palette", "Paleta de comandos", Category::App, &[("Ctrl+Shift+P", true)]),
    action("settings", "Ajustes", Category::App, &[("Ctrl+comma", true)]),
    action("shortcuts", "Atajos de teclado", Category::App, &[]),
    action("stats", "Estadísticas de uso", Category::App, &[("Ctrl+Shift+S", true)]),
    action("scm", "Control de código fuente de la sesión", Category::App, &[("Ctrl+Shift+K", true)]),
    action("toggle-panel", "Mostrar u ocultar la columna de listas", Category::App, &[("Ctrl+Shift+L", true)]),
];

pub fn find(id: &str) -> Option<&'static ActionDef> {
    ACTIONS.iter().find(|a| a.id == id)
}

/// Una combinación de teclas: modificadores y una tecla con nombre de GDK.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
    /// Nombre de la tecla. Las de un solo carácter van en minúscula.
    pub key: String,
}

const MODIFIER_KEYS: &[&str] = &[
    "Control_L", "Control_R", "Shift_L", "Shift_R", "Alt_L", "Alt_R", "Meta_L", "Meta_R", "Super_L",
    "Super_R", "Hyper_L", "Hyper_R", "ISO_Level3_Shift", "ISO_Level5_Shift", "Caps_Lock", "Num_Lock",
];

fn normalize_key(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || MODIFIER_KEYS.contains(&name) {
        return None;
    }
    let key = match name {
        "ISO_Left_Tab" => "Tab".to_string(),
        "KP_Enter" | "Enter" | "Intro" => "Return".to_string(),
        "Esc" => "Escape".to_string(),
        "PageDown" => "Page_Down".to_string(),
        "PageUp" => "Page_Up".to_string(),
        other if other.chars().count() == 1 => other.to_lowercase(),
        other => other.to_string(),
    };
    Some(key)
}

impl KeyCombo {
    /// Construye la combinación a partir de una pulsación. Devuelve `None` si
    /// la tecla es un modificador suelto.
    pub fn from_press(key_name: &str, ctrl: bool, alt: bool, shift: bool, logo: bool) -> Option<Self> {
        Some(Self {
            ctrl,
            alt,
            shift,
            logo,
            key: normalize_key(key_name)?,
        })
    }

    /// Lee el formato canónico, por ejemplo `Ctrl+Shift+N` o `Ctrl+Page_Down`.
    pub fn parse(text: &str) -> Option<Self> {
        let mut combo = Self {
            ctrl: false,
            alt: false,
            shift: false,
            logo: false,
            key: String::new(),
        };
        let text = text.trim();
        // La tecla «+» se escribe al final: `Ctrl++`.
        let (modifiers, key) = match text.strip_suffix("++") {
            Some(rest) => (rest, "plus"),
            None => match text.rsplit_once('+') {
                Some((modifiers, key)) => (modifiers, key),
                None => ("", text),
            },
        };
        for part in modifiers.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_lowercase().as_str() {
                "ctrl" | "control" => combo.ctrl = true,
                "alt" => combo.alt = true,
                "shift" | "mayús" | "mayus" => combo.shift = true,
                "super" | "logo" => combo.logo = true,
                _ => return None,
            }
        }
        combo.key = normalize_key(key)?;
        Some(combo)
    }

    /// Un atajo necesita Ctrl, Alt o Super, salvo las teclas de función. Sin
    /// eso sería imposible escribir con normalidad.
    pub fn is_bindable(&self) -> bool {
        self.ctrl || self.alt || self.logo || self.is_function_key()
    }

    fn is_function_key(&self) -> bool {
        self.key
            .strip_prefix('F')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=12).contains(&n))
    }

    /// Si es razonable que el atajo actúe también con el foco en un terminal.
    ///
    /// Los programas de terminal reciben Ctrl+letra, Alt+tecla, las teclas de
    /// función y Ctrl+flecha, así que quitárselos rompe el shell o Claude
    /// Code. Ctrl+Mayús, Super, Ctrl+Tab, Ctrl+número, Ctrl+coma y Ctrl+RePág
    /// o AvPág no los usa casi nadie dentro del terminal.
    pub fn safe_in_terminal(&self) -> bool {
        if self.logo || (self.ctrl && self.shift) {
            return true;
        }
        if self.ctrl && !self.alt {
            let digit = self.key.len() == 1 && self.key.chars().all(|c| c.is_ascii_digit());
            // Ctrl+coma no tiene código de control: un terminal no lo recibe.
            return digit || matches!(self.key.as_str(), "Tab" | "Page_Down" | "Page_Up" | "comma");
        }
        false
    }

    /// Texto para mostrar, en español: `Ctrl+Mayús+N`.
    pub fn label(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".into());
        }
        if self.alt {
            parts.push("Alt".into());
        }
        if self.shift {
            parts.push("Mayús".into());
        }
        if self.logo {
            parts.push("Súper".into());
        }
        let key = match self.key.as_str() {
            "Return" => "Intro".to_string(),
            "Escape" => "Esc".to_string(),
            "Left" => "←".to_string(),
            "Right" => "→".to_string(),
            "Up" => "↑".to_string(),
            "Down" => "↓".to_string(),
            "Page_Down" => "AvPág".to_string(),
            "Page_Up" => "RePág".to_string(),
            "space" => "Espacio".to_string(),
            "BackSpace" => "Retroceso".to_string(),
            "Delete" => "Supr".to_string(),
            "Home" => "Inicio".to_string(),
            "End" => "Fin".to_string(),
            "plus" => "+".to_string(),
            "minus" => "−".to_string(),
            "comma" => ",".to_string(),
            "period" => ".".to_string(),
            other if other.chars().count() == 1 => other.to_uppercase(),
            other => other.to_string(),
        };
        parts.push(key);
        parts.join("+")
    }
}

impl fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (active, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.logo, "Super"),
        ] {
            if active {
                write!(f, "{name}+")?;
            }
        }
        if self.key.chars().count() == 1 {
            write!(f, "{}", self.key.to_uppercase())
        } else {
            write!(f, "{}", self.key)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub combo: KeyCombo,
    /// Si actúa también cuando el foco está en un terminal.
    pub in_terminal: bool,
}

impl Binding {
    pub fn label(&self) -> String {
        self.combo.label()
    }
}

fn default_bindings(def: &ActionDef) -> Vec<Binding> {
    def.default_keys
        .iter()
        .filter_map(|(keys, in_terminal)| {
            KeyCombo::parse(keys).map(|combo| Binding {
                combo,
                in_terminal: *in_terminal,
            })
        })
        .collect()
}

/// Atajos vigentes de cada acción.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    bindings: BTreeMap<&'static str, Vec<Binding>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: ACTIONS.iter().map(|def| (def.id, default_bindings(def))).collect(),
        }
    }
}

impl Keymap {
    /// Atajos de fábrica más los cambios guardados. Lo que no se entiende se
    /// ignora, para que un ajuste viejo o corrupto no deje la aplicación sin
    /// teclado.
    pub fn from_json(overrides: &str) -> Self {
        let mut keymap = Self::default();
        let Ok(Value::Object(entries)) = serde_json::from_str::<Value>(overrides) else {
            return keymap;
        };
        for (id, value) in &entries {
            let (Some(def), Some(list)) = (find(id), value.as_array()) else {
                continue;
            };
            let bindings = list
                .iter()
                .filter_map(|item| {
                    let combo = KeyCombo::parse(item.get("keys")?.as_str()?)?;
                    let in_terminal = item
                        .get("terminal")
                        .and_then(Value::as_bool)
                        .unwrap_or_else(|| combo.safe_in_terminal());
                    combo.is_bindable().then_some(Binding { combo, in_terminal })
                })
                .collect();
            keymap.set(def.id, bindings);
        }
        keymap
    }

    /// Solo las acciones que difieren de fábrica.
    pub fn overrides_json(&self) -> String {
        let mut entries = Map::new();
        for def in ACTIONS {
            if self.is_default(def.id) {
                continue;
            }
            let list: Vec<Value> = self
                .bindings(def.id)
                .iter()
                .map(|b| json!({ "keys": b.combo.to_string(), "terminal": b.in_terminal }))
                .collect();
            entries.insert(def.id.to_string(), Value::Array(list));
        }
        Value::Object(entries).to_string()
    }

    pub fn bindings(&self, id: &str) -> &[Binding] {
        self.bindings.get(id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Texto del primer atajo de una acción, para tooltips y la paleta.
    pub fn label(&self, id: &str) -> Option<String> {
        self.bindings(id).first().map(Binding::label)
    }

    /// Texto de todos los atajos de una acción.
    pub fn labels(&self, id: &str) -> String {
        self.bindings(id)
            .iter()
            .map(|b| {
                if b.in_terminal {
                    b.label()
                } else {
                    format!("{} (fuera del terminal)", b.label())
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Acción que corresponde a una pulsación.
    pub fn lookup(&self, combo: &KeyCombo, focus_in_terminal: bool) -> Option<&'static str> {
        self.bindings.iter().find_map(|(id, bindings)| {
            bindings
                .iter()
                .any(|b| b.combo == *combo && (b.in_terminal || !focus_in_terminal))
                .then_some(*id)
        })
    }

    /// Acción que ya usa una combinación, distinta de `except`.
    pub fn owner(&self, combo: &KeyCombo, except: &str) -> Option<&'static ActionDef> {
        self.bindings
            .iter()
            .find(|(id, bindings)| **id != except && bindings.iter().any(|b| b.combo == *combo))
            .and_then(|(id, _)| find(id))
    }

    /// Fija los atajos de una acción. Una combinación solo puede pertenecer a
    /// una acción, así que se le quita a las demás. Devuelve las afectadas.
    pub fn set(&mut self, id: &str, bindings: Vec<Binding>) -> Vec<&'static str> {
        let Some(def) = find(id) else {
            return Vec::new();
        };
        let mut displaced = Vec::new();
        for (other, list) in self.bindings.iter_mut() {
            if *other == def.id {
                continue;
            }
            let before = list.len();
            list.retain(|b| !bindings.iter().any(|new| new.combo == b.combo));
            if list.len() != before {
                displaced.push(*other);
            }
        }
        let mut unique: Vec<Binding> = Vec::new();
        for binding in bindings {
            if !unique.iter().any(|b| b.combo == binding.combo) {
                unique.push(binding);
            }
        }
        self.bindings.insert(def.id, unique);
        displaced
    }

    pub fn reset(&mut self, id: &str) -> Vec<&'static str> {
        match find(id) {
            Some(def) => self.set(def.id, default_bindings(def)),
            None => Vec::new(),
        }
    }

    pub fn is_default(&self, id: &str) -> bool {
        find(id).is_some_and(|def| self.bindings(id) == default_bindings(def).as_slice())
    }

    pub fn is_all_default(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn combo(text: &str) -> KeyCombo {
        KeyCombo::parse(text).expect("combinación válida")
    }

    #[test]
    fn registry_is_consistent() {
        let mut ids = std::collections::HashSet::new();
        let mut combos = std::collections::HashSet::new();
        for def in ACTIONS {
            assert!(ids.insert(def.id), "acción repetida: {}", def.id);
            assert!(!def.title.is_empty());
            for (keys, _) in def.default_keys {
                let parsed = KeyCombo::parse(keys).unwrap_or_else(|| panic!("atajo ilegible: {keys}"));
                assert!(parsed.is_bindable(), "atajo sin modificador: {keys}");
                assert!(combos.insert(parsed), "atajo de fábrica repetido: {keys}");
            }
        }
    }

    #[test]
    fn default_terminal_scope_matches_the_heuristic() {
        for def in ACTIONS {
            for (keys, in_terminal) in def.default_keys {
                assert_eq!(combo(keys).safe_in_terminal(), *in_terminal, "{keys}");
            }
        }
    }

    #[test]
    fn combos_parse_print_and_label() {
        let c = combo("Ctrl+Shift+N");
        assert!(c.ctrl && c.shift && !c.alt && !c.logo);
        assert_eq!(c.key, "n");
        assert_eq!(c.to_string(), "Ctrl+Shift+N");
        assert_eq!(c.label(), "Ctrl+Mayús+N");
        assert_eq!(combo("ctrl + page_down"), combo("Ctrl+page_down"));
        assert_eq!(combo("Ctrl+Page_Down").label(), "Ctrl+AvPág");
        assert_eq!(combo("Ctrl+Shift+Return").label(), "Ctrl+Mayús+Intro");
        assert_eq!(combo("Ctrl++").key, "plus");
        assert_eq!(combo("F5").to_string(), "F5");
        assert_eq!(KeyCombo::parse("Hiper+N"), None);
        assert_eq!(KeyCombo::parse("Ctrl+"), None);
        assert_eq!(KeyCombo::parse(&combo("Alt+Super+Left").to_string()), Some(combo("Alt+Super+Left")));
    }

    #[test]
    fn presses_are_normalized() {
        let tab = KeyCombo::from_press("ISO_Left_Tab", true, false, true, false).expect("tecla");
        assert_eq!(tab, combo("Ctrl+Shift+Tab"));
        let upper = KeyCombo::from_press("N", true, false, true, false).expect("tecla");
        assert_eq!(upper, combo("Ctrl+Shift+N"));
        let enter = KeyCombo::from_press("KP_Enter", true, false, true, false).expect("tecla");
        assert_eq!(enter, combo("Ctrl+Shift+Return"));
        assert_eq!(KeyCombo::from_press("Control_L", true, false, false, false), None);
    }

    #[test]
    fn bindable_and_terminal_safety() {
        assert!(!combo("N").is_bindable());
        assert!(!combo("Shift+N").is_bindable());
        assert!(combo("F5").is_bindable());
        assert!(combo("Alt+N").is_bindable());
        assert!(combo("Ctrl+Shift+N").safe_in_terminal());
        assert!(combo("Ctrl+Tab").safe_in_terminal());
        assert!(combo("Ctrl+3").safe_in_terminal());
        assert!(combo("Super+N").safe_in_terminal());
        assert!(!combo("Ctrl+N").safe_in_terminal());
        assert!(!combo("Ctrl+Left").safe_in_terminal());
        assert!(!combo("Alt+N").safe_in_terminal());
        assert!(!combo("F5").safe_in_terminal());
    }

    #[test]
    fn lookup_respects_the_terminal() {
        let keymap = Keymap::default();
        assert_eq!(keymap.lookup(&combo("Ctrl+Shift+N"), true), Some("new-session"));
        assert_eq!(keymap.lookup(&combo("Ctrl+N"), false), Some("new-session"));
        assert_eq!(keymap.lookup(&combo("Ctrl+N"), true), None);
        assert_eq!(keymap.lookup(&combo("Ctrl+B"), true), None);
        assert_eq!(keymap.lookup(&combo("Ctrl+4"), true), Some("goto-session-4"));
        assert_eq!(keymap.lookup(&combo("Ctrl+Shift+Z"), false), None);
    }

    #[test]
    fn setting_a_binding_takes_it_from_its_previous_owner() {
        let mut keymap = Keymap::default();
        let taken = combo("Ctrl+Shift+D");
        assert_eq!(keymap.owner(&taken, "new-group").map(|d| d.id), Some("split-right"));
        let displaced = keymap.set(
            "new-group",
            vec![Binding {
                combo: taken.clone(),
                in_terminal: true,
            }],
        );
        assert_eq!(displaced, vec!["split-right"]);
        assert_eq!(keymap.lookup(&taken, true), Some("new-group"));
        assert!(keymap.bindings("split-right").is_empty());
        assert!(!keymap.is_default("split-right"));

        // Restablecer la acción original le devuelve la combinación.
        let displaced = keymap.reset("split-right");
        assert_eq!(displaced, vec!["new-group"]);
        assert_eq!(keymap.lookup(&taken, true), Some("split-right"));
        assert!(keymap.is_all_default());
    }

    #[test]
    fn overrides_round_trip_and_bad_data_is_ignored() {
        let mut keymap = Keymap::default();
        assert_eq!(keymap.overrides_json(), "{}");
        keymap.set(
            "new-session",
            vec![Binding {
                combo: combo("Alt+N"),
                in_terminal: false,
            }],
        );
        keymap.set("toggle-sidebar", Vec::new());
        let json = keymap.overrides_json();
        let restored = Keymap::from_json(&json);
        assert_eq!(restored, keymap);
        assert_eq!(restored.lookup(&combo("Alt+N"), true), None);
        assert_eq!(restored.lookup(&combo("Alt+N"), false), Some("new-session"));
        assert_eq!(restored.lookup(&combo("Ctrl+Shift+B"), true), None);
        assert_eq!(restored.labels("new-session"), "Alt+N (fuera del terminal)");

        assert_eq!(Keymap::from_json("no es json"), Keymap::default());
        assert_eq!(Keymap::from_json("[1, 2]"), Keymap::default());
        let partial = Keymap::from_json(
            r#"{"inexistente": [{"keys": "Ctrl+Shift+Q"}], "new-group": [{"keys": "N"}, {"keys": "Ctrl+Shift+G"}, {"keys": 7}]}"#,
        );
        assert_eq!(partial.lookup(&combo("Ctrl+Shift+Q"), true), None);
        assert_eq!(partial.labels("new-group"), "Ctrl+Mayús+G");
    }
}
