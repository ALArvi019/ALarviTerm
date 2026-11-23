//! Barra inferior. Solo muestra datos comprobados: sesión activa, estado,
//! directorio, rama de git, modelo, consumo del equipo, límites de la cuenta,
//! recuento y versiones detectadas. El consumo y los límites se amplían en un
//! panel al pasar el ratón.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::time::Instant;

use gtk::glib;
use gtk::prelude::*;
use alarviterm_core::limits::{self, RateLimits, SessionContext, Window};
use alarviterm_core::paths::display_path;
use alarviterm_core::sessions::SessionView;
use alarviterm_core::sysinfo::{compact_bytes, GitSummary, SelfUsage, SystemSample, TopProcesses};

use super::status;

pub struct Footer {
    pub root: gtk::Box,
    dot: gtk::Box,
    session: gtk::Label,
    state: gtk::Label,
    /// Modo de permisos de Claude Code en la sesión a la vista.
    mode: gtk::Label,
    cwd: gtk::Label,
    git: gtk::Label,
    model: gtk::Label,
    usage: gtk::Box,
    cpu: gtk::Label,
    cpu_chart: gtk::DrawingArea,
    /// Últimas muestras de CPU de la sesión activa, para la mini gráfica.
    cpu_history: Rc<RefCell<VecDeque<f64>>>,
    memory: gtk::Label,
    /// Consumo de la propia aplicación.
    app: gtk::Box,
    app_cpu: gtk::Label,
    app_chart: gtk::DrawingArea,
    app_history: Rc<RefCell<VecDeque<f64>>>,
    app_memory: gtk::Label,
    /// CPU, memoria y red del equipo, con su panel ampliado.
    system: SystemMeter,
    limits: gtk::MenuButton,
    context: Meter,
    five_hour: Meter,
    seven_day: Meter,
    /// Si hay contexto y si hay límites que enseñar: el botón que los agrupa
    /// se ve con cualquiera de los dos.
    shown: Cell<(bool, bool)>,
    limits_age: gtk::Label,
    limits_heading: gtk::Label,
    counts: gtk::Label,
    backend: gtk::Label,
    claude: gtk::Label,
    /// Qué datos opcionales quiere ver el usuario: git, modelo y consumo.
    parts: Cell<(bool, bool, bool)>,
}

impl Footer {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        root.add_css_class("mt-footer");

        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("mt-dot");
        dot.set_valign(gtk::Align::Center);
        let session = label(true);
        session.add_css_class("mt-footer-strong");
        let state = label(false);
        let mode = label(false);
        mode.add_css_class("mt-footer-mode");
        mode.set_visible(false);
        let cwd = label(true);
        cwd.set_hexpand(true);
        cwd.set_halign(gtk::Align::Start);
        cwd.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        let git = label(false);
        // Un clic en la rama abre el control de código fuente de la sesión.
        git.add_css_class("mt-footer-link");
        let open_scm = gtk::GestureClick::new();
        open_scm.connect_released(|gesture, _, _, _| {
            if let Some(widget) = gesture.widget() {
                let _ = widget.activate_action("win.scm", None);
            }
        });
        git.add_controller(open_scm);
        let model = label(false);
        for extra in [&git, &model] {
            extra.set_visible(false);
        }

        // Consumo de la sesión activa: gráfica de CPU, su valor y la memoria.
        let cpu_history = Rc::new(RefCell::new(VecDeque::with_capacity(SAMPLES)));
        let cpu_chart = line_chart(cpu_history.clone(), 110, 22, 100.0);
        let cpu = value_label();
        let memory = value_label();
        let usage = panel_section("Sesión activa", &cpu_chart, &cpu, &memory);

        // Consumo de la propia aplicación, con sus clientes de terminal.
        let app_history = Rc::new(RefCell::new(VecDeque::with_capacity(SAMPLES)));
        let app_chart = line_chart(app_history.clone(), 110, 22, 100.0);
        let app_cpu = value_label();
        let app_memory = value_label();
        let app = panel_section("ALarviTerm", &app_chart, &app_cpu, &app_memory);
        let system = SystemMeter::new(&usage, &app);

