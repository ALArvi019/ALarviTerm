//! Ventana principal y coordinación entre barra lateral, terminales y gestor
//! de sesiones.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, gio, glib};
use alarviterm_core::actions::{self, Keymap};
use alarviterm_core::claude::{self, conversations, AuthHint, Resume};
use alarviterm_core::limits;
use alarviterm_core::layout::{Direction, Layout, NodeId, Orientation, Preset, Side};
use alarviterm_core::model::{NewProject, NewSession, SessionKind};
use alarviterm_core::monitor::{self, Phase, StateStore};
use alarviterm_core::paths::{display_path, home_dir, Paths};
use alarviterm_core::persistence::Database;
use alarviterm_core::plugins::{self, Plugin, PluginStatus, SessionInfo};
use alarviterm_core::search::{self, SortOrder, StatusFilter};
use alarviterm_core::sessions::tmux::{find_tmux, TmuxOptions};
use alarviterm_core::sessions::{
    Liveness, SessionBackend, SessionManager, SessionView, Snapshot, TmuxBackend,
};
use alarviterm_core::settings::Settings;
use alarviterm_core::sysinfo::{self, GitSummary, Usage};

use super::codebar::{CodeBar, CodeBarCallbacks};
use super::dialogs::{self, NewSessionDefaults};
use super::scm::{CodeCallbacks, CodeView};
use super::dnd::SessionRef;
use super::footer::Footer;
use super::icons::{self, Icon};
use super::keys;
use super::palette::{self, Command};
use super::panes::{self, ChildKind, DropZone, PaneCallbacks, PaneView, TabInfo};
use super::prefs;
use super::projects::{self, ManagerCallbacks};
use super::shortcuts;
use super::stats;
use super::sidebar::{self, RowButton, RowExtra, RowTarget, SearchCallbacks, Sidebar};
use super::sidepanel::SidePanel;
use super::status;
use super::style;
use super::terminal;

/// Cada cuánto se consulta al backend mientras la ventana está activa. Es una
/// sola ejecución de tmux de pocos milisegundos. Con la ventana en segundo
/// plano no se consulta nada.
const REFRESH_SECONDS: u32 = 10;

/// Cada cuánto se mide el consumo del equipo para el pie.
const SYSTEM_SECONDS: u32 = 2;

/// Conjunto de sesiones sobre el que actúa una orden en bloque.
#[derive(Debug, Clone, Copy)]
enum Batch {
    Group(i64),
    Project(i64),
}

/// Margen para agrupar los eventos de hooks que llegan en ráfaga.
const EVENT_DEBOUNCE: Duration = Duration::from_millis(80);

/// Distancia mínima entre dos recargas provocadas por eventos de hooks.
const EVENT_SPACING: Duration = Duration::from_millis(300);

/// Cada cuánto se vuelven a consultar git y el consumo de la sesión a la vista
/// mientras no cambie de sesión. Cada consulta lanza `git status` y recorre
/// los procesos del equipo.
const DETAILS_EVERY: Duration = Duration::from_secs(4);

/// Tiempo que un terminal puede pasar oculto antes de desconectarse.
const HIDDEN_TERMINAL_TTL: Duration = Duration::from_secs(300);

/// El mismo tiempo, que `ALARVITERM_HIDDEN_TTL` permite acortar, en segundos,
/// para pruebas y mediciones.
fn hidden_terminal_ttl() -> Duration {
    std::env::var("ALARVITERM_HIDDEN_TTL")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(HIDDEN_TERMINAL_TTL)
}

const PAGE_EMPTY: &str = "page-empty";
const PAGE_PANES: &str = "page-panes";
const LEFT_SESSIONS: &str = "left-sessions";
const LEFT_CODE: &str = "left-code";
const CENTER_TERMINALS: &str = "center-terminals";
const CENTER_CODE: &str = "center-code";

const KEY_LAST_SESSION: &str = "ui.last_session";
const KEY_LAYOUT: &str = "ui.layout";
const KEY_SIDEBAR_WIDTH: &str = "ui.sidebar_width";
const KEY_SIDEBAR_VISIBLE: &str = "ui.sidebar_visible";
const KEY_WINDOW_WIDTH: &str = "ui.window_width";
const KEY_WINDOW_HEIGHT: &str = "ui.window_height";
const KEY_UNGROUPED_COLLAPSED: &str = "ui.ungrouped_collapsed";
const KEY_KEYMAP: &str = "keys.overrides";

const PRESETS: [(&str, &str); 5] = [
    ("Un panel", "single"),
    ("Dos columnas", "columns2"),
    ("Dos filas", "rows2"),
    ("Tres columnas", "columns3"),
    ("Cuadrícula de cuatro", "grid4"),
];

struct Inner {
    window: adw::ApplicationWindow,
    manager: RefCell<SessionManager>,
    snapshot: RefCell<Snapshot>,
    signature: RefCell<Option<String>>,
    /// Qué paneles hay y qué sesión muestra cada uno. Es la única fuente de
    /// verdad: los widgets se derivan de aquí en `render`.
    layout: RefCell<Layout>,
    maximized: Cell<Option<NodeId>>,
    /// Activo mientras se reconstruyen widgets, para ignorar los cambios de
    /// foco y de separadores que provoca la propia reconstrucción.
    rendering: Cell<bool>,
    tree_signature: RefCell<Option<String>>,
    saved_layout: RefCell<String>,
    views: RefCell<HashMap<NodeId, Rc<PaneView>>>,
    paneds: RefCell<Vec<gtk::Paned>>,
    terminals: RefCell<HashMap<String, vte::Terminal>>,
    /// Sesiones vivas cuyo terminal perdió la conexión. No se reconectan solas
    /// para no entrar en bucle si la conexión falla de inmediato.
    detached: RefCell<HashSet<String>>,
    ungrouped_collapsed: Cell<bool>,
    backend_label: String,
    backend_ok: bool,
    /// Vigilancia del directorio de estado. Debe vivir tanto como la ventana.
    state_monitor: RefCell<Option<gio::FileMonitor>>,
    reload_pending: Cell<bool>,
    /// Última fase conocida de cada sesión, para avisar solo de los cambios.
    phases: RefCell<HashMap<String, Phase>>,
    /// Sesión cuyos datos muestra el pie y consulta en curso, si la hay.
    details_for: RefCell<Option<String>>,
    details_busy: Cell<bool>,
    /// Cuándo se consultaron por última vez git y el consumo de la sesión.
    details_at: Cell<Option<Instant>>,
    /// Cuándo se hizo la última recarga provocada por eventos de hooks.
    event_reload_at: Cell<Option<Instant>>,
    cpu_sample: RefCell<Option<(String, u64, Instant)>>,
    /// Transcripciones ya leídas, para el panel de estadísticas.
    stats_library: stats::SharedLibrary,
    /// Muestra anterior de CPU de la propia aplicación.
    app_sample: Cell<Option<(u64, Instant)>>,
    /// Tabla de procesos de la muestra anterior, mientras el panel del equipo
    /// está abierto.
    process_sample: RefCell<Option<(sysinfo::ProcessTable, Instant)>>,
    process_busy: Cell<bool>,
    side_panel: SidePanel,
    /// Columna izquierda entera: las pestañas y lo que enseñan.
    left: gtk::Box,
    left_stack: gtk::Stack,
    sessions_tab: gtk::ToggleButton,
    code_tab: gtk::ToggleButton,
    /// Terminales o comparación de código.
    center: gtk::Stack,
    codebar: Rc<CodeBar>,
    code_view: Rc<CodeView>,
    code_busy: Cell<bool>,
    /// Carpeta cuyo repositorio hay que desplegar cuando acabe la lectura, y
    /// si además hay que abrir su primer cambio.
    code_expand: RefCell<Option<(std::path::PathBuf, bool)>>,
    /// Atajos vigentes. Lo comparte el editor de atajos.
    keymap: Rc<RefCell<Keymap>>,
    settings: Rc<RefCell<Settings>>,
    /// Para volver a escribir la configuración del backend al cambiar ajustes.
    tmux: TmuxBackend,
    state_dir: std::path::PathBuf,
    /// Desde cuándo lleva cada terminal sin estar a la vista.
    hidden_since: RefCell<HashMap<String, Instant>>,
    /// Shell del entorno, el que se usa si los ajustes no fijan otro.
    env_shell: String,
    /// Botones cuyo tooltip cita el atajo de su acción.
    hinted: Vec<(gtk::Widget, &'static str, &'static str)>,
    /// Complementos instalados por el usuario, leídos al arrancar.
    plugins: Vec<Plugin>,
    /// Última respuesta de estado de cada complemento, por su identificador.
    plugin_status: RefCell<HashMap<String, PluginStatus>>,
    /// Cuándo toca volver a consultar a cada complemento.
    plugin_due: RefCell<HashMap<String, Instant>>,
    plugin_busy: RefCell<HashSet<String>>,
    /// Último problema avisado de cada complemento, para no repetirlo.
    plugin_errors: RefCell<HashMap<String, String>>,

    title: adw::WindowTitle,
    paned: gtk::Paned,
    pages: gtk::Stack,
    /// Contenedor del árbol de paneles.
    holder: gtk::Box,
    sidebar: Sidebar,
    footer: Footer,
    toasts: adw::ToastOverlay,
    layout_menu: gio::Menu,
}

#[derive(Clone)]
pub struct MainWindow(Rc<Inner>);

