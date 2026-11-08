//! The person's messages and the builder's answers are told apart at a glance: a solid bubble
//! for the person, plain text on the page for the builder.
use craft_anything_engine::Mode;
use craft_anything_ui_egui::state::{FeedItem, UserItem};
use craft_anything_ui_egui::theme::Tokens;
use craft_anything_ui_egui::{CraftApp, demo};
use egui::epaint::{RectShape, TextShape};
use egui::{Pos2, Shape};
use egui_kittest::Harness;

const ASKED: &str = "Make the header blue";
const ANSWERED: &str = "The header is blue now.";

/// Every shape painted, with nested lists flattened.
fn flat(shape: &Shape, out: &mut Vec<Shape>) {
    match shape {
        Shape::Vec(v) => v.iter().for_each(|s| flat(s, out)),
        s => out.push(s.clone()),
    }
}

fn painted(theme: &str) -> (Tokens, Vec<Shape>) {
    let theme = theme.to_string();
    let mut h = Harness::builder().with_size(egui::vec2(1280.0, 820.0)).build_eframe(move |cc| {
        CraftApp::setup_context(&cc.egui_ctx);
        let mut app = demo::app(demo::engine(Mode::Beginner, true));
        app.settings.theme = theme.clone();
        demo::fill_project(&mut app);
        app.state.feed = vec![FeedItem::User(UserItem::Text(ASKED.into())), FeedItem::Agent { text: ANSWERED.into(), streaming: false }];
        app
    });
    h.run_steps(4);
    let t = Tokens::get(&h.ctx);
    let mut shapes = Vec::new();
    h.output().shapes.iter().for_each(|c| flat(&c.shape, &mut shapes));
    (t, shapes)
}

fn text<'a>(shapes: &'a [Shape], wanted: &str) -> &'a TextShape {
    shapes
        .iter()
        .find_map(|s| match s {
            Shape::Text(t) if t.galley.text() == wanted => Some(t),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{wanted:?} is painted"))
}

/// The filled rectangles painted under a point.
fn fills_under(shapes: &[Shape], at: Pos2) -> Vec<egui::Color32> {
    shapes
        .iter()
        .filter_map(|s| match s {
            Shape::Rect(RectShape { rect, fill, .. }) if rect.contains(at) && fill.a() > 0 => Some(*fill),
            _ => None,
        })
        .collect()
}

#[test]
fn the_persons_message_is_a_solid_bubble_and_the_builders_answer_has_no_card() {
    for theme in ["studio", "studio-light"] {
        let (t, shapes) = painted(theme);
        let asked = text(&shapes, ASKED);
        let colour = asked.galley.job.sections.first().map(|s| s.format.color);
        assert_eq!(colour, Some(t.on_user_bubble), "{theme}: the person's words are drawn in the bubble's text colour");
        let at = asked.pos + asked.galley.rect.center().to_vec2();
        assert!(fills_under(&shapes, at).contains(&t.user_bubble), "{theme}: the person's message sits on the solid bubble");

        let answered = text(&shapes, ANSWERED);
        let at = answered.pos + answered.galley.rect.center().to_vec2();
        let under = fills_under(&shapes, at);
        assert!(!under.contains(&t.card), "{theme}: the builder's answer has no card behind it: {under:?}");
        assert!(!under.contains(&t.user_bubble), "{theme}: nor the person's bubble");
    }
}