        // Límites de la cuenta: dos medidores y un panel de detalle al pulsar.
        let context = Meter::new("CTX", "Contexto de la sesión");
        let context_heading = gtk::Label::new(Some("Sesión activa"));
        context_heading.add_css_class("mt-stat-heading");
        context_heading.set_xalign(0.0);
        let context_block = gtk::Box::new(gtk::Orientation::Vertical, 12);
        context_block.append(&context_heading);
        context_block.append(&context.detail);
        context.detail.bind_property("visible", &context_block, "visible").sync_create().build();
        let five_hour = Meter::new("5H", "Ventana de 5 horas");
        let seven_day = Meter::new("7D", "Ventana de 7 días");
        let meters = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        meters.append(&context.compact);
        meters.append(&five_hour.compact);
        meters.append(&seven_day.compact);
        let limits_age = gtk::Label::new(None);
        limits_age.add_css_class("mt-stat-note");
        limits_age.set_xalign(0.0);
        limits_age.set_wrap(true);
        let heading = gtk::Label::new(Some("Límites de la cuenta"));
        heading.add_css_class("mt-stat-heading");
        heading.set_xalign(0.0);
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 12);
        panel.add_css_class("mt-stat-panel");
        panel.append(&context_block);
        panel.append(&heading);
        panel.append(&five_hour.detail);
        panel.append(&seven_day.detail);
        panel.append(&limits_age);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&panel));
        popover.set_autohide(false);
        let limits = gtk::MenuButton::new();
        limits.add_css_class("flat");
        limits.add_css_class("mt-stat-button");
        limits.set_child(Some(&meters));
        limits.set_popover(Some(&popover));
        limits.set_direction(gtk::ArrowType::Up);
        limits.set_visible(false);
        open_on_hover(&limits);
        let counts = label(false);
        let backend = label(false);
        let claude = label(false);

        for widget in [
            dot.upcast_ref::<gtk::Widget>(),
            session.upcast_ref(),
            state.upcast_ref(),
            mode.upcast_ref(),
            cwd.upcast_ref(),
            git.upcast_ref(),
            model.upcast_ref(),
            system.root.upcast_ref(),
            limits.upcast_ref(),
            counts.upcast_ref(),
            backend.upcast_ref(),
            claude.upcast_ref(),
        ] {
            root.append(widget);
        }

        Self {
            root,
            dot,
            session,
            state,
            mode,
            cwd,
            git,
            model,
            usage,
            cpu,
            cpu_chart,
            cpu_history,
            memory,
            app,
            app_cpu,
            app_chart,
            app_history,
            app_memory,
            system,
            limits,
            context,
            five_hour,
            seven_day,
            shown: Cell::new((false, false)),
            limits_age,
            limits_heading: heading,
            counts,
            backend,
            claude,
            parts: Cell::new((true, true, true)),
        }
    }

    /// Elige qué datos opcionales se muestran. Los que se apagan se ocultan
    /// ya, los que se encienden aparecen con el siguiente refresco.
    pub fn set_parts(&self, git: bool, model: bool, usage: bool) {
        self.parts.set((git, model, usage));
        if !git {
            self.git.set_visible(false);
        }
        if !model {
            self.model.set_visible(false);
        }
        if !usage {
            self.usage.set_visible(false);
            self.app.set_visible(false);
            self.system.root.set_visible(false);
        }
    }

    /// Rama y cambios sin confirmar del directorio de la sesión.
    pub fn set_git(&self, summary: Option<&GitSummary>) {
        let Some(summary) = summary.filter(|_| self.parts.get().0) else {
            self.git.set_visible(false);
            return;
        };
        let mut text = summary.branch.clone();
        if summary.changes > 0 {
            text.push_str(&format!(" ±{}", summary.changes));
        }
        if summary.ahead > 0 {
            text.push_str(&format!(" ↑{}", summary.ahead));
        }
        if summary.behind > 0 {
            text.push_str(&format!(" ↓{}", summary.behind));
        }
        let changes = match summary.changes {
            0 => "sin cambios pendientes".to_string(),
            1 => "1 fichero con cambios".to_string(),
            n => format!("{n} ficheros con cambios"),
        };
        self.git.set_text(&text);
        self.git.set_tooltip_text(Some(&format!(
            "Rama {}, {changes}. Pulsa para abrir el control de código fuente",
            summary.branch
        )));
        self.git.set_visible(true);
    }

    /// Modelo que respondió en el último turno, según la transcripción.
    pub fn set_model(&self, model: Option<&str>) {
        match model.filter(|_| self.parts.get().1) {
            Some(model) => {
                self.model.set_text(model);
                self.model
                    .set_tooltip_text(Some("Modelo que respondió en el último turno terminado"));
                self.model.set_visible(true);
            }
            None => self.model.set_visible(false),
        }
    }

    /// Consumo del árbol de procesos de la sesión. `cpu` es el porcentaje de
    /// un núcleo desde la muestra anterior, si la hay.
    pub fn set_usage(&self, usage: Option<(u64, Option<f64>, usize)>) {
        let Some((rss_bytes, cpu, processes)) = usage.filter(|_| self.parts.get().2) else {
            // Sin sesión, o con otra: la gráfica empieza de cero.
            self.cpu_history.borrow_mut().clear();
            self.usage.set_visible(false);
            return;
        };
        let megabytes = rss_bytes as f64 / (1024.0 * 1024.0);
        self.memory.set_text(&if megabytes >= 1024.0 {
            format!("{:.1} GB", megabytes / 1024.0)
        } else {
            format!("{megabytes:.0} MB")
        });
        match cpu {
            Some(cpu) => {
                push_sample(&self.cpu_history, cpu);
                self.cpu.set_text(&format!("{cpu:.0}%"));
            }
            None => self.cpu.set_text("…"),
        }
        self.cpu_chart.queue_draw();
        let plural = if processes == 1 { "proceso" } else { "procesos" };
        self.usage.set_tooltip_text(Some(&format!(
            "CPU, en porcentaje de un núcleo, y memoria residente de la sesión ({processes} {plural}). \
             La gráfica recoge las últimas muestras."
        )));
        self.usage.set_visible(true);
    }

    /// CPU, memoria y red del equipo. `None` oculta el medidor.
    pub fn set_system(&self, sample: Option<SystemSample>) {
        match sample.filter(|_| self.parts.get().2) {
            Some(sample) => {
                self.system.push(sample);
                self.system.root.set_visible(true);
            }
            None => self.system.root.set_visible(false),
        }
    }

    /// Si el panel del equipo está a la vista. Solo entonces merece la pena
    /// recorrer todos los procesos.
    pub fn system_panel_open(&self) -> bool {
        self.system.root.popover().is_some_and(|p| p.is_visible())
    }

    /// Programas que más CPU y memoria consumen, para el panel del equipo.
    pub fn set_processes(&self, top: &TopProcesses) {
        self.system.set_processes(top);
    }

    /// Consumo de la propia aplicación. `cpu` es el porcentaje de un núcleo
    /// desde la muestra anterior, si la hay.
    pub fn set_app_usage(&self, usage: Option<(SelfUsage, Option<f64>)>) {
        let Some((usage, cpu)) = usage.filter(|_| self.parts.get().2) else {
            self.app.set_visible(false);
            return;
        };
        let megabytes = |bytes: u64| bytes as f64 / (1024.0 * 1024.0);
        self.app_memory.set_text(&format!("{:.0} MB", megabytes(usage.pss_bytes)));
        match cpu {
            Some(cpu) => {
                push_sample(&self.app_history, cpu);
                self.app_cpu.set_text(&format!("{cpu:.1}%"));
            }
            None => self.app_cpu.set_text("…"),
        }
        self.app_chart.queue_draw();
        let clients = usage.processes.saturating_sub(1);
        let plural = if clients == 1 { "cliente de terminal" } else { "clientes de terminal" };
        self.app.set_tooltip_text(Some(&format!(
            "Consumo de ALarviTerm, sin contar las sesiones.\n\
             CPU en porcentaje de un núcleo, con sus {clients} {plural}.\n\
             Memoria proporcional del proceso: {:.0} MB. Es la suya más su parte de las bibliotecas compartidas.\n\
             Memoria residente con sus clientes: {:.0} MB. Cuenta enteras las bibliotecas que comparte con otros programas.",
            megabytes(usage.pss_bytes),
            megabytes(usage.rss_bytes),
        )));
        self.app.set_visible(true);
    }

    /// Lo que queda de cada ventana de uso de la cuenta y cuánto falta para
    /// que se reinicie. `None` oculta el dato.
    /// `account` es el nombre de la cuenta a la que pertenecen los límites, o
    /// vacío para la predeterminada.
    pub fn set_limits(&self, limits: Option<&RateLimits>, account: &str, now: i64) {
        let of_account = if account.is_empty() { String::new() } else { format!(" {account}") };
        self.limits_heading.set_text(&format!("Límites de la cuenta{of_account}"));
        let Some(limits) = limits.map(|l| l.live(now)).filter(|l| !l.is_empty()) else {
            for part in [&self.five_hour, &self.seven_day] {
                part.set(None, now);
            }
            self.limits_heading.set_visible(false);
            self.limits_age.set_visible(false);
            self.shown.set((self.shown.get().0, false));
            self.limits.set_visible(self.shown.get().0);
            return;
        };
        self.limits_heading.set_visible(true);
        self.limits_age.set_visible(true);
        self.shown.set((self.shown.get().0, true));
        self.five_hour.set(limits.five_hour.as_ref(), now);
        self.seven_day.set(limits.seven_day.as_ref(), now);
        self.limits_age.set_text(&format!(
            "Dato de Claude Code recibido hace {}. Se actualiza cuando una sesión responde.",
            limits::long_duration((now - limits.updated).max(0))
        ));
        self.limits.set_visible(true);
    }

    /// Cuánto de su ventana de contexto lleva usado la sesión a la vista.
    /// La barra es lo usado, como en los límites.
    pub fn set_context(&self, context: Option<&SessionContext>) {
        self.shown.set((context.is_some(), self.shown.get().1));
        match context {
            Some(context) => {
                let remaining = context.remaining_percent();
                self.context.show(
                    remaining,
                    "",
                    &format!(
                        "Queda el {} %. La conversación lleva {} tokens de entrada y {} de salida.",
                        remaining,
                        alarviterm_core::stats::compact(context.input_tokens),
                        alarviterm_core::stats::compact(context.output_tokens),
                    ),
                );
            }
            None => self.context.set(None, 0),
        }
        let (context, limits) = self.shown.get();
        self.limits.set_visible(context || limits);
    }

    pub fn set_session(&self, view: Option<&SessionView>) {
        for class in status::STATUS_CLASSES {
            self.dot.remove_css_class(class);
        }
        match view {
            Some(view) => {
                let info = status::describe(view);
                self.dot.add_css_class(info.class);
                self.dot.set_visible(true);
                self.session.set_text(&view.session.name);
                self.state.set_text(&info.label);
                self.state.set_tooltip_text(Some(&info.tooltip));
                self.cwd.set_text(&display_path(&view.session.cwd));
                let mode = view
                    .agent
                    .as_ref()
                    .filter(|_| view.liveness.is_alive())
                    .and_then(|agent| agent.permission_mode.as_deref())
                    .map(permission_mode);
                match mode {
                    Some((text, open)) => {
                        self.mode.set_text(text);
                        if open {
                            self.mode.add_css_class("mt-footer-warn");
                        } else {
                            self.mode.remove_css_class("mt-footer-warn");
                        }
                        self.mode.set_tooltip_text(Some(
                            "Modo de permisos de Claude Code en esta sesión, según su último evento. \
                             Si lo cambias con Mayús+Tab, aquí se actualiza con la siguiente acción de la sesión.",
                        ));
                        self.mode.set_visible(true);
                    }
                    None => self.mode.set_visible(false),
                }
            }
            None => {
                self.dot.set_visible(false);
                self.session.set_text("");
                self.state.set_text("Sin sesión seleccionada");
                self.state.set_tooltip_text(None);
                self.mode.set_visible(false);
                self.cwd.set_text("");
            }
        }
    }

    /// `asking` son las sesiones detenidas a la espera de un permiso.
    pub fn set_counts(&self, total: usize, alive: usize, asking: usize) {
        let active = if alive == 1 { "activa" } else { "activas" };
        let mut text = match total {
            0 => "sin sesiones".to_string(),
            1 => format!("1 sesión, {alive} {active}"),
            _ => format!("{total} sesiones, {alive} {active}"),
        };
        match asking {
            0 => self.counts.remove_css_class("mt-footer-warn"),
            n => {
                let verb = if n == 1 { "pide" } else { "piden" };
                text.push_str(&format!(", {n} {verb} permiso"));
                self.counts.add_css_class("mt-footer-warn");
            }
        }
        self.counts.set_text(&text);
    }

    /// `problem` marca en ámbar un backend que no responde.
    pub fn set_backend(&self, text: &str, problem: bool) {
        self.backend.set_text(text);
        if problem {
            self.backend.add_css_class("mt-footer-warn");
        } else {
            self.backend.remove_css_class("mt-footer-warn");
        }
    }

    pub fn set_claude(&self, text: &str, tooltip: Option<&str>, problem: bool) {
        self.claude.set_text(text);
        self.claude.set_tooltip_text(tooltip);
        if problem {
            self.claude.add_css_class("mt-footer-warn");
        } else {
            self.claude.remove_css_class("mt-footer-warn");
        }
    }
}