impl MainWindow {
    pub fn build(app: &adw::Application) -> alarviterm_core::Result<Self> {
        let paths = Paths::from_env()?;
        paths.ensure()?;
        let db = Database::open(&paths.database())?;

        let socket =
            std::env::var("ALARVITERM_TMUX_SOCKET").unwrap_or_else(|_| "alarviterm".to_string());
        let tmux = find_tmux().unwrap_or_else(|| "tmux".into());
        let backend = TmuxBackend::new(tmux, socket, paths.tmux_config());
        let store = StateStore::new(paths.state_dir());
        store.ensure()?;
        let settings = Settings::load(&db);
        let state_dir = store.dir().to_path_buf();
        let (backend_label, backend_ok) = match backend.check() {
            Ok(version) => {
                backend.write_config(&tmux_options(&settings, &state_dir))?;
                (version, true)
            }
            Err(e) => {
                log::error!("{e}");
                ("tmux no disponible".to_string(), false)
            }
        };

        let (plugin_list, plugin_problems) = plugins::plugins_dir()
            .map(|dir| plugins::discover(&dir))
            .unwrap_or_default();

        let env_shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".to_string());
        let shell = Some(settings.shell.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| env_shell.clone());
        let mut manager = SessionManager::new(db, Box::new(backend.clone()), shell);
        manager.set_claude_executable(claude::detection::find_executable(claude_override(&settings).as_deref()));
        // Los hooks se registran en un fichero propio que se pasa a cada
        // sesión. La configuración de Claude Code del usuario no se toca.
        match monitor::find_hook_binary() {
            Some(hook) => {
                let file = paths.hook_settings();
                std::fs::write(&file, monitor::hook_settings(&hook, settings.statusline(), Some(&settings.claude_theme))?)?;
                manager.enable_monitor(store.clone(), file);
            }
            None => log::warn!(
                "no se encuentra alarviterm-hook: las sesiones de Claude Code se lanzan sin monitorización"
            ),
        }
        if backend_ok {
            match manager.adopt_orphans() {
                Ok(0) => {}
                Ok(n) => log::warn!("{n} sesiones del backend no estaban registradas y se han adoptado"),
                Err(e) => log::error!("no se pudieron revisar las sesiones huérfanas: {e}"),
            }
        }

        let setting = |key: &str| manager.db().setting(key).ok().flatten();
        let number = |key: &str, default: i32| {
            setting(key).and_then(|v| v.parse::<i32>().ok()).unwrap_or(default)
        };
        let window_size = (number(KEY_WINDOW_WIDTH, 1280), number(KEY_WINDOW_HEIGHT, 800));
        let sidebar_width = number(KEY_SIDEBAR_WIDTH, 270);
        let sidebar_visible = setting(KEY_SIDEBAR_VISIBLE).as_deref() != Some("0");
        let ungrouped_collapsed = setting(KEY_UNGROUPED_COLLAPSED).as_deref() == Some("1");
        let keymap = setting(KEY_KEYMAP).map(|json| Keymap::from_json(&json)).unwrap_or_default();
        // Una disposición guardada ilegible no debe impedir el arranque.
        let mut layout = setting(KEY_LAYOUT)
            .and_then(|json| match Layout::from_json(&json) {
                Ok(layout) => Some(layout),
                Err(e) => {
                    log::warn!("se descarta la disposición guardada: {e}");
                    None
                }
            })
            .unwrap_or_default();
        if layout.sessions().is_empty() {
            // Bases de datos anteriores a los paneles solo recordaban esto.
            if let Some(last) = setting(KEY_LAST_SESSION) {
                layout.show(&last);
            }
        }

        // Cabecera
        let title = adw::WindowTitle::new("ALarviTerm", "");
        let header = adw::HeaderBar::new();
        header.set_title_widget(Some(&title));
        let toggle = gtk::Button::from_icon_name("sidebar-show-symbolic");
        toggle.set_action_name(Some("win.toggle-sidebar"));
        let add = gtk::Button::from_icon_name("list-add-symbolic");
        add.set_action_name(Some("win.new-session"));
        let layout_menu = gio::Menu::new();
        let layouts = gtk::MenuButton::new();
        layouts.set_child(Some(&icons::icon(Icon::Grid, 16)));
        layouts.set_tooltip_text(Some("Disposición de paneles"));
        layouts.set_menu_model(Some(&layout_menu));
        // Con el menú abierto el tooltip taparía su primera entrada.
        layouts.connect_active_notify(|button| button.set_has_tooltip(!button.is_active()));
        header.pack_start(&toggle);
        header.pack_start(&add);
        header.pack_start(&layouts);
        let lists = gtk::Button::from_icon_name("sidebar-show-right-symbolic");
        lists.set_action_name(Some("win.toggle-panel"));
        lists.set_tooltip_text(Some("Columna de listas de la sesión, como sus tareas"));
        header.pack_end(&lists);
        let commands = gtk::Button::from_icon_name("system-search-symbolic");
        commands.set_action_name(Some("win.command-palette"));
        let main_menu = gio::Menu::new();
        main_menu.append_item(&sidebar::plain_item("Paleta de comandos", "win.command-palette"));
        main_menu.append_item(&sidebar::plain_item("Proyectos", "win.projects"));
        main_menu.append_item(&sidebar::plain_item("Atajos de teclado", "win.shortcuts"));
        main_menu.append_item(&sidebar::plain_item("Estadísticas de uso", "win.stats"));
        main_menu.append_item(&sidebar::plain_item("Ajustes", "win.settings"));
        let more = gtk::MenuButton::new();
        more.set_icon_name("open-menu-symbolic");
        more.set_tooltip_text(Some("Menú"));
        more.set_menu_model(Some(&main_menu));
        more.connect_active_notify(|button| button.set_has_tooltip(!button.is_active()));
        header.pack_end(&more);
        header.pack_end(&commands);
        let hinted: Vec<(gtk::Widget, &'static str, &'static str)> = vec![
            (toggle.clone().upcast(), "Mostrar u ocultar la barra lateral", "toggle-sidebar"),
            (add.clone().upcast(), "Nueva sesión", "new-session"),
            (commands.clone().upcast(), "Paleta de comandos", "command-palette"),
        ];

        // Contenido
        let sidebar = Sidebar::new();
        // La columna izquierda tiene dos pestañas: las sesiones y el código.
        // Sus manejadores necesitan la ventana, que aún no existe: la reciben
        // después por este hueco.
        let late: Rc<RefCell<Weak<Inner>>> = Rc::new(RefCell::new(Weak::new()));
        let later = |late: &Rc<RefCell<Weak<Inner>>>| {
            let late = late.clone();
            move || late.borrow().upgrade().map(MainWindow)
        };
        let codebar = {
            let (open, refresh) = (later(&late), later(&late));
            CodeBar::new(CodeBarCallbacks {
                open: Box::new(move |root, change| {
                    if let Some(this) = open() {
                        this.open_code(root, change);
                    }
                }),
                refresh: Box::new(move || {
                    if let Some(this) = refresh() {
                        this.refresh_code();
                    }
                }),
            })
        };
        let code_view = {
            let (close, saved, notify) = (later(&late), later(&late), later(&late));
            CodeView::new(CodeCallbacks {
                close: Box::new(move || {
                    if let Some(this) = close() {
                        this.close_code();
                    }
                }),
                saved: Box::new(move || {
                    if let Some(this) = saved() {
                        this.refresh_open_code();
                    }
                }),
                notify: Box::new(move |message| {
                    if let Some(this) = notify() {
                        this.0.toasts.add_toast(adw::Toast::new(message));
                    }
                }),
            })
        };
        let sessions_tab = gtk::ToggleButton::with_label("Sesiones");
        let code_tab = gtk::ToggleButton::with_label("Código");
        code_tab.set_group(Some(&sessions_tab));
        sessions_tab.set_active(true);
        code_tab.set_tooltip_text(Some("Control de código fuente de las carpetas abiertas"));
        let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tabs.add_css_class("linked");
        tabs.add_css_class("mt-side-tabs");
        tabs.set_homogeneous(true);
        tabs.append(&sessions_tab);
        tabs.append(&code_tab);
        let left_stack = gtk::Stack::new();
        left_stack.set_vexpand(true);
        left_stack.add_named(&sidebar.root, Some(LEFT_SESSIONS));
        left_stack.add_named(&codebar.root, Some(LEFT_CODE));
        let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left.add_css_class("mt-sidebar");
        left.set_size_request(210, -1);
        left.append(&tabs);
        left.append(&left_stack);
        left.set_visible(sidebar_visible);
        let pages = gtk::Stack::new();
        pages.set_hexpand(true);
        pages.set_vexpand(true);
        pages.set_transition_type(gtk::StackTransitionType::None);

        let empty = adw::StatusPage::builder()
            .title("Sin sesiones")
            .description("Crea una sesión de Claude Code o un shell para empezar.")
            .build();
        let empty_button = gtk::Button::with_label("Nueva sesión");
        empty_button.add_css_class("pill");
        empty_button.add_css_class("suggested-action");
        empty_button.set_halign(gtk::Align::Center);
        empty_button.set_action_name(Some("win.new-session"));
        empty.set_child(Some(&empty_button));
        pages.add_named(&empty, Some(PAGE_EMPTY));

        let holder = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pages.add_named(&holder, Some(PAGE_PANES));

        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_start_child(Some(&left));
        let work = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pages.set_vexpand(true);
        work.append(&pages);
        work.set_hexpand(true);
        // A la derecha, la columna con las listas de los complementos.
        let side_panel = SidePanel::new();
        // El centro enseña los terminales o, mientras se revisa código, la
        // comparación de un fichero.
        let center = gtk::Stack::new();
        center.set_hexpand(true);
        center.add_named(&work, Some(CENTER_TERMINALS));
        center.add_named(&code_view.root, Some(CENTER_CODE));
        let middle = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        middle.append(&center);
        middle.append(&side_panel.root);
        paned.set_end_child(Some(&middle));
        paned.set_resize_start_child(false);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);
        paned.set_position(sidebar_width);

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&paned));

        let footer = Footer::new();
        footer.set_parts(settings.footer_git, settings.footer_model, settings.footer_usage);
        style::apply_theme(settings.theme);
        let chrome = adw::ToolbarView::new();
        chrome.add_top_bar(&header);
        chrome.set_content(Some(&toasts));
        chrome.add_bottom_bar(&footer.root);

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("ALarviTerm")
            .default_width(window_size.0)
            .default_height(window_size.1)
            .content(&chrome)
            .build();
        window.set_size_request(640, 360);

        let this = Self(Rc::new(Inner {
            window,
            manager: RefCell::new(manager),
            snapshot: RefCell::new(Snapshot::default()),
            signature: RefCell::new(None),
            layout: RefCell::new(layout),
            maximized: Cell::new(None),
            rendering: Cell::new(false),
            tree_signature: RefCell::new(None),
            saved_layout: RefCell::new(String::new()),
            views: RefCell::new(HashMap::new()),
            paneds: RefCell::new(Vec::new()),
            terminals: RefCell::new(HashMap::new()),
            detached: RefCell::new(HashSet::new()),
            ungrouped_collapsed: Cell::new(ungrouped_collapsed),
            backend_label,
            backend_ok,
            state_monitor: RefCell::new(None),
            reload_pending: Cell::new(false),
            phases: RefCell::new(HashMap::new()),
            details_for: RefCell::new(None),
            details_busy: Cell::new(false),
            details_at: Cell::new(None),
            event_reload_at: Cell::new(None),
            cpu_sample: RefCell::new(None),
            app_sample: Cell::new(None),
            process_sample: RefCell::new(None),
            process_busy: Cell::new(false),
            side_panel,
            left,
            left_stack,
            sessions_tab,
            code_tab,
            center,
            codebar,
            code_view,
            code_busy: Cell::new(false),
            code_expand: RefCell::new(None),
            stats_library: Default::default(),
            keymap: Rc::new(RefCell::new(keymap)),
            settings: Rc::new(RefCell::new(settings)),
            tmux: backend,
            state_dir,
            hidden_since: RefCell::new(HashMap::new()),
            env_shell,
            hinted,
            plugins: plugin_list,
            plugin_status: RefCell::new(HashMap::new()),
            plugin_due: RefCell::new(HashMap::new()),
            plugin_busy: RefCell::new(HashSet::new()),
            plugin_errors: RefCell::new(HashMap::new()),
            title,
            paned,
            pages,
            holder,
            sidebar,
            footer,
            toasts,
            layout_menu,
        }));

        // La ventana es la dueña del estado: lo mantiene vivo hasta destruirse
        // y entonces lo suelta, lo que rompe el ciclo de referencias.
        let keep_alive = RefCell::new(Some(this.clone()));
        this.0.window.connect_destroy(move |_| {
            keep_alive.borrow_mut().take();
        });

        this.install_actions();
        // Acción de aplicación: es la que invocan las notificaciones.
        let show = gio::SimpleAction::new("show-session", Some(glib::VariantTy::STRING));
        let on_show = this.handler(|this, id: String| {
            this.0.window.present();
            this.open_session(&id);
        });
        show.connect_activate(move |_, target| {
            if let Some(id) = target.and_then(|v| v.get::<String>()) {
                on_show(id);
            }
        });
        app.add_action(&show);
        this.install_shortcuts();
        // Con el tema del sistema, el escritorio puede pasar de claro a oscuro.
        let on_dark = this.handler(|this, ()| {
            style::reload_colors();
            this.restyle_terminals();
        });
        adw::StyleManager::default().connect_dark_notify(move |_| on_dark(()));
        this.refresh_hints();
        this.connect_signals();
        this.watch_state(store.dir());
        this.refresh_limits();
        this.refresh_app_usage();
        if this.0.manager.borrow().db().setting("ui.panel_visible").ok().flatten().as_deref() == Some("1") {
            this.0.side_panel.root.set_visible(true);
        }
        *late.borrow_mut() = Rc::downgrade(&this.0);
        {
            let on_tab = this.handler(|this, ()| this.left_tab_changed());
            this.0.code_tab.connect_toggled(move |_| on_tab(()));
        }
        this.detect_claude();
        this.refresh_layout_menu();
        this.reload(false);
        // Con sesiones pero ninguna abierta, se muestra la primera.
        let first = this.0.snapshot.borrow().sessions.first().map(|v| v.session.id.clone());
        match first {
            Some(first) if this.0.layout.borrow().sessions().is_empty() => this.open_session(&first),
            _ => this.render(true),
        }
        for problem in plugin_problems {
            this.toast(&format!("Complemento no cargado. {problem}"));
        }
        if !this.0.plugins.is_empty() {
            this.poll_plugins(true);
            // Un tic por segundo mueve los cronómetros y consulta a cada
            // complemento cuando le toca. Sin complementos no hay temporizador.
            let weak = this.weak();
            glib::timeout_add_seconds_local(1, move || {
                let Some(inner) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                inner.sidebar.tick();
                MainWindow(inner).poll_plugins(false);
                glib::ControlFlow::Continue
            });
        }
        Ok(this)
    }

    pub fn present(&self) {
        self.0.window.present();
    }

    fn weak(&self) -> Weak<Inner> {
        Rc::downgrade(&self.0)
    }

    /// Envuelve un manejador para que solo actúe mientras la ventana exista.
    fn handler<A>(&self, f: impl Fn(&MainWindow, A) + 'static) -> impl Fn(A) + 'static {
        let weak = self.weak();
        move |arg| {
            if let Some(inner) = weak.upgrade() {
                f(&MainWindow(inner), arg);
            }
        }
    }

    // Estado

    /// Sesión visible en el panel con foco.
    fn selected(&self) -> Option<String> {
        self.0.layout.borrow().active_session().map(str::to_string)
    }

    fn view(&self, id: &str) -> Option<SessionView> {
        self.0
            .snapshot
            .borrow()
            .sessions
            .iter()
            .find(|v| v.session.id == id)
            .cloned()
    }

    /// Aviso que se queda a la vista hasta que el usuario lo cierra.
    pub fn notice(&self, message: &str) {
        let toast = adw::Toast::new(message);
        toast.set_timeout(0);
        self.0.toasts.add_toast(toast);
    }

    fn toast(&self, message: &str) {
        log::warn!("{message}");
        self.0.toasts.add_toast(adw::Toast::new(message));
    }

    /// Ejecuta una operación del gestor y muestra el error si falla.
    fn run<T>(&self, op: impl FnOnce(&SessionManager) -> alarviterm_core::Result<T>) -> Option<T> {
        let result = op(&self.0.manager.borrow());
        match result {
            Ok(value) => Some(value),
            Err(e) => {
                self.toast(&e.to_string());
                None
            }
        }
    }

    /// Como `run`, para las operaciones que reescriben varias filas.
    fn run_mut<T>(
        &self,
        op: impl FnOnce(&mut SessionManager) -> alarviterm_core::Result<T>,
    ) -> Option<T> {
        let result = op(&mut self.0.manager.borrow_mut());
        match result {
            Ok(value) => Some(value),
            Err(e) => {
                self.toast(&e.to_string());
                None
            }
        }
    }

    fn save_setting(&self, key: &str, value: &str) {
        if let Err(e) = self.0.manager.borrow().db().set_setting(key, value) {
            log::error!("no se pudo guardar el ajuste {key}: {e}");
        }
    }

    /// Aplica un cambio a la disposición y redibuja. El préstamo se suelta
    /// antes de tocar ningún widget.
    fn edit_layout(&self, focus: bool, edit: impl FnOnce(&mut Layout)) {
        edit(&mut self.0.layout.borrow_mut());
        self.render(focus);
    }

    /// Vuelve a leer base de datos y backend y actualiza toda la interfaz.
    /// `focus` indica si el terminal del panel activo debe tomar el foco.
    fn reload(&self, focus: bool) {
        let Some(mut snapshot) = self.run(|m| m.snapshot()) else {
            return;
        };
        let inner = &self.0;
        search::sort_sessions(&mut snapshot.sessions, inner.settings.borrow().sort);

        let known: HashSet<&str> = snapshot.sessions.iter().map(|v| v.session.id.as_str()).collect();
        inner.layout.borrow_mut().retain_sessions(|id| known.contains(id));
        inner.detached.borrow_mut().retain(|id| known.contains(id.as_str()));

        let alive = snapshot.sessions.iter().filter(|v| v.liveness.is_alive()).count();
        let asking = snapshot
            .sessions
            .iter()
            .filter(|v| v.agent.as_ref().is_some_and(|a| a.phase == Phase::Permission))
            .count();
        inner.footer.set_counts(snapshot.sessions.len(), alive, asking);
        self.notify_attention(&snapshot);
        match &snapshot.backend_error {
            Some(error) => inner.footer.set_backend(&format!("backend: {error}"), true),
            None => inner.footer.set_backend(&inner.backend_label, !inner.backend_ok),
        }

        let signature = inner.sidebar.signature(&snapshot);
        if inner.signature.borrow().as_deref() != Some(signature.as_str()) {
            inner.sidebar.rebuild(
                &snapshot,
                self.selected().as_deref(),
                inner.ungrouped_collapsed.get(),
            );
            *inner.signature.borrow_mut() = Some(signature);
            // Filas nuevas: hay que volver a ponerles lo de los complementos
            // y preguntar por las sesiones que acaban de aparecer.
            *inner.snapshot.borrow_mut() = snapshot.clone();
            self.apply_plugin_extras();
            self.poll_plugins(true);
        } else {
            inner.sidebar.update_status(&snapshot);
        }
        *inner.snapshot.borrow_mut() = snapshot;
        self.render(focus);
        self.refresh_details();
    }

    /// Vigila el directorio donde `alarviterm-hook` y el backend dejan sus
    /// avisos. Así los cambios de estado llegan al momento y sin sondear.
    fn watch_state(&self, dir: &std::path::Path) {
        let file = gio::File::for_path(dir);
        let watcher = match file.monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            Ok(watcher) => watcher,
            Err(e) => {
                log::error!("no se puede vigilar {}: {e}", dir.display());
                return;
            }
        };
        let weak = self.weak();
        watcher.connect_changed(move |_, file, _, _| {
            // Los temporales de la escritura atómica no son un cambio de estado.
            let name = file
                .basename()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let temporary = name.ends_with(".tmp");
            let Some(inner) = weak.upgrade() else {
                return;
            };
            if name == monitor::CLIPBOARD_MARK {
                MainWindow(inner).take_backend_copy();
                return;
            }
            if name.starts_with(limits::LIMITS_MARK) || name.starts_with(limits::CONTEXT_MARK) {
                if !temporary {
                    MainWindow(inner).refresh_limits();
                }
                return;
            }
            if temporary || inner.reload_pending.replace(true) {
                return;
            }
            // Varias sesiones trabajando a la vez emiten decenas de eventos
            // por segundo: se agrupan y además se guarda una distancia mínima
            // entre recargas, que es lo que cuesta.
            let since_last = inner.event_reload_at.get().map_or(EVENT_SPACING, |at| at.elapsed());
            let wait = EVENT_SPACING.saturating_sub(since_last).max(EVENT_DEBOUNCE);
            let weak = Rc::downgrade(&inner);
            glib::timeout_add_local_once(wait, move || {
                if let Some(inner) = weak.upgrade() {
                    inner.reload_pending.set(false);
                    inner.event_reload_at.set(Some(Instant::now()));
                    MainWindow(inner).reload(false);
                }
            });
        });
        *self.0.state_monitor.borrow_mut() = Some(watcher);
    }

    /// Las sesiones tal y como las ven los complementos.
    fn plugin_sessions(&self) -> Vec<SessionInfo> {
        let snapshot = self.0.snapshot.borrow();
        snapshot
            .sessions
            .iter()
            .map(|view| {
                let group = view
                    .session
                    .group_id
                    .and_then(|id| snapshot.groups.iter().find(|g| g.id == id))
                    .map(|g| g.name.as_str());
                let project = view
                    .session
                    .project_id
                    .and_then(|id| snapshot.projects.iter().find(|p| p.id == id))
                    .map(|p| p.name.as_str());
                SessionInfo::from_view(view, group, project)
            })
            .collect()
    }

    /// Consulta el estado a los complementos a los que les toca. Con `force`
    /// no espera a que venza su intervalo. Cada consulta corre fuera del hilo
    /// de la interfaz.
    fn poll_plugins(&self, force: bool) {
        let inner = &self.0;
        if inner.plugins.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut sessions = None;
        for plugin in &inner.plugins {
            let due = inner.plugin_due.borrow().get(&plugin.id).is_none_or(|at| now >= *at);
            if !(force || due) || !inner.plugin_busy.borrow_mut().insert(plugin.id.clone()) {
                continue;
            }
            inner
                .plugin_due
                .borrow_mut()
                .insert(plugin.id.clone(), now + Duration::from_secs(plugin.interval));
            let sessions = sessions.get_or_insert_with(|| self.plugin_sessions()).clone();
            let (plugin, weak) = (plugin.clone(), self.weak());
            glib::spawn_future_local(async move {
                let job = plugin.clone();
                let result = gio::spawn_blocking(move || job.status(&sessions)).await;
                let Some(inner) = weak.upgrade() else {
                    return;
                };
                inner.plugin_busy.borrow_mut().remove(&plugin.id);
                if let Ok(status) = result {
                    MainWindow(inner).plugin_answered(&plugin, status);
                }
            });
        }
    }

    /// Guarda la respuesta de un complemento y la lleva a la barra lateral.
    /// Si falló se conserva lo último que dijo y se avisa una sola vez.
    fn plugin_answered(&self, plugin: &Plugin, status: alarviterm_core::Result<PluginStatus>) {
        let inner = &self.0;
        let error = match status {
            Ok(status) => {
                let error = status.error.clone();
                inner.plugin_status.borrow_mut().insert(plugin.id.clone(), status);
                error.map(|e| format!("{}: {e}", plugin.name))
            }
            Err(e) => Some(e.to_string()),
        };
        let changed = {
            let mut errors = inner.plugin_errors.borrow_mut();
            match &error {
                Some(text) => errors.insert(plugin.id.clone(), text.clone()).as_ref() != Some(text),
                None => {
                    errors.remove(&plugin.id);
                    false
                }
            }
        };
        if let Some(text) = error.filter(|_| changed) {
            self.toast(&text);
        }
        self.apply_plugin_extras();
    }

    /// Reparte en las filas de la barra lateral lo que dijo cada complemento.
    fn apply_plugin_extras(&self) {
        let inner = &self.0;
        if inner.plugins.is_empty() {
            return;
        }
        let statuses = inner.plugin_status.borrow();
        let mut extras: HashMap<String, Vec<RowExtra>> = HashMap::new();
        for plugin in &inner.plugins {
            let Some(status) = statuses.get(&plugin.id) else {
                continue;
            };
            for (session, entry) in &status.sessions {
                extras.entry(session.clone()).or_default().push(RowExtra {
                    badge: entry.badge.clone(),
                    tone: entry.tone,
                    timer_since: entry.timer_since,
                    tooltip: entry.tooltip.clone().or_else(|| Some(plugin.name.clone())),
                    buttons: entry
                        .actions
                        .iter()
                        .filter(|action| action.button)
                        .map(|action| RowButton {
                            label: action.label.clone(),
                            icon: action.icon.clone(),
                            target: plugin_target(&action.id, session, &plugin.id),
                        })
                        .collect(),
                });
            }
        }
        inner.sidebar.set_extras(&extras);
        drop(statuses);
        self.refresh_panel();
    }

    /// Lleva a la columna derecha las listas que los complementos tienen
    /// para la sesión a la vista.
    fn refresh_panel(&self) {
        let inner = &self.0;
        if !inner.side_panel.root.is_visible() {
            return;
        }
        let current = self.selected().and_then(|id| self.view(&id));
        let statuses = inner.plugin_status.borrow();
        let panels: Vec<plugins::Panel> = current
            .iter()
            .flat_map(|view| {
                inner.plugins.iter().filter_map(|plugin| {
                    statuses.get(&plugin.id)?.sessions.get(&view.session.id)?.panel.clone()
                })
            })
            .collect();
        inner.side_panel.set(current.as_ref().map(|view| view.session.name.as_str()), &panels);
    }

    /// Abre el control de código fuente de la carpeta de una sesión.
    /// Lleva a la pestaña de código, con el repositorio de una sesión
    /// desplegado y su primer cambio abierto.
    fn show_scm(&self, id: &str) {
        let Some(view) = self.view(id) else { return };
        let inner = &self.0;
        inner.left.set_visible(true);
        *inner.code_expand.borrow_mut() = Some((view.session.cwd.clone(), true));
        if inner.code_tab.is_active() {
            self.refresh_code();
        } else {
            // Al activarse la pestaña se leen los repositorios.
            inner.code_tab.set_active(true);
        }
    }

    /// Reacciona al cambio de pestaña de la columna izquierda.
    fn left_tab_changed(&self) {
        let inner = &self.0;
        if inner.code_tab.is_active() {
            inner.left_stack.set_visible_child_name(LEFT_CODE);
            // Sin otra indicación, se despliega el repositorio de la sesión a la vista.
            if inner.code_expand.borrow().is_none() {
                let cwd = self.selected().and_then(|id| self.view(&id)).map(|v| v.session.cwd);
                *inner.code_expand.borrow_mut() = cwd.map(|cwd| (cwd, false));
            }
            self.refresh_code();
            return;
        }
        // De vuelta a las sesiones se cierra la comparación, salvo que haya
        // cambios sin guardar y el usuario prefiera quedarse.
        if inner.code_view.is_modified() {
            inner.code_tab.set_active(true);
            let weak = self.weak();
            inner.code_view.guarded(move || {
                if let Some(inner) = weak.upgrade() {
                    inner.sessions_tab.set_active(true);
                }
            });
            return;
        }
        inner.left_stack.set_visible_child_name(LEFT_SESSIONS);
        self.close_code();
    }

    /// Carpetas en las que hay algo abierto: las de los proyectos y las de
    /// las sesiones sin archivar.
    fn code_dirs(&self) -> Vec<std::path::PathBuf> {
        let snapshot = self.0.snapshot.borrow();
        let mut dirs: Vec<std::path::PathBuf> = snapshot.projects.iter().map(|p| p.path.clone()).collect();
        dirs.extend(snapshot.sessions.iter().filter(|v| !v.session.archived).map(|v| v.session.cwd.clone()));
        dirs.sort();
        dirs.dedup();
        dirs
    }

    /// Lee, fuera del hilo de la interfaz, los repositorios de las carpetas
    /// abiertas y sus cambios.
    fn refresh_code(&self) {
        let inner = &self.0;
        if inner.code_busy.replace(true) {
            return;
        }
        inner.codebar.set_loading(true);
        let dirs = self.code_dirs();
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || alarviterm_core::scm::discover(&dirs)).await;
            let Some(inner) = weak.upgrade() else { return };
            inner.code_busy.set(false);
            inner.codebar.set_loading(false);
            let Ok(repos) = result else { return };
            let this = MainWindow(inner);
            let inner = &this.0;
            inner.codebar.set_repos(repos);
            let changes = inner.codebar.total_changes();
            inner.code_tab.set_label(&match changes {
                0 => "Código".to_string(),
                n => format!("Código  {n}"),
            });
            let pending = inner.code_expand.borrow_mut().take();
            if let Some((dir, open_first)) = pending {
                if let Some(root) = inner.codebar.repo_of(&dir) {
                    inner.codebar.expand(&root);
                    if open_first {
                        if let Some(change) = inner.codebar.first_change(&root) {
                            this.open_code(root, change);
                        }
                    }
                }
            }
            // El fichero abierto puede haber cambiado en disco entretanto.
            if let Some((root, path)) = inner.code_view.current() {
                inner.code_view.reload(inner.codebar.change_for(&root, &path).as_ref());
            }
        });
    }

    /// Elimina un árbol de trabajo añadido, tras confirmarlo dos veces: la
    /// primera cuenta qué es y qué arrastra, la segunda lo que se pierde.
    fn remove_worktree(&self, root: std::path::PathBuf) {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let place = display_path(&root);
        let changes = self.0.codebar.changes_in(&root);
        let (branch, pending) = changes.unwrap_or_default();
        let sessions: Vec<String> = self
            .0
            .snapshot
            .borrow()
            .sessions
            .iter()
            .filter(|v| v.session.cwd.starts_with(&root))
            .map(|v| v.session.name.clone())
            .collect();

        let mut first = format!("Carpeta {place}\nRama {branch}\n\n");
        first.push_str(&match pending {
            0 => "No tiene cambios sin confirmar.".to_string(),
            1 => "Tiene 1 fichero con cambios sin confirmar.".to_string(),
            n => format!("Tiene {n} ficheros con cambios sin confirmar."),
        });
        if !sessions.is_empty() {
            first.push_str(&format!(
                "\n\nHay sesiones trabajando en esta carpeta, que se quedarán sin ella: {}.",
                sessions.join(", ")
            ));
        }
        let mut second = format!("Se borrará del disco la carpeta {place}, con todo lo que contiene.");
        if pending > 0 {
            second.push_str(" Los cambios sin confirmar se perderán.");
        }
        second.push_str(&format!(" La rama {branch} se conserva en el repositorio. No se puede deshacer."));

        let weak = self.weak();
        let heading = format!("Eliminar el worktree «{name}»");
        let confirm_again = {
            let (root, heading) = (root.clone(), heading.clone());
            move || {
                let Some(inner) = weak.upgrade() else { return };
                let this = MainWindow(inner);
                let run = {
                    let root = root.clone();
                    this.handler(move |this, ()| this.remove_worktree_now(root.clone(), pending > 0))
                };
                let dialog = dialogs::confirm(
                    &this.0.window,
                    &heading,
                    &second,
                    "Eliminar definitivamente",
                    true,
                    move || run(()),
                );
                this.refocus_after(&dialog);
            }
        };
        let dialog = dialogs::confirm(&self.0.window, &heading, &first, "Continuar", true, confirm_again);
        self.refocus_after(&dialog);
    }

    fn remove_worktree_now(&self, root: std::path::PathBuf, force: bool) {
        let inner = &self.0;
        // Si lo que se está comparando es de esa carpeta, se cierra antes.
        if inner.code_view.current().is_some_and(|(open, _)| open == root) {
            inner.center.set_visible_child_name(CENTER_TERMINALS);
            inner.codebar.select(None);
        }
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let target = root.clone();
            let result = gio::spawn_blocking(move || alarviterm_core::scm::remove_worktree(&target, force)).await;
            let Some(inner) = weak.upgrade() else { return };
            let this = MainWindow(inner);
            match result {
                Ok(Ok(())) => {
                    this.0.toasts.add_toast(adw::Toast::new(&format!("Worktree {} eliminado", display_path(&root))));
                }
                Ok(Err(error)) => this.toast(&format!("No se pudo eliminar el worktree: {error}")),
                Err(_) => this.toast("No se pudo eliminar el worktree"),
            }
            this.refresh_code();
        });
    }

    /// Relee solo los repositorios desplegados. Es lo que se hace cada pocos
    /// segundos: leerlos todos, con decenas de árboles de trabajo, es caro.
    fn refresh_open_code(&self) {
        let inner = &self.0;
        let roots = inner.codebar.expanded_roots();
        if roots.is_empty() || inner.code_busy.replace(true) {
            return;
        }
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || alarviterm_core::scm::refresh(&roots)).await;
            let Some(inner) = weak.upgrade() else { return };
            inner.code_busy.set(false);
            let Ok(repos) = result else { return };
            inner.codebar.update_repos(repos);
            let changes = inner.codebar.total_changes();
            inner.code_tab.set_label(&match changes {
                0 => "Código".to_string(),
                n => format!("Código  {n}"),
            });
            if let Some((root, path)) = inner.code_view.current() {
                inner.code_view.reload(inner.codebar.change_for(&root, &path).as_ref());
            }
        });
    }

    /// Abre un fichero cambiado en la comparación, que ocupa el sitio de los
    /// terminales.
    fn open_code(&self, root: std::path::PathBuf, change: alarviterm_core::scm::Change) {
        let weak = self.weak();
        self.0.code_view.guarded(move || {
            let Some(inner) = weak.upgrade() else { return };
            inner.code_view.open(&root, &change);
            inner.center.set_visible_child_name(CENTER_CODE);
            inner.codebar.select(Some((root.clone(), change.path.clone())));
        });
    }

    /// Cierra la comparación y devuelve los terminales.
    fn close_code(&self) {
        let inner = &self.0;
        if inner.center.visible_child_name().as_deref() != Some(CENTER_CODE) {
            return;
        }
        inner.center.set_visible_child_name(CENTER_TERMINALS);
        inner.codebar.select(None);
        self.render(true);
    }

    fn toggle_panel(&self) {
        let root = &self.0.side_panel.root;
        root.set_visible(!root.is_visible());
        let shown = if root.is_visible() { "1" } else { "0" };
        self.run(|m| m.db().set_setting("ui.panel_visible", shown));
        self.refresh_panel();
    }

    /// Ejecuta una acción de un complemento sobre una sesión y, con su
    /// respuesta, vuelve a pedirle el estado.
    fn plugin_action(&self, target: &str) {
        let mut parts = target.splitn(3, '|');
        let (Some(action), Some(session), Some(plugin_id)) = (parts.next(), parts.next(), parts.next()) else {
            return;
        };
        let inner = &self.0;
        let Some(plugin) = inner.plugins.iter().find(|p| p.id == plugin_id).cloned() else {
            return;
        };
        let sessions = self.plugin_sessions();
        let Some(info) = sessions.iter().find(|s| s.id == session).cloned() else {
            return;
        };
        let (action, weak) = (action.to_string(), self.weak());
        glib::spawn_future_local(async move {
            let job = plugin.clone();
            let result = gio::spawn_blocking(move || {
                let outcome = job.act(&action, &info);
                (outcome, job.status(&sessions))
            })
            .await;
            let (Some(inner), Ok((outcome, status))) = (weak.upgrade(), result) else {
                return;
            };
            let this = MainWindow(inner);
            match outcome {
                Ok(outcome) => {
                    if let Some(message) = &outcome.message {
                        this.0.toasts.add_toast(adw::Toast::new(message));
                    } else if !outcome.ok {
                        this.toast(&format!("{}: la acción no se pudo completar.", plugin.name));
                    }
                    if let Some(url) = &outcome.open_url {
                        gtk::UriLauncher::new(url).launch(Some(&this.0.window), gio::Cancellable::NONE, |result| {
                            if let Err(e) = result {
                                log::warn!("no se pudo abrir la dirección: {e}");
                            }
                        });
                    }
                }
                Err(e) => this.toast(&e.to_string()),
            }
            this.plugin_answered(&plugin, status);
        });
    }

    /// Acciones que los complementos ofrecen para una sesión, con el nombre
    /// del complemento, su etiqueta y el parámetro de `win.plugin-action`.
    fn plugin_actions_for(&self, session: &str) -> Vec<(String, String, String)> {
        let inner = &self.0;
        let statuses = inner.plugin_status.borrow();
        let mut found = Vec::new();
        for plugin in &inner.plugins {
            let actions = statuses
                .get(&plugin.id)
                .and_then(|status| status.sessions.get(session))
                .map(|entry| entry.actions.as_slice())
                .unwrap_or_default();
            for action in actions {
                found.push((
                    plugin.name.clone(),
                    action.label.clone(),
                    plugin_target(&action.id, session, &plugin.id),
                ));
            }
        }
        found
    }

    /// Con el panel del equipo abierto, busca los programas que más consumen.
    /// Recorrer todos los procesos cuesta unos milisegundos: se hace fuera
    /// del hilo de la interfaz y solo mientras alguien lo mira.
    fn refresh_processes(&self) {
        let inner = &self.0;
        if !inner.footer.system_panel_open() {
            inner.process_sample.borrow_mut().take();
            return;
        }
        if inner.process_busy.replace(true) {
            return;
        }
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(sysinfo::process_table).await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            inner.process_busy.set(false);
            let Ok(table) = result else {
                return;
            };
            let now = Instant::now();
            let previous = inner.process_sample.borrow_mut().take();
            let before = previous.as_ref().map(|(table, at)| (table, now.duration_since(*at).as_secs_f64()));
            let top = sysinfo::top_processes(before, &table, super::footer::TOP_PROCESSES);
            inner.footer.set_processes(&top);
            *inner.process_sample.borrow_mut() = Some((table, now));
        });
    }

    /// Mide lo que consume la propia aplicación y lo lleva al pie. La lectura
    /// de `/proc` corre fuera del hilo de la interfaz.
    fn refresh_app_usage(&self) {
        if !self.0.settings.borrow().footer_usage {
            self.0.footer.set_app_usage(None);
            return;
        }
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(sysinfo::self_usage).await;
            let (Some(inner), Ok(Some(usage))) = (weak.upgrade(), result) else {
                return;
            };
            let now = Instant::now();
            let cpu = inner.app_sample.replace(Some((usage.cpu_ticks, now))).and_then(|(ticks, at)| {
                sysinfo::cpu_percent(ticks, usage.cpu_ticks, now.duration_since(at).as_secs_f64())
            });
            inner.footer.set_app_usage(Some((usage, cpu)));
        });
    }

    /// Muestra en el pie los límites de uso que dejó la última línea de
    /// estado de alguna sesión, si el usuario quiere verlos.
    /// Cuenta del proyecto de la sesión a la vista. Vacía: la predeterminada.
    fn current_account(&self) -> String {
        let snapshot = self.0.snapshot.borrow();
        self.selected()
            .and_then(|id| snapshot.sessions.iter().find(|v| v.session.id == id))
            .and_then(|view| view.session.project_id)
            .and_then(|id| snapshot.projects.iter().find(|p| p.id == id))
            .map(|project| project.account.clone())
            .unwrap_or_default()
    }

    fn refresh_limits(&self) {
        let inner = &self.0;
        // Los de la cuenta de la sesión a la vista: cada proyecto puede
        // trabajar con una distinta.
        let account = self.current_account();
        let limits = inner
            .settings
            .borrow()
            .account_limits
            .then(|| limits::read(&inner.state_dir, &account))
            .flatten();
        // Y el contexto que lleva usado esa sesión, que también llega por
        // la línea de estado.
        let context = self.selected().and_then(|id| limits::read_context(&inner.state_dir, &id));
        let alive = self.selected().and_then(|id| self.view(&id)).is_some_and(|v| v.liveness.is_alive());
        inner.footer.set_context(context.filter(|_| alive).as_ref());
        inner.footer.set_limits(limits.as_ref(), &account, alarviterm_core::now_unix());
    }

    /// Lleva al portapapeles del sistema lo que se acaba de copiar dentro de
    /// tmux, que es lo que ocurre al seleccionar con el ratón. El fichero se
    /// borra al leerlo para no dejar en disco lo copiado.
    fn take_backend_copy(&self) {
        let path = self.0.state_dir.join(monitor::CLIPBOARD_MARK);
        let Ok(bytes) = std::fs::read(&path) else {
            // Ya se leyó: el borrado también llega como evento.
            return;
        };
        if let Err(e) = std::fs::remove_file(&path) {
            log::warn!("no se pudo borrar {}: {e}", path.display());
        }
        let text = String::from_utf8_lossy(&bytes);
        if !text.is_empty() {
            self.0.window.clipboard().set_text(&text);
        }
    }

    /// Suelta los terminales que llevan un rato sin verse. Cada uno mantiene
    /// un cliente del backend al que hay que seguir dibujándole la salida. Al
    /// volver a su pestaña se conecta de nuevo, y la sesión no se entera.
    fn trim_hidden_terminals(&self) {
        let ttl = hidden_terminal_ttl();
        let stale: Vec<String> = self
            .0
            .hidden_since
            .borrow()
            .iter()
            .filter(|(_, since)| since.elapsed() >= ttl)
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            log::debug!("terminal oculto {id} desconectado");
            self.0.hidden_since.borrow_mut().remove(&id);
            self.drop_terminal(&id);
        }
    }

    /// Notificación de escritorio cuando una sesión pasa a necesitar al
    /// usuario y la ventana no está delante. Se retira al dejar de hacer falta.
    fn notify_attention(&self, snapshot: &Snapshot) {
        let inner = &self.0;
        let Some(app) = inner.window.application() else {
            return;
        };
        let mut phases = inner.phases.borrow_mut();
        let mut current = HashMap::new();
        for view in &snapshot.sessions {
            let id = &view.session.id;
            let notification_id = format!("session-{id}");
            let Some(agent) = &view.agent else {
                if phases.contains_key(id) {
                    app.withdraw_notification(&notification_id);
                }
                continue;
            };
            let before = phases.get(id).copied();
            current.insert(id.clone(), agent.phase);
            if !agent.phase.needs_user() {
                if before.is_some_and(Phase::needs_user) {
                    app.withdraw_notification(&notification_id);
                }
                continue;
            }
            if inner.window.is_active() {
                continue;
            }
            let wanted = {
                let settings = inner.settings.borrow();
                match agent.phase {
                    Phase::Permission => settings.notify_permission,
                    _ => settings.notify_turn_end,
                }
            };
            if !wanted {
                continue;
            }
            if let Some(body) = status::attention_message(before, agent) {
                let notification = gio::Notification::new(&view.session.name);
                notification.set_body(Some(&body));
                notification.set_default_action_and_target_value(
                    "app.show-session",
                    Some(&id.to_variant()),
                );
                app.send_notification(Some(&notification_id), &notification);
            }
        }
        *phases = current;
    }

    /// Actualiza en el pie los datos que cuesta obtener (git, consumo) de la
    /// sesión activa. La consulta corre fuera del hilo de la interfaz.
    fn refresh_details(&self) {
        let inner = &self.0;
        let current = inner.layout.borrow().active_session().map(str::to_string);
        let target = current.as_deref().and_then(|id| self.view(id));
        if *inner.details_for.borrow() != current {
            // Otra sesión: los datos anteriores ya no valen.
            inner.footer.set_git(None);
            inner.footer.set_usage(None);
            *inner.details_for.borrow_mut() = current.clone();
            inner.details_at.set(None);
        }
        let model = target.as_ref().and_then(|v| v.agent.as_ref()).and_then(|a| a.model.as_deref());
        inner.footer.set_model(model);
        let Some(view) = target else {
            return;
        };
        if inner.details_at.get().is_some_and(|at| at.elapsed() < DETAILS_EVERY) {
            return;
        }
        if inner.details_busy.replace(true) {
            return;
        }
        inner.details_at.set(Some(Instant::now()));
        let id = view.session.id.clone();
        let cwd = view.session.cwd.clone();
        let pid = match view.liveness {
            Liveness::Alive { pid, .. } => pid,
            _ => None,
        };
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                (sysinfo::git_summary(&cwd), pid.and_then(sysinfo::tree_usage))
            })
            .await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            inner.details_busy.set(false);
            let Ok((git, usage)) = result else {
                return;
            };
            MainWindow(inner).show_details(&id, git, usage);
        });
    }

    fn show_details(&self, id: &str, git: Option<GitSummary>, usage: Option<Usage>) {
        let inner = &self.0;
        if inner.details_for.borrow().as_deref() != Some(id) {
            // La sesión activa cambió mientras se consultaba.
            self.refresh_details();
            return;
        }
        inner.footer.set_git(git.as_ref());
        let now = Instant::now();
        let previous = inner.cpu_sample.borrow_mut().take();
        let cpu = usage.and_then(|usage| match previous {
            Some((before_id, ticks, at)) if before_id == id => {
                sysinfo::cpu_percent(ticks, usage.cpu_ticks, now.duration_since(at).as_secs_f64())
            }
            _ => None,
        });
        if let Some(usage) = usage {
            *inner.cpu_sample.borrow_mut() = Some((id.to_string(), usage.cpu_ticks, now));
        }
        inner.footer.set_usage(usage.map(|u| (u.rss_bytes, cpu, u.processes)));
    }

    /// Lleva la disposición a la pantalla: crea o retira paneles, reconstruye
    /// el árbol si cambió su forma, pone cada terminal en su sitio y
    /// actualiza cabecera, footer y barra lateral.
    ///
    /// Trabaja sobre copias del modelo: tocar widgets dispara señales (foco,
    /// separadores) que vuelven a entrar aquí y necesitan el modelo libre.
    fn render(&self, focus: bool) {
        let inner = &self.0;
        inner.rendering.set(true);
        let layout = inner.layout.borrow().clone();
        let snapshot = inner.snapshot.borrow().clone();
        let panes = layout.panes();
        let find = |id: &str| snapshot.sessions.iter().find(|v| v.session.id == id);

        // Maximizar solo tiene sentido sobre el panel con foco y con más de uno.
        let maximized = inner
            .maximized
            .get()
            .filter(|id| panes.len() > 1 && layout.focused() == *id);
        inner.maximized.set(maximized);

        // 1. Una vista por panel.
        let callbacks = self.pane_callbacks();
        let removed: Vec<Rc<PaneView>> = {
            let mut views = inner.views.borrow_mut();
            for pane in &panes {
                views
                    .entry(pane.id)
                    .or_insert_with(|| PaneView::new(pane.id, &callbacks));
            }
            let gone: Vec<NodeId> = views
                .keys()
                .filter(|id| layout.pane(**id).is_none())
                .copied()
                .collect();
            gone.iter().filter_map(|id| views.remove(id)).collect()
        };
        let views = inner.views.borrow().clone();

        // 2. El árbol de divisiones, solo si cambió su forma.
        let signature = format!("{}|{maximized:?}", layout.structure_signature());
        if inner.tree_signature.borrow().as_deref() != Some(signature.as_str()) {
            let old: Vec<gtk::Paned> = inner.paneds.borrow_mut().drain(..).collect();
            for paned in old {
                paned.set_start_child(gtk::Widget::NONE);
                paned.set_end_child(gtk::Widget::NONE);
            }
            while let Some(child) = inner.holder.first_child() {
                inner.holder.remove(&child);
            }
            let mut paneds = Vec::new();
            let root = match maximized {
                Some(id) => views.get(&id).map(|v| v.root.clone().upcast::<gtk::Widget>()),
                None => panes::build_tree(layout.root(), &views, &mut paneds, &self.ratio_callback()),
            };
            if let Some(root) = root {
                root.set_hexpand(true);
                root.set_vexpand(true);
                inner.holder.append(&root);
            }
            *inner.paneds.borrow_mut() = paneds;
            *inner.tree_signature.borrow_mut() = Some(signature);
        }

        // 3. Contenido. Primero se suelta lo que ya no corresponde a cada
        // panel, para que un terminal pueda mudarse a otro.
        for view in &removed {
            view.retain_children(|_| false);
        }
        for pane in &panes {
            if let Some(view) = views.get(&pane.id) {
                view.retain_children(|session| pane.tabs.iter().any(|t| t == session));
            }
        }
        // Una sesión sin pestaña no necesita terminal: al soltarlo se cierra
        // su cliente de conexión y el proceso sigue en el backend.
        let open: HashSet<&str> = layout.sessions().into_iter().collect();
        inner
            .terminals
            .borrow_mut()
            .retain(|id, _| open.contains(id.as_str()));

        // Se anota desde cuándo no se ve cada terminal abierto.
        {
            let visible: HashSet<&str> = panes.iter().filter_map(|p| p.active_session()).collect();
            let terminals = inner.terminals.borrow();
            let mut hidden = inner.hidden_since.borrow_mut();
            hidden.retain(|id, _| terminals.contains_key(id) && !visible.contains(id.as_str()));
            for id in terminals.keys() {
                if !visible.contains(id.as_str()) {
                    hidden.entry(id.clone()).or_insert_with(Instant::now);
                }
            }
        }

        let alone = panes.len() == 1;
        for pane in &panes {
            let Some(view) = views.get(&pane.id) else {
                continue;
            };
            let tabs: Vec<TabInfo> = pane
                .tabs
                .iter()
                .filter_map(|id| find(id))
                .map(|v| TabInfo {
                    session_id: v.session.id.clone(),
                    name: v.session.name.clone(),
                    status_class: panes::status_class(v),
                })
                .collect();
            view.set_tabs(&tabs, pane.active_session());
            if let Some(session) = pane.active_session().and_then(&find) {
                self.ensure_child(view, session, snapshot.backend_error.is_some());
            }
            view.show(pane.active_session());
            view.set_state(
                !alone && pane.id == layout.focused(),
                maximized == Some(pane.id),
                alone,
            );
        }

        // 4. Lo que rodea a los paneles.
        let current = layout.active_session().and_then(find);
        inner.footer.set_session(current);
        self.refresh_limits();
        self.refresh_panel();
        match current {
            Some(view) => {
                inner.title.set_title(&view.session.name);
                inner.title.set_subtitle(&display_path(&view.session.cwd));
            }
            None => {
                inner.title.set_title("ALarviTerm");
                inner.title.set_subtitle("");
            }
        }
        inner.sidebar.select(current.map(|v| v.session.id.as_str()));
        inner.pages.set_visible_child_name(if snapshot.sessions.is_empty() {
            PAGE_EMPTY
        } else {
            PAGE_PANES
        });

        // 5. La disposición se guarda en cuanto cambia.
        let json = layout.to_json();
        if *inner.saved_layout.borrow() != json {
            self.save_setting(KEY_LAYOUT, &json);
            *inner.saved_layout.borrow_mut() = json;
        }

        inner.rendering.set(false);
        let shown = layout.active_session().map(str::to_string);
        if *inner.details_for.borrow() != shown {
            self.refresh_details();
        }
        if focus {
            if let Some(view) = views.get(&layout.focused()) {
                view.focus_content();
            }
        }
    }

    /// Deja en el panel el contenido que corresponde a la sesión: su
    /// terminal, o un aviso si no hay proceso al que conectarse.
    fn ensure_child(&self, view: &PaneView, session: &SessionView, backend_down: bool) {
        let inner = &self.0;
        let id = session.session.id.as_str();
        let desired = if session.liveness == Liveness::Missing {
            ChildKind::Missing
        } else if inner.detached.borrow().contains(id) {
            ChildKind::Detached
        } else {
            ChildKind::Terminal
        };
        if desired == ChildKind::Missing {
            self.forget_terminal(id);
            inner.detached.borrow_mut().remove(id);
        }
        if view.child_kind(id) == Some(desired) {
            return;
        }
        match desired {
            ChildKind::Terminal => {
                if let Some(terminal) = self.terminal_for(session) {
                    view.set_child(id, desired, &terminal);
                }
            }
            _ => {
                let is_claude = session.session.kind == SessionKind::Claude;
                view.set_child(id, desired, &panes::notice(desired, id, is_claude, backend_down));
            }
        }
    }

    /// Terminal conectado a la sesión. Se crea la primera vez que hace falta.
    fn terminal_for(&self, session: &SessionView) -> Option<vte::Terminal> {
        let id = session.session.id.clone();
        if let Some(existing) = self.0.terminals.borrow().get(&id) {
            return Some(existing.clone());
        }
        let attach = self.run(|m| m.backend().attach_command(&session.session.backend_id))?;
        let on_exit = {
            let id = id.clone();
            self.handler(move |this, terminal: vte::Terminal| this.terminal_exited(&id, &terminal))
        };
        // El directorio de la sesión puede haber desaparecido. El del cliente
        // de conexión es irrelevante para el proceso real.
        let cwd = home_dir().unwrap_or_else(|_| "/".into());
        let profile = self
            .0
            .settings
            .borrow()
            .profile(session.session.profile.as_deref())
            .clone();
        let terminal = terminal::spawn(&attach, &cwd, &profile, move |t| on_exit(t.clone()));
        let replaced = self.0.terminals.borrow_mut().insert(id, terminal.clone());
        // Se suelta ya sin la tabla prestada, igual que en `forget_terminal`.
        drop(replaced);
        Some(terminal)
    }

    /// Saca un terminal de la tabla. Al destruirse, VTE puede avisar en ese
    /// mismo instante de que su proceso terminó, y ese aviso consulta la
    /// tabla: por eso el terminal se suelta con la tabla ya devuelta.
    fn forget_terminal(&self, id: &str) -> bool {
        let removed = self.0.terminals.borrow_mut().remove(id);
        removed.is_some()
    }

    /// Retira el terminal de una sesión, esté en el panel que esté.
    fn drop_terminal(&self, id: &str) {
        if self.forget_terminal(id) {
            let views: Vec<Rc<PaneView>> = self.0.views.borrow().values().cloned().collect();
            for view in views {
                if view.child_kind(id) == Some(ChildKind::Terminal) {
                    view.remove_child(id);
                }
            }
        }
    }

    fn terminal_exited(&self, id: &str, terminal: &vte::Terminal) {
        // Un terminal ya retirado (reinicio, borrado) no debe alterar nada.
        let current = self.0.terminals.try_borrow().map(|table| table.get(id) == Some(terminal));
        // Con la tabla en plena modificación, quien la modifica ya se ocupa.
        if !matches!(current, Ok(true)) {
            return;
        }
        self.drop_terminal(id);
        self.0.detached.borrow_mut().insert(id.to_string());
        // Si la sesión ya no existe, `reload` la mostrará como desconectada.
        self.reload(false);
    }

    // Paneles

    fn pane_callbacks(&self) -> PaneCallbacks {
        let focused = self.handler(|this, pane: NodeId| this.pane_focused(pane));
        let dropped = self.handler(|this, (pane, zone, session): (NodeId, DropZone, String)| {
            // Se aplaza: el widget que recibe la suelta puede desaparecer al
            // reorganizar los paneles.
            let weak = this.weak();
            glib::idle_add_local_once(move || {
                if let Some(inner) = weak.upgrade() {
                    MainWindow(inner).session_dropped(pane, zone, &session);
                }
            });
        });
        PaneCallbacks {
            focused: Rc::new(focused),
            dropped: Rc::new(move |pane, zone, session| dropped((pane, zone, session))),
        }
    }

    fn ratio_callback(&self) -> Rc<dyn Fn(NodeId, f64)> {
        let weak = self.weak();
        Rc::new(move |split, ratio| {
            if let Some(inner) = weak.upgrade() {
                if !inner.rendering.get() {
                    inner.layout.borrow_mut().set_ratio(split, ratio);
                }
            }
        })
    }

    /// El usuario ha entrado en un panel, con el teclado o con un clic.
    fn pane_focused(&self, pane: NodeId) {
        if self.0.rendering.get() {
            return;
        }
        let changed = {
            let mut layout = self.0.layout.borrow_mut();
            layout.focused() != pane && layout.set_focused(pane)
        };
        if changed {
            self.render(true);
        }
    }

    /// Muestra la sesión: enfoca el panel donde ya esté abierta, o la abre
    /// como pestaña en el panel con foco.
    fn open_session(&self, id: &str) {
        if self.view(id).is_none() {
            return;
        }
        self.run(|m| m.mark_opened(id));
        self.edit_layout(true, |layout| {
            layout.show(id);
        });
    }

    fn split_pane(&self, pane: NodeId, orientation: Orientation, session: Option<&str>) {
        self.0.maximized.set(None);
        self.edit_layout(true, |layout| {
            if let Some(new) = layout.split(pane, orientation, Side::After) {
                if let Some(session) = session {
                    layout.open(new, session);
                }
            }
        });
    }

    /// Cierra la pestaña activa. Si el panel ya está vacío, cierra el panel.
    fn close_current(&self) {
        let (pane, active) = {
            let layout = self.0.layout.borrow();
            (layout.focused(), layout.active_session().map(str::to_string))
        };
        self.edit_layout(true, |layout| match active {
            Some(session) => {
                layout.close_tab(&session);
            }
            None => {
                layout.close_pane(pane);
            }
        });
    }

    fn toggle_maximize(&self, pane: NodeId) {
        if !self.0.layout.borrow_mut().set_focused(pane) {
            return;
        }
        let maximized = &self.0.maximized;
        maximized.set(if maximized.get() == Some(pane) {
            None
        } else {
            Some(pane)
        });
        self.render(true);
    }

    fn session_dropped(&self, pane: NodeId, zone: DropZone, session: &str) {
        if self.view(session).is_none() {
            return;
        }
        self.0.maximized.set(None);
        self.edit_layout(true, |layout| {
            let source = layout.locate(session);
            let target = match zone {
                DropZone::Center => Some(pane),
                DropZone::Left => layout.split(pane, Orientation::Horizontal, Side::Before),
                DropZone::Right => layout.split(pane, Orientation::Horizontal, Side::After),
                DropZone::Top => layout.split(pane, Orientation::Vertical, Side::Before),
                DropZone::Bottom => layout.split(pane, Orientation::Vertical, Side::After),
            };
            let Some(target) = target else { return };
            layout.open(target, session);
            // Un panel que se queda vacío porque le han sacado su única
            // pestaña ya no pinta nada.
            if let Some(source) = source.filter(|s| *s != target) {
                if layout.pane(source).is_some_and(|p| p.tabs.is_empty()) {
                    layout.close_pane(source);
                    layout.set_focused(target);
                }
            }
        });
    }

    fn apply_preset(&self, preset: Preset) {
        self.0.maximized.set(None);
        self.edit_layout(true, |layout| {
            let sessions: Vec<String> = layout.sessions().into_iter().map(str::to_string).collect();
            *layout = Layout::preset(preset, &sessions);
        });
    }

    fn save_layout_dialog(&self) {
        let on_accept = self.handler(|this, name: String| {
            let json = this.0.layout.borrow().to_json();
            if this.run(|m| m.db().save_layout(&name, &json)).is_some() {
                this.refresh_layout_menu();
                this.0
                    .toasts
                    .add_toast(adw::Toast::new(&format!("Disposición «{name}» guardada")));
            }
        });
        let dialog = dialogs::prompt(
            &self.0.window,
            "Guardar la disposición actual",
            "",
            "Guardar",
            on_accept,
        );
        self.refocus_after(&dialog);
    }

    fn apply_saved_layout(&self, name: &str) {
        let Some(saved) = self.run(|m| m.db().layouts()) else {
            return;
        };
        let Some((_, json)) = saved.into_iter().find(|(n, _)| n == name) else {
            return;
        };
        match Layout::from_json(&json) {
            Ok(mut restored) => {
                let known: HashSet<String> = self
                    .0
                    .snapshot
                    .borrow()
                    .sessions
                    .iter()
                    .map(|v| v.session.id.clone())
                    .collect();
                restored.retain_sessions(|id| known.contains(id));
                self.0.maximized.set(None);
                self.edit_layout(true, |layout| *layout = restored);
            }
            Err(e) => self.toast(&e.to_string()),
        }
    }

    fn delete_saved_layout(&self, name: &str) {
        if self.run(|m| m.db().delete_layout(name)).is_some() {
            self.refresh_layout_menu();
        }
    }

    fn refresh_layout_menu(&self) {
        let menu = &self.0.layout_menu;
        menu.remove_all();

        let presets = gio::Menu::new();
        for (label, name) in PRESETS {
            presets.append_item(&sidebar::menu_item(label, "win.layout-preset", name));
        }
        menu.append_section(None, &presets);

        let saved = self
            .0
            .manager
            .borrow()
            .db()
            .layouts()
            .unwrap_or_else(|e| {
                log::error!("no se pudieron leer las disposiciones guardadas: {e}");
                Vec::new()
            });
        let manage = gio::Menu::new();
        if !saved.is_empty() {
            let apply = gio::Menu::new();
            let delete = gio::Menu::new();
            for (name, _) in &saved {
                apply.append_item(&sidebar::menu_item(name, "win.layout-apply", name));
                delete.append_item(&sidebar::menu_item(name, "win.layout-delete", name));
            }
            menu.append_section(Some("Guardadas"), &apply);
            manage.append_submenu(Some("Eliminar una guardada"), &delete);
        }
        manage.prepend_item(&sidebar::plain_item(
            "Guardar la disposición actual",
            "win.layout-save",
        ));
        menu.append_section(None, &manage);
    }

    /// Shell nuevo en el directorio y grupo de la sesión actual.
    fn new_terminal(&self) {
        let current = self.selected().and_then(|id| self.view(&id));
        let cwd = current
            .as_ref()
            .map(|v| v.session.cwd.clone())
            .filter(|dir| dir.is_dir())
            .or_else(|| home_dir().ok())
            .unwrap_or_else(|| "/".into());
        let folder = cwd
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "inicio".to_string());
        let new = NewSession {
            name: format!("Terminal {folder}"),
            group_id: current.as_ref().and_then(|v| v.session.group_id),
            kind: SessionKind::Shell,
            cwd,
            model: None,
            extra_args: String::new(),
            env: Vec::new(),
            // El terminal nace en el mismo proyecto, con su entorno.
            project_id: current.and_then(|v| v.session.project_id),
            resume: None,
        };
        if let Some(session) = self.run(|m| m.create_session(new)) {
            self.reload(false);
            self.open_session(&session.id);
        }
    }

    /// Una sesión arrastrada dentro de la barra lateral: cambia de grupo y de
    /// posición según dónde se suelte.
    fn sidebar_drop(&self, session: &str, target: RowTarget) {
        let (group, before) = match target {
            RowTarget::Session(other) if other == session => return,
            RowTarget::Session(other) => match self.view(&other) {
                Some(view) => (view.session.group_id, Some(other)),
                None => return,
            },
            RowTarget::Group(group) => (Some(group), None),
            RowTarget::Ungrouped => (None, None),
        };
        if self
            .run_mut(|m| m.db_mut().place_session(session, group, before.as_deref()))
            .is_some()
        {
            self.reload(false);
        }
    }

    // Acciones

    fn add_action(&self, name: &str, f: impl Fn(&MainWindow) + 'static) {
        let action = gio::SimpleAction::new(name, None);
        let run = self.handler(move |this, ()| f(this));
        action.connect_activate(move |_, _| run(()));
        self.0.window.add_action(&action);
    }

    fn add_target_action(&self, name: &str, f: impl Fn(&MainWindow, &str) + 'static) {
        let action = gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
        let run = self.handler(move |this, target: String| f(this, &target));
        action.connect_activate(move |_, parameter| {
            if let Some(target) = parameter.and_then(|p| p.str()) {
                run(target.to_string());
            }
        });
        self.0.window.add_action(&action);
    }

    fn install_actions(&self) {
        self.add_target_action("plugin-action", |this, target| this.plugin_action(target));
        self.add_action("new-session", |this| this.new_session_dialog(None));
        self.add_action("new-terminal", |this| this.new_terminal());
        self.add_action("new-group", |this| this.new_group_dialog());
        self.add_action("toggle-sidebar", |this| this.toggle_sidebar());
        self.add_action("next-session", |this| this.cycle(1));
        self.add_action("previous-session", |this| this.cycle(-1));
        self.add_action("next-attention", |this| this.next_attention());
        self.add_action("command-palette", |this| this.show_palette());
        self.add_action("shortcuts", |this| this.show_shortcuts());
        self.add_action("search-sessions", |this| {
            this.0.left.set_visible(true);
            this.0.sessions_tab.set_active(true);
            this.0.sidebar.toggle_search();
        });
        self.add_action("focus-terminal", |this| this.render(true));
        for n in 1..=9usize {
            self.add_action(&format!("goto-session-{n}"), move |this| {
                if let Some(id) = this.0.sidebar.visible_sessions().get(n - 1) {
                    this.open_session(id);
                }
            });
        }
        // Sobre la sesión del panel con foco, para el teclado y la paleta.
        for (name, target) in [
            ("current-rename", "session-rename"),
            ("current-restart", "session-restart"),
            ("current-continue", "session-continue"),
            ("current-stop", "session-stop"),
            ("current-delete", "session-delete"),
        ] {
            self.add_action(name, move |this| match this.selected() {
                Some(id) => {
                    gio::prelude::ActionGroupExt::activate_action(
                        &this.0.window,
                        target,
                        Some(&id.to_variant()),
                    );
                }
                None => this.toast("No hay ninguna sesión abierta en el panel con foco"),
            });
        }
        self.add_target_action("session-open", |this, id| this.open_session(id));
        self.add_action("settings", |this| this.show_settings());
        self.add_action("stats", |this| this.show_stats());
        self.add_action("toggle-panel", |this| this.toggle_panel());
        self.add_action("scm", |this| {
            if let Some(id) = this.selected() {
                this.show_scm(&id);
            }
        });
        self.add_target_action("session-scm", |this, id| this.show_scm(id));
        self.add_target_action("code-worktree-remove", |this, root| {
            this.remove_worktree(std::path::PathBuf::from(root));
        });
        self.add_action("projects", |this| this.show_projects());
        self.add_action("current-archive", |this| match this.selected() {
            Some(id) => this.set_archived(&id, true),
            None => this.toast("No hay ninguna sesión abierta en el panel con foco"),
        });
        self.add_target_action("session-archive", |this, id| this.set_archived(id, true));
        self.add_target_action("session-unarchive", |this, id| this.set_archived(id, false));
        // Destinos con la forma «sesión|valor». Valor vacío quita el dato.
        self.add_target_action("session-profile", |this, target| {
            let Some((id, name)) = target.split_once('|') else {
                return;
            };
            let name = Some(name).filter(|n| !n.is_empty());
            if this.run(|m| m.set_profile(id, name)).is_some() {
                this.reload(false);
                this.restyle_terminals();
            }
        });
        self.add_target_action("session-project", |this, target| {
            let Some((id, project)) = target.split_once('|') else {
                return;
            };
            if this.run(|m| m.set_project(id, project.parse().ok())).is_some() {
                this.reload(false);
                this.0.toasts.add_toast(adw::Toast::new(
                    "El entorno del proyecto se aplicará cuando la sesión se reinicie",
                ));
            }
        });
        self.add_target_action("project-new-session", |this, project| {
            this.new_session_in(None, project.parse().ok());
        });
        self.add_target_action("project-import", |this, project| {
            if let Ok(id) = project.parse() {
                this.import_conversations(id);
            }
        });
        self.add_target_action("project-new-terminal", |this, project| {
            if let Ok(id) = project.parse() {
                this.new_project_terminal(id);
            }
        });
        self.add_target_action("sort-sessions", |this, order| {
            this.0.settings.borrow_mut().sort = SortOrder::parse(order);
            this.apply_settings();
        });
        self.add_target_action("sidebar-filter", |this, name| {
            this.0.left.set_visible(true);
            this.0.sessions_tab.set_active(true);
            this.0.sidebar.set_status_filter(StatusFilter::parse(name));
        });

        // Sobre el panel con foco.
        self.add_action("split-right", |this| {
            let pane = this.0.layout.borrow().focused();
            this.split_pane(pane, Orientation::Horizontal, None);
        });
        self.add_action("split-down", |this| {
            let pane = this.0.layout.borrow().focused();
            this.split_pane(pane, Orientation::Vertical, None);
        });
        self.add_action("close-current", |this| this.close_current());
        self.add_action("toggle-maximize", |this| {
            let pane = this.0.layout.borrow().focused();
            this.toggle_maximize(pane);
        });
        for (name, step) in [("next-tab", 1), ("previous-tab", -1)] {
            self.add_action(name, move |this| {
                this.edit_layout(true, |layout| {
                    layout.cycle_tab(step);
                });
            });
        }
        for (name, direction) in [
            ("focus-left", Direction::Left),
            ("focus-right", Direction::Right),
            ("focus-up", Direction::Up),
            ("focus-down", Direction::Down),
        ] {
            self.add_action(name, move |this| {
                this.0.maximized.set(None);
                this.edit_layout(true, |layout| {
                    layout.focus_direction(direction);
                });
            });
        }

        // Sobre un panel concreto, desde los botones de su cabecera.
        self.add_target_action("pane-split-right", |this, pane| {
            if let Ok(pane) = pane.parse() {
                this.split_pane(pane, Orientation::Horizontal, None);
            }
        });
        self.add_target_action("pane-split-down", |this, pane| {
            if let Ok(pane) = pane.parse() {
                this.split_pane(pane, Orientation::Vertical, None);
            }
        });
        self.add_target_action("pane-maximize", |this, pane| {
            if let Ok(pane) = pane.parse() {
                this.toggle_maximize(pane);
            }
        });
        self.add_target_action("pane-close", |this, pane| {
            if let Ok(pane) = pane.parse::<NodeId>() {
                this.edit_layout(true, |layout| {
                    layout.close_pane(pane);
                });
            }
        });
        // Destino con la forma «panel|sesión».
        self.add_target_action("tab-select", |this, target| {
            let Some((pane, session)) = target.split_once('|') else {
                return;
            };
            if let Ok(pane) = pane.parse::<NodeId>() {
                this.edit_layout(true, |layout| {
                    layout.open(pane, session);
                });
            }
        });
        // Destino con la forma «cuáles|sesión»: others, left o right.
        self.add_target_action("tab-close-many", |this, target| {
            let Some((which, session)) = target.split_once('|') else {
                return;
            };
            if let Some(which) = alarviterm_core::layout::CloseTabs::parse(which) {
                this.edit_layout(true, |layout| {
                    layout.close_tabs(session, which);
                });
            }
        });
        // Destino con la forma «sesión|destino|lado»: before o after.
        self.add_target_action("tab-move", |this, target| {
            let mut parts = target.split('|');
            let (Some(session), Some(anchor), Some(side)) = (parts.next(), parts.next(), parts.next()) else {
                return;
            };
            this.edit_layout(true, |layout| {
                layout.move_tab(session, anchor, side == "after");
            });
        });
        self.add_target_action("tab-close", |this, session| {
            this.edit_layout(true, |layout| {
                layout.close_tab(session);
            });
        });

        self.add_target_action("layout-preset", |this, name| {
            if let Some(preset) = Preset::parse(name) {
                this.apply_preset(preset);
            }
        });
        self.add_action("layout-save", |this| this.save_layout_dialog());
        self.add_target_action("layout-apply", |this, name| this.apply_saved_layout(name));
        self.add_target_action("layout-delete", |this, name| this.delete_saved_layout(name));

        self.add_target_action("session-open-right", |this, id| {
            let pane = this.0.layout.borrow().focused();
            this.split_pane(pane, Orientation::Horizontal, Some(id));
        });
        self.add_target_action("session-open-down", |this, id| {
            let pane = this.0.layout.borrow().focused();
            this.split_pane(pane, Orientation::Vertical, Some(id));
        });
        self.add_target_action("session-rename", |this, id| this.rename_session(id));
        self.add_target_action("session-restart", |this, id| this.restart_session(id, Resume::Fresh));
        self.add_target_action("session-continue", |this, id| {
            this.restart_session(id, Resume::ContinueLatest)
        });
        self.add_target_action("session-reattach", |this, id| {
            this.0.detached.borrow_mut().remove(id);
            this.open_session(id);
        });
        self.add_target_action("session-stop", |this, id| this.stop_session(id));
        self.add_target_action("session-delete", |this, id| this.delete_session(id));
        // Destino con la forma «sesión|grupo». Grupo vacío significa sin grupo.
        self.add_target_action("session-move", |this, target| {
            let Some((id, group)) = target.split_once('|') else {
                return;
            };
            let group_id = group.parse::<i64>().ok();
            if this.run(|m| m.move_to_group(id, group_id)).is_some() {
                this.reload(false);
            }
        });

        type Pick = fn(i64) -> Batch;
        let batches: [(&str, Pick, bool); 4] = [
            ("group-start", Batch::Group, true),
            ("group-stop", Batch::Group, false),
            ("project-start", Batch::Project, true),
            ("project-stop", Batch::Project, false),
        ];
        for (name, pick, start) in batches {
            self.add_target_action(name, move |this, target| {
                let Ok(id) = target.parse::<i64>() else { return };
                if start {
                    this.start_sessions(pick(id));
                } else {
                    this.stop_sessions(pick(id));
                }
            });
        }
        self.add_target_action("group-new-session", |this, group| {
            this.new_session_dialog(group.parse().ok())
        });
        self.add_target_action("group-rename", |this, group| {
            if let Ok(id) = group.parse::<i64>() {
                this.rename_group(id);
            }
        });
        self.add_target_action("group-delete", |this, group| {
            if let Ok(id) = group.parse::<i64>() {
                this.delete_group(id);
            }
        });
        for (name, delta) in [("group-up", -1), ("group-down", 1)] {
            self.add_target_action(name, move |this, group| {
                if let Ok(id) = group.parse::<i64>() {
                    if this.run_mut(|m| m.db_mut().move_group(id, delta)).is_some() {
                        this.reload(false);
                    }
                }
            });
        }
    }

    fn install_shortcuts(&self) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = self.weak();
        keys.connect_key_pressed(move |_, key, _code, state| {
            let handled = weak
                .upgrade()
                .is_some_and(|inner| MainWindow(inner).handle_key(key, state));
            if handled {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        self.0.window.add_controller(keys);
    }

    /// Atajos globales, según el mapa de teclas. Cada atajo dice si vale
    /// también con el foco en un terminal. Los que no, como Ctrl+N o Ctrl+B,
    /// se le dejan al shell y a Claude Code. Con un diálogo abierto no actúa
    /// ninguno: las teclas son del diálogo.
    fn handle_key(&self, key: gdk::Key, state: gdk::ModifierType) -> bool {
        if self.0.window.visible_dialog().is_some() {
            return false;
        }
        let Some(combo) = keys::combo_from_event(key, state) else {
            return false;
        };
        if !combo.is_bindable() {
            return false;
        }
        let in_terminal = gtk::prelude::GtkWindowExt::focus(&self.0.window)
            .is_some_and(|w| w.is::<vte::Terminal>());
        let action = self.0.keymap.borrow().lookup(&combo, in_terminal);
        match action {
            Some(action) => {
                gio::prelude::ActionGroupExt::activate_action(&self.0.window, action, None);
                true
            }
            None => false,
        }
    }

    /// Pone en los tooltips y en la barra lateral los atajos vigentes.
    fn refresh_hints(&self) {
        let keymap = self.0.keymap.borrow();
        for (widget, text, action) in &self.0.hinted {
            let tooltip = match keymap.label(action) {
                Some(keys) => format!("{text} ({keys})"),
                None => (*text).to_string(),
            };
            widget.set_tooltip_text(Some(&tooltip));
        }
        self.0.sidebar.set_hints(
            keymap.label("search-sessions").as_deref(),
            keymap.label("new-session").as_deref(),
        );
    }

    // Paleta de comandos y atajos

    /// Todo lo que se puede lanzar desde la paleta en este momento.
    fn palette_commands(&self) -> Vec<Command> {
        let keymap = self.0.keymap.borrow();
        let snapshot = self.0.snapshot.borrow();
        let current = self.selected().and_then(|id| self.view(&id));
        let mut commands = Vec::new();

        for def in actions::ACTIONS {
            if !def.in_palette || def.id == "command-palette" {
                continue;
            }
            // Las acciones sobre la sesión actual solo salen si hay una.
            if def.id.starts_with("current-") {
                let Some(view) = &current else { continue };
                if def.id == "current-continue" && view.session.kind != SessionKind::Claude {
                    continue;
                }
            }
            commands.push(Command {
                title: def.title.to_string(),
                hint: def.category.label().to_string(),
                shortcut: keymap.label(def.id),
                action: def.id.to_string(),
                target: None,
            });
        }

        if let Some(view) = &current {
            for (plugin, label, target) in self.plugin_actions_for(&view.session.id) {
                commands.push(Command {
                    title: format!("{plugin}: {label}"),
                    hint: format!("Complemento, {}", view.session.name),
                    shortcut: None,
                    action: "plugin-action".to_string(),
                    target: Some(target),
                });
            }
        }

        let group_name = |id: Option<i64>| {
            id.and_then(|id| snapshot.groups.iter().find(|g| g.id == id))
                .map(|g| g.name.clone())
        };
        for view in &snapshot.sessions {
            let info = status::describe(view);
            let place = group_name(view.session.group_id)
                .unwrap_or_else(|| display_path(&view.session.cwd));
            commands.push(Command {
                title: format!("Ir a {}", view.session.name),
                hint: format!("{}, {place}", info.label),
                shortcut: None,
                action: "session-open".to_string(),
                target: Some(view.session.id.clone()),
            });
        }

        if let Some(view) = &current {
            let id = &view.session.id;
            for group in &snapshot.groups {
                if view.session.group_id != Some(group.id) {
                    commands.push(Command {
                        title: format!("Mover la sesión actual al grupo {}", group.name),
                        hint: "Sesiones".to_string(),
                        shortcut: None,
                        action: "session-move".to_string(),
                        target: Some(format!("{id}|{}", group.id)),
                    });
                }
            }
            if view.session.group_id.is_some() {
                commands.push(Command {
                    title: "Sacar la sesión actual de su grupo".to_string(),
                    hint: "Sesiones".to_string(),
                    shortcut: None,
                    action: "session-move".to_string(),
                    target: Some(format!("{id}|")),
                });
            }
        }

        for project in &snapshot.projects {
            let place = display_path(&project.path);
            for (title, action) in [
                ("Nueva sesión en el proyecto", "project-new-session"),
                ("Nuevo terminal en el proyecto", "project-new-terminal"),
                ("Añadir conversaciones existentes al proyecto", "project-import"),
                ("Encender las sesiones del proyecto", "project-start"),
                ("Apagar las sesiones del proyecto", "project-stop"),
            ] {
                commands.push(Command {
                    title: format!("{title} {}", project.name),
                    hint: place.clone(),
                    shortcut: None,
                    action: action.to_string(),
                    target: Some(project.id.to_string()),
                });
            }
        }
        for order in SortOrder::ALL {
            commands.push(Command {
                title: format!("Ordenar las sesiones: {}", order.label()),
                hint: "Vista".to_string(),
                shortcut: None,
                action: "sort-sessions".to_string(),
                target: Some(order.as_str().to_string()),
            });
        }
        for (label, name) in PRESETS {
            commands.push(Command {
                title: format!("Disposición: {label}"),
                hint: "Paneles".to_string(),
                shortcut: None,
                action: "layout-preset".to_string(),
                target: Some(name.to_string()),
            });
        }
        let saved = self.0.manager.borrow().db().layouts().unwrap_or_default();
        for (name, _) in saved {
            commands.push(Command {
                title: format!("Disposición guardada: {name}"),
                hint: "Paneles".to_string(),
                shortcut: None,
                action: "layout-apply".to_string(),
                target: Some(name),
            });
        }
        for filter in StatusFilter::ALL {
            commands.push(Command {
                title: format!("Filtrar la barra lateral: {}", filter.label()),
                hint: "Vista".to_string(),
                shortcut: None,
                action: "sidebar-filter".to_string(),
                target: Some(filter.as_str().to_string()),
            });
        }
        commands
    }

    fn show_palette(&self) {
        let on_run = self.handler(|this, command: Command| {
            let target = command.target.as_ref().map(|t| t.to_variant());
            gio::prelude::ActionGroupExt::activate_action(&this.0.window, &command.action, target.as_ref());
        });
        let dialog = palette::show(&self.0.window, self.palette_commands(), on_run);
        self.refocus_after(&dialog);
    }

    fn show_shortcuts(&self) {
        let on_change = self.handler(|this, ()| {
            let json = this.0.keymap.borrow().overrides_json();
            this.save_setting(KEY_KEYMAP, &json);
            this.refresh_hints();
        });
        let dialog = shortcuts::show(&self.0.window, self.0.keymap.clone(), move || on_change(()));
        self.refocus_after(&dialog);
    }

    // Ajustes y proyectos

    fn show_stats(&self) {
        let inner = &self.0;
        let Some(data_dir) = inner.state_dir.parent() else {
            return;
        };
        let cache = Paths::at(data_dir.to_path_buf()).stats_cache();
        // Carpetas de cada cuenta con nombre, según los proyectos.
        let mut accounts: Vec<(String, Vec<std::path::PathBuf>)> = Vec::new();
        for project in inner.snapshot.borrow().projects.iter().filter(|p| !p.account.is_empty()) {
            match accounts.iter_mut().find(|(name, _)| name.eq_ignore_ascii_case(&project.account)) {
                Some((_, folders)) => folders.push(project.path.clone()),
                None => accounts.push((project.account.clone(), vec![project.path.clone()])),
            }
        }
        let dialog = stats::show(&inner.window, inner.stats_library.clone(), cache, accounts);
        self.refocus_after(&dialog);
    }

    fn show_settings(&self) {
        let on_change = self.handler(|this, ()| this.apply_settings());
        let on_rename = self.handler(|this, (old, new): (String, String)| {
            this.run(|m| m.db().rename_profile(&old, &new));
        });
        let on_shortcuts = self.handler(|this, ()| this.show_shortcuts());
        let dialog = prefs::show(
            &self.0.window,
            self.0.settings.clone(),
            move || on_change(()),
            move |old, new| on_rename((old.to_string(), new.to_string())),
            move || on_shortcuts(()),
        );
        self.refocus_after(&dialog);
    }

    /// Guarda los ajustes y los lleva a todo lo que depende de ellos.
    fn apply_settings(&self) {
        let inner = &self.0;
        let settings = inner.settings.borrow().clone();
        self.run(|m| settings.save(m.db()));
        style::apply_theme(settings.theme);
        inner
            .footer
            .set_parts(settings.footer_git, settings.footer_model, settings.footer_usage);
        {
            let mut manager = inner.manager.borrow_mut();
            let shell = Some(settings.shell.clone())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| inner.env_shell.clone());
            if manager.shell() != shell {
                manager.set_shell(shell);
            }
        }
        if inner.backend_ok {
            if let Err(e) = inner.tmux.write_config(&tmux_options(&settings, &inner.state_dir)) {
                self.toast(&e.to_string());
            }
        }
        // Las sesiones nuevas llevarán o no la línea de estado que recoge los
        // límites. Las que ya corren conservan lo que tenían al arrancar.
        if let (Some(hook), Some(data_dir)) = (monitor::find_hook_binary(), inner.state_dir.parent()) {
            let file = Paths::at(data_dir.to_path_buf()).hook_settings();
            match monitor::hook_settings(&hook, settings.statusline(), Some(&settings.claude_theme)) {
                Ok(text) => {
                    if let Err(e) = std::fs::write(&file, text) {
                        self.toast(&format!("No se pudieron guardar los ajustes de hooks: {e}"));
                    }
                }
                Err(e) => self.toast(&e.to_string()),
            }
        }
        self.refresh_limits();
        self.refresh_app_usage();
        self.detect_claude();
        self.restyle_terminals();
        // El orden pudo cambiar: se fuerza a rehacer la lista.
        *inner.signature.borrow_mut() = None;
        self.reload(false);
    }

    /// Vuelve a aplicar su perfil a cada terminal abierto.
    fn restyle_terminals(&self) {
        let settings = self.0.settings.borrow();
        let snapshot = self.0.snapshot.borrow();
        for (id, terminal) in self.0.terminals.borrow().iter() {
            let profile = snapshot
                .sessions
                .iter()
                .find(|v| v.session.id == *id)
                .and_then(|v| v.session.profile.as_deref());
            terminal::apply_profile(terminal, settings.profile(profile));
        }
    }

    fn set_archived(&self, id: &str, archived: bool) {
        if self.run(|m| m.set_archived(id, archived)).is_none() {
            return;
        }
        if archived {
            // Deja de estar a la vista, pero su proceso sigue.
            self.0.layout.borrow_mut().close_tab(id);
            self.0.toasts.add_toast(adw::Toast::new(
                "Sesión archivada. Está en el filtro Archivadas de la barra lateral",
            ));
        }
        self.reload(true);
    }

    fn show_projects(&self) {
        let projects = self.0.snapshot.borrow().projects.clone();
        let callbacks = ManagerCallbacks {
            new: {
                let f = self.handler(|this, ()| this.edit_project(None));
                Box::new(move || f(()))
            },
            edit: Box::new(self.handler(|this, id: i64| this.edit_project(Some(id)))),
            delete: Box::new(self.handler(|this, id: i64| this.delete_project(id))),
            launch: Box::new(self.handler(|this, id: i64| this.new_session_in(None, Some(id)))),
            import: Box::new(self.handler(|this, id: i64| this.import_conversations(id))),
            power: Box::new(self.handler(|this, id: i64| {
                let which = Batch::Project(id);
                if this.batch(which).1.iter().any(|v| v.liveness.is_alive()) {
                    this.stop_sessions(which);
                } else {
                    this.start_sessions(which);
                }
            })),
        };
        let dialog = projects::manager(&self.0.window, &projects, callbacks);
        self.refocus_after(&dialog);
    }

    fn edit_project(&self, id: Option<i64>) {
        let existing = id.and_then(|id| {
            self.0.snapshot.borrow().projects.iter().find(|p| p.id == id).cloned()
        });
        let weak = self.weak();
        let dialog = projects::editor(self.0.window.upcast_ref(), existing.as_ref(), move |new: NewProject| {
            let Some(inner) = weak.upgrade() else {
                return Ok(());
            };
            let this = MainWindow(inner);
            let result = {
                let manager = this.0.manager.borrow();
                match id {
                    Some(id) => manager.db().update_project(id, new),
                    None => manager.db().create_project(new).map(|_| ()),
                }
            };
            result.map_err(|e| e.to_string())?;
            this.reload(false);
            // De vuelta a la lista, ya con el cambio.
            let reopen = this.handler(|this, ()| this.show_projects());
            glib::idle_add_local_once(move || reopen(()));
            Ok(())
        });
        self.refocus_after(&dialog);
    }

    fn delete_project(&self, id: i64) {
        let Some(project) = self.0.snapshot.borrow().projects.iter().find(|p| p.id == id).cloned() else {
            return;
        };
        let used = self
            .0
            .snapshot
            .borrow()
            .sessions
            .iter()
            .filter(|v| v.session.project_id == Some(id))
            .count();
        let body = match used {
            0 => "Ninguna sesión lo usa.".to_string(),
            1 => "Su sesión se conserva, ya sin proyecto.".to_string(),
            n => format!("Sus {n} sesiones se conservan, ya sin proyecto."),
        };
        let on_accept = self.handler(move |this, ()| {
            if this.run(|m| m.db().delete_project(id)).is_some() {
                this.reload(false);
            }
        });
        let dialog = dialogs::confirm(
            &self.0.window,
            &format!("Eliminar el proyecto «{}»", project.name),
            &body,
            "Eliminar",
            true,
            move || on_accept(()),
        );
        self.refocus_after(&dialog);
    }

    /// Shell nuevo en la carpeta de un proyecto, con su entorno.
    fn new_project_terminal(&self, project_id: i64) {
        let Some(project) = self
            .0
            .snapshot
            .borrow()
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .cloned()
        else {
            return;
        };
        let new = NewSession {
            name: format!("Terminal {}", project.name),
            group_id: None,
            kind: SessionKind::Shell,
            cwd: project.path.clone(),
            model: None,
            extra_args: String::new(),
            env: Vec::new(),
            project_id: Some(project.id),
            resume: None,
        };
        if let Some(session) = self.run(|m| m.create_session(new)) {
            self.reload(false);
            self.open_session(&session.id);
        }
    }

    /// Salta a la siguiente sesión en la que Claude Code espera al usuario.
    /// Las que piden permiso van antes que las que solo terminaron su turno.
    fn next_attention(&self) {
        let waiting: Vec<String> = {
            let snapshot = self.0.snapshot.borrow();
            let with = |phase: Phase| {
                snapshot
                    .sessions
                    .iter()
                    // Quien tiene trabajo en segundo plano no espera al usuario.
                    .filter(move |v| v.agent.as_ref().is_some_and(|a| a.phase == phase && !a.busy_in_background()))
                    .map(|v| v.session.id.clone())
            };
            with(Phase::Permission).chain(with(Phase::Waiting)).collect()
        };
        if waiting.is_empty() {
            self.0.toasts.add_toast(adw::Toast::new("Ninguna sesión te espera"));
            return;
        }
        let current = self.selected().and_then(|id| waiting.iter().position(|w| *w == id));
        let next = current.map_or(0, |index| (index + 1) % waiting.len());
        self.open_session(&waiting[next]);
    }

    fn connect_signals(&self) {
        let inner = &self.0;

        let on_selected = self.handler(|this, row: Option<gtk::ListBoxRow>| {
            let Some(RowTarget::Session(id)) = row.as_ref().and_then(sidebar::row_target) else {
                return;
            };
            if this.selected().as_deref() != Some(id.as_str()) {
                this.open_session(&id);
            }
        });
        inner
            .sidebar
            .list
            .connect_row_selected(move |_, row| on_selected(row.cloned()));

        let on_activated = self.handler(|this, row: gtk::ListBoxRow| match sidebar::row_target(&row) {
            Some(RowTarget::Group(id)) => this.toggle_group(id),
            Some(RowTarget::Ungrouped) => {
                let collapsed = !this.0.ungrouped_collapsed.get();
                this.0.ungrouped_collapsed.set(collapsed);
                this.save_setting(KEY_UNGROUPED_COLLAPSED, if collapsed { "1" } else { "0" });
                *this.0.signature.borrow_mut() = None;
                this.reload(false);
            }
            // Activar la sesión ya seleccionada devuelve el foco al terminal.
            Some(RowTarget::Session(id)) => this.open_session(&id),
            None => {}
        });
        inner
            .sidebar
            .list
            .connect_row_activated(move |_, row| on_activated(row.clone()));

        let right_click = gtk::GestureClick::new();
        right_click.set_button(gdk::BUTTON_SECONDARY);
        let on_menu = self.handler(|this, (x, y): (f64, f64)| this.show_context_menu(x, y));
        right_click.connect_pressed(move |gesture, _, x, y| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            on_menu((x, y));
        });
        inner.sidebar.list.add_controller(right_click);

        // Doble clic: la sesión ya se abrió con el primero, ahora ocupa todo.
        let double_click = gtk::GestureClick::new();
        double_click.set_button(gdk::BUTTON_PRIMARY);
        let on_double = self.handler(|this, y: f64| {
            if let Some(RowTarget::Session(_)) = this.0.sidebar.target_at(y) {
                let pane = this.0.layout.borrow().focused();
                if this.0.maximized.get() != Some(pane) {
                    this.toggle_maximize(pane);
                }
            }
        });
        double_click.connect_released(move |_, presses, _, y| {
            if presses == 2 {
                on_double(y);
            }
        });
        inner.sidebar.list.add_controller(double_click);

        // Soltar una sesión sobre otra fila la recoloca.
        let drop = gtk::DropTarget::new(SessionRef::static_type(), gdk::DragAction::MOVE);
        let on_drop = Rc::new(self.handler(|this, (session, y): (String, f64)| {
            if let Some(target) = this.0.sidebar.target_at(y) {
                this.sidebar_drop(&session, target);
            }
        }));
        drop.connect_drop(move |_, value, _, y| match value.get::<SessionRef>() {
            Ok(session) => {
                let (on_drop, session) = (on_drop.clone(), session.0);
                glib::idle_add_local_once(move || on_drop((session, y)));
                true
            }
            Err(_) => false,
        });
        inner.sidebar.list.add_controller(drop);

        // Buscador de la barra lateral.
        let on_filter = Rc::new(self.handler(|this, ()| this.reload(false)));
        let on_accept = Rc::new(self.handler(|this, ()| {
            if let Some(first) = this.0.sidebar.visible_sessions().first() {
                this.open_session(first);
            }
        }));
        let on_closed = Rc::new(self.handler(|this, ()| {
            this.reload(false);
            this.render(true);
        }));
        inner.sidebar.connect_search(SearchCallbacks {
            changed: {
                let f = on_filter.clone();
                Rc::new(move || f(()))
            },
            accepted: Rc::new(move || on_accept(())),
            closed: Rc::new(move || on_closed(())),
        });

        let on_active = self.handler(|this, active: bool| {
            if active {
                this.reload(false);
                if this.0.code_tab.is_active() {
                    this.refresh_open_code();
                }
            }
        });
        inner
            .window
            .connect_is_active_notify(move |window| on_active(window.is_active()));

        let weak = self.weak();
        glib::timeout_add_seconds_local(REFRESH_SECONDS, move || {
            let Some(inner) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            // La cuenta atrás de los límites avanza aunque nada cambie.
            MainWindow(inner.clone()).refresh_limits();
            MainWindow(inner.clone()).refresh_app_usage();
            // Con la pestaña de código a la vista, sus cambios se mantienen al día.
            if inner.window.is_active() && inner.code_tab.is_active() && inner.left.is_visible() {
                MainWindow(inner.clone()).refresh_open_code();
            }
            if inner.window.is_active() {
                MainWindow(inner).reload(false);
            }
            glib::ControlFlow::Continue
        });

        // Consumo del equipo: una lectura de /proc cada pocos segundos.
        let weak = self.weak();
        let sample_system = move || {
            let Some(inner) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            let wanted = inner.settings.borrow().footer_usage;
            inner.footer.set_system(wanted.then(sysinfo::system_sample).flatten());
            MainWindow(inner).refresh_processes();
            glib::ControlFlow::Continue
        };
        sample_system();
        glib::timeout_add_seconds_local(SYSTEM_SECONDS, sample_system);

        let weak = self.weak();
        let check_every = hidden_terminal_ttl().as_secs().clamp(1, 30) as u32;
        glib::timeout_add_seconds_local(check_every, move || {
            let Some(inner) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            MainWindow(inner).trim_hidden_terminals();
            glib::ControlFlow::Continue
        });

        let on_close = self.handler(|this, ()| this.save_window_state());
        inner.window.connect_close_request(move |_| {
            on_close(());
            glib::Propagation::Proceed
        });
    }

    fn save_window_state(&self) {
        let inner = &self.0;
        if !inner.window.is_maximized() {
            self.save_setting(KEY_WINDOW_WIDTH, &inner.window.width().to_string());
            self.save_setting(KEY_WINDOW_HEIGHT, &inner.window.height().to_string());
        }
        if inner.left.is_visible() {
            self.save_setting(KEY_SIDEBAR_WIDTH, &inner.paned.position().to_string());
        }
        // Las proporciones de los separadores cambian sin pasar por `render`.
        self.save_setting(KEY_LAYOUT, &inner.layout.borrow().to_json());
        self.save_setting(
            KEY_SIDEBAR_VISIBLE,
            if inner.left.is_visible() { "1" } else { "0" },
        );
    }

    /// Detecta la versión de Claude Code en segundo plano: ejecutar el binario
    /// tarda lo bastante como para notarse en el arranque.
    fn detect_claude(&self) {
        self.0.footer.set_claude("claude: detectando", None, false);
        let weak = self.weak();
        let fixed = claude_override(&self.0.settings.borrow());
        glib::spawn_future_local(async move {
            let install = gio::spawn_blocking(move || claude::detect(fixed.as_deref()))
                .await
                .ok()
                .flatten();
            log::debug!("detección de Claude Code: {install:?}");
            let Some(inner) = weak.upgrade() else {
                return;
            };
            match install {
                Some(install) => {
                    let auth = match install.auth {
                        AuthHint::CredentialsFile => "Hay credenciales guardadas.",
                        AuthHint::ApiKeyInEnvironment => "Hay una clave de API en el entorno.",
                        AuthHint::Unknown => "No se ha podido determinar si hay sesión iniciada.",
                    };
                    let text = match &install.version {
                        // «2.1.330 (Claude Code)» se queda en el número.
                        Some(version) => format!(
                            "claude {}",
                            version.split_whitespace().next().unwrap_or(version)
                        ),
                        None => "claude (versión desconocida)".to_string(),
                    };
                    let tooltip = format!("{}\n{auth}", install.executable.display());
                    inner.footer.set_claude(&text, Some(&tooltip), false);
                    inner
                        .manager
                        .borrow_mut()
                        .set_claude_executable(Some(install.executable));
                }
                None => {
                    inner.manager.borrow_mut().set_claude_executable(None);
                    inner.footer.set_claude(
                        "claude no encontrado",
                        Some("No se encuentra el ejecutable claude. Solo se pueden crear sesiones de shell."),
                        true,
                    );
                }
            }
        });
    }

    // Operaciones de interfaz

    /// Al cerrarse un diálogo el foco vuelve al terminal visible, no al
    /// último botón pulsado.
    fn refocus_after(&self, dialog: &adw::Dialog) {
        let on_closed = self.handler(|this, ()| this.render(true));
        dialog.connect_closed(move |_| on_closed(()));
    }

    fn toggle_sidebar(&self) {
        let root = &self.0.left;
        root.set_visible(!root.is_visible());
        if !root.is_visible() {
            self.render(true);
        }
    }

    fn cycle(&self, step: i64) {
        let order = self.0.sidebar.visible_sessions();
        if order.is_empty() {
            return;
        }
        let current = self
            .selected()
            .and_then(|id| order.iter().position(|o| *o == id));
        let next = match current {
            Some(index) => (index as i64 + step).rem_euclid(order.len() as i64) as usize,
            None => 0,
        };
        self.open_session(&order[next]);
    }

    fn toggle_group(&self, id: i64) {
        let collapsed = self
            .0
            .snapshot
            .borrow()
            .groups
            .iter()
            .find(|g| g.id == id)
            .map(|g| g.collapsed);
        if let Some(collapsed) = collapsed {
            if self.run(|m| m.db().set_group_collapsed(id, !collapsed)).is_some() {
                self.reload(false);
            }
        }
    }

    fn show_context_menu(&self, x: f64, y: f64) {
        let menu = gio::Menu::new();
        match self.0.sidebar.target_at(y) {
            Some(RowTarget::Session(id)) => {
                let Some(view) = self.view(&id) else { return };
                let open = gio::Menu::new();
                open.append_item(&sidebar::menu_item(
                    "Abrir en un panel a la derecha",
                    "win.session-open-right",
                    &id,
                ));
                open.append_item(&sidebar::menu_item(
                    "Abrir en un panel debajo",
                    "win.session-open-down",
                    &id,
                ));
                menu.append_section(None, &open);

                let edit = gio::Menu::new();
                edit.append_item(&sidebar::menu_item("Renombrar", "win.session-rename", &id));
                let groups = gio::Menu::new();
                for group in &self.0.snapshot.borrow().groups {
                    if view.session.group_id != Some(group.id) {
                        groups.append_item(&sidebar::menu_item(
                            &group.name,
                            "win.session-move",
                            &format!("{id}|{}", group.id),
                        ));
                    }
                }
                if view.session.group_id.is_some() {
                    groups.append_item(&sidebar::menu_item(
                        "Sin grupo",
                        "win.session-move",
                        &format!("{id}|"),
                    ));
                }
                if groups.n_items() > 0 {
                    edit.append_submenu(Some("Mover a"), &groups);
                }
                menu.append_section(None, &edit);

                let code = gio::Menu::new();
                code.append_item(&sidebar::menu_item("Control de código fuente", "win.session-scm", &id));
                menu.append_section(None, &code);

                let process = gio::Menu::new();
                let restart = if view.liveness == Liveness::Missing {
                    "Relanzar"
                } else {
                    "Reiniciar"
                };
                process.append_item(&sidebar::menu_item(restart, "win.session-restart", &id));
                if view.session.kind == SessionKind::Claude {
                    process.append_item(&sidebar::menu_item(
                        "Reiniciar continuando la conversación",
                        "win.session-continue",
                        &id,
                    ));
                }
                if view.liveness != Liveness::Missing {
                    process.append_item(&sidebar::menu_item("Detener", "win.session-stop", &id));
                }
                menu.append_section(None, &process);

                let extra = gio::Menu::new();
                for (plugin, label, target) in self.plugin_actions_for(&id) {
                    extra.append_item(&sidebar::menu_item(
                        &format!("{plugin}: {label}"),
                        "win.plugin-action",
                        &target,
                    ));
                }
                if extra.n_items() > 0 {
                    menu.append_section(None, &extra);
                }

                let (projects, profiles, default_profile) = {
                    let settings = self.0.settings.borrow();
                    (
                        self.0.snapshot.borrow().projects.clone(),
                        settings.profiles.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
                        settings.default_profile.clone(),
                    )
                };
                let look = gio::Menu::new();
                if !projects.is_empty() {
                    let choose = gio::Menu::new();
                    for project in &projects {
                        if view.session.project_id != Some(project.id) {
                            choose.append_item(&sidebar::menu_item(
                                &projects::label(project),
                                "win.session-project",
                                &format!("{id}|{}", project.id),
                            ));
                        }
                    }
                    if view.session.project_id.is_some() {
                        choose.append_item(&sidebar::menu_item(
                            "Ninguno",
                            "win.session-project",
                            &format!("{id}|"),
                        ));
                    }
                    look.append_submenu(Some("Proyecto"), &choose);
                }
                if profiles.len() > 1 {
                    let choose = gio::Menu::new();
                    for name in &profiles {
                        let label = if *name == default_profile {
                            format!("{name} (predeterminado)")
                        } else {
                            name.clone()
                        };
                        choose.append_item(&sidebar::menu_item(
                            &label,
                            "win.session-profile",
                            &format!("{id}|{name}"),
                        ));
                    }
                    look.append_submenu(Some("Perfil de terminal"), &choose);
                }
                if look.n_items() > 0 {
                    menu.append_section(None, &look);
                }

                let danger = gio::Menu::new();
                if view.session.archived {
                    danger.append_item(&sidebar::menu_item("Desarchivar", "win.session-unarchive", &id));
                } else {
                    danger.append_item(&sidebar::menu_item("Archivar", "win.session-archive", &id));
                }
                danger.append_item(&sidebar::menu_item("Eliminar", "win.session-delete", &id));
                menu.append_section(None, &danger);
            }
            Some(RowTarget::Group(id)) => {
                let target = id.to_string();
                menu.append_item(&sidebar::menu_item(
                    "Nueva sesión en este grupo",
                    "win.group-new-session",
                    &target,
                ));
                menu.append_item(&sidebar::menu_item("Encender las sesiones del grupo", "win.group-start", &target));
                menu.append_item(&sidebar::menu_item("Apagar las sesiones del grupo", "win.group-stop", &target));
                menu.append_item(&sidebar::menu_item("Renombrar grupo", "win.group-rename", &target));
                menu.append_item(&sidebar::menu_item("Subir grupo", "win.group-up", &target));
                menu.append_item(&sidebar::menu_item("Bajar grupo", "win.group-down", &target));
                menu.append_item(&sidebar::menu_item("Eliminar grupo", "win.group-delete", &target));
            }
            Some(RowTarget::Ungrouped) | None => {
                menu.append_item(&sidebar::plain_item("Nueva sesión", "win.new-session"));
                menu.append_item(&sidebar::plain_item("Nuevo grupo", "win.new-group"));
            }
        }
        self.0.sidebar.show_menu(&menu, x, y);
    }

    fn new_session_dialog(&self, group_id: Option<i64>) {
        self.new_session_in(group_id, None);
    }

    fn new_session_in(&self, group_id: Option<i64>, project_id: Option<i64>) {
        let current = self.selected().and_then(|id| self.view(&id));
        let defaults = NewSessionDefaults {
            groups: self.0.snapshot.borrow().groups.clone(),
            group_id: group_id.or_else(|| current.as_ref().and_then(|v| v.session.group_id)),
            cwd: current
                .map(|v| v.session.cwd)
                .or_else(|| home_dir().ok())
                .unwrap_or_else(|| "/".into()),
            claude_available: self.0.manager.borrow().claude_executable().is_some(),
            projects: self.0.snapshot.borrow().projects.clone(),
            project_id,
            model: self.0.settings.borrow().default_model.clone(),
            extra_args: self.0.settings.borrow().default_args.clone(),
            shell: false,
        };
        let weak = self.weak();
        let dialog = dialogs::new_session(self.0.window.upcast_ref(), defaults, move |new: NewSession| {
            let Some(inner) = weak.upgrade() else {
                return Ok(());
            };
            let this = MainWindow(inner);
            let created = this.0.manager.borrow().create_session(new);
            let session = created.map_err(|e| e.to_string())?;
            this.reload(false);
            this.open_session(&session.id);
            Ok(())
        });
        self.refocus_after(&dialog);
    }

    /// Ofrece las conversaciones de Claude Code guardadas en la carpeta de un
    /// proyecto para reanudarlas como sesiones suyas.
    fn import_conversations(&self, project_id: i64) {
        let Some(project) = self.0.snapshot.borrow().projects.iter().find(|p| p.id == project_id).cloned()
        else {
            return;
        };
        let Some(config) = limits::claude_config_dir() else {
            return;
        };
        let here: HashSet<String> = self
            .0
            .snapshot
            .borrow()
            .sessions
            .iter()
            .filter_map(|v| v.agent.as_ref().and_then(|a| a.conversation.clone()))
            .collect();
        let weak = self.weak();
        glib::spawn_future_local(async move {
            let path = project.path.clone();
            let read = gio::spawn_blocking(move || {
                (conversations::list(&config, &path), conversations::open_now(&config))
            })
            .await;
            let (Some(inner), Ok((found, open))) = (weak.upgrade(), read) else {
                return;
            };
            let this = MainWindow(inner);
            let found = found
                .into_iter()
                .map(|c| {
                    let blocked = if here.contains(&c.id) {
                        Some(dialogs::Blocked::AlreadyHere)
                    } else if open.contains_key(&c.id) {
                        Some(dialogs::Blocked::OpenElsewhere)
                    } else {
                        None
                    };
                    (c, blocked)
                })
                .collect();
            let on_import = {
                let project = project.clone();
                this.handler(move |this, chosen: Vec<(String, String)>| {
                    this.add_conversations(&project, chosen);
                })
            };
            let dialog = dialogs::import_conversations(
                &this.0.window,
                &projects::label(&project),
                found,
                alarviterm_core::now_unix(),
                on_import,
            );
            this.refocus_after(&dialog);
        });
    }

    fn add_conversations(&self, project: &alarviterm_core::model::Project, chosen: Vec<(String, String)>) {
        let extra_args = self.0.settings.borrow().default_args.clone();
        let model = Some(self.0.settings.borrow().default_model.clone());
        // Entre la lista y la pulsación puede haberse abierto en otro sitio.
        let open = limits::claude_config_dir().map(|c| conversations::open_now(&c)).unwrap_or_default();
        let mut first = None;
        for (conversation, name) in chosen {
            if open.contains_key(&conversation) {
                self.toast(&format!("{name} está abierta en otro terminal, no se añade"));
                continue;
            }
            let new = NewSession {
                name,
                group_id: None,
                kind: SessionKind::Claude,
                cwd: project.path.clone(),
                model: model.clone(),
                extra_args: extra_args.clone(),
                env: Vec::new(),
                project_id: Some(project.id),
                resume: Some(conversation),
            };
            if let Some(session) = self.run(|m| m.create_session(new)) {
                first.get_or_insert(session.id);
            }
        }
        self.reload(false);
        if let Some(id) = first {
            self.open_session(&id);
        }
    }

    fn new_group_dialog(&self) {
        let on_accept = self.handler(|this, name: String| {
            if this.run(|m| m.db().create_group(&name)).is_some() {
                this.reload(false);
            }
        });
        let dialog = dialogs::prompt(&self.0.window, "Nuevo grupo", "", "Crear", on_accept);
        self.refocus_after(&dialog);
    }

    fn rename_group(&self, id: i64) {
        let current = self
            .0
            .snapshot
            .borrow()
            .groups
            .iter()
            .find(|g| g.id == id)
            .map(|g| g.name.clone())
            .unwrap_or_default();
        let on_accept = self.handler(move |this, name: String| {
            if this.run(|m| m.db().rename_group(id, &name)).is_some() {
                this.reload(false);
            }
        });
        let dialog = dialogs::prompt(&self.0.window, "Renombrar grupo", &current, "Renombrar", on_accept);
        self.refocus_after(&dialog);
    }

    fn delete_group(&self, id: i64) {
        let on_accept = self.handler(move |this, ()| {
            if this.run(|m| m.db().delete_group(id)).is_some() {
                this.reload(false);
            }
        });
        let dialog = dialogs::confirm(
            &self.0.window,
            "Eliminar el grupo",
            "Las sesiones del grupo no se cierran ni se borran, pasan a «Sin grupo».",
            "Eliminar grupo",
            true,
            move || on_accept(()),
        );
        self.refocus_after(&dialog);
    }

    fn rename_session(&self, id: &str) {
        let Some(view) = self.view(id) else { return };
        let id = id.to_string();
        let on_accept = self.handler(move |this, name: String| {
            if this.run(|m| m.rename(&id, &name)).is_some() {
                this.reload(false);
            }
        });
        let dialog = dialogs::prompt(
            &self.0.window,
            "Renombrar sesión",
            &view.session.name,
            "Renombrar",
            on_accept,
        );
        self.refocus_after(&dialog);
    }

    fn restart_session(&self, id: &str, resume: Resume) {
        let Some(view) = self.view(id) else { return };
        let id = id.to_string();
        let restart = self.handler(move |this, ()| {
            // Se retira el terminal antes para que su cierre no se confunda
            // con una desconexión.
            this.drop_terminal(&id);
            this.0.detached.borrow_mut().remove(&id);
            this.run(|m| m.relaunch(&id, resume.clone()));
            this.reload(false);
            this.open_session(&id);
        });
        if view.liveness.is_alive() {
            let dialog = dialogs::confirm(
                &self.0.window,
                &format!("Reiniciar «{}»", view.session.name),
                "El proceso actual se terminará y se lanzará de nuevo.",
                "Reiniciar",
                true,
                move || restart(()),
            );
            self.refocus_after(&dialog);
        } else {
            restart(());
        }
    }

    /// Sesiones sin archivar de un grupo o de un proyecto, y su nombre.
    fn batch(&self, which: Batch) -> (String, Vec<SessionView>) {
        let snapshot = self.0.snapshot.borrow();
        let name = match which {
            Batch::Group(id) => snapshot.groups.iter().find(|g| g.id == id).map(|g| g.name.clone()),
            Batch::Project(id) => snapshot.projects.iter().find(|p| p.id == id).map(|p| p.name.clone()),
        };
        let sessions = snapshot
            .sessions
            .iter()
            .filter(|v| !v.session.archived)
            .filter(|v| match which {
                Batch::Group(id) => v.session.group_id == Some(id),
                Batch::Project(id) => v.session.project_id == Some(id),
            })
            .cloned()
            .collect();
        (name.unwrap_or_default(), sessions)
    }

    /// Arranca las sesiones paradas de un grupo o de un proyecto. Las de
    /// Claude Code retoman cada una su conversación.
    fn start_sessions(&self, which: Batch) {
        let (name, sessions) = self.batch(which);
        let stopped: Vec<SessionView> = sessions.into_iter().filter(|v| !v.liveness.is_alive()).collect();
        if stopped.is_empty() {
            self.toast(&format!("{name} no tiene sesiones paradas"));
            return;
        }
        let mut started = 0;
        for view in &stopped {
            let id = &view.session.id;
            self.drop_terminal(id);
            self.0.detached.borrow_mut().remove(id);
            if self.run(|m| m.relaunch(id, Resume::ContinueLatest)).is_some() {
                started += 1;
            }
        }
        self.reload(false);
        let plural = if started == 1 { "sesión encendida" } else { "sesiones encendidas" };
        self.0.toasts.add_toast(adw::Toast::new(&format!("{name}: {started} {plural}")));
    }

    /// Detiene las sesiones en marcha de un grupo o de un proyecto. Siguen en
    /// la lista y se arrancan después con su conversación.
    fn stop_sessions(&self, which: Batch) {
        let (name, sessions) = self.batch(which);
        let running: Vec<String> = sessions
            .into_iter()
            .filter(|v| v.liveness.is_alive())
            .map(|v| v.session.id)
            .collect();
        if running.is_empty() {
            self.toast(&format!("{name} no tiene sesiones en marcha"));
            return;
        }
        let count = running.len();
        let stop = self.handler(move |this, ()| {
            for id in &running {
                this.drop_terminal(id);
                this.run(|m| m.stop(id));
            }
            this.reload(false);
        });
        let body = format!(
            "Se terminarán {count} procesos y lo que esté a medias en ellos se interrumpe. Las sesiones siguen \
             en la lista y al arrancarlas retoman su conversación."
        );
        let dialog = dialogs::confirm(
            &self.0.window,
            &format!("Apagar las sesiones de {name}"),
            &body,
            "Apagar todas",
            true,
            move || stop(()),
        );
        self.refocus_after(&dialog);
    }

    fn stop_session(&self, id: &str) {
        let Some(view) = self.view(id) else { return };
        if view.liveness == Liveness::Missing {
            return;
        }
        let id = id.to_string();
        let stop = self.handler(move |this, ()| {
            this.drop_terminal(&id);
            this.run(|m| m.stop(&id));
            this.reload(false);
        });
        if view.liveness.is_alive() {
            let dialog = dialogs::confirm(
                &self.0.window,
                &format!("Detener «{}»", view.session.name),
                "El proceso se terminará. La sesión seguirá en la lista y se podrá relanzar.",
                "Detener",
                true,
                move || stop(()),
            );
            self.refocus_after(&dialog);
        } else {
            stop(());
        }
    }

    fn delete_session(&self, id: &str) {
        let Some(view) = self.view(id) else { return };
        let id = id.to_string();
        let delete = self.handler(move |this, ()| {
            this.drop_terminal(&id);
            this.run(|m| m.delete(&id));
            this.reload(true);
        });
        let body = if view.liveness.is_alive() {
            "El proceso se terminará y la sesión desaparecerá de la lista."
        } else {
            "La sesión desaparecerá de la lista."
        };
        let dialog = dialogs::confirm(
            &self.0.window,
            &format!("Eliminar «{}»", view.session.name),
            body,
            "Eliminar",
            true,
            move || delete(()),
        );
        self.refocus_after(&dialog);
    }
}

