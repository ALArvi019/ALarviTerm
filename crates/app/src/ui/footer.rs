//! Barra inferior. Solo muestra datos comprobados: sesión activa, estado,
//! directorio, rama de git, modelo, consumo, recuento y versiones detectadas.

use std::cell::Cell;

use gtk::prelude::*;
use alarviterm_core::paths::display_path;
use alarviterm_core::sessions::SessionView;
use alarviterm_core::sysinfo::GitSummary;

use super::status;

pub struct Footer {
    pub root: gtk::Box,
    dot: gtk::Box,
    session: gtk::Label,
    state: gtk::Label,
    cwd: gtk::Label,
    git: gtk::Label,
    model: gtk::Label,
    usage: gtk::Label,
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
        let usage = label(false);
        for extra in [&git, &model, &usage] {
            extra.set_visible(false);
        }
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
            self.usage.set_visible(false);
            return;
        };
        let megabytes = rss_bytes as f64 / (1024.0 * 1024.0);
        let text = match cpu {
            Some(cpu) => format!("{cpu:.0}% {megabytes:.0} MB"),
            None => format!("{megabytes:.0} MB"),
        };
        let plural = if processes == 1 { "proceso" } else { "procesos" };
        self.usage.set_text(&text);
        self.usage.set_tooltip_text(Some(&format!(
            "CPU y memoria residente de la sesión ({processes} {plural})"
        )));
        self.usage.set_visible(true);
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

fn label(ellipsize: bool) -> gtk::Label {
    let label = gtk::Label::new(None);
    label.set_xalign(0.0);
    if ellipsize {
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    }
    label
}