/// Nombre corto de un modo de permisos de Claude Code, y si deja actuar sin
/// preguntar, que merece verse resaltado.
fn permission_mode(mode: &str) -> (&str, bool) {
    match mode {
        "default" => ("pregunta", false),
        "acceptEdits" => ("acepta ediciones", false),
        "plan" => ("plan", false),
        "auto" => ("auto", false),
        "dontAsk" => ("no pregunta", true),
        "bypassPermissions" => ("sin permisos", true),
        other => (other, false),
    }
}

/// Muestras que guarda cada gráfica.
const SAMPLES: usize = 60;

/// Clases de color de una barra según lo que queda.
const LEVELS: [&str; 3] = ["l-ok", "l-warn", "l-crit"];

fn level_class(remaining: u32) -> &'static str {
    match remaining {
        0..=19 => LEVELS[2],
        20..=49 => LEVELS[1],
        _ => LEVELS[0],
    }
}

/// Medidor de una ventana de uso: su versión compacta del pie y su fila del
/// panel de detalle. La barra representa lo usado.
struct Meter {
    compact: gtk::Box,
    value: gtk::Label,
    left: gtk::Label,
    bar: gtk::ProgressBar,
    detail: gtk::Box,
    detail_value: gtk::Label,
    detail_bar: gtk::ProgressBar,
    detail_text: gtk::Label,
}

