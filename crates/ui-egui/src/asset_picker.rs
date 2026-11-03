//! The provider · model picker for one asset type: a button that opens a list of the models the
//! providers offer, each with its price, grouped by provider and searchable, plus "Off". It draws
//! what `assets_models_list` gave (kept in a [`ModelsView`]) and nothing else: it never asks a
//! provider and never waits, so Settings and a project's panel can share it.

use crate::i18n::{tr, trf};
use crate::theme::{self, Tokens};
use crate::widgets;
use craft_anything_engine::asset_choice::AssetChoice;
use craft_anything_engines::assets::model::{AssetOption, ListedModel, Pricing, Unit};
use craft_anything_engines::assets::{AssetKind, AssetProviders};
use egui::text::{LayoutJob, TextFormat};
use egui::{Id, Popup, PopupCloseBehavior, Response, RichText, Sense, Stroke, TextEdit, Ui, Vec2};
use serde::Deserialize;
use serde_json::Value;

/// A list read earlier than this long ago is asked for again when a page that shows it opens.
pub const STALE_AFTER_MS: u64 = 10 * 60 * 1000;
/// The width of the picker's button, and of its list.
const BUTTON_WIDTH: f32 = 200.0;
const POPUP_WIDTH: f32 = 420.0;
/// The height of the list, which scrolls when it has more rows.
const LIST_HEIGHT: f32 = 300.0;
/// The most rows drawn at once; a longer list says so and is narrowed by searching.
const MAX_ROWS: usize = 300;

/// One provider's models for one asset type, as `assets_models_list` returns them.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ProviderModels {
    /// The provider's id (`fal`).
    pub provider: String,
    /// Shown to the person ("fal.ai").
    pub name: String,
    /// What the provider bills in; a choice of one of its models counts usage in it.
    pub unit: Unit,
    /// When the list was fetched (milliseconds since 1970). After a failed refresh this is the time
    /// of the list that is kept, not of the failure.
    pub fetched_ms: u64,
    pub models: Vec<ListedModel>,
    /// Why the provider could not be asked the last time, if it could not.
    pub error: Option<String>,
}

/// What the model lists said for one asset type, kept in memory for the screens that show them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ModelsView {
    /// When the oldest of the lists was fetched (milliseconds since 1970); when no provider has
    /// one, when the answer arrived. 0: nothing has been read yet.
    pub fetched_ms: u64,
    /// The providers that have a key and make this type.
    pub providers: Vec<ProviderModels>,
    /// A request for the lists is running.
    pub loading: bool,
}

impl ModelsView {
    /// The view of a `assets_models_list` result, read at `now_ms`.
    pub fn from_result(result: &Value, now_ms: u64) -> Result<ModelsView, String> {
        let list = result.get("providers").cloned().unwrap_or(Value::Array(Vec::new()));
        let providers: Vec<ProviderModels> =
            serde_json::from_value(list).map_err(|e| format!("The model lists came back in a shape this version can't read ({e})."))?;
        let fetched_ms = providers.iter().map(|p| p.fetched_ms).min().unwrap_or(now_ms).max(1);
        Ok(ModelsView { fetched_ms, providers, loading: false })
    }

    /// Whether the lists were read at least once (they may be empty: no provider has a key).
    pub fn is_read(&self) -> bool {
        self.fetched_ms != 0
    }

    /// Whether a page that shows the lists should ask for them again at `now_ms`: never read, or
    /// read longer ago than [`STALE_AFTER_MS`] (a clock that moved back counts as long ago).
    pub fn is_stale(&self, now_ms: u64) -> bool {
        !self.is_read() || self.fetched_ms > now_ms || now_ms - self.fetched_ms > STALE_AFTER_MS
    }
}

/// What the asset type is called to a person ("Pictures"). A type a later build adds is "Assets".
pub fn type_title(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Image => "Pictures",
        AssetKind::Model3d => "3D models",
        AssetKind::Speech => "Speech",
        AssetKind::SoundEffect => "Sound effects",
        AssetKind::Music => "Music",
        _ => "Assets",
    }
}

