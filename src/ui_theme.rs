//! Dark EMEET-style chrome and resolution-independent camera controls.
use eframe::egui::{
    self, Color32, FontId, Response, Sense, Shape, Stroke, StrokeKind, Ui, WidgetInfo, WidgetType,
};
pub const BG: Color32 = Color32::from_rgb(17, 17, 18);
pub const PANEL: Color32 = Color32::from_rgb(30, 31, 33);
pub const CONTROL: Color32 = Color32::from_rgb(39, 40, 42);
pub const BLUE: Color32 = Color32::from_rgb(0, 173, 245);
pub const TEXT: Color32 = Color32::from_rgb(237, 238, 240);
pub const MUTED: Color32 = Color32::from_rgb(158, 163, 173);

pub fn install(ctx: &egui::Context) {
    if let Ok(bytes) = std::fs::read("/System/Library/Fonts/SFNS.ttf") {
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "macOS San Francisco".into(),
            egui::FontData::from_owned(bytes).into(),
        );
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "macOS San Francisco".into());
        ctx.set_fonts(fonts);
    }

    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = PANEL;
    style.visuals.extreme_bg_color = BG;
    style.visuals.faint_bg_color = CONTROL;
    style.visuals.code_bg_color = CONTROL;
    style.visuals.window_stroke = Stroke::new(1.0_f32, CONTROL);
    style.visuals.window_corner_radius = 10.into();
    style.visuals.menu_corner_radius = 6.into();
    style.visuals.selection.bg_fill = Color32::from_rgb(0, 92, 136);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, BLUE);
    style.visuals.hyperlink_color = BLUE;
    style.visuals.slider_trailing_fill = false;
    style.spacing.slider_rail_height = 2.0;
    style.visuals.widgets.noninteractive.bg_fill = CONTROL;
    style.visuals.widgets.noninteractive.weak_bg_fill = CONTROL;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    style.visuals.widgets.noninteractive.corner_radius = 5.into();
    style.visuals.widgets.inactive.bg_fill = CONTROL;
    style.visuals.widgets.inactive.weak_bg_fill = CONTROL;
    style.visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, TEXT);
    style.visuals.widgets.inactive.corner_radius = 5.into();
    style.visuals.widgets.hovered.bg_fill = Color32::from_rgb(49, 51, 54);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(49, 51, 54);
    style.visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    style.visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, TEXT);
    style.visuals.widgets.hovered.corner_radius = 5.into();
    style.visuals.widgets.active.bg_fill = BLUE;
    style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(26, 49, 62);
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, BLUE);
    style.visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, BLUE);
    style.visuals.widgets.active.corner_radius = 5.into();
    style.spacing.item_spacing = egui::vec2(8.0, 10.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.interact_size = egui::vec2(32.0, 32.0);
    style.spacing.slider_width = 180.0;
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, FontId::proportional(16.0));
    ctx.set_style(style);
}

/// `size` is the icon artwork size; its hit target includes six points per side.
/// Every icon is a real button with a tooltip and an accessibility label.
pub fn icon(ui: &mut Ui, name: &str, size: f32, tooltip: &str) -> Response {
    let size = size.max(8.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(size + 12.0, size + 12.0), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), tooltip));
    if ui.is_rect_visible(rect) {
        if response.hovered() || response.has_focus() || response.is_pointer_button_down_on() {
            ui.painter().rect_filled(rect, 5, CONTROL);
        }
        let color = if !ui.is_enabled() {
            MUTED.gamma_multiply(0.45)
        } else if response.is_pointer_button_down_on() {
            BLUE
        } else {
            ui.visuals().text_color()
        };
        paint_icon(ui, rect.shrink(6.0), name, color);
    }
    response.on_hover_text(tooltip)
}

