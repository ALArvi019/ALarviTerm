//! Tema visual: hoja de estilos de la aplicación y paleta del terminal.

use gtk::gdk;
use alarviterm_core::settings::Theme;

/// Colores de la interfaz para el tema oscuro.
const DARK: &str = "
@define-color window_bg_color #0d0e10;
@define-color window_fg_color #d4d7dd;
@define-color view_bg_color #0d0e10;
@define-color view_fg_color #d4d7dd;
@define-color headerbar_bg_color #0d0e10;
@define-color headerbar_fg_color #d4d7dd;
@define-color headerbar_backdrop_color #0d0e10;
@define-color headerbar_shade_color rgba(0, 0, 0, 0);
@define-color sidebar_bg_color #111215;
@define-color popover_bg_color #17181c;
@define-color dialog_bg_color #15161a;
@define-color card_bg_color rgba(255, 255, 255, 0.04);
@define-color accent_bg_color #3b6fd4;
@define-color accent_color #7aa8ff;

@define-color mt_border rgba(255, 255, 255, 0.07);
@define-color mt_dim #7d828c;
@define-color mt_soft #a9aeb8;
@define-color mt_deep #0b0c0e;
@define-color mt_tint rgba(255, 255, 255, 0.05);
@define-color mt_green #3fb950;
@define-color mt_amber #d29922;
@define-color mt_red #f0554c;
@define-color mt_gray #5a5f69;
@define-color mt_blue #4c8dff;
";

/// Colores de la interfaz para el tema claro.
const LIGHT: &str = "
@define-color window_bg_color #f6f6f7;
@define-color window_fg_color #1f2328;
@define-color view_bg_color #ffffff;
@define-color view_fg_color #1f2328;
@define-color headerbar_bg_color #f6f6f7;
@define-color headerbar_fg_color #1f2328;
@define-color headerbar_backdrop_color #f6f6f7;
@define-color headerbar_shade_color rgba(0, 0, 0, 0);
@define-color sidebar_bg_color #ececee;
@define-color popover_bg_color #ffffff;
@define-color dialog_bg_color #fafafa;
@define-color card_bg_color rgba(0, 0, 0, 0.04);
@define-color accent_bg_color #3b6fd4;
@define-color accent_color #2a5bd0;

@define-color mt_border rgba(0, 0, 0, 0.12);
@define-color mt_dim #6a6f78;
@define-color mt_soft #4a4f58;
@define-color mt_deep #ffffff;
@define-color mt_tint rgba(0, 0, 0, 0.05);
@define-color mt_green #1a7f37;
@define-color mt_amber #9a6700;
@define-color mt_red #cf222e;
@define-color mt_gray #8c959f;
@define-color mt_blue #0969da;
";

const CSS: &str = "
headerbar {
  min-height: 38px;
  border-bottom: 1px solid @mt_border;
  box-shadow: none;
}

.mt-sidebar {
  background-color: @sidebar_bg_color;
}
.mt-sidebar list {
  background: none;
}
.mt-sidebar-title {
  font-size: 0.72em;
  font-weight: 700;
  letter-spacing: 0.12em;
  color: @mt_dim;
}
.mt-sidebar row {
  border-radius: 6px;
  margin: 0 6px;
  padding: 0;
  transition: background-color 90ms ease-out;
}
.mt-sidebar row.mt-session {
  padding: 5px 8px 5px 14px;
}
.mt-sidebar row.mt-session:selected {
  background-color: rgba(122, 168, 255, 0.14);
}
.mt-sidebar row.mt-group {
  padding: 4px 8px;
  margin-top: 8px;
}
.mt-group-name {
  font-size: 0.78em;
  font-weight: 700;
  letter-spacing: 0.08em;
  color: @mt_soft;
}
button.mt-group-power {
  min-width: 18px;
  min-height: 18px;
  padding: 0 2px;
  color: @mt_dim;
}
button.mt-group-power.on {
  color: @mt_green;
}
button.mt-group-power:hover {
  color: @window_fg_color;
}
.mt-group-state {
  font-size: 0.78em;
  color: @mt_soft;
}
.mt-group-count {
  font-size: 0.78em;
  color: @mt_dim;
}
.mt-arrow {
  color: @mt_dim;
}
.mt-session-name {
  font-weight: 500;
}
.mt-session-detail {
  font-family: monospace;
  font-size: 0.78em;
  color: @mt_dim;
}
.mt-empty-hint {
  font-size: 0.85em;
  color: @mt_dim;
}

