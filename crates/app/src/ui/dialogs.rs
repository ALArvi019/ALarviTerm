//! Diálogos: confirmación, entrada de texto y alta de sesión.

use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use alarviterm_core::claude::conversations::Conversation;
use alarviterm_core::model::{parse_env_assignments, Group, NewSession, Project, SessionKind};
use alarviterm_core::paths::{display_path, expand_tilde};

const CANCEL: &str = "cancel";
const ACCEPT: &str = "accept";

pub fn confirm(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    body: &str,
    accept_label: &str,
    destructive: bool,
    on_accept: impl Fn() + 'static,
) -> adw::Dialog {
    let dialog = adw::AlertDialog::new(Some(heading), Some(body));
    dialog.add_response(CANCEL, "Cancelar");
    dialog.add_response(ACCEPT, accept_label);
    dialog.set_close_response(CANCEL);
    dialog.set_default_response(Some(if destructive { CANCEL } else { ACCEPT }));
    dialog.set_response_appearance(
        ACCEPT,
        if destructive {
            adw::ResponseAppearance::Destructive
        } else {
            adw::ResponseAppearance::Suggested
        },
    );
    dialog.connect_response(None, move |_, response| {
        if response == ACCEPT {
            on_accept();
        }
    });
    dialog.present(Some(parent));
    dialog.upcast()
}

/// Pide una línea de texto. Devuelve el diálogo, como el resto, para que quien
/// llama pueda reaccionar a su cierre.
///
///  `on_accept` solo recibe texto no vacío.
pub fn prompt(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    initial: &str,
    accept_label: &str,
    on_accept: impl Fn(String) + 'static,
) -> adw::Dialog {
    let entry = gtk::Entry::new();
    entry.set_text(initial);
    entry.set_activates_default(true);

    let dialog = adw::AlertDialog::new(Some(heading), None);
    dialog.set_extra_child(Some(&entry));
    dialog.add_response(CANCEL, "Cancelar");
    dialog.add_response(ACCEPT, accept_label);
    dialog.set_close_response(CANCEL);
    dialog.set_default_response(Some(ACCEPT));
    dialog.set_response_appearance(ACCEPT, adw::ResponseAppearance::Suggested);
    let entry_for_response = entry.clone();
    dialog.connect_response(None, move |_, response| {
        let text = entry_for_response.text().trim().to_string();
        if response == ACCEPT && !text.is_empty() {
            on_accept(text);
        }
    });
    dialog.present(Some(parent));
    entry.grab_focus();
    dialog.upcast()
}

pub struct NewSessionDefaults {
    pub groups: Vec<Group>,
    pub group_id: Option<i64>,
    pub cwd: PathBuf,
    pub claude_available: bool,
    pub projects: Vec<Project>,
    pub project_id: Option<i64>,
    /// Modelo que se propone, tomado de los ajustes.
    pub model: String,
    /// Argumentos adicionales que se proponen, tomados de los ajustes.
    pub extra_args: String,
    /// Si la sesión propuesta es un shell y no Claude Code.
    pub shell: bool,
}