fn paint_icon(ui: &Ui, rect: egui::Rect, name: &str, color: Color32) {
    let painter = ui.painter();
    let p = |x: f32, y: f32| {
        egui::pos2(
            rect.left() + x * rect.width(),
            rect.top() + y * rect.height(),
        )
    };
    let stroke = Stroke::new((rect.width() / 13.0).clamp(1.2, 2.0), color);
    let line = |a: (f32, f32), b: (f32, f32)| {
        painter.line_segment([p(a.0, a.1), p(b.0, b.1)], stroke);
    };
    let path = |points: &[(f32, f32)]| {
        painter.add(Shape::line(
            points.iter().map(|&(x, y)| p(x, y)).collect(),
            stroke,
        ));
    };
    let circle = |x: f32, y: f32, r: f32| {
        painter.circle_stroke(p(x, y), r * rect.width(), stroke);
    };
    let box_outline = |x: f32, y: f32, w: f32, h: f32| {
        painter.rect_stroke(
            egui::Rect::from_min_max(p(x, y), p(x + w, y + h)),
            2,
            stroke,
            StrokeKind::Inside,
        );
    };
    let arc = |x: f32, y: f32, r: f32, start: f32, end: f32| {
        painter.add(Shape::line(
            (0..=20)
                .map(|i| {
                    let a = start + (end - start) * i as f32 / 20.0;
                    p(x + r * a.cos(), y + r * a.sin())
                })
                .collect(),
            stroke,
        ));
    };
    match name {
        "camera" => {
            box_outline(0.1, 0.28, 0.8, 0.58);
            path(&[(0.28, 0.28), (0.36, 0.14), (0.64, 0.14), (0.72, 0.28)]);
            circle(0.5, 0.57, 0.19);
        }
        "record" => {
            circle(0.5, 0.5, 0.39);
            painter.circle_filled(
                p(0.5, 0.5),
                rect.width() * 0.28,
                Color32::from_rgb(231, 68, 73),
            );
        }
        "stop" => {
            painter.rect_filled(
                egui::Rect::from_min_max(p(0.23, 0.23), p(0.77, 0.77)),
                2,
                color,
            );
        }
        "rotate" | "refresh" => {
            arc(0.5, 0.52, 0.34, -1.8, 3.6);
            path(&[(0.68, 0.11), (0.61, 0.28), (0.81, 0.27)]);
        }
        "mirror" => {
            line((0.5, 0.08), (0.5, 0.92));
            path(&[(0.12, 0.8), (0.37, 0.2), (0.37, 0.8), (0.12, 0.8)]);
            painter.add(Shape::convex_polygon(
                vec![p(0.63, 0.2), p(0.88, 0.8), p(0.63, 0.8)],
                color,
                Stroke::NONE,
            ));
        }
        "flip" => {
            line((0.08, 0.5), (0.92, 0.5));
            path(&[(0.2, 0.12), (0.8, 0.37), (0.2, 0.37), (0.2, 0.12)]);
            painter.add(Shape::convex_polygon(
                vec![p(0.2, 0.63), p(0.8, 0.63), p(0.2, 0.88)],
                color,
                Stroke::NONE,
            ));
        }
        "crop" => {
            path(&[(0.12, 0.24), (0.76, 0.24), (0.76, 0.9)]);
            path(&[(0.25, 0.1), (0.25, 0.76), (0.9, 0.76)]);
        }
        "eye" => {
            path(&[
                (0.07, 0.5),
                (0.2, 0.32),
                (0.36, 0.23),
                (0.5, 0.2),
                (0.64, 0.23),
                (0.8, 0.32),
                (0.93, 0.5),
                (0.8, 0.68),
                (0.64, 0.77),
                (0.5, 0.8),
                (0.36, 0.77),
                (0.2, 0.68),
                (0.07, 0.5),
            ]);
            circle(0.5, 0.5, 0.17);
        }
        "fullscreen" => {
            for (x, y, sx, sy) in [
                (0.12, 0.12, 1.0, 1.0),
                (0.88, 0.12, -1.0, 1.0),
                (0.12, 0.88, 1.0, -1.0),
                (0.88, 0.88, -1.0, -1.0),
            ] {
                path(&[(x + sx * 0.23, y), (x, y), (x, y + sy * 0.23)]);
            }
        }
        "settings" => {
            circle(0.5, 0.5, 0.26);
            circle(0.5, 0.5, 0.1);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                line(
                    (0.5 + 0.28 * a.cos(), 0.5 + 0.28 * a.sin()),
                    (0.5 + 0.4 * a.cos(), 0.5 + 0.4 * a.sin()),
                );
            }
        }
        "device" => {
            box_outline(0.16, 0.16, 0.68, 0.47);
            circle(0.5, 0.39, 0.14);
            line((0.5, 0.64), (0.5, 0.84));
            line((0.3, 0.86), (0.7, 0.86));
        }
        "view" => {
            circle(0.5, 0.5, 0.39);
            painter.add(Shape::convex_polygon(
                vec![p(0.42, 0.29), p(0.7, 0.5), p(0.42, 0.71)],
                color,
                Stroke::NONE,
            ));
        }
        "effects" => {
            path(&[
                (0.47, 0.08),
                (0.58, 0.33),
                (0.86, 0.28),
                (0.71, 0.52),
                (0.89, 0.75),
                (0.61, 0.71),
                (0.48, 0.95),
                (0.38, 0.68),
                (0.11, 0.73),
                (0.27, 0.5),
                (0.1, 0.29),
                (0.37, 0.34),
                (0.47, 0.08),
            ]);
        }
        "assist" => {
            box_outline(0.12, 0.12, 0.28, 0.76);
            box_outline(0.52, 0.12, 0.36, 0.31);
            box_outline(0.52, 0.56, 0.36, 0.32);
        }
        "audio" => {
            path(&[
                (0.1, 0.37),
                (0.3, 0.37),
                (0.51, 0.17),
                (0.51, 0.83),
                (0.3, 0.63),
                (0.1, 0.63),
                (0.1, 0.37),
            ]);
            arc(0.48, 0.5, 0.27, -0.85, 0.85);
            arc(0.48, 0.5, 0.43, -0.85, 0.85);
        }
        "vcam" => {
            circle(0.5, 0.35, 0.25);
            circle(0.5, 0.35, 0.1);
            path(&[
                (0.27, 0.64),
                (0.73, 0.64),
                (0.64, 0.84),
                (0.36, 0.84),
                (0.27, 0.64),
            ]);
        }
        "plus" => {
            line((0.5, 0.15), (0.5, 0.85));
            line((0.15, 0.5), (0.85, 0.5));
        }
        "chevron" => {
            path(&[(0.36, 0.2), (0.65, 0.5), (0.36, 0.8)]);
        }
        _ => {
            circle(0.5, 0.5, 0.33);
        }
    }
}

