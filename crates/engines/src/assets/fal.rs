//! Game assets from fal.ai over plain HTTPS: pictures (textures, skies, concept art)
//! and game-ready 3D models with Tripo P1 (a picture in, a low-poly GLB model out, its face count
//! capped). One key (`Authorization: Key …`), paid per generation; every call reports what it cost.
//! Running a model is done by the asset table's endpoints (`assets.toml`) until the live lists
//! replace them; [`Fal::list`] and [`Fal::describe`] read fal's own list of models, each model's
//! inputs and its prices from the platform API.

use super::inputs::{REMEDIATION, clipped, runnable};
use super::model::{ListedModel, MAX_ASSET_BUDGET_USD, ModelDetail, ModelList, Pricing, Rate, Unit};
use super::openapi::input_fields;
use super::tables::{InputRoles, KnownModels};
use super::{AssetKind, AssetProvider, Generated, LegacyRequest, Shape, SizeBy, charged, http};
use craft_anything_evidence::{ErrorCode, ToolError};
use http::{CURSOR_LENGTH, NAME_LENGTH, encode, time_is_up, words};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::time::{Duration, Instant};

/// The provider id the asset table names.
pub const ID: &str = "fal";
/// What people call it.
const NAME: &str = "fal.ai";
const BASE: &str = "https://fal.run";
/// fal's platform API, which lists models and prices (generations are run at [`BASE`]).
const API_BASE: &str = "https://api.fal.ai";
/// Largest download we keep (a textured GLB is a few MB).
const MAX_BYTES: u64 = 200 << 20;

/// How fal is told a picture's shape: the field and its value for the model.
fn fal_size(by: SizeBy, shape: Shape) -> (&'static str, &'static str) {
    match (by, shape) {
        (SizeBy::ImageSize, Shape::Square) => ("image_size", "square_hd"),
        (SizeBy::ImageSize, Shape::Wide) => ("image_size", "landscape_16_9"),
        (SizeBy::ImageSize, Shape::Tall) => ("image_size", "portrait_16_9"),
        (SizeBy::AspectRatio, Shape::Square) => ("aspect_ratio", "1:1"),
        (SizeBy::AspectRatio, Shape::Wide) => ("aspect_ratio", "16:9"),
        (SizeBy::AspectRatio, Shape::Tall) => ("aspect_ratio", "9:16"),
    }
}

/// fal.ai, as an [`AssetProvider`].
pub struct Fal {
    /// Where generations are run.
    base: String,
    /// Where models and prices are listed.
    api_base: String,
    /// How long [`Fal::list`] may go on starting requests.
    list_deadline: Duration,
}

impl Default for Fal {
    fn default() -> Fal {
        Fal { base: BASE.to_string(), api_base: API_BASE.to_string(), list_deadline: LIST_DEADLINE }
    }
}

impl AssetProvider for Fal {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        NAME
    }

    fn keys_page(&self) -> Option<&'static str> {
        Some("https://fal.ai/dashboard/keys")
    }

    fn key_env(&self) -> Option<&'static str> {
        Some("FAL_KEY")
    }

    /// Validate credentials, generate a supported image or mesh, and download it with its catalogue cost.
    fn generate(&self, key: &str, req: &LegacyRequest) -> Result<Generated, ToolError> {
        self.validate_key(key)?;
        let key = key.trim();
        let cost = req.listed_cost();
        // fal answers once the paid generation is done: any failure after that still reports its charge.
        match *req {
            LegacyRequest::Image { model, prompt, shape } => {
                let (field, size) = fal_size(model.size_by, shape);
                let body = json!({"prompt": prompt, field: size, "num_images": 1, "output_format": "png"});
                let v = self.post(key, &model.endpoint, &body, cost)?;
                let made = || {
                    if v.pointer("/has_nsfw_concepts/0").and_then(Value::as_bool) == Some(true) {
                        return Err(ToolError::new(ErrorCode::ProgramFailed, "fal.ai's safety check blocked that picture.")
                            .with_remediation("Describe it differently."));
                    }
                    let url = image_url(&v).ok_or_else(|| unexpected("picture", &v))?;
                    Ok(Generated { bytes: download(&url)?, url, cost_usd: cost })
                };
                made().map_err(|e| charged(e, cost))
            }
            LegacyRequest::Model3d { model, image_url, textured, faces } => {
                let range = model.faces();
                let faces = faces.clamp(*range.start(), *range.end());
                let v = self.post(key, &model.endpoint, &json!({"image_url": image_url, "texture": textured, "face_limit": faces}), cost)?;
                let made = || {
                    let url = model_url(&v).ok_or_else(|| unexpected("3D model", &v))?;
                    Ok(Generated { bytes: download(&url)?, url, cost_usd: cost })
                };
                made().map_err(|e| charged(e, cost))
            }
            _ => Err(ToolError::new(ErrorCode::BadSettings, "fal.ai does not support this asset request.")),
        }
    }
}