.mt-dot {
  min-width: 8px;
  min-height: 8px;
  border-radius: 4px;
  background-color: @mt_gray;
  transition: background-color 150ms ease-out;
}
.mt-dot.st-alive { background-color: @mt_green; }
.mt-dot.st-working { background-color: @mt_blue; }
.mt-dot.st-waiting { background-color: @mt_green; }
.mt-dot.st-background { background-color: #bc8cff; }
.mt-dot.st-attention {
  background-color: @mt_amber;
  box-shadow: 0 0 0 3px alpha(@mt_amber, 0.28);
}
.mt-dot.st-done { background-color: @mt_gray; }
.mt-dot.st-error { background-color: @mt_red; }
.mt-dot.st-missing {
  min-width: 6px;
  min-height: 6px;
  background-color: transparent;
  border: 1px solid @mt_amber;
}

.mt-footer {
  min-height: 24px;
  padding: 0 10px;
  border-top: 1px solid @mt_border;
  background-color: @sidebar_bg_color;
  font-family: monospace;
  font-size: 0.8em;
  color: @mt_dim;
}
.mt-footer .mt-footer-strong {
  color: @window_fg_color;
}
.mt-footer .mt-footer-link:hover {
  color: @window_fg_color;
}
.mt-side-tabs {
  margin: 8px 10px 2px 10px;
}
.mt-side-tabs > button {
  min-height: 26px;
  padding: 0 8px;
  font-size: 0.9em;
  font-weight: 600;
}
.mt-code-files {
  background: none;
}
.mt-code-repo {
  font-weight: 700;
}
.mt-code-repo-row {
  margin-top: 4px;
}
.mt-code-count {
  padding: 0 6px;
  border-radius: 8px;
  background-color: @mt_tint;
  font-size: 0.8em;
  color: @mt_soft;
}
.mt-code-view {
  background-color: @view_bg_color;
}
listbox.mt-code-files > row {
  min-height: 24px;
  padding: 1px 8px;
  margin: 0 6px;
}
.mt-code-summary {
  padding: 10px 14px 6px 14px;
}
.mt-code-folder {
  color: @mt_soft;
}
.mt-code-deleted {
  text-decoration: line-through;
  color: @mt_dim;
}
.mt-code-status {
  font-family: monospace;
  font-weight: 700;
  font-size: 0.85em;
  min-width: 14px;
}
.mt-code-status.s-modified { color: @mt_amber; }
.mt-code-status.s-added { color: @mt_green; }
.mt-code-status.s-deleted { color: @mt_red; }
.mt-code-status.s-conflict { color: @mt_red; }
.mt-code-bar {
  padding: 5px 8px 5px 14px;
  border-bottom: 1px solid @mt_border;
}
.mt-code-title {
  font-family: monospace;
  font-size: 0.92em;
}
.mt-code-caption {
  padding: 4px 12px;
  border-bottom: 1px solid @mt_border;
}
.mt-code-branch {
  font-family: monospace;
  font-size: 0.88em;
  color: @mt_soft;
}
.mt-code {
  font-size: 0.95em;
}
.mt-footer .mt-footer-mode {
  padding: 0 6px;
  border-radius: 4px;
  background-color: @mt_tint;
}
.mt-footer .mt-footer-warn {
  color: @mt_amber;
}

.mt-stat-caption {
  font-size: 0.82em;
  font-weight: 700;
  letter-spacing: 0.06em;
  color: @mt_gray;
}
.mt-stat-value {
  color: @window_fg_color;
}
.mt-stat-note {
  color: @mt_dim;
}
.mt-spark {
  color: @mt_blue;
}
.mt-tab.drop-before {
  box-shadow: inset 2px 0 0 @accent_color;
}
.mt-tab.drop-after {
  box-shadow: inset -2px 0 0 @accent_color;
}
.mt-panel {
  border-left: 1px solid @mt_border;
  background-color: @sidebar_bg_color;
}
.mt-panel-header {
  padding: 8px 6px 4px 14px;
}
.mt-panel-body {
  padding: 6px 14px 16px 14px;
}
.mt-panel-empty {
  color: @mt_dim;
}
.mt-panel-mark {
  color: @mt_blue;
  min-width: 12px;
}
.mt-panel-item.done,
.mt-panel-item.done .mt-panel-mark {
  color: @mt_gray;
}
progressbar.mt-meter-wide > trough { min-width: 200px; }
.mt-net-down,
.mt-footer .mt-net-down {
  color: @mt_blue;
  font-weight: 700;
}
.mt-net-up,
.mt-footer .mt-net-up {
  color: @mt_red;
  font-weight: 700;
}
.mt-chart-box {
  border-radius: 6px;
  background-color: @mt_tint;
}
progressbar.mt-meter-short > trough { min-width: 34px; }
.mt-footer menubutton.mt-stat-button.pinned > button {
  background-color: @mt_tint;
}
.mt-footer menubutton.mt-stat-button > button {
  min-height: 20px;
  padding: 0 6px;
  margin: 1px 0;
  font-weight: normal;
}
progressbar.mt-meter > trough {
  min-width: 46px;
  min-height: 4px;
  border-radius: 2px;
  background-color: @mt_tint;
}
progressbar.mt-meter > trough > progress {
  min-height: 4px;
  border-radius: 2px;
  background-color: @mt_green;
}
progressbar.mt-meter.l-warn > trough > progress { background-color: @mt_amber; }
progressbar.mt-meter.l-crit > trough > progress { background-color: @mt_red; }
progressbar.mt-meter-big > trough,
progressbar.mt-meter-big > trough > progress {
  min-height: 7px;
  border-radius: 4px;
}
progressbar.mt-meter-big > trough { min-width: 260px; }
.mt-stat-panel {
  padding: 8px 6px;
}
.mt-stat-heading {
  font-weight: 700;
}
.mt-stat-big {
  font-weight: 700;
}

.mt-stats {
  padding: 18px 22px 16px 22px;
}
.mt-card {
  padding: 12px 14px;
  border-radius: 10px;
  background-color: @card_bg_color;
}
.mt-card-value {
  font-size: 1.9em;
  font-weight: 300;
}
.mt-delta {
  min-height: 18px;
  padding: 0 7px;
  border-radius: 6px;
  font-size: 0.8em;
  font-weight: 600;
  color: @mt_green;
  background-color: alpha(@mt_green, 0.13);
}
.mt-delta.down {
  color: @mt_soft;
  background-color: @mt_tint;
}
.mt-chart {
  color: @window_fg_color;
}
.mt-chart-out {
  color: @mt_blue;
}
.mt-stat-number {
  font-family: monospace;
  font-size: 0.88em;
}
progressbar.mt-meter-share > trough { min-width: 60px; }
progressbar.mt-meter-share > trough > progress { background-color: @mt_soft; }

.mt-pane-header {
  min-height: 28px;
  padding-right: 3px;
  background-color: @sidebar_bg_color;
  border-bottom: 1px solid @mt_border;
}
.mt-tab {
  padding: 0 4px 0 10px;
  border-right: 1px solid @mt_border;
  font-size: 0.86em;
  color: @mt_dim;
  transition: background-color 90ms ease-out, color 90ms ease-out;
}
.mt-tab:hover {
  color: @window_fg_color;
}
.mt-tab.active {
  color: @window_fg_color;
  background-color: @mt_deep;
}
.mt-pane.focused .mt-tab.active {
  box-shadow: inset 0 2px 0 @accent_color;
}
.mt-tab-empty {
  padding: 0 10px;
  font-size: 0.86em;
  color: @mt_dim;
}
.mt-pane.focused .mt-tab-empty {
  box-shadow: inset 0 2px 0 @accent_color;
}
.mt-icon-button {
  min-width: 22px;
  min-height: 22px;
  padding: 0;
  margin: 0 1px;
  border-radius: 4px;
  color: @mt_dim;
}
.mt-icon-button:hover {
  color: @window_fg_color;
}
.mt-icon-button.active {
  color: @accent_color;
}
.mt-tab-close {
  min-width: 18px;
  min-height: 18px;
  opacity: 0;
}
.mt-tab:hover .mt-tab-close,
.mt-tab.active .mt-tab-close {
  opacity: 1;
}
.mt-pane-placeholder {
  color: @mt_dim;
}
.mt-drop-hint {
  margin: 4px;
  border-radius: 6px;
  border: 1px solid rgba(122, 168, 255, 0.55);
  background-color: rgba(122, 168, 255, 0.14);
}
.mt-sidebar row.mt-drop-row {
  box-shadow: inset 0 2px 0 @accent_color;
}

.mt-kbd {
  padding: 1px 6px;
  border-radius: 4px;
  border: 1px solid @mt_border;
  background-color: @mt_tint;
  font-family: monospace;
  font-size: 0.8em;
  color: @mt_soft;
}
.mt-palette-entry {
  margin: 10px 10px 6px 10px;
  min-height: 34px;
}
.mt-palette-list {
  background: none;
  padding: 0 6px 8px 6px;
}
.mt-palette-list row {
  border-radius: 6px;
  padding: 6px 10px;
}
.mt-palette-list row:selected {
  background-color: rgba(122, 168, 255, 0.16);
}
.mt-palette-hint {
  font-size: 0.82em;
  color: @mt_dim;
}
.mt-filter > button {
  min-height: 24px;
  padding: 0 8px;
  font-size: 0.86em;
}

.mt-terminal {
  padding: 6px 8px;
}
.mt-scheme-alarviterm-dark { background-color: #0b0c0e; }
.mt-scheme-alarviterm-light { background-color: #ffffff; }
.mt-scheme-solarized-dark { background-color: #002b36; }
.mt-scheme-gruvbox-dark { background-color: #282828; }

.mt-project {
  padding: 0 5px;
  border-radius: 4px;
  background-color: @mt_tint;
  font-size: 0.74em;
  font-weight: 600;
  color: @mt_soft;
}
.mt-project.c-blue { color: #4c8dff; }
.mt-project.c-green { color: #3fb950; }
.mt-project.c-amber { color: #d29922; }
.mt-project.c-red { color: #f0554c; }
.mt-project.c-purple { color: #bc8cff; }
.mt-project.c-cyan { color: #39c5cf; }
.mt-project.c-pink { color: #f778ba; }
.mt-plugin-badge {
  padding: 0 5px;
  border-radius: 4px;
  background-color: @mt_tint;
  font-size: 0.74em;
  font-weight: 600;
  color: @mt_soft;
}
.mt-plugin-badge.t-ok { color: #3fb950; }
.mt-plugin-badge.t-warn { color: #d29922; }
.mt-plugin-badge.t-muted { color: @mt_dim; }
.mt-plugin-timer {
  font-family: monospace;
  font-size: 0.8em;
  color: #3fb950;
}
button.mt-plugin-button {
  min-height: 20px;
  min-width: 20px;
  padding: 1px 3px;
  font-size: 0.8em;
}
button.mt-plugin-button image { -gtk-icon-size: 13px; }
.mt-archived .mt-session-name {
  color: @mt_dim;
  font-style: italic;
}
";

thread_local! {
    /// Proveedor de estilos de la aplicación. Se recarga al cambiar de tema.
    static PROVIDER: gtk::CssProvider = gtk::CssProvider::new();
}

/// Registra la hoja de estilos. Se llama una vez, antes de crear la ventana.
pub fn install() {
    let Some(display) = gdk::Display::default() else {
        log::error!("no hay pantalla disponible para aplicar el tema");
        return;
    };
    PROVIDER.with(|provider| {
        gtk::style_context_add_provider_for_display(
            &display,
            provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    });
    apply_theme(Theme::Dark);
}

/// Si la interfaz se está mostrando en oscuro.
pub fn is_dark() -> bool {
    adw::StyleManager::default().is_dark()
}

/// Cambia el tema de la interfaz. Con el del sistema, hay que volver a
/// llamarla cuando el escritorio cambie entre claro y oscuro.
pub fn apply_theme(theme: Theme) {
    let manager = adw::StyleManager::default();
    manager.set_color_scheme(match theme {
        Theme::Dark => adw::ColorScheme::ForceDark,
        Theme::Light => adw::ColorScheme::ForceLight,
        Theme::System => adw::ColorScheme::Default,
    });
    reload_colors();
}

/// Recarga los colores según la interfaz esté en claro o en oscuro.
pub fn reload_colors() {
    let colors = if is_dark() { DARK } else { LIGHT };
    PROVIDER.with(|provider| provider.load_from_string(&format!("{colors}{CSS}")));
}

pub struct TerminalColors {
    pub foreground: gdk::RGBA,
    pub background: gdk::RGBA,
    pub palette: [gdk::RGBA; 16],
    /// Clase CSS que pinta el margen del terminal del mismo color de fondo.
    pub class: &'static str,
}

fn rgb(hex: u32) -> gdk::RGBA {
    let channel = |shift: u32| ((hex >> shift) & 0xff) as f32 / 255.0;
    gdk::RGBA::new(channel(16), channel(8), channel(0), 1.0)
}

/// Clases de los esquemas, para poder quitarlas al cambiar de uno a otro.
pub const SCHEME_CLASSES: [&str; 4] = [
    "mt-scheme-alarviterm-dark",
    "mt-scheme-alarviterm-light",
    "mt-scheme-solarized-dark",
    "mt-scheme-gruvbox-dark",
];

/// Colores de un esquema de [`alarviterm_core::settings::SCHEMES`]. El esquema
/// automático, o uno desconocido, sigue al tema de la interfaz.
pub fn terminal_colors(scheme: &str) -> TerminalColors {
    let (class, foreground, background, palette): (&'static str, u32, u32, [u32; 16]) = match scheme {
        "alarviterm-light" => ALARVITERM_LIGHT,
        "solarized-dark" => SOLARIZED_DARK,
        "gruvbox-dark" => GRUVBOX_DARK,
        "alarviterm-dark" => ALARVITERM_DARK,
        _ if is_dark() => ALARVITERM_DARK,
        _ => ALARVITERM_LIGHT,
    };
    TerminalColors {
        foreground: rgb(foreground),
        background: rgb(background),
        palette: palette.map(rgb),
        class,
    }
}

type Scheme = (&'static str, u32, u32, [u32; 16]);

const ALARVITERM_DARK: Scheme = (
    "mt-scheme-alarviterm-dark",
    0xd4d7dd,
    0x0b0c0e,
    [
        0x1c1e22, 0xf0554c, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39c5cf, 0xb1bac4, 0x6e7681,
        0xff7b72, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d4dd, 0xf0f6fc,
    ],
);

const ALARVITERM_LIGHT: Scheme = (
    "mt-scheme-alarviterm-light",
    0x1f2328,
    0xffffff,
    [
        0x24292f, 0xcf222e, 0x116329, 0x4d2d00, 0x0969da, 0x8250df, 0x1b7c83, 0x6e7781, 0x57606a,
        0xa40e26, 0x1a7f37, 0x633c01, 0x218bff, 0xa475f9, 0x3192aa, 0x8c959f,
    ],
);

const SOLARIZED_DARK: Scheme = (
    "mt-scheme-solarized-dark",
    0x839496,
    0x002b36,
    [
        0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5, 0x586e75,
        0xcb4b16, 0x586e75, 0x657b83, 0x839496, 0x6c71c4, 0x93a1a1, 0xfdf6e3,
    ],
);

const GRUVBOX_DARK: Scheme = (
    "mt-scheme-gruvbox-dark",
    0xebdbb2,
    0x282828,
    [
        0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984, 0x928374,
        0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2,
    ],
);
