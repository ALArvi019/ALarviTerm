//! Panel de estadísticas de uso de Claude Code: tarjetas con los totales del
//! periodo, tokens por día, mapa de horas de trabajo y desgloses por modelo y
//! por proyecto. Los datos salen de `alarviterm_core::stats`.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use adw::prelude::*;
use alarviterm_core::paths::display_path;
use alarviterm_core::stats::{self, Library, Refresh, Row, Summary};
use gtk::{gio, glib};

/// Periodos que se pueden elegir: etiqueta y días. Cero es todo el historial.
const RANGES: [(&str, i64); 4] = [("7 días", 7), ("30 días", 30), ("90 días", 90), ("Todo", 0)];
const DEFAULT_RANGE: usize = 1;
/// Filas que se enseñan de cada desglose. El resto se agrupa.
const MAX_ROWS: usize = 8;
/// Azul de la salida en las gráficas. Se lee bien en los dos temas.
const BLUE: (f64, f64, f64) = (0.30, 0.58, 1.0);

const WEEKDAYS: [&str; 7] = ["L", "M", "X", "J", "V", "S", "D"];
const WEEKDAY_NAMES: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];

/// Biblioteca de transcripciones, compartida con el hilo que la lee. Vive
/// mientras viva la aplicación para no releer la caché en cada apertura.
pub type SharedLibrary = Arc<Mutex<Option<Library>>>;

struct Card {
    root: gtk::Box,
    value: gtk::Label,
    detail: gtk::Label,
    delta: gtk::Label,
    chart: gtk::DrawingArea,
    series: Rc<RefCell<Vec<f64>>>,
}

impl Card {
    fn new(title: &str) -> Self {
        let caption = gtk::Label::new(Some(title));
        caption.add_css_class("mt-stat-caption");
        caption.set_xalign(0.0);
        caption.set_hexpand(true);
        let delta = gtk::Label::new(None);
        delta.add_css_class("mt-delta");
        delta.set_visible(false);
        delta.set_valign(gtk::Align::Center);
        delta.set_tooltip_text(Some("Cambio respecto al periodo anterior de la misma duración"));
        let top = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        // Altura fija: la tarjeta no debe cambiar de tamaño con o sin comparación.
        top.set_size_request(-1, 22);
        top.append(&caption);
        top.append(&delta);

        let value = gtk::Label::new(Some("…"));
        value.add_css_class("mt-card-value");
        value.set_xalign(0.0);
        let detail = gtk::Label::new(None);
        detail.add_css_class("mt-stat-note");
        detail.set_xalign(0.0);
        detail.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let series = Rc::new(RefCell::new(Vec::new()));
        let chart = gtk::DrawingArea::new();
        chart.add_css_class("mt-chart");
        chart.set_content_height(26);
        chart.set_margin_top(6);
        let data = series.clone();
        chart.set_draw_func(move |area, cr, width, height| {
            let (r, g, b) = foreground(area);
            let series = data.borrow();
            let top = series.iter().copied().fold(0.0_f64, f64::max);
            let (width, height) = (f64::from(width), f64::from(height));
            if series.is_empty() {
                return;
            }
            let step = width / series.len() as f64;
            let bar = (step - 1.5).clamp(1.0, 9.0);
            for (i, value) in series.iter().enumerate() {
                let tall = if top > 0.0 { (value / top * (height - 1.0)).max(1.0) } else { 1.0 };
                cr.set_source_rgba(r, g, b, if *value > 0.0 { 0.55 } else { 0.2 });
                cr.rectangle(i as f64 * step + (step - bar) / 2.0, height - tall, bar, tall);
                let _ = cr.fill();
            }
        });

        let root = gtk::Box::new(gtk::Orientation::Vertical, 2);
        root.add_css_class("mt-card");
        root.set_hexpand(true);
        root.append(&top);
        root.append(&value);
        root.append(&detail);
        root.append(&chart);
        Self { root, value, detail, delta, chart, series }
    }

    fn set(&self, value: &str, detail: &str, change: Option<String>, series: Vec<f64>) {
        self.value.set_text(value);
        self.detail.set_text(detail);
        match change {
            Some(text) => {
                if text.starts_with('+') {
                    self.delta.remove_css_class("down");
                } else {
                    self.delta.add_css_class("down");
                }
                self.delta.set_text(&text);
                self.delta.set_visible(true);
            }
            None => self.delta.set_visible(false),
        }
        *self.series.borrow_mut() = series;
        self.chart.queue_draw();
    }
}