impl Fal {
    /// Run one paid request; an unreadable success answer still reports the request's `cost`.
    fn post(&self, key: &str, model: &str, body: &Value, cost: f64) -> Result<Value, ToolError> {
        let url = format!("{}/{model}", self.base);
        // A 3D model takes a minute or two; fal answers once it's done.
        let resp = agent(Duration::from_secs(10 * 60))
            .post(&url)
            .header("Authorization", &format!("Key {key}"))
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(|e| {
                ToolError::new(ErrorCode::EngineMissing, format!("Couldn't reach fal.ai: {e}"))
                    .with_remediation("Check your internet connection and try again.")
            })?;
        let status = resp.status().as_u16();
        let text = resp.into_body().read_to_string().unwrap_or_default();
        match status {
            200..=299 => serde_json::from_str(&text)
                .map_err(|e| charged(ToolError::new(ErrorCode::GarbledOutput, format!("fal.ai sent an unreadable answer: {e}")), cost)),
            401 | 403 => Err(ToolError::new(ErrorCode::BadSettings, "fal.ai didn't accept the key.")
                .with_remediation("Check the fal.ai key in Settings → AI → Game assets.")),
            402 => Err(ToolError::new(ErrorCode::OverLimit, "The fal.ai account is out of credit.").with_remediation("Add credit at fal.ai, then try again.")),
            _ => Err(ToolError::new(ErrorCode::EngineMissing, format!("fal.ai answered {status}: {}", text.chars().take(300).collect::<String>()))),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Listing: the models fal serves, their inputs and their prices (fal's platform API).
// ---------------------------------------------------------------------------------------------

/// Models in a page of fal's list when the page carries their OpenAPI documents, which is the
/// most fal gives: it answers a larger `limit` with HTTP 400 ("exceeds maximum of 10", read on
/// 2026-11-03), where a list without the documents takes up to 100.
const PAGE_SIZE: usize = 10;
/// The most pages read for one list. Ten a page, that is more than twice the picture models fal has.
const MAX_PAGES: usize = 40;
/// The most models taken from one page, whatever it holds.
const MAX_PAGE_MODELS: usize = 100;
/// How many ids one price request carries (fal accepts 1 to 50).
const PRICE_BATCH: usize = 50;
/// The longest model id kept.
const ID_LENGTH: usize = 100;
/// How long one request of the platform API may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a whole [`Fal::list`] (every page, then every batch of prices) goes on starting
/// requests. It is checked before each one, so the call can end up to [`LIST_TIMEOUT`] later.
const LIST_DEADLINE: Duration = Duration::from_secs(3 * 60);

impl Fal {
    /// The models fal lists for `kind` that this app can run, with their prices.
    ///
    /// Every page is a request; fal limits how many it answers without a key, and the whole call
    /// has a deadline, so the list can end early: then the models read so far are returned, and
    /// `error` says the list is incomplete. A failure before anything was read is an error. Prices
    /// need a key: without one every price is [`Pricing::Unknown`]. A price request that fails, or
    /// that does not return all the prices, leaves those models unpriced and is reported in `error`
    /// as well.
    pub fn list(&self, key: Option<&str>, kind: AssetKind) -> Result<ModelList, ToolError> {
        let key = key.map(http::key).transpose()?;
        let deadline = Instant::now() + self.list_deadline;
        let mut models: Vec<ListedModel> = Vec::new();
        let mut listed: HashSet<String> = HashSet::new();
        let (mut pages, mut stopped, mut cut_short) = (0, None, None::<String>);
        'categories: for category in categories(kind) {
            let mut cursor: Option<String> = None;
            loop {
                if pages == MAX_PAGES {
                    cut_short = Some(format!("only the first {MAX_PAGES} pages were read."));
                    break 'categories;
                }
                if time_is_up(deadline) {
                    stopped = Some(http::too_slow(NAME));
                    break 'categories;
                }
                let page = match self.page(key, category, cursor.as_deref()) {
                    Ok(page) => page,
                    Err(e) => {
                        stopped = Some(e);
                        break 'categories;
                    }
                };
                pages += 1;
                for model in &page.models {
                    if let Some(model) = listed_model(model, category, kind)
                        && listed.insert(model.id.clone())
                    {
                        models.push(model);
                    }
                }
                match page.next {
                    Next::Cursor(next) => cursor = Some(next),
                    Next::None => break,
                    Next::Unreachable => {
                        cut_short = Some("fal.ai promised a next page but gave nothing to ask for it with.".to_string());
                        break 'categories;
                    }
                }
            }
        }
        if pages == 0
            && let Some(e) = stopped
        {
            return Err(e);
        }
        let mut problems = Vec::new();
        if let Some(e) = &stopped {
            problems.push(format!("The list is incomplete: {}", words(e)));
        }
        if let Some(why) = cut_short {
            problems.push(format!("The list is incomplete: {why}"));
        }
        if let Some(key) = key
            && let Some(why) = self.fill_prices(key, &mut models, deadline)
        {
            problems.push(format!("Prices could not be read: {why}"));
        }
        Ok(ModelList { models, error: (!problems.is_empty()).then(|| problems.join(" ")) })
    }

    /// One model with all its inputs and its price, as `key` gets them. A model fal no longer
    /// serves, or is retiring, is a [`ErrorCode::BadSettings`] error that names it; a price that
    /// cannot be read is an error too, because a run must not go ahead as if it were free.
    pub fn describe(&self, key: &str, model: &str) -> Result<ModelDetail, ToolError> {
        let key = http::key(key)?;
        if !endpoint_ok(model) {
            return Err(http::bad("That is not an id fal.ai gives its models.").with_remediation(REMEDIATION));
        }
        let url = format!("{}/v1/models?endpoint_id={}&expand=openapi-3.0", self.api_base, encode(model));
        let response = self.send(Some(key), &url)?;
        if response.status().as_u16() == 404 {
            return Err(http::gone(NAME, model));
        }
        let answer = read(response, http::MAX_LIST_PAGE, true)?;
        let found = answer
            .get("models")
            .and_then(Value::as_array)
            .and_then(|models| models.iter().find(|m| m.get("endpoint_id").and_then(Value::as_str) == Some(model)))
            .ok_or_else(|| http::gone(NAME, model))?;
        if is_retiring(found) {
            return Err(http::retiring(NAME, model));
        }
        let fields = input_fields(found.get("openapi").unwrap_or(&Value::Null))?;
        let mut priced = self.fetch_prices(key, &[model])?;
        let pricing = match priced.table.remove(model) {
            Some(pricing) => pricing,
            // The row may be on a page fal did not send: that is not the same as having no price.
            None if priced.more => return Err(http::garbled("fal.ai's prices could not be read.").with_remediation("Try again in a moment.")),
            None => Pricing::Unknown,
        };
        Ok(ModelDetail { id: model.to_string(), name: name_of(found, model), kinds: kinds_of(category_of(found), model), fields, pricing })
    }

    /// One page of a category: the models on it, and how to ask for the next page if there is one.
    fn page(&self, key: Option<&str>, category: &str, cursor: Option<&str>) -> Result<Page, ToolError> {
        let mut url = format!("{}/v1/models?category={}&expand=openapi-3.0&limit={PAGE_SIZE}", self.api_base, encode(category));
        if let Some(cursor) = cursor {
            url.push_str("&cursor=");
            url.push_str(&encode(cursor));
        }
        let mut answer = read(self.send(key, &url)?, http::MAX_LIST_PAGE, key.is_some())?;
        let Some(Value::Array(mut models)) = answer.get_mut("models").map(Value::take) else {
            return Err(http::garbled("fal.ai's model list could not be read."));
        };
        models.truncate(MAX_PAGE_MODELS);
        Ok(Page { models, next: next_of(&answer) })
    }

    /// Put each model's price in `models`, a batch of ids at a time. Stops at the first request
    /// that fails (the rest would fail the same way) or when `deadline` has passed, and returns why;
    /// the models not reached keep [`Pricing::Unknown`], and so do those a batch left out when fal
    /// said it had more prices than it sent.
    fn fill_prices(&self, key: &str, models: &mut [ListedModel], deadline: Instant) -> Option<String> {
        let mut left_out = false;
        for batch in models.chunks_mut(PRICE_BATCH) {
            if time_is_up(deadline) {
                return Some("there was no time left to ask for them.".to_string());
            }
            let ids: Vec<&str> = batch.iter().map(|m| m.id.as_str()).collect();
            match self.fetch_prices(key, &ids) {
                Ok(mut priced) => {
                    for model in batch.iter_mut() {
                        if let Some(pricing) = priced.table.remove(&model.id) {
                            model.pricing = pricing;
                        }
                    }
                    left_out |= priced.more;
                }
                Err(e) => return Some(words(&e)),
            }
        }
        left_out.then(|| "fal.ai said there are more prices than it sent.".to_string())
    }

    /// What fal charges for each of up to [`PRICE_BATCH`] models; a model it names no price for is
    /// absent. Its answer has a cursor field, but the pricing page gives no way to send one back,
    /// so an answer that says there is more is reported as such (`more`) and not followed.
    fn fetch_prices(&self, key: &str, ids: &[&str]) -> Result<Prices, ToolError> {
        let query: Vec<String> = ids.iter().map(|id| format!("endpoint_id={}", encode(id))).collect();
        let url = format!("{}/v1/models/pricing?{}", self.api_base, query.join("&"));
        let answer = read(self.send(Some(key), &url)?, http::MAX_JSON, true)?;
        let rows = answer.get("prices").and_then(Value::as_array).ok_or_else(|| http::garbled("fal.ai's prices could not be read."))?;
        Ok(Prices { table: rows.iter().take(PRICE_BATCH * 2).filter_map(price_row).collect(), more: !matches!(next_of(&answer), Next::None) })
    }

    /// A GET to fal's platform API. The key, when there is one, goes only to this address.
    fn send(&self, key: Option<&str>, url: &str) -> Result<ureq::http::Response<ureq::Body>, ToolError> {
        let request = http::agent(LIST_TIMEOUT).get(url);
        match key {
            Some(key) => request.header("Authorization", &format!("Key {key}")).call(),
            None => request.call(),
        }
        .map_err(|_| {
            ToolError::new(ErrorCode::EngineMissing, "Couldn't reach fal.ai to read its models.")
                .with_remediation("Check your internet connection and try again.")
        })
    }
}

struct Page {
    models: Vec<Value>,
    next: Next,
}

/// Where a list goes on after an answer.
enum Next {
    /// The answer was the last.
    None,
    /// Ask again with this cursor.
    Cursor(String),
    /// The answer says there is more, but gives nothing usable to ask for it with.
    Unreachable,
}

/// What an answer says comes after it: `has_more`, or failing that the presence of a cursor.
fn next_of(answer: &Value) -> Next {
    let cursor = answer.get("next_cursor").and_then(Value::as_str).filter(|c| !c.is_empty());
    match (answer.get("has_more").and_then(Value::as_bool).unwrap_or(cursor.is_some()), cursor) {
        (false, _) => Next::None,
        (true, Some(cursor)) if cursor.len() <= CURSOR_LENGTH => Next::Cursor(cursor.to_string()),
        (true, _) => Next::Unreachable,
    }
}

/// The prices of a batch of models, and whether fal said it had more than it sent.
struct Prices {
    table: HashMap<String, Pricing>,
    more: bool,
}

/// The answer's JSON, or why there is none. fal's rate limit is told apart because it is the one
/// failure that is the person's to fix: a key raises it.
fn read(response: ureq::http::Response<ureq::Body>, limit: u64, key_given: bool) -> Result<Value, ToolError> {
    let status = response.status().as_u16();
    if status == 429 {
        let advice = if key_given {
            "Wait a minute, then refresh the list."
        } else {
            "Wait a minute, then refresh the list, or add a fal.ai key, which raises the limit."
        };
        return Err(ToolError::new(ErrorCode::OverLimit, "fal.ai's rate limit was reached.").with_remediation(advice));
    }
    http::status(status)?;
    http::json_up_to(response.into_body(), limit)
}

/// Whether fal has marked the model as one it is retiring.
fn is_retiring(model: &Value) -> bool {
    model.pointer("/metadata/status").and_then(Value::as_str) == Some("deprecated")
}

/// What the models of a fal category make.
enum Makes {
    /// The category says so.
    One(AssetKind),
    /// The category holds several kinds; the known-models table says which of them a model makes.
    OneOf(&'static [AssetKind]),
}

/// The fal categories this app reads, and what their models make. The one place that says both
/// which categories a kind is listed from ([`categories`]) and what a listed model is ([`kinds_of`]).
const CATEGORIES: &[(&str, Makes)] = &[
    ("text-to-image", Makes::One(AssetKind::Image)),
    ("image-to-3d", Makes::One(AssetKind::Model3d)),
    ("text-to-3d", Makes::One(AssetKind::Model3d)),
    ("text-to-speech", Makes::One(AssetKind::Speech)),
    // fal files speech, sound effects and music models alike under `text-to-audio`.
    ("text-to-audio", Makes::OneOf(&[AssetKind::Speech, AssetKind::SoundEffect, AssetKind::Music])),
];

/// The fal categories whose models may make `kind`, in the order they are read.
fn categories(kind: AssetKind) -> Vec<&'static str> {
    let may = |makes: &Makes| match makes {
        Makes::One(one) => *one == kind,
        Makes::OneOf(some) => some.contains(&kind),
    };
    CATEGORIES.iter().filter(|(_, makes)| may(makes)).map(|(category, _)| *category).collect()
}

/// What the model `id` of a fal `category` makes. The category says so where it can; where it
/// holds several kinds the known-models table does, and a model it does not know makes nothing we
/// can say. A category this app does not read is left to the known-models table as well.
fn kinds_of(category: &str, id: &str) -> Vec<AssetKind> {
    let known = || KnownModels::built_in().kinds(ID, id).unwrap_or_default();
    match CATEGORIES.iter().find(|(name, _)| *name == category) {
        Some((_, Makes::One(one))) => vec![*one],
        Some((_, Makes::OneOf(some))) => known().iter().copied().filter(|kind| some.contains(kind)).collect(),
        None => known().to_vec(),
    }
}

fn category_of(model: &Value) -> &str {
    model.pointer("/metadata/category").and_then(Value::as_str).unwrap_or_default()
}

/// The model of one list entry, if it makes `kind` and its inputs are ones this app can fill.
/// `category` is the one the page was asked for; the category a model says of itself is not used.
fn listed_model(model: &Value, category: &str, kind: AssetKind) -> Option<ListedModel> {
    let id = model.get("endpoint_id")?.as_str().filter(|id| endpoint_ok(id))?;
    if is_retiring(model) {
        return None;
    }
    let kinds = kinds_of(category, id);
    if !kinds.contains(&kind) {
        return None;
    }
    let fields = input_fields(model.get("openapi")?).ok()?;
    let ignores = runnable(&fields, kind, InputRoles::built_in()).ok()?;
    Some(ListedModel { provider: ID.into(), id: id.into(), name: name_of(model, id), kinds, pricing: Pricing::Unknown, ignores })
}

/// The name to show: fal's, else the known-models table's, else the id.
fn name_of(model: &Value, id: &str) -> String {
    let fals = model.pointer("/metadata/display_name").and_then(Value::as_str);
    let table = KnownModels::built_in().name(ID, id);
    [fals, table]
        .into_iter()
        .flatten()
        .map(|name| clipped(name, NAME_LENGTH))
        .map(|name| name.trim().to_string())
        .find(|name| !name.is_empty())
        .unwrap_or_else(|| id.to_string())
}

/// Whether `id` is one fal's endpoints can be called by and is safe in a URL: path segments of
/// letters, digits, `-`, `_` and `.`, none empty or made of dots only.
fn endpoint_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= ID_LENGTH
        && id.split('/').all(|part| {
            !part.is_empty() && !part.bytes().all(|b| b == b'.') && part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        })
}

/// A model's id and price out of one row of fal's price list. A row with a price that is not a
/// number is kept, as no price: fal named the model, but gave nothing to count with.
fn price_row(row: &Value) -> Option<(String, Pricing)> {
    let id = row.get("endpoint_id")?.as_str()?;
    let price = row.get("unit_price").and_then(Value::as_f64).unwrap_or(f64::NAN);
    let unit = row.get("unit").and_then(Value::as_str).unwrap_or_default();
    let currency = row.get("currency").and_then(Value::as_str).unwrap_or_default();
    Some((id.to_string(), pricing_of(price, unit, currency)))
}

/// A price turned into how it is charged.
type RateOf = fn(f64) -> Rate;

/// The billing units read as a price before the run, as one unit per run or per megapixel.
///
/// What fal documents (https://fal.ai/docs/platform-apis/v1/models/pricing and the billing guide at
/// https://fal.ai/docs/documentation/model-apis/pricing, read 2026-11-03):
/// - `image` and `video` are the unit strings of the pricing page ("'image', 'video', or
///   provider-specific GPU/compute unit"); its example is `unit: image`.
/// - The guide says models charge "per request" and "per megapixel" without spelling the unit, so
///   `request` and `megapixel` are readings of that prose (`megapixels` was the brief's spelling).
///   None of the four was seen from the live API: its prices need a key.
///
/// A unit not named here (the GPU or compute seconds some models fall back to, a plain "second"
/// that may be either, a spelling of characters, outputs or generations nobody has documented, or
/// anything fal adds) is a price per a quantity this app cannot know before the run: the model's
/// cost is known only after it.
const BILLED_PER: &[(&str, RateOf)] =
    &[("image", Rate::PerRequest), ("video", Rate::PerRequest), ("request", Rate::PerRequest), ("megapixel", Rate::PerMegapixel)];

/// What a price from fal's price list means. Only a plain dollar amount is a price (Rulings 5 and
/// 16): a number that is not finite, is negative or is above the most a project may spend, or a
/// price in another currency, is no price at all.
fn pricing_of(unit_price: f64, unit: &str, currency: &str) -> Pricing {
    if !currency.eq_ignore_ascii_case("usd") || !unit_price.is_finite() || !(0.0..=MAX_ASSET_BUDGET_USD).contains(&unit_price) {
        return Pricing::Unknown;
    }
    // "Megapixels", "GPU-seconds" → "megapixel", "gpu_second".
    let spelled = unit.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    let spelled = spelled.strip_suffix('s').unwrap_or(&spelled);
    match BILLED_PER.iter().find(|(name, _)| *name == spelled) {
        Some((_, rate)) => Pricing::Before { unit: Unit::Usd, rate: rate(unit_price) },
        None => Pricing::After { unit: Unit::Usd },
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .user_agent("CraftAnything")
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(timeout))
        .timeout_recv_body(Some(Duration::from_secs(10 * 60)))
        .build()
        .new_agent()
}

/// Fetch a generated file (fal answers with its address, or a `data:` URI in sync mode).
fn download(url: &str) -> Result<Vec<u8>, ToolError> {
    if let Some(data) = url.strip_prefix("data:") {
        let b64 =
            data.split_once(";base64,").map(|(_, b)| b).ok_or_else(|| ToolError::new(ErrorCode::GarbledOutput, "fal.ai sent a file in an unknown form."))?;
        return base64_decode(b64);
    }
    let resp =
        agent(Duration::from_secs(120)).get(url).call().map_err(|e| ToolError::new(ErrorCode::EngineMissing, format!("Couldn't download the file: {e}")))?;
    if !resp.status().is_success() {
        return Err(ToolError::new(ErrorCode::EngineMissing, format!("Downloading the file failed ({}).", resp.status().as_u16())));
    }
    let mut bytes = Vec::new();
    resp.into_body()
        .into_reader()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| ToolError::new(ErrorCode::EngineMissing, format!("The download broke off: {e}")))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(ToolError::new(ErrorCode::OverLimit, "The generated file is too large."));
    }
    Ok(bytes)
}