/// The few words that say how long ago `then_ms` was at `now_ms` ("just now", "2 minutes ago").
pub fn ago(now_ms: u64, then_ms: u64) -> String {
    let minutes = now_ms.saturating_sub(then_ms) / 60_000;
    match minutes {
        0 => tr("just now"),
        1 => tr("1 minute ago"),
        2..=59 => trf("{n} minutes ago", &[("n", &minutes.to_string())]),
        60..=119 => tr("1 hour ago"),
        _ => trf("{n} hours ago", &[("n", &(minutes / 60).to_string())]),
    }
}

/// The words for an option a model does nothing with ("shape").
fn option_word(option: AssetOption) -> &'static str {
    match option {
        AssetOption::Shape => "shape",
        AssetOption::Faces => "faces",
        AssetOption::Textured => "textures",
        AssetOption::Duration => "length",
        AssetOption::Instrumental => "instrumental",
        AssetOption::Voice => "voice",
    }
}

/// "ignores: shape, faces", or nothing when the model takes every option.
fn ignores_note(model: &ListedModel) -> Option<String> {
    if model.ignores.is_empty() {
        return None;
    }
    let words: Vec<String> = model.ignores.iter().map(|o| tr(option_word(*o))).collect();
    Some(trf("ignores: {options}", &[("options", &words.join(", "))]))
}

/// A provider's name as the view knows it (its list is there while its key is saved); else the name
/// a built-in provider gives itself; else its id.
fn provider_name(id: &str, view: Option<&ModelsView>) -> String {
    view.and_then(|v| v.providers.iter().find(|p| p.provider == id))
        .map(|p| p.name.clone())
        .or_else(|| AssetProviders::built_in().get(id).map(|p| p.name().to_string()))
        .unwrap_or_else(|| id.to_string())
}

/// What the picker's button says for `choice`: "Off", or "Scenario · Gemini 3.1".
pub fn choice_words(choice: &AssetChoice, view: Option<&ModelsView>) -> String {
    let Some(provider) = &choice.provider else { return tr("Off") };
    let model = if choice.model_name.is_empty() { &choice.model } else { &choice.model_name };
    format!("{} · {model}", provider_name(provider, view))
}

/// The choice picking `model` of `provider` makes.
fn chosen(provider: &ProviderModels, model: &ListedModel) -> AssetChoice {
    AssetChoice {
        provider: Some(provider.provider.clone()),
        model: model.id.clone(),
        model_name: model.name.clone(),
        unit: provider.unit,
        pricing: model.pricing.clone(),
    }
}

/// Whether `model` of `provider` matches the words typed in the search box.
fn matches(provider: &ProviderModels, model: &ListedModel, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty() || [model.name.as_str(), model.id.as_str(), provider.name.as_str()].iter().any(|s| s.to_lowercase().contains(&q))
}

/// One model as a line of the list: its name, its price (in the warning colour when unknown) and
/// what it ignores.
fn model_line(t: &Tokens, kind: AssetKind, model: &ListedModel) -> LayoutJob {
    let font = egui::FontId::proportional(theme::SMALL);
    let part = |color| TextFormat { font_id: font.clone(), color, ..Default::default() };
    let mut job = LayoutJob::default();
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.append(&model.name, 0.0, part(t.text));
    let price = tr(&model.pricing.shown(kind));
    let unknown = matches!(model.pricing, Pricing::Unknown);
    job.append(" · ", 0.0, part(t.text_faint));
    job.append(&price, 0.0, part(if unknown { t.warning } else { t.text_dim }));
    if let Some(note) = ignores_note(model) {
        job.append(&format!("   {note}"), 0.0, part(t.text_faint));
    }
    job
}

/// A row of the list.
enum Entry<'a> {
    Off,
    Provider(&'a ProviderModels),
    Model(&'a ProviderModels, &'a ListedModel),
}

/// The list's rows for the words typed: "Off", then each provider that has a model matching them,
/// with those models.
fn entries<'a>(view: &'a ModelsView, query: &str) -> Vec<Entry<'a>> {
    let mut rows = vec![Entry::Off];
    for provider in &view.providers {
        let found: Vec<&ListedModel> = provider.models.iter().filter(|m| matches(provider, m, query)).collect();
        if found.is_empty() {
            continue;
        }
        rows.push(Entry::Provider(provider));
        rows.extend(found.into_iter().map(|m| Entry::Model(provider, m)));
    }
    rows
}