struct Panel {
    library: SharedLibrary,
    cache: PathBuf,
    range: Cell<usize>,
    /// Cuentas con nombre y sus carpetas, según los proyectos.
    accounts: Vec<(String, Vec<PathBuf>)>,
    /// 0: todas las cuentas. 1: la predeterminada. Después, las de `accounts`.
    account: Cell<usize>,
    /// Número de la última consulta lanzada: las respuestas viejas se tiran.
    request: Cell<u64>,
    summary: Rc<RefCell<Summary>>,
    spinner: gtk::Spinner,
    status: gtk::Label,
    period: gtk::Label,
    cards: [Card; 4],
    days_note: gtk::Label,
    days_chart: gtk::DrawingArea,
    days_scale: [gtk::Label; 2],
    days_axis: [gtk::Label; 3],
    heat_note: gtk::Label,
    heat_chart: gtk::DrawingArea,
    models_note: gtk::Label,
    models: gtk::Grid,
    projects_note: gtk::Label,
    projects: gtk::Grid,
}

/// Color de primer plano de un widget, para dibujar con el del tema.
fn foreground(widget: &impl IsA<gtk::Widget>) -> (f64, f64, f64) {
    let color = widget.color();
    (f64::from(color.red()), f64::from(color.green()), f64::from(color.blue()))
}

fn section(title: &str) -> (gtk::Box, gtk::Label) {
    let caption = gtk::Label::new(Some(title));
    caption.add_css_class("mt-stat-caption");
    caption.set_xalign(0.0);
    let rule = gtk::Separator::new(gtk::Orientation::Horizontal);
    rule.set_hexpand(true);
    rule.set_valign(gtk::Align::Center);
    let note = gtk::Label::new(None);
    note.add_css_class("mt-stat-note");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    head.append(&caption);
    head.append(&rule);
    head.append(&note);
    (head, note)
}

fn note_label() -> gtk::Label {
    let label = gtk::Label::new(None);
    label.add_css_class("mt-stat-note");
    label
}

fn short_date(ts: i64) -> String {
    glib::DateTime::from_unix_local(ts)
        .and_then(|at| at.format("%-d %b"))
        .map(|text| text.to_string())
        .unwrap_or_default()
}

/// Gráfica de tokens por día: la entrada hacia arriba y la salida hacia
/// abajo, cada una con su propia escala.
fn draw_days(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, width: i32, height: i32, summary: &Summary) {
    let (r, g, b) = foreground(area);
    let (width, height) = (f64::from(width), f64::from(height));
    let baseline = (height * 0.66).round();
    cr.set_source_rgba(r, g, b, 0.18);
    cr.rectangle(0.0, baseline, width, 1.0);
    let _ = cr.fill();
    let days = &summary.days;
    if days.is_empty() {
        return;
    }
    let top_input = days.iter().map(|d| d.input).max().unwrap_or(0).max(1) as f64;
    let top_output = days.iter().map(|d| d.output).max().unwrap_or(0).max(1) as f64;
    let step = width / days.len() as f64;
    let bar = (step * 0.62).clamp(1.0, 22.0);
    for (i, day) in days.iter().enumerate() {
        let x = i as f64 * step + (step - bar) / 2.0;
        let up = day.input as f64 / top_input * (baseline - 2.0);
        let down = day.output as f64 / top_output * (height - baseline - 3.0);
        if day.input > 0 {
            cr.set_source_rgba(r, g, b, 0.45);
            cr.rectangle(x, baseline - up.max(1.0), bar, up.max(1.0));
            let _ = cr.fill();
        }
        if day.output > 0 {
            cr.set_source_rgba(BLUE.0, BLUE.1, BLUE.2, 0.95);
            cr.rectangle(x, baseline + 1.0, bar, down.max(1.0));
            let _ = cr.fill();
        }
    }
}

