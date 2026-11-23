//! Proyectos: la lista para gestionarlos y el formulario de cada uno.

use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use alarviterm_core::model::{
    format_env_assignments, parse_env_assignments, NewProject, Project, PROJECT_COLORS,
};
use alarviterm_core::paths::{display_path, expand_tilde, home_dir};

/// Texto corto que identifica al proyecto en listas: icono y nombre.
pub fn label(project: &Project) -> String {
    if project.icon.is_empty() {
        project.name.clone()
    } else {
        format!("{} {}", project.icon, project.name)
    }
}

/// Imagen del proyecto a tamaño de icono, si tiene una y se puede leer.
pub fn image(project: &Project, size: i32) -> Option<gtk::Image> {
    if project.image.is_empty() || !std::path::Path::new(&project.image).is_file() {
        return None;
    }
    let image = gtk::Image::from_file(&project.image);
    image.set_pixel_size(size);
    Some(image)
}

/// Etiqueta del proyecto para la fila de una sesión: su imagen o su icono de
/// texto, y el nombre, con su color.
pub fn chip(project: &Project) -> gtk::Box {
    let chip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    chip.add_css_class("mt-project");
    if let Some(class) = color_class(project) {
        chip.add_css_class(&class);
    }
    let text = match image(project, 13) {
        Some(picture) => {
            chip.append(&picture);
            project.name.clone()
        }
        None => label(project),
    };
    let name = gtk::Label::new(Some(&text));
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(12);
    chip.append(&name);
    chip.set_valign(gtk::Align::Center);
    chip.set_tooltip_text(Some(&format!("Proyecto {}", project.name)));
    chip
}

/// Clase CSS del color de un proyecto, si tiene.
pub fn color_class(project: &Project) -> Option<String> {
    (!project.color.is_empty()).then(|| format!("c-{}", project.color))
}

/// Lo que la ventana hace con cada orden de la lista.
pub struct ManagerCallbacks {
    pub new: Box<dyn Fn()>,
    pub edit: Box<dyn Fn(i64)>,
    pub delete: Box<dyn Fn(i64)>,
    /// Sesión nueva de Claude Code en el proyecto.
    pub launch: Box<dyn Fn(i64)>,
    /// Traer conversaciones de Claude Code que ya existen en su carpeta.
    pub import: Box<dyn Fn(i64)>,
    /// Encender las sesiones del proyecto si están paradas, o apagarlas.
    pub power: Box<dyn Fn(i64)>,
}

/// Lista de proyectos. Se cierra antes de cada orden y la ventana la vuelve a
/// abrir si procede, así siempre muestra datos al día.
pub fn manager(
    parent: &impl IsA<gtk::Widget>,
    projects: &[Project],
    callbacks: ManagerCallbacks,
) -> adw::Dialog {
    let callbacks = Rc::new(callbacks);
    let dialog = adw::Dialog::builder()
        .title("Proyectos")
        .content_width(600)
        .content_height(520)
        .build();

    let group = adw::PreferencesGroup::new();
    group.set_description(Some(
        "Un proyecto es una carpeta con nombre. Sus sesiones arrancan en ella, heredan sus \
         variables de entorno y ejecutan antes su comando de inicio.",
    ));
    for project in projects {
        let row = adw::ActionRow::builder()
            .title(glib_escape(&label(project)))
            .subtitle(glib_escape(&display_path(&project.path)))
            .build();
        let chip = gtk::Label::new(Some("●"));
        chip.add_css_class("mt-project");
        if let Some(picture) = image(project, 20) {
            row.add_prefix(&picture);
        } else if let Some(class) = color_class(project) {
            chip.add_css_class(&class);
            row.add_prefix(&chip);
        }
        type Order = fn(&ManagerCallbacks, i64);
        let buttons: [(&str, &str, Order); 5] = [
            ("system-shutdown-symbolic", "Encender las sesiones de este proyecto, o apagarlas si están en marcha", |c, id| (c.power)(id)),
            ("list-add-symbolic", "Nueva sesión de Claude Code en este proyecto", |c, id| (c.launch)(id)),
            ("document-open-recent-symbolic", "Añadir conversaciones existentes", |c, id| (c.import)(id)),
            ("document-edit-symbolic", "Editar", |c, id| (c.edit)(id)),
            ("user-trash-symbolic", "Eliminar", |c, id| (c.delete)(id)),
        ];
        for (icon, tooltip, order) in buttons {
            let button = gtk::Button::from_icon_name(icon);
            button.add_css_class("flat");
            button.set_valign(gtk::Align::Center);
            button.set_tooltip_text(Some(tooltip));
            let (callbacks, dialog, id) = (callbacks.clone(), dialog.clone(), project.id);
            button.connect_clicked(move |_| {
                dialog.close();
                order(&callbacks, id);
            });
            row.add_suffix(&button);
        }
        group.add(&row);
    }
    if projects.is_empty() {
        let empty = adw::ActionRow::builder().title("Aún no hay proyectos").build();
        empty.add_css_class("dim-label");
        group.add(&empty);
    }

    let page = adw::PreferencesPage::new();
    page.add(&group);

    let add = gtk::Button::with_label("Nuevo proyecto");
    add.add_css_class("suggested-action");
    {
        let (callbacks, dialog) = (callbacks.clone(), dialog.clone());
        add.connect_clicked(move |_| {
            dialog.close();
            (callbacks.new)();
        });
    }
    let header = adw::HeaderBar::new();
    header.pack_start(&add);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&page));
    dialog.set_child(Some(&view));
    dialog.present(Some(parent));
    dialog
}

