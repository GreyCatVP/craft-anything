//! Keep measured message heights so offscreen prose needs no text layout.

use egui::{Id, Rect, Ui, Vec2};

#[derive(Clone, Copy)]
struct Measured {
    revision: Id,
    size: Vec2,
}

/// Measure new or changed prose once; retain its space when it scrolls out of view.
pub(crate) fn show(ui: &mut Ui, text: &str, glossary: bool, draw: impl FnOnce(&mut Ui)) {
    let id = ui.next_auto_id().with("message-height");
    let style = ui.style();
    let revision = Id::new((
        text,
        glossary,
        ui.available_width().to_bits(),
        ui.ctx().pixels_per_point().to_bits(),
        &style.text_styles,
        style.spacing.item_spacing.x.to_bits(),
        style.spacing.item_spacing.y.to_bits(),
    ));
    if let Some(measured) = ui.data(|d| d.get_temp::<Measured>(id)).filter(|m| m.revision == revision)
        && !ui.is_rect_visible(Rect::from_min_size(ui.next_widget_position(), measured.size))
    {
        ui.allocate_space(measured.size);
        return;
    }
    let size = ui.scope(draw).response.rect.size();
    ui.data_mut(|d| d.insert_temp(id, Measured { revision, size }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_history_only_lays_out_visible_messages_after_measurement() {
        let ctx = egui::Context::default();
        let mut draws = 0;
        let mut frame = || {
            let mut height = 0.0;
            let mut output =
                ctx.run_ui(egui::RawInput { screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))), ..Default::default() }, |ui| {
                    height = egui::ScrollArea::vertical()
                        .max_height(200.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            for i in 0..1_000 {
                                ui.push_id(i, |ui| {
                                    show(ui, "An unchanged message", false, |ui| {
                                        draws += 1;
                                        ui.label("An unchanged message");
                                    })
                                });
                            }
                        })
                        .content_size
                        .y;
                });
            output.textures_delta.clear();
            (height, draws)
        };
        let (first_height, _) = frame();
        let (_, before) = frame();
        let (height, after) = frame();
        assert!((height - first_height).abs() < 0.5, "scroll extent is preserved");
        assert!((1..50).contains(&(after - before)), "only visible rows are laid out, got {}", after - before);
    }

    #[test]
    fn offscreen_prose_keeps_its_height_and_remeasures_content_width_and_fonts() {
        let ctx = egui::Context::default();
        let mut draws = 0;
        let mut frame = |text: &str, width: f32, font_size: f32, visible: bool| {
            let mut height = 0.0;
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                ui.set_width(width);
                ui.style_mut().text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(font_size));
                ui.set_clip_rect(if visible { Rect::EVERYTHING } else { Rect::NOTHING });
                let before = ui.next_widget_position().y;
                show(ui, text, false, |ui| {
                    draws += 1;
                    ui.add(egui::Label::new(text).wrap());
                });
                height = ui.next_widget_position().y - before;
            });
            output.textures_delta.clear();
            height
        };
        let first = frame("A message", 300.0, 14.0, false);
        assert_eq!(frame("A message", 300.0, 14.0, false), first);
        frame("New words", 300.0, 14.0, false);
        frame("New words", 200.0, 14.0, false);
        frame("New words", 200.0, 18.0, false);
        frame("New words", 200.0, 18.0, true);
        assert_eq!(draws, 5, "cached offscreen text is skipped, every layout change and visible message is drawn");
    }
}