pub fn tab(ui: &mut Ui, label: &str, selected: bool, width: f32) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width.max(24.0), 44.0), Sense::click());
    response.widget_info(|| {
        WidgetInfo::selected(
            WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    let color = if selected {
        BLUE
    } else if response.hovered() {
        TEXT
    } else {
        MUTED
    };
    let icon_name = match label {
        "Visão" => Some("view"),
        "Efeitos" => Some("effects"),
        "Assistência" => Some("assist"),
        "Live" => Some("camera"),
        "Áudio" => Some("audio"),
        "V-Cam" => Some("vcam"),
        _ => None,
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(14.0), color);
    let artwork_width = if icon_name.is_some() { 20.0 } else { 0.0 };
    let left = rect.center().x - (galley.size().x + artwork_width) / 2.0;
    if let Some(name) = icon_name {
        let artwork = egui::Rect::from_min_size(
            egui::pos2(left, rect.center().y - 8.0),
            egui::vec2(16.0, 16.0),
        );
        paint_icon(ui, artwork, name, color);
    }
    ui.painter().galley(
        egui::pos2(
            left + artwork_width,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color,
    );
    if selected {
        let half = 6.0;
        ui.painter().line_segment(
            [
                egui::pos2(rect.center().x - half, rect.bottom() - 1.0),
                egui::pos2(rect.center().x + half, rect.bottom() - 1.0),
            ],
            Stroke::new(2.0_f32, BLUE),
        );
    }
    response
}

pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(title).size(15.0).color(TEXT));
    ui.add_space(8.0);
}