/// The button: the choice in words and a chevron, in the look of the app's fields.
fn picker_button(ui: &mut Ui, id: Id, name: &str, words: &str, width: f32) -> Response {
    let t = Tokens::get(ui.ctx());
    let (_, rect) = ui.allocate_space(Vec2::new(width, 30.0));
    let resp = ui.interact(rect, id, Sense::click());
    let open = Popup::is_id_open(ui.ctx(), id.with("popup"));
    let hover = widgets::hover_t(ui, &resp);
    ui.painter().rect(
        rect,
        t.radius,
        theme::mix(t.field, t.hover, hover * 0.5),
        Stroke::new(1.0, theme::mix(t.field_border, t.accent, if open { 1.0 } else { hover * 0.5 })),
        egui::StrokeKind::Inside,
    );
    let mut job = LayoutJob::default();
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.max_width = rect.width() - 38.0;
    job.append(words, 0.0, TextFormat { font_id: egui::FontId::proportional(theme::SMALL), color: t.text, ..Default::default() });
    let galley = ui.painter().layout_job(job);
    ui.painter().galley_with_override_text_color(egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0), galley, t.text);
    crate::icons::paint(ui, egui::Rect::from_center_size(egui::pos2(rect.right() - 16.0, rect.center().y), Vec2::splat(14.0)), "chevron-down", 14.0, t.icon);
    widgets::focus_ring(ui, &resp, rect);
    resp.widget_info(|| egui::WidgetInfo {
        label: Some(tr(name)),
        current_text_value: Some(words.to_string()),
        ..egui::WidgetInfo::new(egui::WidgetType::ComboBox)
    });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A provider and model picker for `kind`: a button saying `current`, opening a searchable list of
/// the models in `view` grouped by provider, each with its price (or "price unknown") and the
/// options it ignores, and "Off". Returns the choice when another one is picked. `view` is what
/// `assets_models_list` last said; `None` before anything was read. `id` names this picker among
/// the others on a screen.
pub fn asset_picker(ui: &mut Ui, id: Id, kind: AssetKind, current: &AssetChoice, view: Option<&ModelsView>) -> Option<AssetChoice> {
    let name = trf("{type} model", &[("type", &tr(type_title(kind)))]);
    let words = choice_words(current, view);
    let button = picker_button(ui, id, &name, &words, BUTTON_WIDTH).on_hover_text(&words);
    let search_id = id.with("search");
    // A click opens the list afresh: empty search, and the cursor in it.
    if button.clicked() {
        ui.data_mut(|d| {
            d.insert_temp(search_id, String::new());
            d.insert_temp(search_id.with("focus"), true);
        });
    }
    let mut picked = None;
    Popup::menu(&button).id(id.with("popup")).close_behavior(PopupCloseBehavior::CloseOnClickOutside).width(POPUP_WIDTH).show(|ui| {
        ui.set_min_width(POPUP_WIDTH);
        picked = list(ui, search_id, kind, current, view);
        if picked.is_some() {
            ui.close();
        }
    });
    // Picking what is already chosen changes nothing.
    picked.filter(|p| p.provider != current.provider || p.model != current.model || p.unit != current.unit)
}

