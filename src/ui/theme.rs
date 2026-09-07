use eframe::egui::{self, Color32, FontId, RichText, Stroke, Vec2};

pub const BG: Color32 = Color32::from_rgb(8, 13, 22);
pub const PANEL: Color32 = Color32::from_rgb(13, 21, 34);
pub const CARD: Color32 = Color32::from_rgb(18, 29, 45);
pub const BORDER: Color32 = Color32::from_rgb(35, 51, 70);
pub const MUTED: Color32 = Color32::from_rgb(133, 154, 176);
pub const TEXT: Color32 = Color32::from_rgb(225, 234, 244);
pub const CYAN: Color32 = Color32::from_rgb(53, 219, 239);
pub const AMBER: Color32 = Color32::from_rgb(255, 207, 123);
pub const GREEN: Color32 = Color32::from_rgb(93, 224, 165);
pub const RED: Color32 = Color32::from_rgb(255, 108, 126);

pub fn install(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = BG;
    v.faint_bg_color = CARD;
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = Color32::from_rgb(21, 76, 92);
    v.selection.stroke = Stroke::new(1.0_f32, CYAN);
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    for widgets in [&mut v.widgets.inactive, &mut v.widgets.noninteractive] {
        widgets.bg_fill = CARD;
        widgets.weak_bg_fill = CARD;
        widgets.bg_stroke = Stroke::new(1.0_f32, BORDER);
        widgets.fg_stroke = Stroke::new(1.0_f32, MUTED);
        widgets.corner_radius = egui::CornerRadius::same(5);
    }
    v.widgets.hovered.bg_fill = Color32::from_rgb(29, 51, 69);
    v.widgets.hovered.weak_bg_fill = v.widgets.hovered.bg_fill;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, CYAN);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT);
    v.widgets.active.bg_fill = Color32::from_rgb(24, 73, 88);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, CYAN);
    v.widgets.active.corner_radius = egui::CornerRadius::same(5);
    v.widgets.hovered.corner_radius = egui::CornerRadius::same(5);
    v.handle_shape = egui::style::HandleShape::Rect { aspect_ratio: 0.22 };
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 7.0);
    style.spacing.interact_size.y = 30.0;
    style.spacing.slider_width = 120.0;
    style.spacing.slider_rail_height = 3.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(11.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, FontId::proportional(18.0));
    ctx.set_style(style);
}

pub fn chip(ui: &mut egui::Ui, label: &str, active: bool) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(label).size(11.0).strong().color(if active {
            CYAN
        } else {
            MUTED
        }))
        .selected(active)
        .fill(if active {
            Color32::from_rgb(16, 54, 68)
        } else {
            CARD
        })
        .stroke(Stroke::new(
            1.0_f32,
            if active {
                Color32::from_rgb(31, 121, 141)
            } else {
                BORDER
            },
        ))
        .min_size(Vec2::new(42.0, 28.0)),
    )
}

// Un vrai bouton egui : focus clavier, activation Espace/Entrée et état sélectionné.
pub fn toggle(ui: &mut egui::Ui, value: &mut bool, label: &str, tip: &str) -> bool {
    let response = chip(ui, label, *value).on_hover_text(tip);
    if response.clicked() {
        *value = !*value;
        true
    } else {
        false
    }
}

pub fn caption(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(RichText::new(text).color(MUTED).size(10.0).strong());
}

pub fn card() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(8)
        .inner_margin(12)
}

pub fn waterfall_color(t: f32) -> Color32 {
    let stops = [
        (0.0, [8.0, 13.0, 22.0]),
        (0.22, [17.0, 29.0, 63.0]),
        (0.48, [30.0, 85.0, 149.0]),
        (0.74, [46.0, 204.0, 220.0]),
        (1.0, [255.0, 207.0, 123.0]),
    ];
    for pair in stops.windows(2) {
        if t <= pair[1].0 {
            let blend = ((t - pair[0].0) / (pair[1].0 - pair[0].0)).clamp(0.0, 1.0);
            let rgb: [u8; 3] = std::array::from_fn(|i| {
                (pair[0].1[i] + (pair[1].1[i] - pair[0].1[i]) * blend) as u8
            });
            return Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        }
    }
    AMBER
}
