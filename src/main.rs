mod app;
mod engine;

use app::DockerDesktopApp;
use eframe::egui::{self, FontFamily, FontId, TextStyle};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([1480.0, 920.0]),
        ..Default::default()
    };

    eframe::run_native(
        "Docker RS Desktop",
        options,
        Box::new(|cc| {
            configure_fonts(&cc.egui_ctx);
            configure_theme(&cc.egui_ctx);
            Ok(Box::new(DockerDesktopApp::new(cc)))
        }),
    )
}

fn configure_fonts(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 8.0);
    style.spacing.menu_margin = egui::Margin::same(10);
    style.spacing.indent = 18.0;
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(26.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(16.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(15.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(14.0, FontFamily::Monospace),
        ),
        (
            TextStyle::Small,
            FontId::new(13.0, FontFamily::Proportional),
        ),
    ]
    .into();
    ctx.set_style(style);
}

fn configure_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(16, 22, 32);
    visuals.window_fill = egui::Color32::from_rgb(13, 18, 28);
    visuals.extreme_bg_color = egui::Color32::from_rgb(9, 13, 21);
    visuals.faint_bg_color = egui::Color32::from_rgb(22, 31, 44);
    visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(18, 24, 36);
    visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(27, 37, 52);
    visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(34, 96, 196);
    visuals.widgets.active.bg_fill = egui::Color32::from_rgb(16, 124, 202);
    visuals.selection.bg_fill = egui::Color32::from_rgb(16, 124, 202);
    visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(221, 228, 237);
    visuals.widgets.hovered.fg_stroke.color = egui::Color32::from_rgb(245, 248, 252);
    visuals.widgets.active.fg_stroke.color = egui::Color32::from_rgb(250, 252, 255);
    visuals.widgets.noninteractive.fg_stroke.color = egui::Color32::from_rgb(194, 205, 217);
    visuals.override_text_color = Some(egui::Color32::from_rgb(227, 233, 240));
    ctx.set_visuals(visuals);
}