/// The inside of the open list; the choice when a row is clicked.
fn list(ui: &mut Ui, search_id: Id, kind: AssetKind, current: &AssetChoice, view: Option<&ModelsView>) -> Option<AssetChoice> {
    let t = Tokens::get(ui.ctx());
    let mut query: String = ui.data(|d| d.get_temp(search_id)).unwrap_or_default();
    let field = ui.add(
        TextEdit::singleline(&mut query)
            .id(search_id.with("field"))
            .hint_text(tr("Search models"))
            .desired_width(f32::INFINITY)
            .frame(widgets::text_field_frame_compact(&t)),
    );
    widgets::named(field.clone(), "Search models");
    if ui.data_mut(|d| d.remove_temp::<bool>(search_id.with("focus"))).is_some() {
        field.request_focus();
    }
    ui.data_mut(|d| d.insert_temp(search_id, query.clone()));
    ui.add_space(4.0);

    let Some(view) = view.filter(|v| v.is_read() || v.loading) else {
        words(ui, &tr("The model lists couldn't be read. Try Refresh in Settings → Builds."), t.warning);
        return off_row(ui, current);
    };
    if !view.is_read() {
        asking(ui);
        return off_row(ui, current);
    }
    // Why a provider's list is not up to date, where it is not.
    for p in view.providers.iter().filter(|p| p.error.is_some()) {
        let error = p.error.as_deref().unwrap_or_default();
        words(ui, &trf("Couldn't ask {name}: {error}", &[("name", &p.name), ("error", error)]), t.warning);
    }
    if view.providers.is_empty() {
        words(
            ui,
            &trf("No provider with a saved key makes {type}. Save a key in Settings → Builds.", &[("type", &tr(type_title(kind)).to_lowercase())]),
            t.text_dim,
        );
    }
    let mut rows = entries(view, &query);
    if rows.len() == 1 && !view.providers.is_empty() {
        words(ui, &tr("No model matches that."), t.text_dim);
    }
    let cut = rows.len().saturating_sub(MAX_ROWS);
    rows.truncate(MAX_ROWS);
    if cut > 0 {
        words(ui, &trf("Showing the first {n} rows. Type to narrow the list.", &[("n", &MAX_ROWS.to_string())]), t.text_dim);
    }
    let mut picked = None;
    // A fixed height: a popup is sized from its last frame, so a list that shrank (nothing matched
    // the search) could never grow back.
    egui::ScrollArea::vertical().min_scrolled_height(LIST_HEIGHT).max_height(LIST_HEIGHT).auto_shrink([false, false]).show(ui, |ui| {
        for row in &rows {
            match row {
                Entry::Off => {
                    let on = current.provider.is_none();
                    if ui.add(egui::Button::selectable(on, tr("Off")).truncate()).clicked() && !on {
                        picked = Some(AssetChoice::off());
                    }
                }
                Entry::Provider(p) => {
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(theme::label_text(&p.name, 12.0).color(t.text_faint)).truncate());
                }
                Entry::Model(p, m) => {
                    let on = current.provider.as_deref() == Some(p.provider.as_str()) && current.model == m.id;
                    let line = model_line(&t, kind, m);
                    let full = line.text.clone();
                    let r = ui.add(egui::Button::selectable(on, line).truncate()).on_hover_text(full);
                    if r.clicked() {
                        picked = Some(chosen(p, m));
                    }
                }
            }
        }
    });
    if view.loading {
        asking(ui);
    }
    picked
}

/// "Off" alone, for when there is nothing to choose from.
fn off_row(ui: &mut Ui, current: &AssetChoice) -> Option<AssetChoice> {
    let on = current.provider.is_none();
    (ui.selectable_label(on, tr("Off")).clicked() && !on).then(AssetChoice::off)
}

/// A wrapped note in the list.
fn words(ui: &mut Ui, text: &str, color: egui::Color32) {
    ui.add(egui::Label::new(RichText::new(text).size(theme::CAPTION).color(color)).wrap());
}