/// Mapa de calor: una fila por día de la semana y una columna por hora.
fn draw_heat(area: &gtk::DrawingArea, cr: &gtk::cairo::Context, width: i32, height: i32, summary: &Summary) {
    let (r, g, b) = foreground(area);
    let (width, height) = (f64::from(width), f64::from(height));
    let (left, bottom) = (16.0, 14.0);
    let cell_w = (width - left) / 24.0;
    let cell_h = (height - bottom) / 7.0;
    let top = summary.heat.iter().flatten().copied().max().unwrap_or(0) as f64;
    cr.select_font_face("sans-serif", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
    cr.set_font_size(10.0);
    for (row, hours) in summary.heat.iter().enumerate() {
        let y = row as f64 * cell_h;
        cr.set_source_rgba(r, g, b, 0.6);
        cr.move_to(0.0, y + cell_h / 2.0 + 3.5);
        let _ = cr.show_text(WEEKDAYS[row]);
        for (hour, count) in hours.iter().enumerate() {
            // Raíz cuadrada: sin ella unas pocas horas punta apagan el resto.
            let level = if top > 0.0 { (*count as f64 / top).sqrt() } else { 0.0 };
            cr.set_source_rgba(r, g, b, 0.07 + level * 0.83);
            cr.rectangle(left + hour as f64 * cell_w + 1.0, y + 1.0, cell_w - 2.0, cell_h - 2.0);
            let _ = cr.fill();
        }
    }
    cr.set_source_rgba(r, g, b, 0.6);
    for hour in [0, 6, 12, 18] {
        cr.move_to(left + f64::from(hour) * cell_w + 1.0, height - 2.0);
        let _ = cr.show_text(&hour.to_string());
    }
}

fn model_name(raw: &str) -> String {
    raw.strip_prefix("claude-").unwrap_or(raw).to_string()
}

/// Rellena un desglose: nombre, barra con su parte de la entrada y cifras.
fn fill_rows(grid: &gtk::Grid, rows: &[Row], count_title: &str, name: impl Fn(&str) -> String) {
    while let Some(child) = grid.first_child() {
        grid.remove(&child);
    }
    for (column, title) in [(2, "entrada"), (3, "salida"), (4, count_title)] {
        let head = note_label();
        head.set_text(title);
        head.set_xalign(1.0);
        grid.attach(&head, column, 0, 1, 1);
    }
    let top = rows.first().map_or(1, |r| r.input.max(1)) as f64;
    let mut shown: Vec<Row> = rows.iter().take(MAX_ROWS).cloned().collect();
    if rows.len() > MAX_ROWS {
        let rest = &rows[MAX_ROWS..];
        shown.push(Row {
            name: format!("{} más", rest.len()),
            input: rest.iter().map(|r| r.input).sum(),
            output: rest.iter().map(|r| r.output).sum(),
            messages: rest.iter().map(|r| r.messages).sum(),
        });
    }
    for (index, row) in shown.iter().enumerate() {
        let line = index as i32 + 1;
        let grouped = rows.len() > MAX_ROWS && index == MAX_ROWS;
        let label = gtk::Label::new(Some(&if grouped { row.name.clone() } else { name(&row.name) }));
        label.set_xalign(0.0);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
        label.set_width_chars(16);
        label.set_max_width_chars(22);
        label.set_tooltip_text((!grouped).then_some(row.name.as_str()));
        if grouped {
            label.add_css_class("mt-stat-note");
        }
        let bar = gtk::ProgressBar::new();
        bar.add_css_class("mt-meter");
        bar.add_css_class("mt-meter-share");
        bar.set_fraction((row.input as f64 / top).clamp(0.0, 1.0));
        bar.set_valign(gtk::Align::Center);
        bar.set_hexpand(true);
        bar.set_visible(!grouped);
        grid.attach(&label, 0, line, 1, 1);
        grid.attach(&bar, 1, line, 1, 1);
        for (column, text) in [
            (2, stats::compact(row.input)),
            (3, stats::compact(row.output)),
            (4, stats::compact(row.messages)),
        ] {
            let cell = gtk::Label::new(Some(&text));
            cell.set_xalign(1.0);
            cell.add_css_class("mt-stat-number");
            grid.attach(&cell, column, line, 1, 1);
        }
    }
    if rows.is_empty() {
        let empty = note_label();
        empty.set_text("Sin datos en este periodo.");
        empty.set_xalign(0.0);
        grid.attach(&empty, 0, 1, 5, 1);
    }
}

/// Inicio del periodo: la medianoche local de hace `days - 1` días, para que
/// cada barra sea un día natural completo.
fn range_start(days: i64) -> Option<i64> {
    if days == 0 {
        return None;
    }
    let now = glib::DateTime::now_local().ok()?;
    let midnight = glib::DateTime::from_local(now.year(), now.month(), now.day_of_month(), 0, 0, 0.0).ok()?;
    Some(midnight.to_unix() - (days - 1) * 86_400)
}

impl Panel {
    /// Lanza la lectura en otro hilo y pinta el resultado cuando llega.
    fn load(self: &Rc<Self>) {
        let request = self.request.get() + 1;
        self.request.set(request);
        let days = RANGES[self.range.get()].1;
        let start = range_start(days);
        let now = alarviterm_core::now_unix();
        // También hace falta el periodo anterior, para comparar.
        let since = start.map_or(0, |s| s - (now - s) - 86_400);
        self.spinner.set_visible(true);
        self.spinner.start();
        self.status.set_text("Leyendo las transcripciones de Claude Code…");

        let (library, cache) = (self.library.clone(), self.cache.clone());
        let (accounts, account) = (self.accounts.clone(), self.account.get());
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                let root = stats::transcripts_dir()?;
                let mut guard = library.lock().ok()?;
                let library = guard.get_or_insert_with(|| Library::load(&cache));
                let report = library.refresh(&root, since);
                if report.read > 0 {
                    if let Err(e) = library.save(&cache) {
                        log::warn!("no se pudo guardar la caché de estadísticas: {e}");
                    }
                }
                let named = |cwd: &str, folders: &[PathBuf]| folders.iter().any(|f| stats::is_under(cwd, f));
                let keep = |cwd: &str| match account {
                    0 => true,
                    1 => !accounts.iter().any(|(_, folders)| named(cwd, folders)),
                    n => accounts.get(n - 2).is_some_and(|(_, folders)| named(cwd, folders)),
                };
                Some((library.summarize_where(start, now, stats::system_local, &keep), report))
            })
            .await;
            let Some(panel) = weak.upgrade() else {
                return;
            };
            if panel.request.get() != request {
                return;
            }
            panel.spinner.stop();
            panel.spinner.set_visible(false);
            match result {
                Ok(Some((summary, report))) => panel.show(summary, report, now),
                _ => panel.status.set_text("No se pudo leer la carpeta de Claude Code."),
            }
        });
    }

    fn show(&self, summary: Summary, report: Refresh, now: i64) {
        let totals = summary.totals;
        let previous = summary.previous;
        let delta = |current: u64, pick: fn(&stats::Totals) -> u64| {
            previous.as_ref().and_then(|p| stats::change(current, pick(p)))
        };
        let series = |pick: fn(&stats::Day) -> u64| summary.days.iter().map(|d| pick(d) as f64).collect::<Vec<_>>();

        self.period.set_text(&match summary.days.first() {
            Some(first) => format!("{} a hoy", short_date(first.ts)),
            None => "Sin actividad registrada".to_string(),
        });
        self.cards[0].set(
            &stats::compact(totals.input_total()),
            &match totals.cache_percent() {
                Some(percent) => format!("{percent} % leído de caché"),
                None => String::new(),
            },
            delta(totals.input_total(), |t| t.input_total()),
            series(|d| d.input),
        );
        self.cards[1].set(
            &stats::compact(totals.output),
            &match totals.turns {
                0 => String::new(),
                turns => format!("{} por turno", stats::compact(totals.output / turns)),
            },
            delta(totals.output, |t| t.output),
            series(|d| d.output),
        );
        self.cards[2].set(
            &stats::compact(totals.sessions),
            &format!("{} turnos, {} respuestas", stats::compact(totals.turns), stats::compact(totals.messages)),
            delta(totals.sessions, |t| t.sessions),
            series(|d| d.turns),
        );
        self.cards[3].set(
            &stats::compact_duration(totals.active_secs),
            "estimado por la actividad",
            delta(totals.active_secs, |t| t.active_secs),
            series(|d| d.active_secs),
        );

        let active = summary.active_days();
        self.days_note.set_text(&match active {
            0 => String::new(),
            _ => format!(
                "{active} de {} días con actividad, {} al día",
                summary.days.len(),
                stats::compact((totals.input_total() + totals.output) / active as u64)
            ),
        });
        let top_input = summary.days.iter().map(|d| d.input).max().unwrap_or(0);
        let top_output = summary.days.iter().map(|d| d.output).max().unwrap_or(0);
        self.days_scale[0].set_text(&format!("{}\nentrada", stats::compact(top_input)));
        self.days_scale[1].set_text(&format!("{}\nsalida", stats::compact(top_output)));
        let middle = summary.days.get(summary.days.len() / 2);
        self.days_axis[0].set_text(&summary.days.first().map(|d| short_date(d.ts)).unwrap_or_default());
        self.days_axis[1].set_text(&middle.filter(|_| summary.days.len() > 4).map(|d| short_date(d.ts)).unwrap_or_default());
        self.days_axis[2].set_text(if summary.days.is_empty() { "" } else { "hoy" });

        let peak = (0..7)
            .flat_map(|day| (0..24).map(move |hour| (day, hour)))
            .max_by_key(|(day, hour)| summary.heat[*day][*hour])
            .filter(|(day, hour)| summary.heat[*day][*hour] > 0);
        self.heat_note.set_text(&match peak {
            Some((day, hour)) => format!("más actividad: {} a las {hour} h", WEEKDAY_NAMES[day]),
            None => String::new(),
        });

        let plural = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        self.models_note.set_text(&plural(summary.models.len(), "modelo", "modelos"));
        self.projects_note.set_text(&plural(summary.projects.len(), "carpeta", "carpetas"));
        fill_rows(&self.models, &summary.models, "respuestas", model_name);
        fill_rows(&self.projects, &summary.projects, "respuestas", |path| {
            display_path(std::path::Path::new(path))
        });

        self.status.set_text(&match report.read {
            0 => format!("{} transcripciones, al día.", report.files),
            read => format!(
                "{} transcripciones, {read} leídas ahora ({} MB).",
                report.files,
                report.bytes_read / (1024 * 1024)
            ),
        });
        let _ = now;
        *self.summary.borrow_mut() = summary;
        self.days_chart.queue_draw();
        self.heat_chart.queue_draw();
    }
}