impl Meter {
    fn new(short: &str, long: &str) -> Self {
        let bar = gtk::ProgressBar::new();
        bar.add_css_class("mt-meter");
        bar.set_valign(gtk::Align::Center);
        let value = value_label();
        let left = gtk::Label::new(None);
        left.add_css_class("mt-stat-note");
        let compact = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        compact.add_css_class("mt-stat");
        compact.append(&caption(short));
        compact.append(&bar);
        compact.append(&value);
        compact.append(&left);

        let title = gtk::Label::new(Some(long));
        title.set_xalign(0.0);
        title.set_hexpand(true);
        let detail_value = gtk::Label::new(None);
        detail_value.add_css_class("mt-stat-big");
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        top.append(&title);
        top.append(&detail_value);
        let detail_bar = gtk::ProgressBar::new();
        detail_bar.add_css_class("mt-meter");
        detail_bar.add_css_class("mt-meter-big");
        let detail_text = gtk::Label::new(None);
        detail_text.add_css_class("mt-stat-note");
        detail_text.set_xalign(0.0);
        let detail = gtk::Box::new(gtk::Orientation::Vertical, 5);
        detail.append(&top);
        detail.append(&detail_bar);
        detail.append(&detail_text);

        Self { compact, value, left, bar, detail, detail_value, detail_bar, detail_text }
    }