fn image_url(v: &Value) -> Option<String> {
    v.pointer("/images/0/url").and_then(Value::as_str).map(str::to_string)
}

/// The GLB: Tripo lists it under `model_urls`; its main `model_mesh` can be an FBX instead.
fn model_url(v: &Value) -> Option<String> {
    let glb = v.pointer("/model_urls/glb/url").and_then(Value::as_str);
    let mesh = v.get("model_mesh").filter(|m| {
        let named = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase();
        named("file_name").ends_with(".glb") || named("content_type") == "model/gltf-binary"
    });
    glb.or_else(|| mesh.and_then(|m| m.get("url")?.as_str())).filter(|u| !u.is_empty()).map(str::to_string)
}

fn unexpected(what: &str, v: &Value) -> ToolError {
    ToolError::new(ErrorCode::GarbledOutput, format!("fal.ai didn't return a {what}."))
        .with_details(json!({"answer": v.to_string().chars().take(500).collect::<String>()}))
}

/// A `data:` URI for a local picture (fal accepts these for some models; links always work).
pub fn data_uri(bytes: &[u8], media_type: &str) -> String {
    format!("data:{media_type};base64,{}", base64_encode(bytes))
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, ToolError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.decode(s.trim()).map_err(|e| ToolError::new(ErrorCode::GarbledOutput, format!("invalid base64: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::Catalogue;
    use craft_anything_platform::sync::Mutex;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::Arc;

    /// A tiny local server that answers each request in turn and records what it was sent.
    fn serve(answers: Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        std::thread::spawn(move || {
            for (status, body) in answers {
                let (mut stream, _) = l.accept().unwrap();
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    r.read_line(&mut line).unwrap();
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                    head.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut req_body = vec![0; len];
                r.read_exact(&mut req_body).unwrap();
                s2.lock().unwrap().push(format!("{head}{}", String::from_utf8_lossy(&req_body)));
                write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
                    .unwrap();
            }
        });
        (base, seen)
    }

    /// A picture with FLUX.1 [dev].
    fn image(fal: &Fal, key: &str, prompt: &str, shape: Shape) -> Result<Generated, ToolError> {
        let c = Catalogue::built_in();
        fal.generate(key, &LegacyRequest::Image { model: c.image_model(Some("flux-dev"))?, prompt, shape })
    }

    fn model_3d(fal: &Fal, key: &str, image_url: &str, textured: bool, faces: u32) -> Result<Generated, ToolError> {
        let c = Catalogue::fallback();
        fal.generate(key, &LegacyRequest::Model3d { model: c.model_3d(None)?, image_url, textured, faces })
    }

    #[test]
    fn a_picture_then_a_model_from_its_address() {
        let png = base64_encode(b"\x89PNG fake");
        let glb = base64_encode(b"glTF fake");
        let (base, seen) = serve(vec![
            (200, json!({"images": [{"url": format!("data:image/png;base64,{png}"), "content_type": "image/png"}], "has_nsfw_concepts": [false]}).to_string()),
            (200, json!({"model_mesh": {"url": "https://x/m.fbx", "file_name": "m.fbx"}, "model_urls": {"glb": {"url": format!("data:model/gltf-binary;base64,{glb}")}}}).to_string()),
        ]);
        let fal = Fal { base, ..Fal::default() };
        let pic = image(&fal, " k-123 ", "a wooden crate, game asset", Shape::Square).unwrap();
        assert_eq!(pic.bytes, b"\x89PNG fake");
        assert!((pic.cost_usd - 0.05).abs() < 1e-9, "a conservative two decimal megapixels, not a verified invoice charge");
        let model = model_3d(&fal, "k-123", "https://v3.fal.media/files/crate.png", true, 2000).unwrap();
        assert_eq!(model.bytes, b"glTF fake", "the GLB, not the FBX");
        assert!((model.cost_usd - 0.50).abs() < 1e-9, "a textured Tripo P1 model");
        let seen = seen.lock().unwrap();
        assert!(seen[0].starts_with("POST /fal-ai/flux/dev "), "{}", seen[0]);
        // Header names aren't case-sensitive; the key goes in fal's "Key …" form.
        assert!(seen[0].to_ascii_lowercase().contains("authorization: key k-123"), "{}", seen[0]);
        assert!(seen[0].contains("\"image_size\":\"square_hd\"") && seen[0].contains("\"output_format\":\"png\""));
        assert!(
            seen[1].starts_with("POST /tripo3d/p1/image-to-3d ") && seen[1].contains("\"texture\":true") && seen[1].contains("\"face_limit\":2000"),
            "{}",
            seen[1]
        );
        assert!(seen[1].contains("\"image_url\":\"https://v3.fal.media/files/crate.png\""));
    }

    #[test]
    fn nano_banana_pro_is_asked_for_an_aspect_ratio() {
        let png = base64_encode(b"PNG fake");
        let (base, seen) = serve(vec![(200, json!({"images": [{"url": format!("data:image/png;base64,{png}")}], "description": ""}).to_string())]);
        let c = Catalogue::built_in();
        let req = LegacyRequest::Image { model: c.image_model(None).unwrap(), prompt: "a night sky", shape: Shape::Wide };
        let pic = Fal { base, ..Fal::default() }.generate("k", &req).unwrap();
        assert_eq!((pic.bytes.as_slice(), pic.cost_usd), (&b"PNG fake"[..], 0.15));
        let seen = seen.lock().unwrap();
        assert!(seen[0].starts_with("POST /fal-ai/nano-banana-pro "), "{}", seen[0]);
        assert!(seen[0].contains("\"aspect_ratio\":\"16:9\"") && !seen[0].contains("image_size"), "{}", seen[0]);
    }

    #[test]
    fn only_a_glb_is_taken_as_the_model() {
        let fbx_only = json!({"model_mesh": {"url": "https://x/m.fbx", "file_name": "m.fbx"}, "model_urls": {}});
        assert_eq!(model_url(&fbx_only), None);
        let glb_mesh = json!({"model_mesh": {"url": "https://x/m.glb", "file_name": "m.glb"}});
        assert_eq!(model_url(&glb_mesh).as_deref(), Some("https://x/m.glb"));
    }

    #[test]
    fn refusals_say_what_to_do() {
        let (base, _) = serve(vec![
            (401, "{}".into()),
            (402, "{}".into()),
            (200, json!({"images": [{"url": "data:image/png;base64,AA=="}], "has_nsfw_concepts": [true]}).to_string()),
        ]);
        let fal = Fal { base, ..Fal::default() };
        for code in [ErrorCode::BadSettings, ErrorCode::OverLimit] {
            let refused = image(&fal, "bad", "x", Shape::Wide).unwrap_err();
            assert_eq!(refused.code, code);
            assert_eq!(super::super::charged_usd(&refused), None, "a refused request is not charged");
        }
        let blocked = image(&fal, "bad", "x", Shape::Wide).unwrap_err();
        assert!(blocked.message.contains("safety check"));
        assert_eq!(super::super::charged_usd(&blocked), Some(0.025), "a finished generation is charged even when blocked");
    }

    /// A finished generation that returns no usable output still reports what it cost.
    #[test]
    fn a_finished_generation_without_output_reports_its_charge() {
        let (base, _) = serve(vec![(200, "not json".into()), (200, json!({"model_urls": {}}).to_string())]);
        let fal = Fal { base, ..Fal::default() };
        let unreadable = image(&fal, "k", "x", Shape::Square).unwrap_err();
        assert_eq!(unreadable.code, ErrorCode::GarbledOutput);
        assert_eq!(super::super::charged_usd(&unreadable), Some(0.05));
        let missing = model_3d(&fal, "k", "https://v3.fal.media/files/crate.png", true, 2000).unwrap_err();
        assert_eq!(super::super::charged_usd(&missing), Some(0.50));
        assert!(missing.details.as_ref().is_some_and(|d| d.get("answer").is_some()), "the provider's answer is kept beside the charge");
    }

    #[test]
    fn shapes_and_data_uris() {
        assert_eq!(Shape::parse("wide"), Some(Shape::Wide));
        assert!(Shape::parse("huge").is_none());
        let uri = data_uri(b"hi", "image/png");
        assert_eq!(uri, "data:image/png;base64,aGk=");
        assert_eq!(download(&uri).unwrap(), b"hi");
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;
    use crate::assets::inputs::{FieldKind, MAX_FIELDS};
    use crate::assets::model::AssetOption;

    // The platform API these tests stand in for, read on 2026-11-03:
    //
    // * `GET https://api.fal.ai/v1/models` (https://fal.ai/docs/platform-apis/v1/models): `category`,
    //   `cursor`, `limit`, `expand=openapi-3.0` and a repeatable `endpoint_id`; answers
    //   `{"models": [{"endpoint_id", "metadata": {"display_name", "category", "status", ...},
    //   "openapi"}], "next_cursor", "has_more"}`. A key is optional and raises the rate limit; the
    //   answer to a limit it will not give is HTTP 400, and past the rate limit HTTP 429. With
    //   `expand=openapi-3.0` it gave at most 10 models a page (`limit=11`: 400, "Requested limit 11
    //   exceeds maximum of 10 for this query"); an unknown `endpoint_id` gave HTTP 404.
    // * `GET https://api.fal.ai/v1/models/pricing?endpoint_id=…` (key required, 1 to 50 ids,
    //   https://fal.ai/docs/platform-apis/v1/models/pricing): `{"prices": [{"endpoint_id",
    //   "unit_price", "unit", "currency"}], "next_cursor", "has_more"}`. That page names the `unit`
    //   strings `image` and `video`, "or provider-specific GPU/compute unit", and its example is
    //   `{"endpoint_id": "fal-ai/flux/dev", "unit_price": 0.025, "unit": "image", "currency": "USD"}`.
    //   The billing guide (https://fal.ai/docs/documentation/model-apis/pricing) says image models
    //   charge "per image or per megapixel", video "per second ... or a flat rate", others "per
    //   request or compute seconds", without spelling those units. So `megapixels` and `gpu_second`
    //   below are this app's reading of them, not strings seen from the live API (that needs a key,
    //   and the tests use none): a unit not recognised is a cost known only after the run.

    const KEY: &str = "k-123";

    /// A model's OpenAPI document with `input` as the body of its endpoint.
    fn model(id: &str, name: &str, category: &str, input: Value) -> Value {
        json!({
            "endpoint_id": id,
            "metadata": {"display_name": name, "category": category, "status": "active"},
            "openapi": {
                "openapi": "3.0.4",
                "paths": {format!("/{id}"): {"post": {"requestBody": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Input"}}}}}}},
                "components": {"schemas": {"Input": input, "ImageSize": {"type": "object", "properties": {"width": {"type": "integer"}, "height": {"type": "integer"}}}}}
            }
        })
    }

    fn prompt_only() -> Value {
        json!({"properties": {"prompt": {"type": "string"}}, "required": ["prompt"]})
    }

    /// A picture model that makes every shape.
    fn picture_model(id: &str, name: &str) -> Value {
        let input = json!({
            "properties": {
                "prompt": {"type": "string"},
                "image_size": {"anyOf": [{"$ref": "#/components/schemas/ImageSize"}, {"type": "string", "enum": ["square_hd", "landscape_16_9", "portrait_16_9"]}], "default": "square_hd"}
            },
            "required": ["prompt"]
        });
        model(id, name, "text-to-image", input)
    }

    fn page(models: Vec<Value>, next: Option<&str>) -> String {
        json!({"models": models, "next_cursor": next, "has_more": next.is_some()}).to_string()
    }

    fn prices(rows: &[(&str, f64, &str)]) -> String {
        let rows: Vec<Value> = rows.iter().map(|(id, price, unit)| json!({"endpoint_id": id, "unit_price": price, "unit": unit, "currency": "USD"})).collect();
        json!({"prices": rows, "next_cursor": null, "has_more": false}).to_string()
    }

    fn fal(api_base: String) -> Fal {
        Fal { api_base, ..Fal::default() }
    }

    fn ids(list: &ModelList) -> Vec<&str> {
        list.models.iter().map(|m| m.id.as_str()).collect()
    }

    fn found<'a>(list: &'a ModelList, id: &str) -> &'a ListedModel {
        list.models.iter().find(|m| m.id == id).unwrap_or_else(|| panic!("{id} is not in {:?}", ids(list)))
    }

    #[test]
    fn models_of_a_category_are_listed_with_their_prices() {
        let (base, seen) = http::tests::serve(|_| {
            vec![
                (
                    200,
                    page(
                        vec![
                            picture_model("fal-ai/flux/schnell", "FLUX.1 [schnell]"),
                            model("fal-ai/flux/dev", "FLUX.1 [dev]", "text-to-image", prompt_only()),
                            // The model needs a mask this app has nothing to put in: it cannot be run, so it is not offered.
                            model(
                                "acme/needs-mask",
                                "Needs a mask",
                                "text-to-image",
                                json!({"properties": {"prompt": {"type": "string"}, "mask_url": {"type": "string"}}, "required": ["prompt", "mask_url"]}),
                            ),
                        ],
                        Some("Mg"),
                    ),
                ),
                (
                    200,
                    page(
                        vec![
                            model("acme/gpu-model", "GPU model", "text-to-image", prompt_only()),
                            model("acme/unpriced", "Unpriced", "text-to-image", prompt_only()),
                            model("acme/negative", "Negative", "text-to-image", prompt_only()),
                            model("acme/euro", "Euro", "text-to-image", prompt_only()),
                        ],
                        None,
                    ),
                ),
                (
                    200,
                    json!({"prices": [
                        {"endpoint_id": "fal-ai/flux/schnell", "unit_price": 0.003, "unit": "image", "currency": "USD"},
                        {"endpoint_id": "fal-ai/flux/dev", "unit_price": 0.025, "unit": "megapixels", "currency": "USD"},
                        {"endpoint_id": "acme/gpu-model", "unit_price": 0.0011, "unit": "gpu_second", "currency": "USD"},
                        {"endpoint_id": "acme/negative", "unit_price": -1.0, "unit": "image", "currency": "USD"},
                        {"endpoint_id": "acme/euro", "unit_price": 0.04, "unit": "image", "currency": "EUR"},
                        {"endpoint_id": "acme/elsewhere", "unit_price": 9.0, "unit": "image", "currency": "USD"}
                    ], "next_cursor": null, "has_more": false})
                    .to_string(),
                ),
            ]
        });
        let list = fal(base).list(Some(&format!("  {KEY} ")), AssetKind::Image).unwrap();
        assert_eq!(list.error, None);
        assert_eq!(ids(&list), ["fal-ai/flux/schnell", "fal-ai/flux/dev", "acme/gpu-model", "acme/unpriced", "acme/negative", "acme/euro"]);
        let usd = |rate| Pricing::Before { unit: Unit::Usd, rate };
        let schnell = found(&list, "fal-ai/flux/schnell");
        assert_eq!((schnell.provider.as_str(), schnell.name.as_str(), &schnell.kinds), ("fal", "FLUX.1 [schnell]", &vec![AssetKind::Image]));
        assert_eq!(schnell.pricing, usd(Rate::PerRequest(0.003)));
        assert!(schnell.ignores.is_empty(), "it makes every shape");
        let dev = found(&list, "fal-ai/flux/dev");
        assert_eq!(dev.pricing, usd(Rate::PerMegapixel(0.025)));
        assert_eq!(dev.ignores, [AssetOption::Shape], "no size input: the shape is among what it ignores");
        assert_eq!(found(&list, "acme/gpu-model").pricing, Pricing::After { unit: Unit::Usd });
        for unreadable in ["acme/unpriced", "acme/negative", "acme/euro"] {
            assert_eq!(found(&list, unreadable).pricing, Pricing::Unknown, "{unreadable}");
        }
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(seen[0].starts_with("GET /v1/models?category=text-to-image&expand=openapi-3.0&limit=10 "), "{}", seen[0]);
        assert!(seen[1].starts_with("GET /v1/models?category=text-to-image&expand=openapi-3.0&limit=10&cursor=Mg "), "{}", seen[1]);
        assert!(
            seen[2].starts_with("GET /v1/models/pricing?endpoint_id=fal-ai/flux/schnell&endpoint_id=fal-ai/flux/dev&endpoint_id=acme/gpu-model&"),
            "{}",
            seen[2]
        );
        assert!(!seen[2].contains("needs-mask"), "a model that cannot run is not priced");
        for request in seen.iter() {
            assert!(request.to_ascii_lowercase().contains("authorization: key k-123\r\n"), "the key, trimmed, in fal's form: {request}");
        }
    }

    #[test]
    fn without_a_key_models_are_listed_and_not_priced() {
        let (base, seen) = http::tests::serve(|_| vec![(200, page(vec![picture_model("fal-ai/flux/schnell", "FLUX.1 [schnell]")], None))]);
        let list = fal(base).list(None, AssetKind::Image).unwrap();
        assert_eq!((ids(&list), list.error.as_deref()), (vec!["fal-ai/flux/schnell"], None));
        assert_eq!(list.models[0].pricing, Pricing::Unknown);
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "prices need a key, so none are asked for");
        assert!(!seen[0].to_ascii_lowercase().contains("authorization"), "{}", seen[0]);
    }

    #[test]
    fn a_rate_limit_part_way_returns_what_was_read_and_says_so() {
        let (base, seen) = http::tests::serve(|_| {
            vec![
                (200, page(vec![picture_model("acme/one", "One")], Some("Mg"))),
                (200, page(vec![picture_model("acme/two", "Two")], Some("Mw"))),
                (429, json!({"error": {"type": "rate_limited", "message": "SECRET-BODY"}}).to_string()),
                (200, prices(&[("acme/one", 0.04, "image"), ("acme/two", 0.05, "image")])),
            ]
        });
        let list = fal(base).list(Some(KEY), AssetKind::Image).unwrap();
        assert_eq!(ids(&list), ["acme/one", "acme/two"], "the pages read before the limit are kept");
        let error = list.error.as_deref().unwrap();
        assert!(error.contains("incomplete") && error.contains("rate limit"), "{error}");
        assert!(!error.contains("SECRET-BODY"), "a provider's words are not repeated: {error}");
        assert_eq!(found(&list, "acme/two").pricing, Pricing::Before { unit: Unit::Usd, rate: Rate::PerRequest(0.05) }, "and the models read are priced");
        assert_eq!(seen.lock().unwrap().len(), 4);

        // Without a key the message says a key helps; with nothing read at all it is an error.
        let (base, _) = http::tests::serve(|_| vec![(200, page(vec![picture_model("acme/one", "One")], Some("Mg"))), (429, "{}".into())]);
        let list = fal(base).list(None, AssetKind::Image).unwrap();
        assert_eq!(ids(&list), ["acme/one"]);
        assert!(list.error.as_deref().is_some_and(|e| e.contains("fal.ai key")), "{:?}", list.error);
        let (base, _) = http::tests::serve(|_| vec![(429, "{}".into())]);
        let err = fal(base).list(Some(KEY), AssetKind::Image).unwrap_err();
        assert_eq!(err.code, ErrorCode::OverLimit);
        let (base, _) = http::tests::serve(|_| vec![(200, "not json".into())]);
        assert_eq!(fal(base).list(Some(KEY), AssetKind::Image).unwrap_err().code, ErrorCode::GarbledOutput);
    }

    #[test]
    fn prices_that_cannot_be_read_are_reported_beside_the_models() {
        let (base, _) = http::tests::serve(|_| vec![(200, page(vec![picture_model("acme/one", "One")], None)), (401, "{}".into())]);
        let list = fal(base).list(Some(KEY), AssetKind::Image).unwrap();
        assert_eq!(ids(&list), ["acme/one"]);
        assert_eq!(list.models[0].pricing, Pricing::Unknown);
        assert!(list.error.as_deref().is_some_and(|e| e.starts_with("Prices could not be read")), "{:?}", list.error);
    }

    #[test]
    fn a_list_stops_at_the_page_limit_and_says_so() {
        let (base, seen) = http::tests::serve(|_| (0..MAX_PAGES).map(|i| (200, page(vec![picture_model(&format!("acme/m{i}"), "M")], Some("Mg")))).collect());
        let list = fal(base).list(None, AssetKind::Image).unwrap();
        assert_eq!(list.models.len(), MAX_PAGES);
        assert_eq!(seen.lock().unwrap().len(), MAX_PAGES);
        assert!(list.error.as_deref().is_some_and(|e| e.contains("incomplete")), "{:?}", list.error);
    }

    #[test]
    fn audio_category_uses_the_known_models_table() {
        // fal files speech, sound effects and music models alike as `text-to-audio`.
        let audio = || {
            let with_length =
                json!({"properties": {"prompt": {"type": "string"}, "duration": {"type": "number", "minimum": 1, "maximum": 30}}, "required": ["prompt"]});
            vec![
                model("cassetteai/music-generator", "Music generator", "text-to-audio", with_length.clone()),
                model("fal-ai/elevenlabs/sound-effects/v2", "Sound effects", "text-to-audio", with_length),
                model("fal-ai/csm-1b", "CSM-1B", "text-to-audio", json!({"properties": {"text": {"type": "string"}}, "required": ["text"]})),
                // Not in the table: nothing says what it makes, so it is offered for none of the three.
                model("acme/mystery-audio", "Mystery", "text-to-audio", prompt_only()),
            ]
        };
        let (base, seen) = http::tests::serve(|_| vec![(200, page(audio(), None))]);
        let music = fal(base).list(None, AssetKind::Music).unwrap();
        assert_eq!(ids(&music), ["cassetteai/music-generator"]);
        assert_eq!(music.models[0].kinds, [AssetKind::Music]);
        assert_eq!(music.models[0].ignores, [AssetOption::Instrumental], "a length, but no instrumental switch");
        assert!(seen.lock().unwrap()[0].starts_with("GET /v1/models?category=text-to-audio&expand=openapi-3.0&limit=10 "));

        let (base, _) = http::tests::serve(|_| vec![(200, page(audio(), None))]);
        assert_eq!(ids(&fal(base).list(None, AssetKind::SoundEffect).unwrap()), ["fal-ai/elevenlabs/sound-effects/v2"]);

        // Speech is also fal's own `text-to-speech` category, which needs no table.
        let (base, seen) = http::tests::serve(|_| {
            vec![
                (
                    200,
                    page(
                        vec![model(
                            "acme/native-tts",
                            "Native TTS",
                            "text-to-speech",
                            json!({"properties": {"text": {"type": "string"}}, "required": ["text"]}),
                        )],
                        None,
                    ),
                ),
                (200, page(audio(), None)),
            ]
        });
        let speech = fal(base).list(None, AssetKind::Speech).unwrap();
        assert_eq!(ids(&speech), ["acme/native-tts", "fal-ai/csm-1b"]);
        assert_eq!(speech.models[1].kinds, [AssetKind::Speech]);
        let seen = seen.lock().unwrap();
        assert!(seen[0].starts_with("GET /v1/models?category=text-to-speech&"), "{}", seen[0]);
        assert!(seen[1].starts_with("GET /v1/models?category=text-to-audio&") && !seen[1].contains("cursor"), "{}", seen[1]);
    }

    #[test]
    fn three_d_models_come_from_both_categories_without_repeats() {
        let from_picture = || json!({"properties": {"image_url": {"type": "string"}}, "required": ["image_url"]});
        let (base, seen) = http::tests::serve(|_| {
            vec![
                (200, page(vec![model("tripo3d/p1/image-to-3d", "Tripo P1", "image-to-3d", from_picture())], None)),
                (
                    200,
                    page(
                        vec![
                            model("acme/text-to-3d", "Text to 3D", "text-to-3d", prompt_only()),
                            model("tripo3d/p1/image-to-3d", "Tripo again", "text-to-3d", from_picture()),
                        ],
                        None,
                    ),
                ),
            ]
        });
        let list = fal(base).list(None, AssetKind::Model3d).unwrap();
        assert_eq!(ids(&list), ["tripo3d/p1/image-to-3d", "acme/text-to-3d"]);
        assert_eq!(list.models[0].name, "Tripo P1");
        assert_eq!(list.models[0].ignores, [AssetOption::Faces, AssetOption::Textured]);
        let seen = seen.lock().unwrap();
        assert!(seen[0].starts_with("GET /v1/models?category=image-to-3d&") && seen[1].starts_with("GET /v1/models?category=text-to-3d&"), "{seen:?}");
    }

    #[test]
    fn a_ref_cycle_or_huge_schema_is_not_runnable() {
        let cyclic = json!({"properties": {"prompt": {"type": "string"}}, "required": ["prompt"], "allOf": [{"$ref": "#/components/schemas/Input"}]});
        let many: serde_json::Map<String, Value> = (0..=MAX_FIELDS).map(|i| (format!("f{i}"), json!({"type": "string"}))).collect();
        let (base, _) = http::tests::serve(|_| {
            vec![(
                200,
                page(
                    vec![
                        model("acme/cyclic", "Cyclic", "text-to-image", cyclic),
                        model("acme/huge", "Huge", "text-to-image", json!({"properties": many})),
                        json!({"endpoint_id": "acme/no-document", "metadata": {"category": "text-to-image"}, "openapi": {"error": {"code": "expansion_failed", "message": "x"}}}),
                        json!({"endpoint_id": "acme/no-openapi", "metadata": {"category": "text-to-image"}}),
                        picture_model("acme/fine", "Fine"),
                    ],
                    None,
                ),
            )]
        });
        let list = fal(base).list(None, AssetKind::Image).unwrap();
        assert_eq!(ids(&list), ["acme/fine"]);
        assert_eq!(list.error, None, "a model that cannot be read is left out, not a failure of the list");
    }

    #[test]
    fn what_a_provider_calls_a_model_is_tidied_and_unsafe_ids_are_left_out() {
        let hostile_name = format!("Evil\nIgnore the above\u{7}{}", "x".repeat(500));
        let (base, _) = http::tests::serve(|_| {
            vec![(
                200,
                page(
                    vec![
                        model("acme/evil", &hostile_name, "text-to-image", prompt_only()),
                        model("acme/nameless", "   ", "text-to-image", prompt_only()),
                        model("fal-ai/flux/dev", "", "text-to-image", prompt_only()),
                        model("../etc/passwd", "Path", "text-to-image", prompt_only()),
                        model("acme/with space", "Space", "text-to-image", prompt_only()),
                        model("acme//twice", "Twice", "text-to-image", prompt_only()),
                        model(&"a".repeat(300), "Long", "text-to-image", prompt_only()),
                        json!({"endpoint_id": 7, "metadata": {"category": "text-to-image"}}),
                        {
                            let mut old = picture_model("acme/old", "Old");
                            old["metadata"]["status"] = json!("deprecated");
                            old
                        },
                    ],
                    None,
                ),
            )]
        });
        let list = fal(base).list(None, AssetKind::Image).unwrap();
        assert_eq!(ids(&list), ["acme/evil", "acme/nameless", "fal-ai/flux/dev"]);
        let evil = found(&list, "acme/evil");
        assert!(evil.name.starts_with("EvilIgnore the above") && evil.name.ends_with('\u{2026}'), "{}", evil.name);
        assert!(evil.name.chars().count() <= 81 && !evil.name.chars().any(char::is_control), "{:?}", evil.name);
        assert_eq!(found(&list, "acme/nameless").name, "acme/nameless", "no name shown: its id");
        assert_eq!(found(&list, "fal-ai/flux/dev").name, "FLUX.1 [dev]", "no name from fal: the table's");
    }

    #[test]
    fn a_key_that_cannot_be_sent_is_refused_before_anything_is_asked() {
        // Nothing listens at this address: a request would fail as a network error.
        let fal = fal("http://127.0.0.1:9".into());
        for key in ["", "  ", "k\u{e9}y", "k\nx"] {
            assert_eq!(fal.list(Some(key), AssetKind::Image).unwrap_err().code, ErrorCode::BadSettings, "{key:?}");
            assert_eq!(fal.describe(key, "fal-ai/flux/dev").unwrap_err().code, ErrorCode::BadSettings, "{key:?}");
        }
        for model in ["", "../x", "a b", "a//b", "a/./b", "/lead", "trail/", "q?x=1", "é"] {
            let err = fal.describe(KEY, model).unwrap_err();
            assert_eq!(err.code, ErrorCode::BadSettings, "{model:?}");
            assert!(err.remediation.is_some());
        }
    }

    #[test]
    fn a_model_is_described_with_its_inputs_and_price() {
        let (base, seen) = http::tests::serve(|_| {
            vec![(200, page(vec![picture_model("fal-ai/flux/schnell", "FLUX.1 [schnell]")], None)), (200, prices(&[("fal-ai/flux/schnell", 0.003, "image")]))]
        });
        let detail = fal(base).describe(KEY, "fal-ai/flux/schnell").unwrap();
        assert_eq!((detail.id.as_str(), detail.name.as_str(), &detail.kinds), ("fal-ai/flux/schnell", "FLUX.1 [schnell]", &vec![AssetKind::Image]));
        assert_eq!(detail.pricing, Pricing::Before { unit: Unit::Usd, rate: Rate::PerRequest(0.003) });
        let size = detail.fields.iter().find(|f| f.name == "image_size").unwrap();
        assert_eq!(size.kind, FieldKind::Choice(vec!["square_hd".into(), "landscape_16_9".into(), "portrait_16_9".into()]));
        assert!(detail.fields.iter().any(|f| f.name == "prompt" && f.required));
        let seen = seen.lock().unwrap();
        assert!(seen[0].starts_with("GET /v1/models?endpoint_id=fal-ai/flux/schnell&expand=openapi-3.0 "), "{}", seen[0]);
        assert!(seen[1].starts_with("GET /v1/models/pricing?endpoint_id=fal-ai/flux/schnell "), "{}", seen[1]);
        assert!(seen.iter().all(|r| r.to_ascii_lowercase().contains("authorization: key k-123\r\n")));
    }

    #[test]
    fn a_model_fal_has_no_price_for_or_that_makes_nothing_we_know_is_still_described() {
        // A model's category does not decide what it can be used for, so a model the table does not
        // know has no kinds; the price may be missing too.
        let (base, _) =
            http::tests::serve(|_| vec![(200, page(vec![model("acme/mystery-audio", "Mystery", "text-to-audio", prompt_only())], None)), (200, prices(&[]))]);
        let detail = fal(base).describe(KEY, "acme/mystery-audio").unwrap();
        assert_eq!((detail.kinds.len(), detail.pricing), (0, Pricing::Unknown));
        // A price that cannot be read is an error, not a free run.
        let (base, _) = http::tests::serve(|_| vec![(200, page(vec![picture_model("acme/one", "One")], None)), (401, "{}".into())]);
        assert_eq!(fal(base).describe(KEY, "acme/one").unwrap_err().code, ErrorCode::BadSettings);
        // And an input document that cannot be read is an error that says what to do.
        let (base, _) = http::tests::serve(|_| vec![(200, page(vec![json!({"endpoint_id": "acme/one", "metadata": {"category": "text-to-image"}})], None))]);
        let err = fal(base).describe(KEY, "acme/one").unwrap_err();
        assert_eq!(err.code, ErrorCode::GarbledOutput);
        assert!(err.remediation.is_some());
    }

    #[test]
    fn a_removed_model_is_a_clear_error() {
        let (base, _) = http::tests::serve(|_| {
            vec![
                (404, json!({"error": {"type": "not_found", "message": "SECRET-BODY"}}).to_string()),
                (200, page(vec![], None)),
                (200, page(vec![picture_model("acme/other", "Other")], None)),
            ]
        });
        let fal = fal(base);
        for _ in 0..3 {
            let err = fal.describe(KEY, "acme/gone").unwrap_err();
            assert_eq!(err.code, ErrorCode::BadSettings);
            assert!(err.message.contains("acme/gone") && !err.message.contains("SECRET-BODY"), "{}", err.message);
            let remediation = err.remediation.as_deref().unwrap();
            assert!(remediation.contains("Choose another model") && remediation.contains("Generated Assets"), "{remediation}");
        }
    }

    /// The design: a chosen model that is deprecated at the provider fails with a typed error that
    /// names it and says to choose another, the same as one that has gone.
    #[test]
    fn a_model_being_retired_is_a_clear_error_too() {
        let mut retiring = picture_model("acme/retiring", "Retiring");
        retiring["metadata"]["status"] = json!("deprecated");
        let (base, seen) = http::tests::serve(|_| vec![(200, page(vec![retiring], None))]);
        let err = fal(base).describe(KEY, "acme/retiring").unwrap_err();
        assert_eq!(err.code, ErrorCode::BadSettings);
        assert!(err.message.contains("acme/retiring") && err.message.contains("retired"), "{}", err.message);
        let remediation = err.remediation.as_deref().unwrap();
        assert!(remediation.contains("Choose another model") && remediation.contains("Generated Assets"), "{remediation}");
        assert_eq!(seen.lock().unwrap().len(), 1, "no price is asked for a model that cannot be used");
        // A model whose status fal gives as active, or does not give, is described.
        let mut bare = picture_model("acme/bare", "Bare");
        bare["metadata"].as_object_mut().unwrap().remove("status");
        let (base, _) = http::tests::serve(|_| {
            vec![(200, page(vec![picture_model("acme/active", "Active")], None)), (200, prices(&[])), (200, page(vec![bare], None)), (200, prices(&[]))]
        });
        let fal = fal(base);
        assert!(fal.describe(KEY, "acme/active").is_ok());
        assert!(fal.describe(KEY, "acme/bare").is_ok());
    }

    #[test]
    fn a_page_is_read_only_up_to_its_bound() {
        let crowd: Vec<Value> = (0..MAX_PAGE_MODELS + 50).map(|i| picture_model(&format!("acme/m{i}"), "M")).collect();
        let (base, _) = http::tests::serve(|_| vec![(200, page(crowd, None))]);
        let list = fal(base).list(None, AssetKind::Image).unwrap();
        assert_eq!(list.models.len(), MAX_PAGE_MODELS, "a server that sends more than a page can hold is not believed");
        assert_eq!(list.models.last().map(|m| m.id.as_str()), Some("acme/m99"));
    }

    #[test]
    fn a_page_larger_than_the_limit_is_refused() {
        // One byte over what a page may be: the whole body is read, the page is not.
        let (base, _) = http::tests::serve(|_| vec![(200, "x".repeat(usize::try_from(http::MAX_LIST_PAGE).unwrap() + 1))]);
        let err = fal(base).list(Some(KEY), AssetKind::Image).unwrap_err();
        assert_eq!(err.code, ErrorCode::OverLimit);
        assert!(!err.message.contains("xxxx"), "{}", err.message);
    }

    #[test]
    fn a_next_page_that_cannot_be_asked_for_ends_the_list_and_says_so() {
        for (cursor, has_more) in [(json!("a".repeat(10_000)), true), (json!(null), true), (json!(""), true), (json!("a".repeat(CURSOR_LENGTH + 1)), false)] {
            let first = json!({"models": [picture_model("acme/one", "One")], "next_cursor": cursor, "has_more": has_more}).to_string();
            let (base, seen) = http::tests::serve(|_| vec![(200, first)]);
            let list = fal(base).list(None, AssetKind::Image).unwrap();
            assert_eq!(ids(&list), ["acme/one"]);
            assert_eq!(seen.lock().unwrap().len(), 1, "no cursor, no second page");
            // A page that says it is the last one is just the last one, whatever else it carries.
            assert_eq!(list.error.is_some(), has_more, "{cursor}: {:?}", list.error);
            if has_more {
                assert!(list.error.as_deref().is_some_and(|e| e.contains("incomplete") && e.contains("next page")), "{:?}", list.error);
            }
        }
        // A cursor of the longest allowed size is followed.
        let longest = "a".repeat(CURSOR_LENGTH);
        let (base, seen) = http::tests::serve(|_| vec![(200, page(vec![], Some(&longest))), (200, page(vec![], None))]);
        assert_eq!(fal(base).list(None, AssetKind::Image).unwrap().error, None);
        assert!(seen.lock().unwrap()[1].contains(&format!("&cursor={longest} ")));
    }

    /// The deadline covers the whole call: pages and price batches alike.
    #[test]
    fn a_list_that_takes_too_long_is_cut_short() {
        // The one answer arrives after the deadline, so no second page and no prices are asked for.
        let (base, seen) = http::tests::serve_after(Duration::from_millis(800), |_| vec![(200, page(vec![picture_model("acme/one", "One")], Some("Mg")))]);
        let slow = Fal { list_deadline: Duration::from_millis(500), ..fal(base) };
        let list = slow.list(Some(KEY), AssetKind::Image).unwrap();
        assert_eq!(ids(&list), ["acme/one"], "what was read is kept");
        assert_eq!(list.models[0].pricing, Pricing::Unknown);
        let error = list.error.as_deref().unwrap();
        assert!(error.contains("incomplete") && error.contains("cut short"), "{error}");
        assert_eq!(seen.lock().unwrap().len(), 1);
        // Out of time before the first request: nothing was read, so it is an error.
        let none = Fal { list_deadline: Duration::ZERO, ..fal("http://127.0.0.1:9".into()) };
        assert_eq!(none.list(Some(KEY), AssetKind::Image).unwrap_err().code, ErrorCode::TimedOut);
    }

    #[test]
    fn prices_that_say_there_are_more_are_not_left_silently_unknown() {
        for more in [json!({"next_cursor": "Mg", "has_more": true}), json!({"next_cursor": "Mg"}), json!({"next_cursor": null, "has_more": true})] {
            let mut answer = json!({"prices": [{"endpoint_id": "acme/one", "unit_price": 0.04, "unit": "image", "currency": "USD"}]});
            answer.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
            let (base, _) = http::tests::serve(|_| {
                vec![(200, page(vec![picture_model("acme/one", "One"), picture_model("acme/two", "Two")], None)), (200, answer.to_string())]
            });
            let list = fal(base).list(Some(KEY), AssetKind::Image).unwrap();
            assert_eq!(list.models[0].pricing, Pricing::Before { unit: Unit::Usd, rate: Rate::PerRequest(0.04) }, "what came is used");
            assert_eq!(list.models[1].pricing, Pricing::Unknown);
            assert!(list.error.as_deref().is_some_and(|e| e.starts_with("Prices could not be read") && e.contains("more")), "{more}: {:?}", list.error);
        }
        // A price list that says it is complete is complete, though a model has no row in it.
        let (base, _) = http::tests::serve(|_| vec![(200, page(vec![picture_model("acme/one", "One")], None)), (200, prices(&[]))]);
        assert_eq!(fal(base).list(Some(KEY), AssetKind::Image).unwrap().error, None);
        // For one model the price is needed: a list that says it is incomplete, with no row, is no price.
        let (base, _) = http::tests::serve(|_| {
            vec![(200, page(vec![picture_model("acme/one", "One")], None)), (200, json!({"prices": [], "next_cursor": "Mg", "has_more": true}).to_string())]
        });
        assert_eq!(fal(base).describe(KEY, "acme/one").unwrap_err().code, ErrorCode::GarbledOutput);
    }

    /// One table says which categories a kind is read from and what a model in a category makes.
    #[test]
    fn the_category_asked_for_decides_what_a_model_makes() {
        // The page asked for text-to-3d echoes another category on its model: the page decides.
        let echoed = model("acme/echo", "Echo", "image-to-video", json!({"properties": {"image_url": {"type": "string"}}, "required": ["image_url"]}));
        let (base, _) = http::tests::serve(|_| vec![(200, page(vec![echoed], None)), (200, page(vec![], None))]);
        let list = fal(base).list(None, AssetKind::Model3d).unwrap();
        assert_eq!(ids(&list), ["acme/echo"]);
        assert_eq!(list.models[0].kinds, [AssetKind::Model3d]);
        // The same table in both directions: every kind has a category, and a category that
        // settles its kind is read for that kind alone.
        for kind in AssetKind::ALL {
            assert!(!categories(kind).is_empty(), "{kind:?}");
            for category in categories(kind) {
                let makes = kinds_of(category, "acme/unknown");
                assert!(makes.is_empty() || makes == [kind], "{category} for {kind:?}: {makes:?}");
            }
        }
        assert_eq!(categories(AssetKind::Music), ["text-to-audio"]);
        assert_eq!(categories(AssetKind::Speech), ["text-to-speech", "text-to-audio"]);
        // A model in an audio category makes what the table says of it, among the audio kinds only.
        assert_eq!(kinds_of("text-to-audio", "fal-ai/csm-1b"), [AssetKind::Speech]);
        assert!(kinds_of("text-to-audio", "fal-ai/flux/dev").is_empty(), "the table says it makes pictures: not an audio kind");
        assert!(kinds_of("text-to-audio", "acme/unknown").is_empty());
        // And a category the table has no row for falls back to the known-models table.
        assert_eq!(kinds_of("image-to-image", "fal-ai/flux/dev"), [AssetKind::Image]);
        assert!(kinds_of("image-to-image", "acme/unknown").is_empty());
    }

    #[test]
    fn units_are_read_as_how_a_price_is_charged() {
        let rate = |unit: &str, price: f64| pricing_of(price, unit, "USD");
        let usd = |rate| Pricing::Before { unit: Unit::Usd, rate };
        for unit in ["image", "Images", "video", "request", "Requests"] {
            assert_eq!(rate(unit, 0.04), usd(Rate::PerRequest(0.04)), "{unit}");
        }
        for unit in ["megapixels", "megapixel", "Megapixel"] {
            assert_eq!(rate(unit, 0.025), usd(Rate::PerMegapixel(0.025)), "{unit}");
        }
        // Everything else is a quantity this app cannot know before the run: GPU time, a plain
        // "second" (video seconds or compute seconds, the page does not say), a unit nobody has
        // documented (spellings of a character, an output, a generation, ...) or one not seen yet.
        for unit in [
            "gpu_second",
            "gpu-seconds",
            "second",
            "H100_second",
            "character",
            "1000_characters",
            "audio_second",
            "output_second",
            "output",
            "generation",
            "mp",
            "",
            "something new",
        ] {
            assert_eq!(rate(unit, 0.01), Pricing::After { unit: Unit::Usd }, "{unit:?}");
        }
        // A price that is not a plain dollar amount is no price at all (Ruling 5): not a number, below
        // nothing, or (Ruling 16) above the most a project may spend, which no real model charges.
        for price in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.001, -1e300, 1e300, MAX_ASSET_BUDGET_USD + 0.01] {
            assert_eq!(rate("image", price), Pricing::Unknown, "{price}");
            assert_eq!(rate("gpu_second", price), Pricing::Unknown, "{price}");
        }
        assert_eq!(rate("image", MAX_ASSET_BUDGET_USD), usd(Rate::PerRequest(MAX_ASSET_BUDGET_USD)), "the most is still a price");
        let absurd = price_row(&json!({"endpoint_id": "acme/absurd", "unit_price": 1e300, "unit": "image", "currency": "USD"}));
        assert_eq!(absurd, Some(("acme/absurd".to_string(), Pricing::Unknown)));
        assert_eq!(rate("image", 0.0), usd(Rate::PerRequest(0.0)), "free is a price");
        for currency in ["EUR", "", "credits"] {
            assert_eq!(pricing_of(1.0, "image", currency), Pricing::Unknown, "{currency:?}");
        }
        assert_eq!(pricing_of(1.0, "image", "usd"), usd(Rate::PerRequest(1.0)));
    }

    #[test]
    fn endpoint_ids_are_checked() {
        for ok in ["fal-ai/flux/dev", "tripo3d/p1/image-to-3d", "fal-ai/flux-2/klein/9b", "a.b/c_d"] {
            assert!(endpoint_ok(ok), "{ok}");
        }
        for bad in ["", "/a", "a/", "a//b", "../a", "a/..", "a/./b", "a b", "a?b", "a#b", "a\\b", "a:b", "é", &"a".repeat(101)] {
            assert!(!endpoint_ok(bad), "{bad:?}");
        }
    }
}
