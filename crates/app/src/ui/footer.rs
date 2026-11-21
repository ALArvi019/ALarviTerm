//! Barra inferior. Solo muestra datos comprobados: sesión activa, estado,
//! directorio, rama de git, modelo, consumo, recuento y versiones detectadas.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use alarviterm_core::limits::{self, RateLimits, Window};
use alarviterm_core::paths::display_path;
use alarviterm_core::sessions::SessionView;
use alarviterm_core::sysinfo::{GitSummary, SelfUsage};

use super::status;

pub struct Footer {
    pub root: gtk::Box,
    dot: gtk::Box,
    session: gtk::Label,
    state: gtk::Label,
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
    limits: gtk::MenuButton,
    five_hour: Meter,
    seven_day: Meter,
    limits_age: gtk::Label,
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
        let cwd = label(true);
        cwd.set_hexpand(true);
        cwd.set_halign(gtk::Align::Start);
        cwd.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        let git = label(false);
        let model = label(false);
        for extra in [&git, &model] {
            extra.set_visible(false);
        }

        // Consumo de la sesión activa: gráfica de CPU, su valor y la memoria.
        let cpu_history = Rc::new(RefCell::new(VecDeque::with_capacity(CPU_SAMPLES)));
        let cpu_chart = sparkline(cpu_history.clone());
        let cpu = value_label();
        let memory = value_label();
        let usage = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        usage.add_css_class("mt-stat");
        usage.append(&caption("CPU"));
        usage.append(&cpu_chart);
        usage.append(&cpu);
        usage.append(&caption("MEM"));
        usage.append(&memory);
        usage.set_visible(false);

        // Consumo de la propia aplicación, con sus clientes de terminal.
        let app_history = Rc::new(RefCell::new(VecDeque::with_capacity(CPU_SAMPLES)));
        let app_chart = sparkline(app_history.clone());
        let app_cpu = value_label();
        let app_memory = value_label();
        let app = gtk::Box::new(gtk::Orientation::Horizontal, 5);
        app.add_css_class("mt-stat");
        app.append(&caption("APP"));
        app.append(&app_chart);
        app.append(&app_cpu);
        app.append(&app_memory);
        app.set_visible(false);

        // Límites de la cuenta: dos medidores y un panel de detalle al pulsar.
        let five_hour = Meter::new("5H", "Ventana de 5 horas");
        let seven_day = Meter::new("7D", "Ventana de 7 días");
        let meters = gtk::Box::new(gtk::Orientation::Horizontal, 12);
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
        panel.append(&heading);
        panel.append(&five_hour.detail);
        panel.append(&seven_day.detail);
        panel.append(&limits_age);
        let popover = gtk::Popover::new();
        popover.set_child(Some(&panel));
        let limits = gtk::MenuButton::new();
        limits.add_css_class("flat");
        limits.add_css_class("mt-stat-button");
        limits.set_child(Some(&meters));
        limits.set_popover(Some(&popover));
        limits.set_direction(gtk::ArrowType::Up);
        limits.set_tooltip_text(Some("Límites de uso de la cuenta. Pulsa para ver el detalle."));
        limits.set_visible(false);
        let counts = label(false);
        let backend = label(false);
        let claude = label(false);

        for widget in [
            dot.upcast_ref::<gtk::Widget>(),
            session.upcast_ref(),
            state.upcast_ref(),
            cwd.upcast_ref(),
            git.upcast_ref(),
            model.upcast_ref(),
            usage.upcast_ref(),
            limits.upcast_ref(),
            app.upcast_ref(),
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
            limits,
            five_hour,
            seven_day,
            limits_age,
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
        self.git.set_tooltip_text(Some(&format!("Rama {}, {changes}", summary.branch)));
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
                let mut history = self.cpu_history.borrow_mut();
                if history.len() == CPU_SAMPLES {
                    history.pop_front();
                }
                history.push_back(cpu);
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
                let mut history = self.app_history.borrow_mut();
                if history.len() == CPU_SAMPLES {
                    history.pop_front();
                }
                history.push_back(cpu);
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
    pub fn set_limits(&self, limits: Option<&RateLimits>, now: i64) {
        let Some(limits) = limits.map(|l| l.live(now)).filter(|l| !l.is_empty()) else {
            self.limits.set_visible(false);
            return;
        };
        self.five_hour.set(limits.five_hour.as_ref(), now);
        self.seven_day.set(limits.seven_day.as_ref(), now);
        self.limits_age.set_text(&format!(
            "Dato de Claude Code recibido hace {}. Se actualiza cuando una sesión responde.",
            limits::long_duration((now - limits.updated).max(0))
        ));
        self.limits.set_visible(true);
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
            }
            None => {
                self.dot.set_visible(false);
                self.session.set_text("");
                self.state.set_text("Sin sesión seleccionada");
                self.state.set_tooltip_text(None);
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

/// Muestras de CPU que guarda la mini gráfica.
const CPU_SAMPLES: usize = 30;

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
/// panel de detalle. La barra representa lo que queda.
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
        let remaining = window.remaining_percent();
        let seconds = window.seconds_left(now);
        let fraction = f64::from(remaining) / 100.0;
        for bar in [&self.bar, &self.detail_bar] {
            bar.set_fraction(fraction);
            for class in LEVELS {
                bar.remove_css_class(class);
            }
            bar.add_css_class(level_class(remaining));
        }
        self.value.set_text(&format!("{remaining}%"));
        self.left.set_text(&limits::short_duration(seconds).replace(' ', ""));
        self.detail_value.set_text(&format!("{remaining} % libre"));
        self.detail_text.set_text(&format!(
            "Usado el {} %. Se reinicia en {}, {}.",
            100 - remaining,
            limits::long_duration(seconds),
            reset_clock(window, now),
        ));
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

/// Gráfica de línea diminuta con relleno. La escala llega al menos al 100 %
/// de un núcleo y crece si alguna muestra lo supera.
fn sparkline(history: Rc<RefCell<VecDeque<f64>>>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.add_css_class("mt-spark");
    area.set_content_width(40);
    area.set_content_height(13);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(move |area, cr, width, height| {
        let history = history.borrow();
        let (width, height) = (f64::from(width), f64::from(height));
        let color = area.color();
        let (r, g, b) = (f64::from(color.red()), f64::from(color.green()), f64::from(color.blue()));
        // Línea de base, para que una gráfica sin datos no parezca un hueco.
        cr.set_source_rgba(r, g, b, 0.25);
        cr.rectangle(0.0, height - 1.0, width, 1.0);
        let _ = cr.fill();
        if history.len() < 2 {
            return;
        }
        let top = history.iter().copied().fold(100.0_f64, f64::max);
        let step = width / (CPU_SAMPLES - 1) as f64;
        let start = width - step * (history.len() - 1) as f64;
        let point = |i: usize, v: f64| (start + step * i as f64, height - 1.0 - (v / top).clamp(0.0, 1.0) * (height - 2.0));
        for (i, v) in history.iter().enumerate() {
            let (x, y) = point(i, *v);
            if i == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        cr.set_source_rgba(r, g, b, 1.0);
        cr.set_line_width(1.2);
        let _ = cr.stroke_preserve();
        cr.line_to(width, height);
        cr.line_to(start, height);
        cr.close_path();
        cr.set_source_rgba(r, g, b, 0.22);
        let _ = cr.fill();
    });
    area
}

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