    fn set(&self, window: Option<&Window>, now: i64) {
        let Some(window) = window else {
            self.compact.set_visible(false);
            self.detail.set_visible(false);
            return;
        };
        let seconds = window.seconds_left(now);
        self.show(
            window.remaining_percent(),
            &limits::short_duration(seconds).replace(' ', ""),
            &format!(
                "Queda el {} %. Se reinicia en {}, {}.",
                window.remaining_percent(),
                limits::long_duration(seconds),
                reset_clock(window, now),
            ),
        );
    }

    /// Pinta el medidor con lo usado, una nota corta y el texto del panel. La
    /// barra se llena según se gasta y cambia de color cuando queda poco.
    fn show(&self, remaining: u32, note: &str, detail: &str) {
        let used = 100 - remaining.min(100);
        let fraction = f64::from(used) / 100.0;
        for bar in [&self.bar, &self.detail_bar] {
            bar.set_fraction(fraction);
            for class in LEVELS {
                bar.remove_css_class(class);
            }
            bar.add_css_class(level_class(remaining));
        }
        self.value.set_text(&format!("{used}%"));
        self.left.set_text(note);
        self.left.set_visible(!note.is_empty());
        self.detail_value.set_text(&format!("{used} % usado"));
        self.detail_text.set_text(detail);
        self.compact.set_visible(true);
        self.detail.set_visible(true);
    }
}

/// Etiqueta pequeña que nombra un dato, al estilo de un panel de medidores.
fn caption(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("mt-stat-caption");
    label
}

fn value_label() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("mt-stat-value");
    label
}

/// Gráfica de línea con relleno. La escala llega al menos a `floor` y crece
/// si alguna muestra lo supera.
fn line_chart(history: Rc<RefCell<VecDeque<f64>>>, width: i32, height: i32, floor: f64) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.add_css_class("mt-spark");
    area.set_content_width(width);
    area.set_content_height(height);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(move |area, cr, width, height| {
        let (width, height) = (f64::from(width), f64::from(height));
        let color = area.color();
        let rgb = (f64::from(color.red()), f64::from(color.green()), f64::from(color.blue()));
        // Línea de base, para que una gráfica sin datos no parezca un hueco.
        cr.set_source_rgba(rgb.0, rgb.1, rgb.2, 0.25);
        cr.rectangle(0.0, height - 1.0, width, 1.0);
        let _ = cr.fill();
        let history = history.borrow();
        let top = history.iter().copied().fold(floor, f64::max);
        draw_series(cr, &history, rgb, top, (0.0, width), height - 1.0, -(height - 2.0));
    });
    area
}

/// Dibuja una serie como línea con relleno. `base` es la altura del cero y
/// `reach` cuánto se aleja de ella el valor `top`: negativo sube, positivo baja.
fn draw_series(
    cr: &gtk::cairo::Context,
    history: &VecDeque<f64>,
    (r, g, b): (f64, f64, f64),
    top: f64,
    (left, width): (f64, f64),
    base: f64,
    reach: f64,
) {
    if history.len() < 2 || top <= 0.0 {
        return;
    }
    let step = width / (SAMPLES - 1) as f64;
    let start = left + width - step * (history.len() - 1) as f64;
    for (i, value) in history.iter().enumerate() {
        let (x, y) = (start + step * i as f64, base + (value / top).clamp(0.0, 1.0) * reach);
        if i == 0 {
            cr.move_to(x, y);
        } else {
            cr.line_to(x, y);
        }
    }
    cr.set_source_rgba(r, g, b, 1.0);
    cr.set_line_width(1.2);
    let _ = cr.stroke_preserve();
    cr.line_to(left + width, base);
    cr.line_to(start, base);
    cr.close_path();
    cr.set_source_rgba(r, g, b, 0.22);
    let _ = cr.fill();
}

fn push_sample(history: &RefCell<VecDeque<f64>>, value: f64) {
    let mut history = history.borrow_mut();
    if history.len() == SAMPLES {
        history.pop_front();
    }
    history.push_back(value);
}

fn rgb_of(widget: &impl IsA<gtk::Widget>) -> (f64, f64, f64) {
    let color = widget.color();
    (f64::from(color.red()), f64::from(color.green()), f64::from(color.blue()))
}

/// Abre el panel de un botón al pasar el ratón por encima y lo cierra al
/// salir. Un clic lo deja fijo hasta el siguiente clic. El panel no captura
/// el foco, así que el terminal sigue escribiendo.
fn open_on_hover(button: &gtk::MenuButton) {
    let pinned = Rc::new(Cell::new(false));
    let motion = gtk::EventControllerMotion::new();
    let target = button.clone();
    motion.connect_enter(move |_, _, _| target.popup());
    let (target, fixed) = (button.clone(), pinned.clone());
    motion.connect_leave(move |_| {
        if !fixed.get() {
            target.popdown();
        }
    });
    button.add_controller(motion);

    // Se atiende antes que el propio botón, que con un clic cerraría el panel
    // que el ratón acaba de abrir.
    let click = gtk::GestureClick::new();
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let target = button.clone();
    click.connect_pressed(move |gesture, _, _, _| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        pinned.set(!pinned.get());
        if pinned.get() {
            target.add_css_class("pinned");
            target.popup();
        } else {
            target.remove_css_class("pinned");
        }
    });
    button.add_controller(click);
}

