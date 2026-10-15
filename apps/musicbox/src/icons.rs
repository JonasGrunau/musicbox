//! musicbox's icons, drawn with the painter (no image or font assets). Each icon is designed on a
//! 16 × 16 grid and scales with the rectangle it is painted into.

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, vec2};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Collection,
    Playlist,
    Folder,
    Smart,
    History,
    Tag,
    Device,
    Xml,
    Play,
    Pause,
    Search,
    Lock,
    Unlock,
    ChevronRight,
    ChevronDown,
    Close,
    Logo,
}

/// Allocates a `size` square and paints `icon` into it.
pub fn show(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    paint(ui.painter(), rect, icon, color);
    response
}

pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = rect.width().min(rect.height()) / 16.0;
    let o = rect.center() - vec2(8.0 * s, 8.0 * s);
    // A point on the 16 × 16 design grid.
    let p = |x: f32, y: f32| -> Pos2 { o + vec2(x * s, y * s) };
    let line = Stroke::new((1.4 * s).max(1.0), color);
    let seg = |a: Pos2, b: Pos2| painter.line_segment([a, b], line);
    let poly = |points: Vec<Pos2>| {
        painter.add(Shape::line(points, line));
    };
    let closed = |points: Vec<Pos2>| {
        painter.add(Shape::closed_line(points, line));
    };
    let fill = |points: Vec<Pos2>| {
        painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
    };
    match icon {
        Icon::Collection => {
            // A record: outer ring, groove, label.
            painter.circle_stroke(p(8.0, 8.0), 6.2 * s, line);
            painter.circle_stroke(p(8.0, 8.0), 3.6 * s, Stroke::new(line.width * 0.7, color.gamma_multiply(0.6)));
            painter.circle_filled(p(8.0, 8.0), 1.4 * s, color);
        }
        Icon::Playlist => {
            for (i, w) in [(0.0, 10.0), (1.0, 10.0), (2.0, 6.0)] {
                let y = 4.0 + i * 4.0;
                painter.circle_filled(p(2.6, y), 0.95 * s, color);
                seg(p(5.0, y), p(5.0 + w, y));
            }
        }
        Icon::Folder => {
            closed(vec![p(1.5, 3.5), p(6.0, 3.5), p(7.5, 5.0), p(14.5, 5.0), p(14.5, 12.5), p(1.5, 12.5)]);
        }
        Icon::Smart => {
            // A funnel: lists filtered by rules.
            closed(vec![p(2.0, 3.0), p(14.0, 3.0), p(9.5, 8.5), p(9.5, 13.5), p(6.5, 12.0), p(6.5, 8.5)]);
        }
        Icon::History => {
            painter.circle_stroke(p(8.0, 8.0), 6.0 * s, line);
            poly(vec![p(8.0, 4.5), p(8.0, 8.0), p(10.8, 9.8)]);
        }
        Icon::Tag => {
            closed(vec![p(2.0, 2.5), p(8.5, 2.5), p(14.0, 8.0), p(8.5, 13.5), p(2.0, 7.0)]);
            painter.circle_filled(p(5.2, 5.6), 1.1 * s, color);
        }
        Icon::Device => {
            // A USB stick lying down: body on the left, connector on the right.
            painter.rect_stroke(Rect::from_min_max(p(1.0, 4.5), p(11.0, 11.5)), 1.5 * s, line, egui::StrokeKind::Middle);
            closed(vec![p(11.0, 6.0), p(15.0, 6.0), p(15.0, 10.0), p(11.0, 10.0)]);
            painter.circle_filled(p(4.5, 8.0), 1.1 * s, color);
        }
        Icon::Xml => {
            closed(vec![p(3.0, 1.5), p(10.0, 1.5), p(13.0, 4.5), p(13.0, 14.5), p(3.0, 14.5)]);
            poly(vec![p(10.0, 1.5), p(10.0, 4.5), p(13.0, 4.5)]);
            seg(p(5.5, 8.0), p(10.5, 8.0));
            seg(p(5.5, 11.0), p(9.0, 11.0));
        }
        Icon::Play => fill(vec![p(4.0, 2.0), p(14.0, 8.0), p(4.0, 14.0)]),
        Icon::Pause => {
            painter.rect_filled(Rect::from_min_max(p(3.5, 2.5), p(6.5, 13.5)), 0.8 * s, color);
            painter.rect_filled(Rect::from_min_max(p(9.5, 2.5), p(12.5, 13.5)), 0.8 * s, color);
        }
        Icon::Search => {
            painter.circle_stroke(p(6.8, 6.8), 4.6 * s, line);
            seg(p(10.2, 10.2), p(14.0, 14.0));
        }
        Icon::Lock | Icon::Unlock => {
            painter.rect_filled(Rect::from_min_max(p(3.0, 7.0), p(13.0, 14.5)), 1.5 * s, color);
            let shackle = if icon == Icon::Lock {
                vec![p(5.2, 7.0), p(5.2, 4.8), p(6.2, 3.0), p(8.0, 2.3), p(9.8, 3.0), p(10.8, 4.8), p(10.8, 7.0)]
            } else {
                vec![p(5.2, 7.0), p(5.2, 4.8), p(6.2, 3.0), p(8.0, 2.3), p(9.8, 3.0), p(10.8, 4.8)]
            };
            poly(shackle);
        }
        Icon::ChevronRight => {
            poly(vec![p(6.0, 4.0), p(10.0, 8.0), p(6.0, 12.0)]);
        }
        Icon::ChevronDown => {
            poly(vec![p(4.0, 6.0), p(8.0, 10.0), p(12.0, 6.0)]);
        }
        Icon::Close => {
            seg(p(4.5, 4.5), p(11.5, 11.5));
            seg(p(11.5, 4.5), p(4.5, 11.5));
        }
        Icon::Logo => {
            // A black rounded box with a fine edge, holding three level bars in `color` (white):
            // the same mark as the app icon (`app_icon.rs`).
            painter.rect_filled(rect, 4.0 * s, Color32::from_rgb(0x0B, 0x0B, 0x0B));
            painter.rect_stroke(rect, 4.0 * s, Stroke::new(1.0, Color32::from_rgb(0x48, 0x48, 0x48)), egui::StrokeKind::Inside);
            let bar = |x: f32, h: f32| {
                painter.rect_filled(Rect::from_min_max(p(x, 8.0 - h), p(x + 2.0, 8.0 + h)), 1.0 * s, color);
            };
            bar(4.0, 2.0);
            bar(7.0, 4.5);
            bar(10.0, 3.0);
        }
    }
}

/// A frameless square button showing an icon; hover and press tint its background.
pub fn button(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label));
    let visuals = ui.style().interact(&response);
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(rect, crate::theme::RADIUS, visuals.weak_bg_fill);
    }
    let inner = rect.shrink(size * 0.22);
    paint(ui.painter(), inner, icon, if response.hovered() { crate::theme::TEXT_STRONG } else { color });
    response
}