/// Las filas de libadwaita interpretan marcado en sus títulos.
fn glib_escape(text: &str) -> String {
    gtk::glib::markup_escape_text(text).to_string()
}

/// Formulario de alta o edición. `on_save` devuelve el error a mostrar. Con
/// `Ok` el diálogo se cierra.
pub fn editor(
    parent: &gtk::Window,
    existing: Option<&Project>,
    on_save: impl Fn(NewProject) -> Result<(), String> + 'static,
) -> adw::Dialog {
    let name = adw::EntryRow::builder().title("Nombre").build();
    let directory = adw::EntryRow::builder().title("Carpeta").build();
    let browse = gtk::Button::from_icon_name("folder-open-symbolic");
    browse.set_valign(gtk::Align::Center);
    browse.add_css_class("flat");
    browse.set_tooltip_text(Some("Elegir carpeta"));
    directory.add_suffix(&browse);

    let colors: Vec<&str> = PROJECT_COLORS.iter().map(|(_, label, _)| *label).collect();
    let color = adw::ComboRow::builder()
        .title("Color")
        .model(&gtk::StringList::new(&colors))
        .build();
    let icon = adw::EntryRow::builder()
        .title("Icono (uno o dos caracteres, puede ser un emoji)")
        .build();
    let picture = adw::EntryRow::builder()
        .title("Imagen (un fichero PNG, SVG, ICO o JPEG, opcional)")
        .build();
    let pick = gtk::Button::from_icon_name("image-x-generic-symbolic");
    pick.set_valign(gtk::Align::Center);
    pick.add_css_class("flat");
    pick.set_tooltip_text(Some("Elegir imagen"));
    picture.add_suffix(&pick);
    {
        let (picture, parent) = (picture.clone(), parent.clone());
        pick.connect_clicked(move |_| {
            let filter = gtk::FileFilter::new();
            filter.add_pixbuf_formats();
            filter.set_name(Some("Imágenes"));
            let chooser = gtk::FileDialog::builder().title("Imagen del proyecto").modal(true).build();
            chooser.set_default_filter(Some(&filter));
            let picture = picture.clone();
            chooser.open(Some(&parent), gio::Cancellable::NONE, move |result| {
                // Cancelar el selector llega como error, no hay nada que avisar.
                if let Some(path) = result.ok().and_then(|file| file.path()) {
                    picture.set_text(&display_path(&path));
                }
            });
        });
    }
    let env = adw::EntryRow::builder()
        .title("Variables de entorno (CLAVE=valor ...)")
        .build();
    let startup = adw::EntryRow::builder()
        .title("Comando de inicio (se ejecuta antes de cada sesión)")
        .build();
    let account = adw::EntryRow::builder()
        .title("Cuenta de Claude (vacío: la predeterminada)")
        .build();
    account.set_tooltip_text(Some(
        "Un nombre a tu elección para la cuenta con la que trabajan las sesiones de este proyecto, \
         por ejemplo cuando el comando de inicio pone otro token. Separa los límites de uso y las \
         estadísticas de cada cuenta. Aquí no se guarda ninguna credencial.",
    ));

    match existing {
        Some(project) => {
            name.set_text(&project.name);
            directory.set_text(&display_path(&project.path));
            let index = PROJECT_COLORS
                .iter()
                .position(|(id, _, _)| *id == project.color)
                .unwrap_or(0);
            color.set_selected(index as u32);
            icon.set_text(&project.icon);
            env.set_text(&format_env_assignments(&project.env));
            startup.set_text(&project.startup);
            account.set_text(&project.account);
            if !project.image.is_empty() {
                picture.set_text(&display_path(std::path::Path::new(&project.image)));
            }
        }
        None => {
            if let Ok(home) = home_dir() {
                directory.set_text(&display_path(&home));
            }
        }
    }

    let basics = adw::PreferencesGroup::new();
    basics.add(&name);
    basics.add(&directory);
    let look = adw::PreferencesGroup::builder().title("Aspecto").build();
    look.add(&color);
    look.add(&icon);
    look.add(&picture);
    let launch = adw::PreferencesGroup::builder()
        .title("Entorno de sus sesiones")
        .description(
            "El comando de inicio corre en el shell justo antes de la sesión, por ejemplo para \
             activar un entorno virtual. Si falla, la sesión no arranca y el error queda a la vista.",
        )
        .build();
    launch.add(&env);
    launch.add(&startup);
    launch.add(&account);

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
    content.append(&look);
    content.append(&launch);
    content.append(&error);

    let cancel = gtk::Button::with_label("Cancelar");
    let save = gtk::Button::with_label(if existing.is_some() { "Guardar" } else { "Crear" });
    save.add_css_class("suggested-action");
    let header = adw::HeaderBar::new();
    header.set_show_start_title_buttons(false);
    header.set_show_end_title_buttons(false);
    header.pack_start(&cancel);
    header.pack_end(&save);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&content));

    let dialog = adw::Dialog::builder()
        .title(if existing.is_some() { "Editar proyecto" } else { "Nuevo proyecto" })
        .content_width(540)
        .child(&view)
        .build();
    dialog.set_default_widget(Some(&save));
    dialog.set_focus(Some(&name));

    {
        let (parent, directory) = (parent.clone(), directory.clone());
        browse.connect_clicked(move |_| {
            let chooser = gtk::FileDialog::builder().title("Carpeta del proyecto").modal(true).build();
            let current = expand_tilde(&directory.text());
            if current.is_dir() {
                chooser.set_initial_folder(Some(&gio::File::for_path(current)));
            }
            let directory = directory.clone();
            chooser.select_folder(Some(&parent), gio::Cancellable::NONE, move |result| {
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
        let (account, picture) = (account.clone(), picture.clone());
        let (name, directory, color, icon, env, startup, error) = (
            name.clone(),
            directory.clone(),
            color.clone(),
            icon.clone(),
            env.clone(),
            startup.clone(),
            error.clone(),
        );
        Rc::new(move || {
            let path = expand_tilde(&directory.text());
            let typed = name.text().trim().to_string();
            // Sin nombre se usa el de la carpeta.
            let fallback = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let result = parse_env_assignments(&env.text())
                .map_err(|e| e.to_string())
                .and_then(|env| {
                    on_save(NewProject {
                        name: if typed.is_empty() { fallback } else { typed },
                        path: path.clone(),
                        color: PROJECT_COLORS
                            .get(color.selected() as usize)
                            .map(|(id, _, _)| (*id).to_string())
                            .unwrap_or_default(),
                        icon: icon.text().to_string(),
                        env,
                        startup: startup.text().to_string(),
                        account: account.text().to_string(),
                        image: match picture.text().trim() {
                            "" => String::new(),
                            path => expand_tilde(path).to_string_lossy().into_owned(),
                        },
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
        save.connect_clicked(move |_| submit());
    }
    for row in [&name, &directory, &icon, &env, &startup] {
        let submit = submit.clone();
        row.connect_entry_activated(move |_| submit());
    }

    dialog.present(Some(parent));
    dialog
}