/// "Asking the providers…" with a spinner: a refresh is running.
fn asking(ui: &mut Ui) {
    let t = Tokens::get(ui.ctx());
    ui.horizontal(|ui| {
        ui.add(widgets::Spinner::new().size(14.0).color(t.accent));
        ui.label(RichText::new(tr("Asking the providers for their models…")).size(theme::CAPTION).color(t.text_dim));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_engines::assets::model::Rate;

    fn model(id: &str, name: &str, pricing: Pricing) -> ListedModel {
        ListedModel { provider: "fal".into(), id: id.into(), name: name.into(), kinds: vec![AssetKind::Image], pricing, ignores: Vec::new() }
    }

    fn provider(models: Vec<ListedModel>) -> ProviderModels {
        ProviderModels { provider: "fal".into(), name: "fal.ai".into(), unit: Unit::Usd, fetched_ms: 5, models, error: None }
    }

    #[test]
    fn a_view_is_read_from_the_tools_result_and_dated_by_its_oldest_list() {
        let result = serde_json::json!({"kind": "image", "providers": [
            {"provider": "fal", "name": "fal.ai", "unit": "usd", "fetched_ms": 900, "models": [], "error": null},
            {"provider": "scenario", "name": "Scenario", "unit": "credits", "fetched_ms": 400, "models": [], "error": "down"},
        ]});
        let view = ModelsView::from_result(&result, 1_000).unwrap();
        assert_eq!((view.fetched_ms, view.providers.len(), view.loading), (400, 2, false));
        assert_eq!(view.providers[1].unit, Unit::Credits);
        // Nobody has a key: dated by the answer, and read.
        let none = ModelsView::from_result(&serde_json::json!({"providers": []}), 1_000).unwrap();
        assert!(none.is_read() && none.providers.is_empty() && none.fetched_ms == 1_000);
        assert!(ModelsView::from_result(&serde_json::json!({"providers": [{"provider": 3}]}), 1).is_err());
        assert!(!ModelsView::default().is_read(), "nothing is read until a result arrives");
    }

    #[test]
    fn lists_go_stale_after_ten_minutes_and_when_the_clock_moves_back() {
        let view = ModelsView { fetched_ms: 1_000_000, providers: Vec::new(), loading: false };
        assert!(!view.is_stale(1_000_000 + STALE_AFTER_MS));
        assert!(view.is_stale(1_000_000 + STALE_AFTER_MS + 1));
        assert!(view.is_stale(999_999), "a list from the future is not trusted");
        assert!(ModelsView::default().is_stale(5), "nothing read yet");
    }

    #[test]
    fn search_matches_a_models_name_id_or_provider_and_keeps_the_providers_in_order() {
        let view = ModelsView {
            fetched_ms: 1,
            providers: vec![provider(vec![
                model("fal-ai/flux/dev", "FLUX.1 [dev]", Pricing::Unknown),
                model("fal-ai/nano", "Nano Banana Pro", Pricing::Unknown),
            ])],
            loading: false,
        };
        let names = |q: &str| -> Vec<String> {
            entries(&view, q)
                .into_iter()
                .map(|e| match e {
                    Entry::Off => "Off".to_string(),
                    Entry::Provider(p) => p.name.clone(),
                    Entry::Model(_, m) => m.name.clone(),
                })
                .collect()
        };
        assert_eq!(names(""), ["Off", "fal.ai", "FLUX.1 [dev]", "Nano Banana Pro"]);
        assert_eq!(names("  FLUX "), ["Off", "fal.ai", "FLUX.1 [dev]"], "case and padding don't matter");
        assert_eq!(names("ai/nano"), ["Off", "fal.ai", "Nano Banana Pro"], "the id counts");
        assert_eq!(names("fal.ai").len(), 4, "the provider's name matches all its models");
        assert_eq!(names("zzz"), ["Off"], "a provider with no match leaves its header out");
    }

    #[test]
    fn a_picked_model_becomes_a_choice_in_its_providers_unit() {
        let p = ProviderModels { unit: Unit::Credits, ..provider(Vec::new()) };
        let m = model("m1", "Model 1", Pricing::Before { unit: Unit::Credits, rate: Rate::PerRequest(20.0) });
        let c = chosen(&p, &m);
        assert_eq!((c.provider.as_deref(), c.model.as_str(), c.model_name.as_str(), c.unit), (Some("fal"), "m1", "Model 1", Unit::Credits));
        assert_eq!(c.pricing, m.pricing);
        assert_eq!(choice_words(&c, None), "fal.ai · Model 1", "a built-in provider's own name stands in while its list isn't there");
        assert_eq!(choice_words(&c, Some(&ModelsView { fetched_ms: 1, providers: vec![p], loading: false })), "fal.ai · Model 1");
        let other = AssetChoice { provider: Some("a-fork".into()), ..c };
        assert_eq!(choice_words(&other, None), "a-fork · Model 1", "the id stands in for a provider nobody knows");
        assert_eq!(choice_words(&AssetChoice::off(), None), "Off");
    }

    #[test]
    fn what_a_model_ignores_is_said_in_words() {
        let mut m = model("m", "M", Pricing::Unknown);
        assert_eq!(ignores_note(&m), None);
        m.ignores = vec![AssetOption::Shape, AssetOption::Faces];
        assert_eq!(ignores_note(&m).as_deref(), Some("ignores: shape, faces"));
    }

    #[test]
    fn times_are_said_in_minutes_then_hours() {
        let at = |minutes: u64| ago(minutes * 60_000 + 5, 5);
        assert_eq!((at(0).as_str(), at(1).as_str(), at(2).as_str(), at(59).as_str()), ("just now", "1 minute ago", "2 minutes ago", "59 minutes ago"));
        assert_eq!((at(60).as_str(), at(125).as_str()), ("1 hour ago", "2 hours ago"));
        assert_eq!(ago(5, 10), "just now", "a clock that moved back is not a time in the future");
    }
}
