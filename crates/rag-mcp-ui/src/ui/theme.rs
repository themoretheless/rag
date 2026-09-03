//! Shared native design tokens and egui styling.
//!
//! The palette mirrors the browser console so both clients communicate the
//! same L0-L4 storage model without coupling either frontend to the other.

use egui::{Color32, CornerRadius, FontFamily, FontId, Frame, Margin, Stroke, TextStyle};

pub const BG: Color32 = Color32::from_rgb(14, 17, 23);
pub const RAIL: Color32 = Color32::from_rgb(9, 11, 16);
pub const SURFACE: Color32 = Color32::from_rgb(20, 24, 33);
pub const SURFACE_2: Color32 = Color32::from_rgb(25, 30, 41);
pub const HOVER: Color32 = Color32::from_rgb(31, 36, 47);
pub const ACTIVE: Color32 = Color32::from_rgb(24, 55, 86);
pub const TEXT: Color32 = Color32::from_rgb(227, 226, 222);
pub const MUTED: Color32 = Color32::from_rgb(155, 154, 151);
pub const FAINT: Color32 = Color32::from_rgb(132, 131, 126);
pub const BORDER: Color32 = Color32::from_rgb(43, 47, 56);
pub const BORDER_STRONG: Color32 = Color32::from_rgb(62, 67, 78);
pub const ACCENT: Color32 = Color32::from_rgb(82, 156, 202);
pub const DANGER: Color32 = Color32::from_rgb(255, 115, 105);
pub const OK: Color32 = Color32::from_rgb(77, 171, 154);
pub const WARN: Color32 = Color32::from_rgb(240, 182, 74);

pub const L0: Color32 = Color32::from_rgb(110, 168, 255);
pub const L1: Color32 = Color32::from_rgb(45, 212, 191);
pub const L2: Color32 = Color32::from_rgb(240, 182, 74);
pub const L3: Color32 = Color32::from_rgb(167, 139, 250);
pub const L4: Color32 = Color32::from_rgb(244, 114, 182);

pub fn install(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::Dark);
    let mut style = (*ctx.style_of(egui::Theme::Dark)).clone();
    {
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(11.0, 6.0);
        style.spacing.interact_size.y = 30.0;
        style.spacing.scroll.bar_width = 8.0;
        style.spacing.scroll.floating_width = 6.0;
        style.spacing.scroll.bar_outer_margin = 2.0;
        style.text_styles.insert(
            TextStyle::Heading,
            FontId::new(22.0, FontFamily::Proportional),
        );
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
        style.text_styles.insert(
            TextStyle::Button,
            FontId::new(13.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        );
        style.text_styles.insert(
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        );

        let visuals = &mut style.visuals;
        visuals.dark_mode = true;
        visuals.override_text_color = Some(TEXT);
        visuals.weak_text_color = Some(MUTED);
        visuals.panel_fill = BG;
        visuals.window_fill = SURFACE;
        visuals.window_stroke = Stroke::new(1.0, BORDER_STRONG);
        visuals.window_corner_radius = CornerRadius::same(10);
        visuals.menu_corner_radius = CornerRadius::same(8);
        visuals.faint_bg_color = SURFACE_2;
        visuals.extreme_bg_color = RAIL;
        visuals.text_edit_bg_color = Some(SURFACE_2);
        visuals.code_bg_color = RAIL;
        visuals.hyperlink_color = L0;
        visuals.warn_fg_color = WARN;
        visuals.error_fg_color = DANGER;
        visuals.selection.bg_fill = ACTIVE;
        visuals.selection.stroke = Stroke::new(1.0, ACCENT);
        visuals.striped = true;

        let noninteractive = &mut visuals.widgets.noninteractive;
        noninteractive.bg_fill = SURFACE;
        noninteractive.weak_bg_fill = SURFACE;
        noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        noninteractive.corner_radius = CornerRadius::same(8);
        noninteractive.fg_stroke = Stroke::new(1.0, TEXT);

        let inactive = &mut visuals.widgets.inactive;
        inactive.bg_fill = SURFACE_2;
        inactive.weak_bg_fill = SURFACE_2;
        inactive.bg_stroke = Stroke::new(1.0, BORDER);
        inactive.corner_radius = CornerRadius::same(7);
        inactive.fg_stroke = Stroke::new(1.0, MUTED);

        let hovered = &mut visuals.widgets.hovered;
        hovered.bg_fill = HOVER;
        hovered.weak_bg_fill = HOVER;
        hovered.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
        hovered.corner_radius = CornerRadius::same(7);
        hovered.fg_stroke = Stroke::new(1.0, TEXT);

        let active = &mut visuals.widgets.active;
        active.bg_fill = ACTIVE;
        active.weak_bg_fill = ACTIVE;
        active.bg_stroke = Stroke::new(1.0, ACCENT);
        active.corner_radius = CornerRadius::same(7);
        active.fg_stroke = Stroke::new(1.0, TEXT);

        visuals.widgets.open = visuals.widgets.active;
    }
    ctx.set_style_of(egui::Theme::Dark, style);
}

pub fn rail_frame() -> Frame {
    Frame::new()
        .fill(RAIL)
        .stroke(Stroke::new(1.0, BORDER))
        .inner_margin(Margin::symmetric(4, 8))
}

pub fn topbar_frame() -> Frame {
    Frame::new()
        .fill(BG)
        .stroke(Stroke::new(1.0, BORDER))
        .inner_margin(Margin::symmetric(14, 8))
}

pub fn toolbar_frame() -> Frame {
    Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .inner_margin(Margin::symmetric(14, 6))
}

pub fn card() -> Frame {
    Frame::new()
        .fill(SURFACE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
}

pub fn inset() -> Frame {
    Frame::new()
        .fill(SURFACE_2)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(10))
}

pub fn layer_color(layer: &str) -> Color32 {
    match layer.to_ascii_lowercase().as_str() {
        "l0" | "raw" | "document" | "code" => L0,
        "l1" | "chunk" | "retrieval" | "search" => L1,
        "l2" | "graph" | "entity" => L2,
        "l3" | "wiki" | "knowledge" => L3,
        "l4" | "agent" | "activity" | "diary" => L4,
        _ => MUTED,
    }
}

pub fn rgba(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_layers_share_the_browser_palette_contract() {
        assert_eq!(layer_color("raw"), L0);
        assert_eq!(layer_color("search"), L1);
        assert_eq!(layer_color("graph"), L2);
        assert_eq!(layer_color("wiki"), L3);
        assert_eq!(layer_color("diary"), L4);
        assert_eq!(layer_color("activity"), L4);
    }
}