/// Formulario de alta. `on_create` devuelve el error a mostrar si la sesión
/// no se pudo crear. Con `Ok` el diálogo se cierra.
pub fn new_session(
    parent: &gtk::Window,
    defaults: NewSessionDefaults,
    on_create: impl Fn(NewSession) -> Result<(), String> + 'static,
) -> adw::Dialog {
    let name = adw::EntryRow::builder().title("Nombre").build();

    let directory = adw::EntryRow::builder().title("Directorio de trabajo").build();
    directory.set_text(&display_path(&defaults.cwd));
    let browse = gtk::Button::from_icon_name("folder-open-symbolic");
    browse.set_valign(gtk::Align::Center);
    browse.add_css_class("flat");
    browse.set_tooltip_text(Some("Elegir directorio"));
    directory.add_suffix(&browse);

    let kinds = gtk::StringList::new(&["Claude Code", "Shell"]);
    let kind = adw::ComboRow::builder().title("Tipo").model(&kinds).build();
    if !defaults.claude_available {
        kind.set_selected(1);
        kind.set_subtitle("No se ha encontrado el ejecutable claude");
    } else if defaults.shell {
        kind.set_selected(1);
    }

    let mut project_names = vec!["Ninguno".to_string()];
    project_names.extend(defaults.projects.iter().map(super::projects::label));
    let project_refs: Vec<&str> = project_names.iter().map(String::as_str).collect();
    let project = adw::ComboRow::builder()
        .title("Proyecto")
        .model(&gtk::StringList::new(&project_refs))
        .build();
    project.set_visible(!defaults.projects.is_empty());
    if let Some(index) = defaults
        .project_id
        .and_then(|id| defaults.projects.iter().position(|p| p.id == id))
    {
        project.set_selected(index as u32 + 1);
        directory.set_text(&display_path(&defaults.projects[index].path));
    }
    // Elegir un proyecto lleva la sesión a su carpeta.
    {
        let (projects, directory) = (defaults.projects.clone(), directory.clone());
        project.connect_selected_notify(move |row| {
            if let Some(chosen) = (row.selected() as usize).checked_sub(1).and_then(|i| projects.get(i)) {
                directory.set_text(&display_path(&chosen.path));
            }
        });
    }

    let mut group_names = vec!["Sin grupo".to_string()];
    group_names.extend(defaults.groups.iter().map(|g| g.name.clone()));
    let group_refs: Vec<&str> = group_names.iter().map(String::as_str).collect();
    let group = adw::ComboRow::builder()
        .title("Grupo")
        .model(&gtk::StringList::new(&group_refs))
        .build();
    if let Some(index) = defaults
        .group_id
        .and_then(|id| defaults.groups.iter().position(|g| g.id == id))
    {
        group.set_selected(index as u32 + 1);
    }

    let model = adw::EntryRow::builder()
        .title("Modelo (vacío usa el predeterminado)")
        .build();
    model.set_text(&defaults.model);
    let extra_args = adw::EntryRow::builder().title("Argumentos adicionales").build();
    extra_args.set_text(&defaults.extra_args);
    let env = adw::EntryRow::builder()
        .title("Variables de entorno (CLAVE=valor ...)")
        .build();

    let basics = adw::PreferencesGroup::new();
    basics.add(&name);
    basics.add(&project);
    basics.add(&directory);
    basics.add(&kind);
    basics.add(&group);

    let claude_options = adw::PreferencesGroup::builder().title("Claude Code").build();
    claude_options.add(&model);
    claude_options.add(&extra_args);

    let advanced = adw::PreferencesGroup::builder().title("Entorno").build();
    advanced.add(&env);

    let error = gtk::Label::new(None);
    error.add_css_class("error");
    error.set_wrap(true);
    error.set_xalign(0.0);
    error.set_visible(false);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.set_margin_top(12);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);
    content.append(&basics);
    content.append(&claude_options);
    content.append(&advanced);
    content.append(&error);

    let cancel = gtk::Button::with_label("Cancelar");
    let create = gtk::Button::with_label("Crear");
    create.add_css_class("suggested-action");

    let header = adw::HeaderBar::new();
    header.set_show_start_title_buttons(false);
    header.set_show_end_title_buttons(false);
    header.pack_start(&cancel);
    header.pack_end(&create);

    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&content));

    let dialog = adw::Dialog::builder()
        .title("Nueva sesión")
        .content_width(520)
        .child(&view)
        .build();
    dialog.set_default_widget(Some(&create));
    dialog.set_focus(Some(&name));

    // Las opciones de Claude Code solo tienen sentido para ese tipo.
    let sync_kind = {
        let claude_options = claude_options.clone();
        move |row: &adw::ComboRow| claude_options.set_visible(row.selected() == 0)
    };
    sync_kind(&kind);
    kind.connect_selected_notify(sync_kind);

    {
        let parent = parent.clone();
        let directory = directory.clone();
        browse.connect_clicked(move |_| {
            let chooser = gtk::FileDialog::builder()
                .title("Directorio de trabajo")
                .modal(true)
                .build();
            let current = expand_tilde(&directory.text());
            if current.is_dir() {
                chooser.set_initial_folder(Some(&gio::File::for_path(current)));
            }
            let directory = directory.clone();
            chooser.select_folder(Some(&parent), gio::Cancellable::NONE, move |result| {
                // Cancelar el selector llega como error, no hay nada que avisar.
                if let Some(path) = result.ok().and_then(|file| file.path()) {
                    directory.set_text(&display_path(&path));
                }
            });
        });
    }

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }

    let submit: Rc<dyn Fn()> = {
        let dialog = dialog.clone();
        let groups = defaults.groups.clone();
        let projects = defaults.projects.clone();
        let project = project.clone();
        let (name, directory, kind, group, model, extra_args, env, error) = (
            name.clone(),
            directory.clone(),
            kind.clone(),
            group.clone(),
            model.clone(),
            extra_args.clone(),
            env.clone(),
            error.clone(),
        );
        Rc::new(move || {
            let cwd = expand_tilde(&directory.text());
            let is_claude = kind.selected() == 0;
            let result = parse_env_assignments(&env.text())
                .map_err(|e| e.to_string())
                .and_then(|env| {
                    let typed = name.text().trim().to_string();
                    // Sin nombre se usa el del directorio.
                    let fallback = cwd
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    on_create(NewSession {
                        name: if typed.is_empty() { fallback } else { typed },
                        group_id: match group.selected() {
                            0 => None,
                            index => groups.get(index as usize - 1).map(|g| g.id),
                        },
                        kind: if is_claude {
                            SessionKind::Claude
                        } else {
                            SessionKind::Shell
                        },
                        cwd: cwd.clone(),
                        model: is_claude.then(|| model.text().to_string()),
                        extra_args: if is_claude {
                            extra_args.text().to_string()
                        } else {
                            String::new()
                        },
                        env,
                        project_id: (project.selected() as usize)
                            .checked_sub(1)
                            .and_then(|i| projects.get(i))
                            .map(|p| p.id),
                        resume: None,
                    })
                });
            match result {
                Ok(()) => {
                    dialog.close();
                }
                Err(message) => {
                    error.set_text(&message);
                    error.set_visible(true);
                }
            }
        })
    };

    {
        let submit = submit.clone();
        create.connect_clicked(move |_| submit());
    }
    for row in [&name, &directory, &model, &extra_args, &env] {
        let submit = submit.clone();
        row.connect_entry_activated(move |_| submit());
    }

    dialog.present(Some(parent));
    dialog
}


