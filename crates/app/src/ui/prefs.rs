//! Ventana de ajustes. Cada cambio se aplica y se guarda al momento.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use adw::prelude::*;
use alarviterm_core::claude::detection::find_executable;
use alarviterm_core::search::SortOrder;
use alarviterm_core::settings::{self, Cursor, Profile, Settings, Theme};

/// Aviso de que un perfil pasó de un nombre a otro.
type RenameFn = Box<dyn Fn(&str, &str)>;

struct Prefs {
    settings: Rc<RefCell<Settings>>,
    /// Se llama tras cada cambio, con los ajustes ya modificados.
    on_change: Box<dyn Fn()>,
    /// Un perfil cambió de nombre: las sesiones que lo usaban deben seguirle.
    on_rename: RenameFn,
    profiles: adw::PreferencesGroup,
    profile_rows: RefCell<Vec<adw::ExpanderRow>>,
}

impl Prefs {
    fn change(&self, edit: impl FnOnce(&mut Settings)) {
        {
            let mut settings = self.settings.borrow_mut();
            edit(&mut settings);
            settings.normalize();
        }
        (self.on_change)();
    }
}

fn combo(title: &str, labels: &[&str], selected: usize) -> adw::ComboRow {
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(labels))
        .build();
    row.set_selected(selected as u32);
    row
}

fn switch(title: &str, subtitle: &str, active: bool) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().title(title).active(active).build();
    if !subtitle.is_empty() {
        row.set_subtitle(subtitle);
    }
    row
}

/// Fila de texto que solo guarda al confirmar, no en cada pulsación.
fn entry(title: &str, text: &str) -> adw::EntryRow {
    let row = adw::EntryRow::builder().title(title).show_apply_button(true).build();
    row.set_text(text);
    row
}