/// Apartado del panel ampliado para un consumo con gráfica de CPU: el de la
/// sesión activa o el de la propia aplicación.
fn panel_section(title: &str, chart: &gtk::DrawingArea, cpu: &gtk::Label, memory: &gtk::Label) -> gtk::Box {
    let heading = gtk::Label::new(Some(title));
    heading.add_css_class("mt-stat-heading");
    heading.set_xalign(0.0);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.append(&caption("CPU"));
    row.append(chart);
    cpu.set_width_chars(5);
    cpu.set_xalign(1.0);
    row.append(cpu);
    let gap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    gap.set_hexpand(true);
    row.append(&gap);
    row.append(&caption("MEM"));
    row.append(memory);
    let section = gtk::Box::new(gtk::Orientation::Vertical, 6);
    section.append(&heading);
    section.append(&row);
    section.set_visible(false);
    section
}

/// Medidor del equipo: CPU, memoria y red en el pie, y un panel con las
/// gráficas grandes que se abre al pasar el ratón.
struct SystemMeter {
    root: gtk::MenuButton,
    previous: Cell<Option<(SystemSample, Instant)>>,
    /// Contadores de red de la primera muestra, para el total de la sesión.
    first: Cell<Option<SystemSample>>,
    cpu_history: Rc<RefCell<VecDeque<f64>>>,
    down_history: Rc<RefCell<VecDeque<f64>>>,
    up_history: Rc<RefCell<VecDeque<f64>>>,
    charts: [gtk::DrawingArea; 3],
    cpu: gtk::Label,
    memory: gtk::Label,
    memory_bar: gtk::ProgressBar,
    down: gtk::Label,
    up: gtk::Label,
    big_cpu: gtk::Label,
    cpu_note: gtk::Label,
    big_memory: gtk::Label,
    big_memory_bar: gtk::ProgressBar,
    memory_note: gtk::Label,
    big_down: gtk::Label,
    big_up: gtk::Label,
    net_note: gtk::Label,
    /// Filas de programas: por CPU y por memoria, cada una nombre y valor.
    top_cpu: Vec<(gtk::Label, gtk::Label)>,
    top_memory: Vec<(gtk::Label, gtk::Label)>,
}

/// Programas que se listan en cada clasificación del panel.
pub const TOP_PROCESSES: usize = 5;

