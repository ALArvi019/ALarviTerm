//! Iconos dibujados a mano. Son cinco trazos y así no dependen del tema de
//! iconos instalado, que cambia entre escritorios.

use gtk::prelude::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    SplitRight,
    SplitDown,
    Maximize,
    Close,
    Grid,
}

pub fn icon(kind: Icon, size: i32) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(size);
    area.set_content_height(size);
    area.set_halign(gtk::Align::Center);
    area.set_valign(gtk::Align::Center);
    area.set_draw_func(move |area, cr, width, height| {
        let color = area.color();
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()),
        );
        cr.set_line_width(1.2);
        let (w, h) = (f64::from(width), f64::from(height));
        // Medio píxel de margen para que las líneas caigan en la rejilla.
        let (x0, y0, x1, y1) = (1.5, 2.5, w - 1.5, h - 2.5);
        let frame = || cr.rectangle(x0, y0, x1 - x0, y1 - y0);
        match kind {
            Icon::SplitRight => {
                frame();
                cr.move_to((w / 2.0).floor() + 0.5, y0);
                cr.line_to((w / 2.0).floor() + 0.5, y1);
            }
            Icon::SplitDown => {
                frame();
                cr.move_to(x0, (h / 2.0).floor() + 0.5);
                cr.line_to(x1, (h / 2.0).floor() + 0.5);
            }
            Icon::Maximize => {
                frame();
                cr.move_to(x0, y0 + 2.0);
                cr.line_to(x1, y0 + 2.0);
            }
            Icon::Grid => {
                frame();
                cr.move_to((w / 2.0).floor() + 0.5, y0);
                cr.line_to((w / 2.0).floor() + 0.5, y1);
                cr.move_to(x0, (h / 2.0).floor() + 0.5);
                cr.line_to(x1, (h / 2.0).floor() + 0.5);
            }
            Icon::Close => {
                let inset = (w * 0.28).max(2.5);
                cr.move_to(inset, inset);
                cr.line_to(w - inset, h - inset);
                cr.move_to(w - inset, inset);
                cr.line_to(inset, h - inset);
            }
        }
        if let Err(e) = cr.stroke() {
            log::debug!("no se pudo dibujar un icono: {e}");
        }
    });
    area
}

/// Botón discreto con uno de los iconos y una acción de ventana.
pub fn button(kind: Icon, size: i32, tooltip: &str, action: &str, target: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.set_child(Some(&icon(kind, size)));
    button.add_css_class("flat");
    button.add_css_class("mt-icon-button");
    button.set_valign(gtk::Align::Center);
    button.set_focus_on_click(false);
    button.set_tooltip_text(Some(tooltip));
    button.set_action_name(Some(action));
    button.set_action_target_value(Some(&target.to_variant()));
    button
}