pub fn show(
    parent: &impl IsA<gtk::Widget>,
    settings: Rc<RefCell<Settings>>,
    on_change: impl Fn() + 'static,
    on_rename: impl Fn(&str, &str) + 'static,
    on_shortcuts: impl Fn() + 'static,
) -> adw::Dialog {
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Ajustes");
    dialog.set_search_enabled(false);
    dialog.set_content_width(680);
    dialog.set_content_height(640);

    let profiles = adw::PreferencesGroup::builder()
        .title("Perfiles de terminal")
        .description("Cada sesión usa el perfil predeterminado salvo que elijas otro en su menú.")
        .build();
    let prefs = Rc::new(Prefs {
        settings,
        on_change: Box::new(on_change),
        on_rename: Box::new(on_rename),
        profiles,
        profile_rows: RefCell::new(Vec::new()),
    });
    let current = prefs.settings.borrow().clone();

    // General
    let general = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();

    let look = adw::PreferencesGroup::builder().title("Apariencia").build();
    let themes: Vec<&str> = Theme::ALL.iter().map(|t| t.label()).collect();
    let theme = combo(
        "Tema",
        &themes,
        Theme::ALL.iter().position(|t| *t == current.theme).unwrap_or(0),
    );
    {
        let prefs = prefs.clone();
        theme.connect_selected_notify(move |row| {
            let theme = Theme::ALL.get(row.selected() as usize).copied().unwrap_or_default();
            prefs.change(|s| s.theme = theme);
        });
    }
    look.add(&theme);
    general.add(&look);

    let sessions = adw::PreferencesGroup::builder().title("Sesiones").build();
    let orders: Vec<&str> = SortOrder::ALL.iter().map(|o| o.label()).collect();
    let sort = combo(
        "Orden en la barra lateral",
        &orders,
        SortOrder::ALL.iter().position(|o| *o == current.sort).unwrap_or(0),
    );
    sort.set_subtitle("Dentro de cada grupo. Arrastrar solo reordena con el orden manual.");
    {
        let prefs = prefs.clone();
        sort.connect_selected_notify(move |row| {
            let order = SortOrder::ALL.get(row.selected() as usize).copied().unwrap_or_default();
            prefs.change(|s| s.sort = order);
        });
    }
    sessions.add(&sort);
    general.add(&sessions);

    let footer = adw::PreferencesGroup::builder()
        .title("Pie de la ventana")
        .description("Datos de la sesión activa y de la cuenta.")
        .build();
    type Flag = fn(&mut Settings) -> &mut bool;
    let footer_rows: [(&str, &str, bool, Flag); 5] = [
        ("Rama de git", "Rama y ficheros con cambios.", current.footer_git, |s| &mut s.footer_git),
        ("Modelo", "El que respondió en el último turno.", current.footer_model, |s| &mut s.footer_model),
        ("Consumo", "CPU, memoria y red del equipo. Al pasar el ratón, también la sesión activa y la propia aplicación.", current.footer_usage, |s| &mut s.footer_usage),
        (
            "Límites de la cuenta",
            "Lo que queda de las ventanas de 5 horas y de 7 días. Se aplica a las sesiones que se inicien después.",
            current.account_limits,
            |s| &mut s.account_limits,
        ),
        (
            "Tu línea de estado de Claude Code",
            "La que tienes en tus ajustes de Claude Code, bajo el cuadro de texto de cada sesión. Apagada, sus datos se ven solo en este pie. Se aplica a las sesiones que se inicien después.",
            current.session_statusline,
            |s| &mut s.session_statusline,
        ),
    ];
    let notice_rows: [(&str, &str, bool, Flag); 2] = [
        (
            "Cuando una sesión pide permiso",
            "",
            current.notify_permission,
            |s| &mut s.notify_permission,
        ),
        (
            "Cuando una sesión termina su turno",
            "",
            current.notify_turn_end,
            |s| &mut s.notify_turn_end,
        ),
    ];
    let notices = adw::PreferencesGroup::builder()
        .title("Notificaciones de escritorio")
        .description("Solo con la ventana en segundo plano.")
        .build();
    for (group, rows) in [(&footer, &footer_rows[..]), (&notices, &notice_rows[..])] {
        for (title, subtitle, active, field) in rows {
            let row = switch(title, subtitle, *active);
            let (prefs, field) = (prefs.clone(), *field);
            row.connect_active_notify(move |row| {
                let active = row.is_active();
                prefs.change(|s| *field(s) = active);
            });
            group.add(&row);
        }
    }
    general.add(&footer);
    general.add(&notices);

    let keys = adw::PreferencesGroup::builder().title("Teclado").build();
    let shortcuts = adw::ActionRow::builder()
        .title("Atajos de teclado")
        .subtitle("Ver y cambiar el atajo de cada acción.")
        .activatable(true)
        .build();
    shortcuts.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    shortcuts.connect_activated(move |_| on_shortcuts());
    keys.add(&shortcuts);
    general.add(&keys);
    dialog.add(&general);

    // Terminal
    let terminal = adw::PreferencesPage::builder()
        .title("Terminal")
        .icon_name("utilities-terminal-symbolic")
        .build();
    let add = gtk::Button::from_icon_name("list-add-symbolic");
    add.add_css_class("flat");
    add.set_tooltip_text(Some("Añadir un perfil"));
    {
        let prefs = prefs.clone();
        add.connect_clicked(move |_| {
            prefs.change(|s| {
                let name = s.free_profile_name("Perfil nuevo");
                s.profiles.push(Profile {
                    name,
                    ..s.profile(None).clone()
                });
            });
            fill_profiles(&prefs, None);
        });
    }
    prefs.profiles.set_header_suffix(Some(&add));
    terminal.add(&prefs.profiles);
    fill_profiles(&prefs, None);

    let history_group = adw::PreferencesGroup::builder().title("Historial").build();
    let history = adw::SpinRow::with_range(
        f64::from(settings::MIN_HISTORY),
        f64::from(settings::MAX_HISTORY),
        1_000.0,
    );
    history.set_title("Líneas que se guardan por sesión");
    history.set_subtitle("Vale para las sesiones que se lancen a partir de ahora.");
    history.set_value(f64::from(current.history_limit));
    {
        let prefs = prefs.clone();
        history.connect_value_notify(move |row| {
            let lines = row.value() as u32;
            prefs.change(|s| s.history_limit = lines);
        });
    }
    history_group.add(&history);
    terminal.add(&history_group);
    dialog.add(&terminal);

    // Programas
    let programs = adw::PreferencesPage::builder()
        .title("Programas")
        .icon_name("application-x-executable-symbolic")
        .build();
    let claude = adw::PreferencesGroup::builder().title("Claude Code").build();
    let claude_path = entry("Ejecutable (vacío lo busca en el PATH)", &current.claude_path);
    let claude_note = adw::ActionRow::new();
    claude_note.add_css_class("dim-label");
    let describe_claude = {
        let note = claude_note.clone();
        move |path: &str| {
            let found = find_executable(Some(path).filter(|p| !p.is_empty()).map(Path::new));
            note.set_title(&match (found, path.is_empty()) {
                (Some(found), _) => format!("Se usará {}", found.display()),
                (None, true) => "No hay ningún ejecutable claude en el PATH".to_string(),
                (None, false) => "Esa ruta no es un ejecutable. No se podrán lanzar sesiones.".to_string(),
            });
        }
    };
    describe_claude(&current.claude_path);
    {
        let prefs = prefs.clone();
        claude_path.connect_apply(move |row| {
            let path = row.text().trim().to_string();
            describe_claude(&path);
            prefs.change(|s| s.claude_path = path);
        });
    }
    let model = entry("Modelo propuesto al crear una sesión", &current.default_model);
    {
        let prefs = prefs.clone();
        model.connect_apply(move |row| {
            let model = row.text().trim().to_string();
            prefs.change(|s| s.default_model = model);
        });
    }
    let args = entry("Argumentos propuestos al crear una sesión", &current.default_args);
    {
        let prefs = prefs.clone();
        args.connect_apply(move |row| {
            let args = row.text().trim().to_string();
            prefs.change(|s| s.default_args = args);
        });
    }
    claude.add(&claude_path);
    claude.add(&claude_note);
    claude.add(&model);
    let theme = entry("Tema de Claude Code en las sesiones (vacío: el tuyo)", &current.claude_theme);
    theme.set_tooltip_text(Some(
        "Nombre de un tema de Claude Code, como dark o custom:mi-tema, solo para las sesiones de \
         ALarviTerm. Con un tema propio se cambian, entre otros, los colores del cuadro de texto. \
         Se aplica a las sesiones que se inicien después.",
    ));
    {
        let prefs = prefs.clone();
        theme.connect_apply(move |row| {
            let theme = row.text().trim().to_string();
            prefs.change(|s| s.claude_theme = theme);
        });
    }
    claude.add(&args);
    claude.add(&theme);
    programs.add(&claude);

    let shell_group = adw::PreferencesGroup::builder()
        .title("Shell")
        .description("Con el que se lanzan las sesiones nuevas y las que se reinicien.")
        .build();
    let shell = entry("Ejecutable (vacío usa el de la variable SHELL)", &current.shell);
    {
        let prefs = prefs.clone();
        shell.connect_apply(move |row| {
            let path = row.text().trim().to_string();
            if !path.is_empty() && !Path::new(&path).is_file() {
                row.add_css_class("error");
                return;
            }
            row.remove_css_class("error");
            prefs.change(|s| s.shell = path);
        });
    }
    shell_group.add(&shell);
    programs.add(&shell_group);
    dialog.add(&programs);

    // La ventana mantiene vivo el estado mientras exista.
    let keep = RefCell::new(Some(prefs));
    dialog.connect_closed(move |_| {
        keep.borrow_mut().take();
    });
    dialog.present(Some(parent));
    dialog.upcast()
}