/// Ventana mínima para explicar por qué la aplicación no puede arrancar.
/// Parámetro de `win.plugin-action`. El complemento va al final porque su
/// identificador es un nombre de carpeta y puede llevar cualquier carácter.
fn plugin_target(action: &str, session: &str, plugin: &str) -> String {
    format!("{action}|{session}|{plugin}")
}

fn tmux_options(settings: &Settings, state_dir: &std::path::Path) -> TmuxOptions {
    TmuxOptions {
        history_limit: settings.history_limit,
        notify_file: Some(state_dir.join(monitor::BACKEND_MARK)),
        clipboard_file: Some(state_dir.join(monitor::CLIPBOARD_MARK)),
    }
}

/// Ruta del ejecutable de Claude Code fijada en los ajustes, si la hay.
fn claude_override(settings: &Settings) -> Option<std::path::PathBuf> {
    Some(settings.claude_path.trim())
        .filter(|p| !p.is_empty())
        .map(std::path::PathBuf::from)
}

pub fn present_startup_error(app: &adw::Application, message: &str) {
    let page = adw::StatusPage::builder()
        .title("ALarviTerm no puede iniciarse")
        .description(message)
        .build();
    let layout = adw::ToolbarView::new();
    layout.add_top_bar(&adw::HeaderBar::new());
    layout.set_content(Some(&page));
    adw::ApplicationWindow::builder()
        .application(app)
        .title("ALarviTerm")
        .default_width(560)
        .default_height(360)
        .content(&layout)
        .build()
        .present();
}