/// Abre el panel de estadísticas.
///
/// `accounts` son las cuentas con nombre y las carpetas de sus proyectos. Con
/// alguna, el panel deja ver el uso de cada una por separado.
pub fn show(
    parent: &impl IsA<gtk::Widget>,
    library: SharedLibrary,
    cache: PathBuf,
    accounts: Vec<(String, Vec<PathBuf>)>,
) -> adw::Dialog {
    let summary = Rc::new(RefCell::new(Summary::default()));

    let cards = [
        Card::new("TOKENS DE ENTRADA"),
        Card::new("TOKENS DE SALIDA"),
        Card::new("SESIONES"),
        Card::new("TIEMPO ACTIVO"),
    ];
    cards[3].root.set_tooltip_text(Some(
        "Estimación: suma los huecos de menos de cinco minutos entre mensajes seguidos de una misma sesión.",
    ));
    let card_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    card_row.set_homogeneous(true);
    for card in &cards {
        card_row.append(&card.root);
    }

    // Tokens por día.
    let (days_head, days_note) = section("TOKENS POR DÍA");
    let days_chart = gtk::DrawingArea::new();
    days_chart.add_css_class("mt-chart");
    days_chart.set_content_height(150);
    days_chart.set_hexpand(true);
    let data = summary.clone();
    days_chart.set_draw_func(move |area, cr, w, h| draw_days(area, cr, w, h, &data.borrow()));
    let days_scale = [note_label(), note_label()];
    days_scale[0].set_valign(gtk::Align::Start);
    days_scale[0].set_vexpand(true);
    days_scale[1].set_valign(gtk::Align::End);
    days_scale[1].add_css_class("mt-chart-out");
    let scale = gtk::Box::new(gtk::Orientation::Vertical, 0);
    scale.set_size_request(64, -1);
    for label in &days_scale {
        label.set_xalign(0.0);
        scale.append(label);
    }
    let days_axis = [note_label(), note_label(), note_label()];
    let axis = gtk::CenterBox::new();
    axis.set_start_widget(Some(&days_axis[0]));
    axis.set_center_widget(Some(&days_axis[1]));
    axis.set_end_widget(Some(&days_axis[2]));
    axis.set_margin_start(64 + 8);
    let plot = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    plot.append(&scale);
    plot.append(&days_chart);
    let days_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    days_box.set_hexpand(true);
    days_box.append(&days_head);
    days_box.append(&plot);
    days_box.append(&axis);

    // Mapa de horas.
    let (heat_head, heat_note) = section("CUÁNDO TRABAJAN");
    let heat_chart = gtk::DrawingArea::new();
    heat_chart.add_css_class("mt-chart");
    heat_chart.set_content_height(150);
    heat_chart.set_content_width(400);
    heat_chart.set_tooltip_text(Some("Respuestas del modelo por día de la semana y hora local. Más claro, más actividad."));
    let data = summary.clone();
    heat_chart.set_draw_func(move |area, cr, w, h| draw_heat(area, cr, w, h, &data.borrow()));
    let heat_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
    heat_box.set_hexpand(false);
    heat_box.set_size_request(400, -1);
    heat_box.append(&heat_head);
    heat_box.append(&heat_chart);

    let charts = gtk::Box::new(gtk::Orientation::Horizontal, 28);
    charts.append(&days_box);
    charts.append(&heat_box);

    // Desgloses.
    let breakdown = |title: &str| {
        let (head, note) = section(title);
        let grid = gtk::Grid::new();
        grid.set_column_spacing(14);
        grid.set_row_spacing(5);
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 8);
        holder.set_hexpand(true);
        holder.append(&head);
        holder.append(&grid);
        (holder, note, grid)
    };
    let (models_box, models_note, models) = breakdown("MODELOS");
    let (projects_box, projects_note, projects) = breakdown("CARPETAS");
    let tables = gtk::Box::new(gtk::Orientation::Horizontal, 28);
    tables.set_homogeneous(true);
    tables.append(&models_box);
    tables.append(&projects_box);

    let status = note_label();
    status.set_xalign(0.0);
    status.set_hexpand(true);
    let scope = note_label();
    scope.set_xalign(0.0);
    scope.set_wrap(true);
    scope.set_text(
        "Datos de las transcripciones locales de Claude Code. Incluyen todas las sesiones de este equipo, \
         no solo las abiertas desde ALarviTerm.",
    );

    let body = gtk::Box::new(gtk::Orientation::Vertical, 22);
    body.add_css_class("mt-stats");
    body.append(&card_row);
    body.append(&charts);
    body.append(&tables);
    body.append(&scope);
    body.append(&status);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_child(Some(&body));

    // Cabecera: periodo a la izquierda, selector a la derecha.
    let period = note_label();
    let spinner = gtk::Spinner::new();
    let choices = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    choices.add_css_class("linked");
    let mut buttons: Vec<gtk::ToggleButton> = Vec::new();
    for (index, (label, _)) in RANGES.iter().enumerate() {
        let button = gtk::ToggleButton::with_label(label);
        if let Some(first) = buttons.first() {
            button.set_group(Some(first));
        }
        button.set_active(index == DEFAULT_RANGE);
        choices.append(&button);
        buttons.push(button);
    }
    let title = adw::WindowTitle::new("Uso de Claude Code", "");
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&title));
    header.pack_start(&period);
    header.pack_end(&choices);
    let mut account_names = vec!["Todas las cuentas".to_string(), "Cuenta predeterminada".to_string()];
    account_names.extend(accounts.iter().map(|(name, _)| format!("Cuenta {name}")));
    let account_refs: Vec<&str> = account_names.iter().map(String::as_str).collect();
    let account_choice = gtk::DropDown::from_strings(&account_refs);
    account_choice.set_visible(!accounts.is_empty());
    account_choice.set_tooltip_text(Some(
        "El uso se reparte por carpeta: lo hecho en la carpeta de un proyecto cuenta para la cuenta \
         que ese proyecto tiene puesta. El resto va a la predeterminada.",
    ));
    header.pack_end(&account_choice);
    header.pack_end(&spinner);
    let view = adw::ToolbarView::new();
    view.add_top_bar(&header);
    view.set_content(Some(&scroller));

    let panel = Rc::new(Panel {
        library,
        cache,
        range: Cell::new(DEFAULT_RANGE),
        accounts,
        account: Cell::new(0),
        request: Cell::new(0),
        summary,
        spinner,
        status,
        period,
        cards,
        days_note,
        days_chart,
        days_scale,
        days_axis,
        heat_note,
        heat_chart,
        models_note,
        models,
        projects_note,
        projects,
    });
    for (index, button) in buttons.iter().enumerate() {
        let weak = Rc::downgrade(&panel);
        button.connect_toggled(move |button| {
            if let Some(panel) = weak.upgrade().filter(|_| button.is_active()) {
                panel.range.set(index);
                panel.load();
            }
        });
    }
    {
        let weak = Rc::downgrade(&panel);
        account_choice.connect_selected_notify(move |choice| {
            if let Some(panel) = weak.upgrade() {
                panel.account.set(choice.selected() as usize);
                panel.load();
            }
        });
    }
    panel.load();

    let dialog = adw::Dialog::builder()
        .title("Estadísticas de uso")
        .content_width(1080)
        .content_height(760)
        .child(&view)
        .build();
    // El panel vive mientras viva el diálogo.
    let keep = RefCell::new(Some(panel));
    dialog.connect_closed(move |_| {
        keep.borrow_mut().take();
    });
    dialog.present(Some(parent));
    dialog
}