/// Reconstruye la lista de perfiles. `expanded` es el que queda abierto.
fn fill_profiles(prefs: &Rc<Prefs>, expanded: Option<&str>) {
    for row in prefs.profile_rows.borrow_mut().drain(..) {
        prefs.profiles.remove(&row);
    }
    let settings = prefs.settings.borrow().clone();
    let mut rows = Vec::new();
    for profile in &settings.profiles {
        let row = profile_row(prefs, profile, &settings);
        row.set_expanded(expanded == Some(profile.name.as_str()));
        prefs.profiles.add(&row);
        rows.push(row);
    }
    *prefs.profile_rows.borrow_mut() = rows;
}

fn profile_row(prefs: &Rc<Prefs>, profile: &Profile, settings: &Settings) -> adw::ExpanderRow {
    let is_default = settings.default_profile == profile.name;
    let row = adw::ExpanderRow::builder().title(&profile.name).build();
    row.set_subtitle(&if is_default {
        format!("{}, predeterminado", profile.font)
    } else {
        profile.font.clone()
    });

    // Cada control modifica el perfil con este nombre, si sigue existiendo.
    let edit = {
        let (prefs, name) = (prefs.clone(), profile.name.clone());
        Rc::new(move |change: &dyn Fn(&mut Profile)| {
            prefs.change(|s| {
                if let Some(profile) = s.profiles.iter_mut().find(|p| p.name == name) {
                    change(profile);
                }
            });
        })
    };

    let name = entry("Nombre", &profile.name);
    {
        let (prefs, old) = (prefs.clone(), profile.name.clone());
        name.connect_apply(move |row| {
            let new = row.text().trim().to_string();
            let taken = prefs.settings.borrow().profiles.iter().any(|p| p.name == new);
            if new.is_empty() || (taken && new != old) {
                row.add_css_class("error");
                return;
            }
            if new == old {
                return;
            }
            (prefs.on_rename)(&old, &new);
            prefs.change(|s| {
                if s.default_profile == old {
                    s.default_profile = new.clone();
                }
                if let Some(profile) = s.profiles.iter_mut().find(|p| p.name == old) {
                    profile.name = new.clone();
                }
            });
            fill_profiles(&prefs, Some(&new));
        });
    }
    row.add_row(&name);

    let font_button = gtk::FontDialogButton::new(Some(
        gtk::FontDialog::builder().title("Fuente del terminal").modal(true).build(),
    ));
    font_button.set_font_desc(&gtk::pango::FontDescription::from_string(&profile.font));
    font_button.set_valign(gtk::Align::Center);
    {
        let (edit, row) = (edit.clone(), row.downgrade());
        font_button.connect_font_desc_notify(move |button| {
            let Some(font) = button.font_desc().map(|d| d.to_string()) else {
                return;
            };
            if let Some(row) = row.upgrade() {
                row.set_subtitle(&font);
            }
            edit(&|p| p.font = font.clone());
        });
    }
    let font = adw::ActionRow::builder().title("Fuente").build();
    font.add_suffix(&font_button);
    row.add_row(&font);

    let schemes: Vec<&str> = settings::SCHEMES.iter().map(|(_, label)| *label).collect();
    let scheme = combo(
        "Colores",
        &schemes,
        settings::SCHEMES
            .iter()
            .position(|(id, _)| *id == profile.scheme)
            .unwrap_or(0),
    );
    {
        let edit = edit.clone();
        scheme.connect_selected_notify(move |row| {
            if let Some((id, _)) = settings::SCHEMES.get(row.selected() as usize) {
                edit(&|p| p.scheme = (*id).to_string());
            }
        });
    }
    row.add_row(&scheme);

    let cursors: Vec<&str> = Cursor::ALL.iter().map(|c| c.label()).collect();
    let cursor = combo(
        "Cursor",
        &cursors,
        Cursor::ALL.iter().position(|c| *c == profile.cursor).unwrap_or(0),
    );
    {
        let edit = edit.clone();
        cursor.connect_selected_notify(move |row| {
            let cursor = Cursor::ALL.get(row.selected() as usize).copied().unwrap_or_default();
            edit(&|p| p.cursor = cursor);
        });
    }
    row.add_row(&cursor);

    let blink = switch(
        "Cursor parpadeante",
        "Despierta el proceso varias veces por segundo.",
        profile.blink,
    );
    {
        let edit = edit.clone();
        blink.connect_active_notify(move |row| {
            let active = row.is_active();
            edit(&|p| p.blink = active);
        });
    }
    row.add_row(&blink);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(8);
    buttons.set_margin_bottom(8);
    buttons.set_margin_end(12);
    if !is_default {
        let make_default = gtk::Button::with_label("Usar como predeterminado");
        let (prefs, name) = (prefs.clone(), profile.name.clone());
        make_default.connect_clicked(move |_| {
            prefs.change(|s| s.default_profile = name.clone());
            fill_profiles(&prefs, Some(&name));
        });
        buttons.append(&make_default);
    }
    if settings.profiles.len() > 1 {
        let delete = gtk::Button::with_label("Eliminar");
        delete.add_css_class("destructive-action");
        let (prefs, name) = (prefs.clone(), profile.name.clone());
        delete.connect_clicked(move |_| {
            prefs.change(|s| s.profiles.retain(|p| p.name != name));
            fill_profiles(&prefs, None);
        });
        buttons.append(&delete);
    }
    if buttons.first_child().is_some() {
        let holder = gtk::ListBoxRow::new();
        holder.set_activatable(false);
        holder.set_child(Some(&buttons));
        row.add_row(&holder);
    }
    row
}