impl SystemMeter {
    fn new(session: &gtk::Box, app: &gtk::Box) -> Self {
        let history = || Rc::new(RefCell::new(VecDeque::with_capacity(SAMPLES)));
        let (cpu_history, down_history, up_history) = (history(), history(), history());

        // Versión del pie.
        let cpu = value_label();
        cpu.set_width_chars(4);
        cpu.set_xalign(1.0);
        let memory = value_label();
        memory.set_width_chars(4);
        memory.set_xalign(1.0);
        let memory_bar = gtk::ProgressBar::new();
        memory_bar.add_css_class("mt-meter");
        memory_bar.add_css_class("mt-meter-short");
        memory_bar.set_valign(gtk::Align::Center);
        let rate = |class: &str| {
            let label = value_label();
            label.set_width_chars(8);
            label.set_xalign(1.0);
            let arrow = gtk::Label::new(Some(if class == "mt-net-down" { "↓" } else { "↑" }));
            arrow.add_css_class(class);
            (arrow, label)
        };
        let (down_arrow, down) = rate("mt-net-down");
        let (up_arrow, up) = rate("mt-net-up");
        let small_cpu = line_chart(cpu_history.clone(), 40, 13, 100.0);
        let compact = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        compact.add_css_class("mt-stat");
        compact.append(&caption("CPU"));
        compact.append(&small_cpu);
        compact.append(&cpu);
        compact.append(&caption("MEM"));
        compact.append(&memory_bar);
        compact.append(&memory);
        compact.append(&caption("RED"));
        compact.append(&down_arrow);
        compact.append(&down);
        compact.append(&up_arrow);
        compact.append(&up);

        // Panel ampliado.
        let title_row = |title: &str, values: &[&gtk::Widget]| {
            let name = gtk::Label::new(Some(title));
            name.set_xalign(0.0);
            name.set_hexpand(true);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.append(&name);
            for value in values {
                row.append(*value);
            }
            row
        };
        let note = || {
            let label = gtk::Label::new(None);
            label.add_css_class("mt-stat-note");
            label.set_xalign(0.0);
            label
        };
        let big = || {
            let label = gtk::Label::new(None);
            label.add_css_class("mt-stat-big");
            label
        };
        let (big_cpu, big_memory, big_down, big_up) = (big(), big(), big(), big());
        let (cpu_note, memory_note, net_note) = (note(), note(), note());
        let big_cpu_chart = line_chart(cpu_history.clone(), 300, 54, 100.0);
        big_cpu_chart.add_css_class("mt-chart-box");
        let big_memory_bar = gtk::ProgressBar::new();
        big_memory_bar.add_css_class("mt-meter");
        big_memory_bar.add_css_class("mt-meter-big");

        let big_down_arrow = gtk::Label::new(Some("↓"));
        big_down_arrow.add_css_class("mt-net-down");
        let big_up_arrow = gtk::Label::new(Some("↑"));
        big_up_arrow.add_css_class("mt-net-up");
        let net_chart = gtk::DrawingArea::new();
        net_chart.add_css_class("mt-chart-box");
        net_chart.set_content_width(300);
        net_chart.set_content_height(70);
        {
            let (down, up) = (down_history.clone(), up_history.clone());
            let (down_color, up_color) = (big_down_arrow.clone(), big_up_arrow.clone());
            net_chart.set_draw_func(move |area, cr, width, height| {
                let (width, height) = (f64::from(width), f64::from(height));
                let middle = (height / 2.0).floor();
                let axis = rgb_of(area);
                cr.set_source_rgba(axis.0, axis.1, axis.2, 0.25);
                cr.rectangle(0.0, middle, width, 1.0);
                let _ = cr.fill();
                // La bajada crece hacia arriba y la subida hacia abajo, cada
                // una con su escala, con un mínimo para que el ruido no la llene.
                let (down, up) = (down.borrow(), up.borrow());
                let top = |series: &VecDeque<f64>| series.iter().copied().fold(NET_FLOOR, f64::max);
                draw_series(cr, &down, rgb_of(&down_color), top(&down), (0.0, width), middle, -(middle - 2.0));
                draw_series(cr, &up, rgb_of(&up_color), top(&up), (0.0, width), middle + 1.0, height - middle - 3.0);
            });
        }

        let heading = gtk::Label::new(Some("Equipo"));
        heading.add_css_class("mt-stat-heading");
        heading.set_xalign(0.0);
        let block = |widgets: &[&gtk::Widget]| {
            let holder = gtk::Box::new(gtk::Orientation::Vertical, 5);
            for widget in widgets {
                holder.append(*widget);
            }
            holder
        };
        let panel = gtk::Box::new(gtk::Orientation::Vertical, 14);
        panel.add_css_class("mt-stat-panel");
        panel.append(&heading);
        panel.append(&block(&[
            title_row("CPU", &[big_cpu.upcast_ref()]).upcast_ref(),
            big_cpu_chart.upcast_ref(),
            cpu_note.upcast_ref(),
        ]));
        panel.append(&block(&[
            title_row("Memoria", &[big_memory.upcast_ref()]).upcast_ref(),
            big_memory_bar.upcast_ref(),
            memory_note.upcast_ref(),
        ]));
        panel.append(&block(&[
            title_row(
                "Red",
                &[big_down_arrow.upcast_ref(), big_down.upcast_ref(), big_up_arrow.upcast_ref(), big_up.upcast_ref()],
            )
            .upcast_ref(),
            net_chart.upcast_ref(),
            net_note.upcast_ref(),
        ]));
        // Programas que más consumen, en dos columnas.
        let ranking = |title: &str| {
            let column = gtk::Grid::new();
            column.set_column_spacing(8);
            column.set_row_spacing(3);
            column.set_hexpand(true);
            let head = caption(title);
            head.set_xalign(0.0);
            column.attach(&head, 0, 0, 2, 1);
            let rows: Vec<(gtk::Label, gtk::Label)> = (0..TOP_PROCESSES)
                .map(|index| {
                    let name = gtk::Label::new(None);
                    name.set_xalign(0.0);
                    name.set_hexpand(true);
                    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    name.set_max_width_chars(13);
                    let value = value_label();
                    value.set_xalign(1.0);
                    column.attach(&name, 0, index as i32 + 1, 1, 1);
                    column.attach(&value, 1, index as i32 + 1, 1, 1);
                    (name, value)
                })
                .collect();
            (column, rows)
        };
        let (cpu_column, top_cpu) = ranking("MÁS CPU");
        let (memory_column, top_memory) = ranking("MÁS MEMORIA");
        let rankings = gtk::Box::new(gtk::Orientation::Horizontal, 18);
        rankings.append(&cpu_column);
        rankings.append(&memory_column);
        let programs = gtk::Label::new(Some("Programas"));
        programs.add_css_class("mt-stat-heading");
        programs.set_xalign(0.0);
        panel.append(&block(&[programs.upcast_ref(), rankings.upcast_ref()]));
        panel.append(session);
        panel.append(app);

        let popover = gtk::Popover::new();
        popover.set_child(Some(&panel));
        popover.set_autohide(false);
        let root = gtk::MenuButton::new();
        root.add_css_class("flat");
        root.add_css_class("mt-stat-button");
        root.set_child(Some(&compact));
        root.set_popover(Some(&popover));
        root.set_direction(gtk::ArrowType::Up);
        root.set_visible(false);
        open_on_hover(&root);

        Self {
            root,
            previous: Cell::new(None),
            first: Cell::new(None),
            cpu_history,
            down_history,
            up_history,
            charts: [small_cpu, big_cpu_chart, net_chart],
            cpu,
            memory,
            memory_bar,
            down,
            up,
            big_cpu,
            cpu_note,
            big_memory,
            big_memory_bar,
            memory_note,
            big_down,
            big_up,
            net_note,
            top_cpu,
            top_memory,
        }
    }