/// Por qué una conversación no se puede traer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    /// Otro Claude Code la tiene abierta ahora mismo.
    OpenElsewhere,
    /// Ya es una sesión de ALarviTerm.
    AlreadyHere,
}

/// Conversaciones de Claude Code que ya existen, para elegir cuáles pasan a
/// ser sesiones. Cada una puede llevar la carpeta en la que vive, que se
/// enseña en su fila. `on_import` recibe las elegidas con el nombre que tendrá
/// cada sesión.
pub fn import_conversations(
    parent: &impl IsA<gtk::Widget>,
    title: &str,
    description: &str,
    found: Vec<(Conversation, Option<Blocked>, Option<String>)>,
    now: i64,
    on_import: impl Fn(Vec<(String, String)>) + 'static,
) -> adw::Dialog {
    let dialog = adw::Dialog::builder().title(title).content_width(620).content_height(560).build();

    let group = adw::PreferencesGroup::new();
    group.set_description(Some(description));
    let import = gtk::Button::with_label("Añadir");
    import.add_css_class("suggested-action");
    import.set_sensitive(false);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Filtrar por nombre o carpeta"));
    search.set_visible(found.len() > 8);
    let mut rows: Vec<(adw::ActionRow, String)> = Vec::new();
    let mut checks: Vec<(gtk::CheckButton, String, String)> = Vec::new();
    for (conversation, blocked, folder) in &found {
        let short: String = conversation.id.chars().take(8).collect();
        let name = conversation.title.clone().unwrap_or_else(|| format!("Conversación {short}"));
        let mut details: Vec<String> = folder.iter().cloned().collect();
        details.extend([ago(now - conversation.modified), size_text(conversation.size)]);
        match blocked {
            Some(Blocked::OpenElsewhere) => details.push("abierta en otro terminal".into()),
            Some(Blocked::AlreadyHere) => details.push("ya es una sesión de ALarviTerm".into()),
            None => {}
        }
        let row = adw::ActionRow::builder()
            .title(gtk::glib::markup_escape_text(&name))
            .subtitle(details.join("  ·  "))
            .build();
        let check = gtk::CheckButton::new();
        check.set_valign(gtk::Align::Center);
        row.add_prefix(&check);
        if blocked.is_some() {
            row.set_sensitive(false);
        } else {
            row.set_activatable_widget(Some(&check));
            checks.push((check, conversation.id.clone(), name.clone()));
        }
        group.add(&row);
        rows.push((row, format!("{name} {}", folder.as_deref().unwrap_or("")).to_lowercase()));
    }
    search.connect_search_changed(move |entry| {
        let wanted = entry.text().trim().to_lowercase();
        for (row, name) in &rows {
            row.set_visible(wanted.is_empty() || name.contains(&wanted));
        }
    });
    if found.is_empty() {
        let empty = adw::ActionRow::builder()
            .title("No hay conversaciones guardadas")
            .build();
        empty.add_css_class("dim-label");
        group.add(&empty);
    }

    let checks = Rc::new(checks);
    for (check, _, _) in checks.iter() {
        let (checks, import) = (checks.clone(), import.clone());
        check.connect_toggled(move |_| {
            let chosen = checks.iter().filter(|(c, _, _)| c.is_active()).count();
            import.set_sensitive(chosen > 0);
            import.set_label(&if chosen > 1 { format!("Añadir {chosen}") } else { "Añadir".to_string() });
        });
    }
    {
        let (checks, dialog) = (checks.clone(), dialog.clone());
        import.connect_clicked(move |_| {
            let chosen: Vec<(String, String)> = checks
                .iter()
                .filter(|(c, _, _)| c.is_active())
                .map(|(_, id, name)| (id.clone(), name.clone()))
                .collect();
            dialog.close();
            on_import(chosen);
        });
    }

    let page = adw::PreferencesPage::new();
    page.add(&group);
    let header = adw::HeaderBar::new();
    header.pack_end(&import);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    search.set_margin_start(12);
    search.set_margin_end(12);
    search.set_margin_bottom(6);
    view.add_top_bar(&search);
    view.set_content(Some(&page));
    dialog.set_child(Some(&view));
    dialog.present(Some(parent));
    dialog
}

/// Antigüedad en pocas palabras.
fn ago(seconds: i64) -> String {
    match seconds.max(0) {
        s if s < 90 => "ahora mismo".to_string(),
        s if s < 3600 => format!("hace {} min", s / 60),
        s if s < 2 * 86_400 => format!("hace {} h", s / 3600),
        s => format!("hace {} días", s / 86_400),
    }
}

fn size_text(bytes: u64) -> String {
    match bytes {
        b if b < 1024 * 1024 => format!("{} KB", b.div_ceil(1024)),
        b => format!("{} MB", b / (1024 * 1024)),
    }
}