    fn set_processes(&self, top: &TopProcesses) {
        let name = |row: &alarviterm_core::sysinfo::ProcessRow| match row.count {
            1 => row.name.clone(),
            count => format!("{} ×{count}", row.name),
        };
        for (index, (label, value)) in self.top_cpu.iter().enumerate() {
            match top.by_cpu.get(index) {
                Some(row) => {
                    label.set_text(&name(row));
                    value.set_text(&format!("{:.0}%", row.cpu_percent));
                }
                None => {
                    // Con la primera muestra aún no se sabe la CPU de nadie.
                    label.set_text(if index == 0 && top.by_cpu.is_empty() { "midiendo…" } else { "" });
                    value.set_text("");
                }
            }
        }
        for (index, (label, value)) in self.top_memory.iter().enumerate() {
            match top.by_memory.get(index) {
                Some(row) => {
                    label.set_text(&name(row));
                    value.set_text(&compact_bytes(row.rss_bytes as f64));
                }
                None => {
                    label.set_text("");
                    value.set_text("");
                }
            }
        }
    }

    fn push(&self, sample: SystemSample) {
        let now = Instant::now();
        let previous = self.previous.replace(Some((sample, now)));
        let first = self.first.get().unwrap_or(sample);
        self.first.set(Some(first));

        let percent = sample.mem_percent();
        self.memory.set_text(&format!("{percent:.0}%"));
        self.big_memory.set_text(&format!(
            "{} de {}",
            compact_bytes(sample.mem_used() as f64),
            compact_bytes(sample.mem_total as f64)
        ));
        // Aquí la barra es lo ocupado: el color avisa cuando queda poco.
        let free = (100.0 - percent).round().clamp(0.0, 100.0) as u32;
        for bar in [&self.memory_bar, &self.big_memory_bar] {
            bar.set_fraction(percent / 100.0);
            for class in LEVELS {
                bar.remove_css_class(class);
            }
            bar.add_css_class(level_class(free.saturating_mul(2)));
        }
        self.memory_note.set_text(&match sample.swap_total {
            0 => format!("{percent:.0} % en uso, sin intercambio"),
            total => format!(
                "{percent:.0} % en uso, intercambio {} de {}",
                compact_bytes(sample.swap_used() as f64),
                compact_bytes(total as f64)
            ),
        });
        let cores = if sample.cores == 1 { "núcleo".to_string() } else { format!("{} núcleos", sample.cores) };
        self.cpu_note.set_text(&format!("carga media {:.2}, {cores}", sample.load));

        let Some((before, at)) = previous else {
            // Hacen falta dos muestras para saber cuánto se usa.
            for label in [&self.cpu, &self.down, &self.up, &self.big_cpu, &self.big_down, &self.big_up] {
                label.set_text("…");
            }
            return;
        };
        if let Some(cpu) = sample.cpu_percent_since(&before) {
            push_sample(&self.cpu_history, cpu);
            self.cpu.set_text(&format!("{cpu:.0}%"));
            self.big_cpu.set_text(&format!("{cpu:.0} %"));
        }
        if let Some((down, up)) = sample.net_rates_since(&before, now.duration_since(at).as_secs_f64()) {
            push_sample(&self.down_history, down);
            push_sample(&self.up_history, up);
            let (down, up) = (format!("{}/s", compact_bytes(down)), format!("{}/s", compact_bytes(up)));
            self.down.set_text(&down);
            self.up.set_text(&up);
            self.big_down.set_text(&down);
            self.big_up.set_text(&up);
        }
        self.net_note.set_text(&format!(
            "desde que se abrió: {} recibidos, {} enviados",
            compact_bytes(sample.rx_bytes.saturating_sub(first.rx_bytes) as f64),
            compact_bytes(sample.tx_bytes.saturating_sub(first.tx_bytes) as f64)
        ));
        for chart in &self.charts {
            chart.queue_draw();
        }
    }
}

/// Escala mínima de las gráficas de red, en bytes por segundo.
const NET_FLOOR: f64 = 50.0 * 1024.0;

/// Hora local del reinicio. Con más de un día por delante lleva el día.
fn reset_clock(window: &Window, now: i64) -> String {
    let format = if window.seconds_left(now) >= 86_400 { "%A a las %H:%M" } else { "a las %H:%M" };
    glib::DateTime::from_unix_local(window.resets_at)
        .and_then(|at| at.format(format))
        .map(|text| text.to_string())
        .unwrap_or_else(|_| "hora desconocida".to_string())
}

fn label(ellipsize: bool) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_xalign(0.0);
    if ellipsize {
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    }
    label
}
