//! Scenario's asynchronous generation, in credits, with a mandatory price check: every run is quoted
//! by a dry run before it is submitted, and what it cost is read from the finished job.
//! [`Scenario::list`] and [`Scenario::describe`] read Scenario's own list of public models, each
//! model's inputs and what it costs in credits; a run sends the request laid out in the model's
//! inputs ([`map_request`]).
use super::inputs::{FieldKind, InputField, MAX_CHOICES, MAX_FIELDS, Mapped, REMEDIATION, map_request, runnable};
use super::model::{AssetOption, ListedModel, MAX_ASSET_CREDITS, ModelDetail, ModelList, Pricing, Rate, Unit};
use super::tables::{InputRoles, KnownModels};
use super::{AssetKind, AssetProvider, AssetRequest, Described, Generated, charged, http};
use craft_anything_evidence::{ErrorCode, ToolError};
use http::{CURSOR_LENGTH, time_is_up, words};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub const ID: &str = "scenario";
/// What people call it.
const NAME: &str = "Scenario";
/// The largest inline picture (a `data:` URI) sent to start a model from.
const MAX_INLINE_PICTURE: usize = 28 << 20;
/// The longest description or text sent.
const MAX_TEXT: usize = 10_000;

pub struct Scenario {
    base: String,
    timeout: Duration,
    poll_interval: Duration,
    /// How long [`Scenario::list`] may go on starting requests.
    list_deadline: Duration,
    /// The models described lately.
    described: Described,
}
impl Default for Scenario {
    /// Use the production API with a ten-minute workflow deadline and two-second polling interval.
    fn default() -> Self {
        Self {
            base: "https://api.cloud.scenario.com/v1".into(),
            timeout: Duration::from_secs(600),
            poll_interval: Duration::from_secs(2),
            list_deadline: LIST_DEADLINE,
            described: Described::default(),
        }
    }
}

/// Where a run of `model` is sent, and the request laid out in its inputs. A picture to start from
/// must be a Scenario asset id, an inline picture or an HTTPS address; nothing else is sent.
fn request(model: &ModelDetail, req: &AssetRequest) -> Result<(String, Mapped), ToolError> {
    if http::id(&model.id).is_err() {
        return Err(http::bad("That is not an id Scenario gives its models.").with_remediation(REMEDIATION));
    }
    match *req {
        AssetRequest::Image { prompt: text, .. }
        | AssetRequest::Speech { text, .. }
        | AssetRequest::SoundEffect { prompt: text, .. }
        | AssetRequest::Music { prompt: text, .. } => http::text(text, MAX_TEXT)?,
        AssetRequest::Model3d { image_url, prompt, .. } => {
            if let Some(url) = image_url.filter(|u| !u.trim().is_empty()) {
                picture_ok(url)?;
            }
            if let Some(text) = prompt.filter(|p| !p.trim().is_empty()) {
                http::text(text, MAX_TEXT)?;
            }
        }
    }
    Ok((format!("generate/custom/{}", model.id), map_request(&model.fields, req, InputRoles::built_in())?))
}

/// Whether a picture can be sent to Scenario: an asset id of its own, an inline picture of bounded
/// size, or an HTTPS address without credentials.
fn picture_ok(url: &str) -> Result<(), ToolError> {
    if url.starts_with("asset_") {
        http::id(url)
    } else if url.starts_with("data:image/") {
        if url.len() > MAX_INLINE_PICTURE || !url.contains(";base64,") {
            return Err(http::bad("The Scenario input image is too large or malformed."));
        }
        Ok(())
    } else {
        http::media_url(url).map_err(|_| http::bad("The picture to start from must be an HTTPS address, a Scenario asset or an inline picture."))
    }
}

/// Return the remaining workflow time, or a timeout warning that the remote job may still charge.
fn remaining(start: Instant, timeout: Duration) -> Result<Duration, ToolError> {
    timeout.checked_sub(start.elapsed()).filter(|v| !v.is_zero()).ok_or_else(|| {
        ToolError::new(ErrorCode::ProgramFailed, "Scenario generation timed out. The remote job may still finish and incur a charge.")
            .with_remediation("Check the job in Scenario before generating again.")
    })
}

/// A number of credits Scenario reported that can be believed: finite, not negative, and no more
/// than a limit can be.
fn believable(credits: f64) -> bool {
    credits.is_finite() && (0.0..=MAX_ASSET_CREDITS).contains(&credits)
}

impl Scenario {
    /// Send an authenticated JSON request, requiring status 269 to confirm an API-tier price check.
    /// `charge` is the quoted cost of a paid submission: once Scenario accepts it, an unreadable
    /// answer still reports that charge. Refusals (non-success statuses) are never charged.
    fn call(&self, auth: &str, path: &str, body: Option<&Value>, timeout: Duration, charge: Option<f64>) -> Result<Value, ToolError> {
        let agent = http::agent(timeout);
        let url = format!("{}/{path}", self.base);
        let response = match body {
            Some(body) => agent.post(&url).header("Authorization", auth).header("Content-Type", "application/json").send(body.to_string()),
            // A GET (a job's state, an asset's address) starts nothing paid, so it may be asked again.
            None => http::get(|| agent.get(&url).header("Authorization", auth).call()),
        }
        .map_err(|_| http::network())?;
        http::status(response.status().as_u16())?;
        if path.ends_with("?dryRun=api") && response.status().as_u16() != 269 {
            return Err(http::garbled("Scenario did not confirm an API-tier price check; check your account before trying again."));
        }
        http::json(response.into_body()).map_err(|e| match charge {
            Some(cost) => charged(e, cost),
            None => e,
        })
    }

    /// Scenario's quote for one run, in credits, from a dry run of the very request that would be
    /// submitted. A quote that is missing, not a number, negative or more than a limit can be is
    /// refused, and so is an answer that started a job: nothing is submitted after either. A quote
    /// of nothing is a free run.
    fn quote(&self, auth: &str, path: &str, body: &Value, start: Instant) -> Result<f64, ToolError> {
        let quote = self.call(auth, &format!("{path}?dryRun=api"), Some(body), remaining(start, self.timeout)?, None)?;
        // Fail closed if a server ignores dryRun: never follow it with another paid request.
        if quote.get("job").is_some() || quote.get("jobId").is_some() {
            return Err(http::garbled("Scenario returned a job during the price check; check your account before trying again."));
        }
        quote
            .get("creativeUnitsCost")
            .and_then(Value::as_f64)
            .filter(|&credits| believable(credits))
            .ok_or_else(|| http::garbled("Scenario did not provide a valid cost quote; generation was not started."))
    }
}
impl AssetProvider for Scenario {
    /// Return the identifier a project's choice names.
    fn id(&self) -> &'static str {
        ID
    }
    /// Return the provider's display name.
    fn name(&self) -> &'static str {
        NAME
    }
    /// Name the environment variable containing the combined key and secret.
    fn key_env(&self) -> Option<&'static str> {
        Some("SCENARIO_API_KEY")
    }
    /// Return the Scenario account site used for credential setup.
    fn keys_page(&self) -> Option<&'static str> {
        Some("https://app.scenario.com")
    }
    /// Request a separate masked input for the secret paired with the API key.
    fn key_secret_label(&self) -> Option<&'static str> {
        Some("API secret")
    }
    /// Require a header-safe credential with nonempty key and secret components separated by a colon.
    fn validate_key(&self, key: &str) -> Result<(), ToolError> {
        let key = http::key(key)?;
        if !key.split_once(':').is_some_and(|(k, s)| !k.is_empty() && !s.is_empty()) {
            return Err(http::bad("Enter both the Scenario API key and API secret in Settings → AI."));
        }
        Ok(())
    }
    fn unit(&self) -> Unit {
        Unit::Credits
    }
    fn list_models(&self, key: Option<&str>, kind: AssetKind) -> Result<ModelList, ToolError> {
        self.list(key, kind)
    }
    fn describe(&self, key: &str, model: &str) -> Result<ModelDetail, ToolError> {
        Scenario::describe(self, key, model)
    }
    /// Scenario's own quote for this very run (a dry run that starts nothing), whatever the
    /// model's listed price says.
    fn estimate(&self, key: &str, model: &ModelDetail, req: &AssetRequest) -> Result<Option<f64>, ToolError> {
        let auth = self.auth(key)?;
        let (path, Mapped { body, .. }) = request(model, req)?;
        Ok(Some(self.quote(&auth, &path, &Value::Object(body), Instant::now())?))
    }
    /// Quote the run, submit it once, and poll and download within one deadline. What it cost is
    /// the finished job's own count of credits, else the quote.
    fn generate(&self, key: &str, model: &ModelDetail, req: &AssetRequest) -> Result<Generated, ToolError> {
        self.generate_within(key, model, req, None)
    }
    /// [`AssetProvider::generate`], refusing with nothing spent when the fresh quote is above
    /// `approved`, the quote the limit was checked against: the price rose since it was checked.
    fn generate_within(&self, key: &str, model: &ModelDetail, req: &AssetRequest, approved: Option<f64>) -> Result<Generated, ToolError> {
        let auth = self.auth(key)?;
        let (path, Mapped { body, ignored }) = request(model, req)?;
        let body = Value::Object(body);
        let start = Instant::now();
        let quote = self.quote(&auth, &path, &body, start)?;
        if let Some(approved) = approved
            && quote > approved + 1e-9
        {
            return Err(ToolError::new(
                ErrorCode::OverLimit,
                format!("Scenario's price rose since it was checked ({quote} credits now, {approved} credits approved); nothing was started."),
            )
            .with_remediation("Try again: the limit is checked against the new price."));
        }
        // Accepted from its successful status on: an unreadable answer and every later failure report the quoted charge.
        let submitted = self.call(&auth, &path, Some(&body), remaining(start, self.timeout)?, Some(quote))?;
        let (url, bytes, billed) = self.finish(&auth, &submitted, start).map_err(|e| charged(e, quote))?;
        Ok(Generated { url, bytes, cost: Some(billed.unwrap_or(quote)), ignored })
    }
}

impl Scenario {
    /// Poll a submitted job, resolve its asset and download it within the workflow deadline. Also
    /// the credits the finished job says it cost (`job.billing.cuCost`), when it says so believably.
    fn finish(&self, auth: &str, submitted: &Value, start: Instant) -> Result<(String, Vec<u8>, Option<f64>), ToolError> {
        let job_id = submitted.pointer("/job/jobId").and_then(Value::as_str).ok_or_else(|| http::garbled("Scenario returned no job identifier."))?;
        http::id(job_id)?;
        loop {
            let job = self.call(auth, &format!("jobs/{job_id}"), None, remaining(start, self.timeout)?, None)?;
            match job.pointer("/job/status").and_then(Value::as_str) {
                Some("success") => {
                    let billed = job.pointer("/job/billing/cuCost").and_then(Value::as_f64).filter(|&credits| believable(credits));
                    let asset_id =
                        job.pointer("/job/metadata/assetIds/0").and_then(Value::as_str).ok_or_else(|| http::garbled("Scenario completed without an asset."))?;
                    http::id(asset_id)?;
                    let asset = self.call(auth, &format!("assets/{asset_id}"), None, remaining(start, self.timeout)?, None)?;
                    let url = asset
                        .pointer("/asset/url")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| http::garbled("Scenario returned no asset download address."))?
                        .to_string();
                    let bytes = http::download(&url, remaining(start, self.timeout)?)?;
                    return Ok((url, bytes, billed));
                }
                Some("failed" | "failure" | "cancelled" | "canceled") => {
                    return Err(ToolError::new(ErrorCode::ProgramFailed, "Scenario could not complete this generation.")
                        .with_remediation("Check the job in Scenario; revise the prompt or input before retrying."));
                }
                Some("queued" | "in-progress" | "pending" | "processing" | "warming-up" | "finalizing") => {
                    std::thread::sleep(self.poll_interval.min(remaining(start, self.timeout)?))
                }
                _ => return Err(http::garbled("Scenario returned an unknown job status.")),
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Listing: the models Scenario serves, their inputs and what they cost in credits.
// ---------------------------------------------------------------------------------------------

/// Models in a page of the public list: the most Scenario gives (`pageSize` has a maximum of 500,
/// https://docs.scenario.com/api/resources/models/methods/list, read 2026-11-03).
const PAGE_SIZE: usize = 500;
/// The most pages read for one list. Five hundred a page, that is several times the models Scenario has.
const MAX_PAGES: usize = 8;
/// The most models taken from one page, whatever it holds.
const MAX_PAGE_MODELS: usize = PAGE_SIZE;
/// The most models whose inputs and prices are asked for in one list: one request each.
const MAX_DESCRIBED: usize = 300;
/// How many of those requests are made at the same time.
const WORKERS: usize = 4;
/// The longest input name read.
const FIELD_NAME_LENGTH: usize = 128;
/// The most tags, capabilities and price variants of one model that are read.
const MAX_LABELS: usize = 100;
/// How long one request for a page of the list, or for one model, may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(60);
const MODEL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a whole [`Scenario::list`] (every page, then every model) goes on starting requests. It
/// is checked before each one, so the call can end up to [`LIST_TIMEOUT`] later.
const LIST_DEADLINE: Duration = Duration::from_secs(3 * 60);

/// What the models of each Scenario `capability` make. The one place that says both which models a
/// kind is listed from and what a listed model is. A capability that names several kinds settles
/// none of them: [`audio_kinds`] does.
const CAPABILITIES: &[(&str, &[AssetKind])] = &[
    ("txt2img", &[AssetKind::Image]),
    ("img23d", &[AssetKind::Model3d]),
    ("txt23d", &[AssetKind::Model3d]),
    // Speech, sound effects and music are all `txt2audio`; only free-form tags tell them apart.
    ("txt2audio", &[AssetKind::Speech, AssetKind::SoundEffect, AssetKind::Music]),
];

/// The tags, compared without regard to case, that say what a `txt2audio` model makes.
const AUDIO_TAGS: &[(&str, AssetKind)] = &[
    ("tts", AssetKind::Speech),
    ("text-to-speech", AssetKind::Speech),
    ("sfx", AssetKind::SoundEffect),
    ("sound-effects", AssetKind::SoundEffect),
    ("music", AssetKind::Music),
];

/// Said beside a list made without a key.
const NO_KEY: &str = "Add a Scenario key in Settings → Builds → Asset provider keys to list every model for this type and what each costs; \
                      without one only the models this app already knows are listed.";

impl Scenario {
    /// The models Scenario lists for `kind` that this app can run, with their prices in credits.
    ///
    /// The public list needs no key, but a model's inputs and prices do: without a key only the
    /// models the known-models table says make `kind` are listed, with no inputs read and no price
    /// ([`Rate::Quoted`] with no typical quote), and `error` says to add a key. With one, each model
    /// of the kind is asked for (at most [`MAX_DESCRIBED`], [`WORKERS`] at a time) and kept only when
    /// the app can fill its required inputs.
    ///
    /// Every page and every model is a request, and the whole call has a deadline, so the list can
    /// end early: then the models read so far are returned, and `error` says the list is incomplete.
    /// A failure before anything was read is an error. What Scenario says of a model stands against
    /// the table: a model that is gone, retiring, refused to this account, answered with something no
    /// use, or whose inputs this app cannot fill is left out even when the table knows it. Only a
    /// model that was not heard about falls back to the table (it is offered, with no price, when the
    /// table says it makes `kind`): one whose request failed in passing (the network, a timeout,
    /// HTTP 408 or 425, a server error), one the reading stopped before (the key was refused or a
    /// limit reached) and one not reached before the time or the bound was up.
    pub fn list(&self, key: Option<&str>, kind: AssetKind) -> Result<ModelList, ToolError> {
        let auth = key.map(|key| self.auth(key)).transpose()?;
        let deadline = Instant::now() + self.list_deadline;
        let Pages { candidates, cut_short } = self.read_pages(kind, deadline)?;
        let mut problems: Vec<String> = cut_short.into_iter().map(|why| format!("The list is incomplete: {why}")).collect();
        let readings = match &auth {
            Some(auth) => {
                let (readings, more) = self.read_models(auth, &candidates, kind, deadline);
                problems.extend(more);
                readings
            }
            None => {
                problems.push(NO_KEY.to_string());
                candidates.iter().map(|_| Reading::NotReached).collect()
            }
        };
        let mut models = Vec::new();
        for (candidate, reading) in candidates.iter().zip(readings) {
            models.extend(match reading {
                Reading::Runs { pricing, ignores } => Some(listed(candidate, pricing, ignores)),
                // Scenario's answer, or its word that this account may not use the model, says it
                // cannot be run: the table's word does not stand against it.
                Reading::LeftOut | Reading::Unusable => None,
                // Nothing was heard about the model, or the request failed in a way that may pass.
                Reading::Stopped(_) | Reading::Failed | Reading::NotReached => known(candidate, kind),
            });
        }
        Ok(ModelList { models, error: (!problems.is_empty()).then(|| problems.join(" ")) })
    }

    /// One model with all its inputs and its price, as `key` gets them. A model Scenario no longer
    /// serves, or is retiring, is a [`ErrorCode::BadSettings`] error that names it. Its kinds are those
    /// its capabilities and tags settle, else the known-models table's. What Scenario answered is
    /// used again for ten minutes.
    pub fn describe(&self, key: &str, model: &str) -> Result<ModelDetail, ToolError> {
        let auth = self.auth(key)?;
        if http::id(model).is_err() {
            return Err(http::bad("That is not an id Scenario gives its models.").with_remediation(REMEDIATION));
        }
        self.described.get_or(&auth, model, || self.fetch_detail(&auth, model))
    }

    /// [`Scenario::describe`], asked of Scenario.
    fn fetch_detail(&self, auth: &str, model: &str) -> Result<ModelDetail, ToolError> {
        let found = self.fetch(auth, model).map_err(Trouble::into_error)?.ok_or_else(|| http::gone(NAME, model))?;
        let mut kinds = kinds_of(&found, model);
        if kinds.is_empty() {
            kinds = KnownModels::built_in().kinds(ID, model).unwrap_or_default().to_vec();
        }
        detail_of(&found, model, kinds)
    }

    /// The `Authorization` header for a key and secret. The key goes only to Scenario's API.
    fn auth(&self, key: &str) -> Result<String, ToolError> {
        self.validate_key(key)?;
        use base64::Engine as _;
        Ok(format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(http::key(key)?)))
    }

    /// The models of the public list that make `kind`, page by page. A page that cannot be read
    /// ends the reading, and is an error only when it was the first.
    fn read_pages(&self, kind: AssetKind, deadline: Instant) -> Result<Pages, ToolError> {
        let mut candidates = Vec::new();
        let mut listed: HashSet<String> = HashSet::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        let cut_short = loop {
            if pages == MAX_PAGES {
                break Some(format!("only the first {MAX_PAGES} pages were read."));
            }
            if time_is_up(deadline) {
                if pages == 0 {
                    return Err(http::too_slow(NAME));
                }
                break Some(words(&http::too_slow(NAME)));
            }
            let page = match self.page(token.as_deref()) {
                Ok(page) => page,
                Err(e) if pages == 0 => return Err(e),
                Err(e) => break Some(words(&e)),
            };
            pages += 1;
            for model in &page.models {
                if let Some(candidate) = candidate(model, kind)
                    && listed.insert(candidate.id.clone())
                {
                    candidates.push(candidate);
                }
            }
            match page.next {
                Next::Last => break None,
                Next::Token(next) => token = Some(next),
                Next::Unusable => break Some("Scenario gave a next page that this app cannot ask for.".to_string()),
            }
        };
        Ok(Pages { candidates, cut_short })
    }

    /// One page of the public list, and how to ask for the next one if there is one. No key is sent.
    fn page(&self, token: Option<&str>) -> Result<Page, ToolError> {
        let mut url = format!("{}/models/public?pageSize={PAGE_SIZE}", self.base);
        if let Some(token) = token {
            url.push_str("&paginationToken=");
            url.push_str(&http::encode(token));
        }
        let mut answer = read(self.send(None, &url, LIST_TIMEOUT)?, http::MAX_LIST_PAGE)?;
        let Some(Value::Array(mut models)) = answer.get_mut("models").map(Value::take) else {
            return Err(http::garbled("Scenario's model list could not be read."));
        };
        models.truncate(MAX_PAGE_MODELS);
        Ok(Page { models, next: next_of(&answer) })
    }

    /// Ask for the inputs and price of the first [`MAX_DESCRIBED`] of `candidates`, [`WORKERS`] at a
    /// time, one reading per candidate, and say what went wrong. Stops asking when Scenario refuses
    /// the key or its rate limit is reached (every other request would fail the same way) and when
    /// `deadline` has passed; the models not reached are [`Reading::NotReached`].
    fn read_models(&self, auth: &str, candidates: &[Candidate], kind: AssetKind, deadline: Instant) -> (Vec<Reading>, Vec<String>) {
        let reach = candidates.len().min(MAX_DESCRIBED);
        let (next, stop) = (AtomicUsize::new(0), AtomicBool::new(false));
        let (next, stop) = (&next, &stop);
        let done: Vec<(usize, Reading)> = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..WORKERS.min(reach))
                .filter_map(move |_| {
                    std::thread::Builder::new()
                        .spawn_scoped(scope, move || {
                            let mut mine = Vec::new();
                            while !stop.load(Ordering::Relaxed) && !time_is_up(deadline) {
                                let at = next.fetch_add(1, Ordering::Relaxed);
                                let Some(candidate) = candidates.get(at).filter(|_| at < reach) else { break };
                                let reading = self.read_one(auth, candidate, kind);
                                if matches!(reading, Reading::Stopped(_)) {
                                    stop.store(true, Ordering::Relaxed);
                                }
                                mine.push((at, reading));
                            }
                            mine
                        })
                        .ok()
                })
                .collect();
            // A worker that panicked has nothing to hand over: its models count as not reached.
            workers.into_iter().flat_map(|worker| worker.join().unwrap_or_default()).collect()
        });
        let mut readings: Vec<Reading> = candidates.iter().map(|_| Reading::NotReached).collect();
        for (at, reading) in done {
            if let Some(slot) = readings.get_mut(at) {
                *slot = reading;
            }
        }
        let mut problems = Vec::new();
        let refused = readings.iter().find_map(|r| match r {
            Reading::Stopped(e) => Some(words(e)),
            _ => None,
        });
        let failed = readings.iter().filter(|r| matches!(r, Reading::Failed | Reading::Unusable)).count();
        let cut = readings.iter().take(reach).any(|r| matches!(r, Reading::NotReached));
        if let Some(why) = &refused {
            problems.push(format!("Inputs and prices could not be read: {why}"));
        } else if cut {
            problems.push(format!("The list is incomplete: {}", words(&http::too_slow(NAME))));
        }
        if failed > 0 {
            problems.push(format!("{failed} model{} could not be read.", if failed == 1 { "" } else { "s" }));
        }
        if candidates.len() > MAX_DESCRIBED {
            problems.push(format!("The list is incomplete: only the first {MAX_DESCRIBED} of {} models were read.", candidates.len()));
        }
        (readings, problems)
    }

    /// What Scenario says of one model, as far as a list needs it: what it costs and what it ignores
    /// if this app can run it as `kind`. Its inputs are not kept.
    fn read_one(&self, auth: &str, candidate: &Candidate, kind: AssetKind) -> Reading {
        match self.fetch(auth, &candidate.id) {
            Ok(Some(model)) => match read_inputs(&model, &candidate.id) {
                // The price is that of the type asked for, whatever the answer says the model makes.
                Ok(fields) => match runnable(&fields, kind, InputRoles::built_in()) {
                    Ok(ignores) => Reading::Runs { pricing: pricing_of(&model, &[kind]), ignores },
                    Err(_) => Reading::LeftOut,
                },
                Err(_) => Reading::LeftOut,
            },
            Ok(None) => Reading::LeftOut,
            Err(Trouble::Stops(e)) => Reading::Stopped(e),
            Err(Trouble::Passing(_)) => Reading::Failed,
            Err(Trouble::Unusable(_)) => Reading::Unusable,
        }
    }

    /// The model `GET /models/{id}` answers with, or `None` when Scenario says there is none (404).
    fn fetch(&self, auth: &str, id: &str) -> Result<Option<Value>, Trouble> {
        let response = self.send(Some(auth), &format!("{}/models/{id}", self.base), MODEL_TIMEOUT).map_err(Trouble::Passing)?;
        let status = response.status().as_u16();
        if status == 404 {
            return Ok(None);
        }
        if let Err(e) = http::status(status) {
            return Err(match status {
                // The key is refused, or the account is out of credit or over a rate limit: so it is for every model.
                401 | 402 | 429 => Trouble::Stops(e),
                // This account may not use this model; others are another matter.
                403 => Trouble::Unusable(e),
                // A request that timed out, one answered too early, or a server in trouble may go
                // through next time.
                408 | 425 | 500..=599 => Trouble::Passing(e),
                // Any other answer (a bad request, a model that is gone, a redirect) is Scenario's word
                // that this model cannot be got this way, and waiting will not change it.
                _ => Trouble::Unusable(e),
            });
        }
        let mut answer = http::json_up_to(response.into_body(), http::MAX_JSON).map_err(|e| match e.code {
            ErrorCode::OverLimit => Trouble::Unusable(too_large()),
            ErrorCode::GarbledOutput => Trouble::Unusable(e),
            // The body broke off on the way.
            _ => Trouble::Passing(e),
        })?;
        match answer.get_mut("model").map(Value::take) {
            Some(model @ Value::Object(_)) => Ok(Some(model)),
            _ => Err(Trouble::Unusable(http::garbled("Scenario's answer about the model could not be read."))),
        }
    }

    /// A GET to Scenario's API. `auth` is only for the requests that need it.
    fn send(&self, auth: Option<&str>, url: &str, timeout: Duration) -> Result<ureq::http::Response<ureq::Body>, ToolError> {
        http::get(|| {
            let request = http::agent(timeout).get(url);
            match auth {
                Some(auth) => request.header("Authorization", auth).call(),
                None => request.call(),
            }
        })
        .map_err(|_| http::cannot_reach(NAME))
    }
}

/// The models of the pages read, and why the reading ended early if it did.
struct Pages {
    candidates: Vec<Candidate>,
    cut_short: Option<String>,
}

struct Page {
    models: Vec<Value>,
    next: Next,
}

/// Where a list goes on after a page.
enum Next {
    /// The page was the last.
    Last,
    /// Ask again with this token.
    Token(String),
    /// The page gives a token that cannot be sent back.
    Unusable,
}

/// What a page says comes after it: no token (or an empty one) is the end.
fn next_of(answer: &Value) -> Next {
    match answer.get("nextPaginationToken") {
        None | Some(Value::Null) => Next::Last,
        Some(Value::String(token)) if token.is_empty() => Next::Last,
        Some(Value::String(token)) if token.len() <= CURSOR_LENGTH => Next::Token(token.clone()),
        Some(_) => Next::Unusable,
    }
}

/// A model of the public list that makes the kind asked for.
struct Candidate {
    id: String,
    name: String,
    kinds: Vec<AssetKind>,
}

/// What came of asking for one model.
enum Reading {
    /// This app can run it: what it costs and which options it ignores.
    Runs { pricing: Pricing, ignores: Vec<AssetOption> },
    /// Scenario has no such model any more, or it is retiring, or its inputs cannot be read or filled.
    LeftOut,
    /// The request was refused for every model, and the reading stopped.
    Stopped(ToolError),
    /// The request failed in a way that may pass (the network, a timeout, HTTP 408 or 425, a server
    /// error): nothing is known of the model.
    Failed,
    /// The answer is no use, or Scenario refused it (the account may not use the model, or any other
    /// 4xx or 3xx answer): the model cannot be run.
    Unusable,
    /// No request was made: the time was up, the reading stopped, or the list was too long.
    NotReached,
}

/// Why a model could not be got from Scenario, by what it says of the other models.
enum Trouble {
    /// Every other request would meet it too: the key was refused (401), or the account is out of
    /// credit (402) or over a rate limit (429).
    Stops(ToolError),
    /// The request failed in a way that may pass: the network, a timeout, HTTP 408 or 425, a server
    /// error (5xx).
    Passing(ToolError),
    /// The answer is of no use (too large, unreadable, no model in it), or Scenario refused it: the
    /// account may not use this model (403), the request was wrong or the model is gone (any other
    /// 4xx), or it was redirected (3xx). That says nothing of the other models.
    Unusable(ToolError),
}

impl Trouble {
    fn into_error(self) -> ToolError {
        match self {
            Trouble::Stops(e) | Trouble::Passing(e) | Trouble::Unusable(e) => e,
        }
    }
}

/// The answer's JSON, or why there is none.
fn read(response: ureq::http::Response<ureq::Body>, limit: u64) -> Result<Value, ToolError> {
    http::status(response.status().as_u16())?;
    http::json_up_to(response.into_body(), limit)
}

/// The inputs of a model Scenario answered with. A model that is gone or retiring is refused
/// before anything else about it is read.
fn read_inputs(model: &Value, id: &str) -> Result<Vec<InputField>, ToolError> {
    if is_deleted(model) {
        return Err(http::gone(NAME, id));
    }
    if is_retiring(model) {
        return Err(http::retiring(NAME, id));
    }
    scenario_fields(model.get("inputs").unwrap_or(&Value::Null))
}

/// What a model Scenario answered with comes to, as the `kinds` it makes (the answer need not say).
fn detail_of(model: &Value, id: &str, kinds: Vec<AssetKind>) -> Result<ModelDetail, ToolError> {
    let fields = read_inputs(model, id)?;
    let pricing = pricing_of(model, &kinds);
    Ok(ModelDetail { id: id.to_string(), name: name_of(model, id), kinds, fields, pricing, unit_price: None })
}

fn listed(candidate: &Candidate, pricing: Pricing, ignores: Vec<AssetOption>) -> ListedModel {
    ListedModel { provider: ID.into(), id: candidate.id.clone(), name: candidate.name.clone(), kinds: candidate.kinds.clone(), pricing, ignores }
}

/// A model whose inputs are unknown, offered because the known-models table says it makes `kind`.
fn known(candidate: &Candidate, kind: AssetKind) -> Option<ListedModel> {
    let makes = KnownModels::built_in().kinds(ID, &candidate.id)?;
    makes.contains(&kind).then(|| listed(candidate, quoted(None), Vec::new()))
}

/// The strings of a list in a model (`tags`, `capabilities`), at most [`MAX_LABELS`] of them.
fn labels<'a>(model: &'a Value, key: &str) -> impl Iterator<Item = &'a str> {
    model.get(key).and_then(Value::as_array).into_iter().flatten().take(MAX_LABELS).filter_map(Value::as_str)
}

/// The model of a public list entry, if it is one this app may use for `kind`: a `custom` model, with
/// an id that is safe in a URL and that is not retiring.
///
/// Only `custom` models (606 of the 766 public ones on 2026-11-03) are listed, not the LoRA-type ones
/// (`flux.1-lora`, ... 160 of them, many with `txt2img`): they are style variants of base models, and
/// they would use up the [`MAX_DESCRIBED`] models read for one list. The run-any-model
/// call would take them (https://docs.scenario.com/api/resources/generate/methods/run_model says
/// "any model"), so offering them is a matter of dropping this check.
fn candidate(model: &Value, kind: AssetKind) -> Option<Candidate> {
    let id = model.get("id")?.as_str().filter(|id| http::id(id).is_ok())?;
    if model.get("type").and_then(Value::as_str) != Some("custom") || is_retiring(model) || is_deleted(model) {
        return None;
    }
    let kinds = kinds_of(model, id);
    kinds.contains(&kind).then(|| Candidate { id: id.to_string(), name: name_of(model, id), kinds })
}

/// What the model makes, from its capabilities. A capability that settles its kind says it; one that
/// names several is settled by [`audio_kinds`].
fn kinds_of(model: &Value, id: &str) -> Vec<AssetKind> {
    let mut kinds: Vec<AssetKind> = Vec::new();
    for capability in labels(model, "capabilities") {
        let Some((_, makes)) = CAPABILITIES.iter().find(|(name, _)| *name == capability) else { continue };
        let settled = match makes {
            [one] => vec![*one],
            several => audio_kinds(model, id, several),
        };
        for kind in settled {
            if !kinds.contains(&kind) {
                kinds.push(kind);
            }
        }
    }
    kinds
}

/// Which of `may` a model makes when its capability does not say: the one its tags name, if they
/// name one; else what the known-models table says of it. Tags that name several, or none, settle nothing.
fn audio_kinds(model: &Value, id: &str, may: &[AssetKind]) -> Vec<AssetKind> {
    let mut tagged: Vec<AssetKind> = Vec::new();
    for tag in labels(model, "tags") {
        let named = AUDIO_TAGS.iter().find(|(name, _)| name.eq_ignore_ascii_case(tag.trim())).map(|(_, kind)| *kind);
        if let Some(kind) = named.filter(|kind| may.contains(kind))
            && !tagged.contains(&kind)
        {
            tagged.push(kind);
        }
    }
    if let [one] = tagged.as_slice() {
        return vec![*one];
    }
    KnownModels::built_in().kinds(ID, id).unwrap_or_default().iter().copied().filter(|kind| may.contains(kind)).collect()
}

/// Whether Scenario marks the model as one it is retiring: the tag `Deprecated`, `deprecated:<the
/// model that replaces it>` or `sc:legacy` (every `sc:legacy` model of the list read on 2026-11-03
/// was also `Deprecated`).
fn is_retiring(model: &Value) -> bool {
    const REPLACED_BY: &str = "deprecated:";
    labels(model, "tags").map(str::trim).any(|tag| {
        tag.eq_ignore_ascii_case("deprecated")
            || tag.eq_ignore_ascii_case("sc:legacy")
            || tag.get(..REPLACED_BY.len()).is_some_and(|start| start.eq_ignore_ascii_case(REPLACED_BY))
    })
}

/// Whether the model has been deleted (a `status` the model page names).
fn is_deleted(model: &Value) -> bool {
    model.get("status").and_then(Value::as_str) == Some("deleted")
}

/// The name to show: Scenario's, else the known-models table's, else the id.
fn name_of(model: &Value, id: &str) -> String {
    http::shown_name(model.get("name").and_then(Value::as_str), KnownModels::built_in().name(ID, id), id)
}

fn quoted(typical: Option<f64>) -> Pricing {
    Pricing::Before { unit: Unit::Credits, rate: Rate::Quoted { typical } }
}

/// What a model costs: every run is quoted before it starts (`?dryRun=api`), so the price is a quote,
/// and `typical` is the usual one if the model's statistics give it. A statistic that is not a
/// number this app can believe (not finite, negative, or more than a limit can be) is no statistic:
/// the price is still a quote, so the model is still tracked, and a limit is checked against the
/// run's own quote, not this number.
fn pricing_of(model: &Value, kinds: &[AssetKind]) -> Pricing {
    quoted(typical_cost(model, kinds).filter(|cost| cost.is_finite() && (0.0..=MAX_ASSET_CREDITS).contains(cost)))
}

/// The median credits of one asset (`performanceStats.variants[].costPerAssetP50CU`) of the variant
/// the model's `default` names, else the first; among the variants of the capabilities that make
/// `kinds` (a model that also edits pictures has a price for that as well).
fn typical_cost(model: &Value, kinds: &[AssetKind]) -> Option<f64> {
    let stats = model.get("performanceStats")?;
    let fitting: Vec<&Value> = stats
        .get("variants")?
        .as_array()?
        .iter()
        .take(MAX_LABELS)
        .filter(|variant| variant.get("capability").and_then(Value::as_str).is_none_or(|capability| can_make(capability, kinds)))
        .collect();
    let named = stats.get("default").and_then(Value::as_str);
    let chosen = named.and_then(|key| fitting.iter().find(|variant| variant.get("variantKey").and_then(Value::as_str) == Some(key))).or(fitting.first())?;
    chosen.get("costPerAssetP50CU")?.as_f64()
}

/// Whether a model of `capability` makes any of `kinds`.
fn can_make(capability: &str, kinds: &[AssetKind]) -> bool {
    CAPABILITIES.iter().any(|(name, makes)| *name == capability && makes.iter().any(|kind| kinds.contains(kind)))
}

/// The input fields of a model from its `inputs`
/// (https://docs.scenario.com/api/resources/models/methods/retrieve, read 2026-11-03): each has a
/// `name`, which is the key of the body that runs the model, and a `type`. Only kinds this app can
/// fill are anything but [`FieldKind::Other`]: a string (a choice, when `allowedValues` lists some),
/// a number (not one from a fixed set), a boolean, a single picture (`file` of `kind` `image`, the
/// default) and a list of pictures (`file_array` likewise). A list of anything else and the inputs
/// of other models are `Other`, and are not read into. An input is required when its `required.always` is true;
/// one that is only required when another is, or is not, given is not, because this app gives only
/// what it fills.
///
/// The provider's list is bounded like fal's: more than [`MAX_FIELDS`] inputs, a choice with more
/// than [`MAX_CHOICES`] values or a name over [`FIELD_NAME_LENGTH`] bytes is refused, not cut short.
fn scenario_fields(inputs: &Value) -> Result<Vec<InputField>, ToolError> {
    let items = inputs.as_array().ok_or_else(unreadable)?;
    if items.len() > MAX_FIELDS {
        return Err(too_big());
    }
    let mut named: HashSet<&str> = HashSet::new();
    let mut fields = Vec::with_capacity(items.len());
    for item in items {
        let name = item.get("name").and_then(Value::as_str).filter(|name| !name.is_empty()).ok_or_else(unreadable)?;
        if name.len() > FIELD_NAME_LENGTH {
            return Err(too_big());
        }
        // The first of two inputs of one name is the one that counts.
        if named.insert(name) {
            let has_default = item.get("default").is_some_and(|default| !default.is_null());
            fields.push(InputField { name: name.to_string(), kind: field_kind(item)?, required: is_required(item), has_default });
        }
    }
    Ok(fields)
}

fn field_kind(item: &Value) -> Result<FieldKind, ToolError> {
    // A list with nothing in it limits nothing.
    let allowed = item.get("allowedValues").and_then(Value::as_array).filter(|values| !values.is_empty());
    if allowed.is_some_and(|values| values.len() > MAX_CHOICES) {
        return Err(too_big());
    }
    let limit = |key: &str| item.get(key).and_then(Value::as_f64).filter(|x| x.is_finite());
    Ok(match item.get("type").and_then(Value::as_str) {
        Some("string") => match allowed {
            None => FieldKind::Text,
            Some(values) => {
                let words: Vec<String> = values.iter().filter_map(Value::as_str).map(str::to_string).collect();
                if words.len() == values.len() { FieldKind::Choice(words) } else { FieldKind::Other }
            }
        },
        Some("number") if allowed.is_none() => FieldKind::Number { min: limit("min"), max: limit("max") },
        Some("boolean") => FieldKind::Bool,
        // A file is a picture unless its `kind` says otherwise; a list of files likewise.
        Some("file") if is_picture(item) => FieldKind::Image,
        Some("file_array") if is_picture(item) => FieldKind::Images,
        _ => FieldKind::Other,
    })
}

/// Whether a `file` or `file_array` input holds pictures: its `kind` is `image`, which is also what
/// an input that does not say is taken to be.
fn is_picture(item: &Value) -> bool {
    item.get("kind").and_then(Value::as_str).unwrap_or("image") == "image"
}

/// Whether the model refuses a run without the input: `required.always` (a plain `true` is read too).
fn is_required(item: &Value) -> bool {
    match item.get("required") {
        Some(Value::Bool(always)) => *always,
        Some(Value::Object(conditions)) => conditions.get("always").and_then(Value::as_bool) == Some(true),
        _ => false,
    }
}

/// One model's answer is larger than this app reads. [`ErrorCode::OverLimit`] is this app's code for
/// a size it will not read (as in `http::bytes` and `too_big`), never a word about Scenario's rate
/// limit: that is told by the HTTP status, in [`Trouble`], and nothing here reads it from a code.
fn too_large() -> ToolError {
    ToolError::new(ErrorCode::OverLimit, "Scenario's description of this model is larger than this app reads.").with_remediation(REMEDIATION)
}

fn unreadable() -> ToolError {
    ToolError::new(ErrorCode::GarbledOutput, "The description Scenario gives of this model's inputs could not be read.").with_remediation(REMEDIATION)
}

/// The inputs of a model are more than this app reads; the same code as [`too_large`], for the same reason.
fn too_big() -> ToolError {
    ToolError::new(ErrorCode::OverLimit, "The description Scenario gives of this model's inputs is larger than this app reads.").with_remediation(REMEDIATION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{Shape, charged_amount, was_charged};
    use serde_json::json;

    const KEY: &str = "test-key:test-secret";

    fn input(name: &str, kind: FieldKind, required: bool) -> InputField {
        InputField { name: name.into(), kind, required, has_default: !required }
    }

    /// Gemini 3.1 on Scenario as its inputs describe it: a prompt, an aspect ratio and a count.
    fn gemini() -> ModelDetail {
        ModelDetail {
            id: "model_google-gemini-3-1-flash".into(),
            name: "Gemini 3.1".into(),
            kinds: vec![AssetKind::Image],
            fields: vec![
                input("prompt", FieldKind::Text, true),
                input("aspectRatio", FieldKind::Choice(vec!["1:1".into(), "16:9".into(), "9:16".into()]), false),
                input("numOutputs", FieldKind::Number { min: Some(1.0), max: Some(4.0) }, false),
            ],
            pricing: Pricing::Before { unit: Unit::Credits, rate: Rate::Quoted { typical: Some(100.0) } },
            unit_price: None,
        }
    }

    /// Meshy on Scenario: a list of pictures in, a face count and texturing.
    fn meshy() -> ModelDetail {
        ModelDetail {
            id: "model_meshy-7-1-img23d".into(),
            name: "Meshy".into(),
            kinds: vec![AssetKind::Model3d],
            fields: vec![
                input("image", FieldKind::Images, true),
                input("shouldTexture", FieldKind::Bool, false),
                input("targetPolycount", FieldKind::Number { min: Some(100.0), max: Some(300_000.0) }, false),
            ],
            pricing: Pricing::Before { unit: Unit::Credits, rate: Rate::Quoted { typical: None } },
            unit_price: None,
        }
    }

    fn wide() -> AssetRequest<'static> {
        AssetRequest::Image { prompt: "a wooden crate", shape: Shape::Wide }
    }

    /// Run the shared image fixture against a test-configured Scenario provider.
    fn image(provider: &Scenario) -> Result<Generated, ToolError> {
        provider.generate(KEY, &gemini(), &wide())
    }

    fn quick(base: String) -> Scenario {
        Scenario { base, poll_interval: Duration::ZERO, ..Scenario::default() }
    }

    /// Check the full quoted image workflow and ensure only API requests carry authentication.
    #[test]
    fn price_submit_poll_resolve_and_download_keep_auth_off_media_requests() {
        let (base, seen) = http::tests::serve(|base| {
            vec![
                (269, json!({"creativeUnitsCost":19}).to_string()),
                (200, json!({"job":{"jobId":"job_test"}}).to_string()),
                (200, json!({"job":{"status":"in-progress"}}).to_string()),
                (200, json!({"job":{"status":"success","billing":{"cuCost":17},"metadata":{"assetIds":["asset_test"]}}}).to_string()),
                (200, json!({"asset":{"url":format!("{base}/media.png")}}).to_string()),
                (200, "PNG fake".into()),
            ]
        });
        let result = image(&quick(base)).unwrap();
        assert_eq!(result.bytes, b"PNG fake");
        assert_eq!(result.cost, Some(17.0), "the credits the finished job says it cost, never dollars");
        assert!(result.ignored.is_empty());
        let seen = seen.lock().unwrap();
        assert!(seen[0].starts_with("POST /generate/custom/model_google-gemini-3-1-flash?dryRun=api "));
        assert!(seen[1].starts_with("POST /generate/custom/model_google-gemini-3-1-flash "));
        assert_eq!(seen[0].split("\r\n\r\n").nth(1), seen[1].split("\r\n\r\n").nth(1), "the request quoted is the request submitted");
        assert!(seen[1].contains("\"aspectRatio\":\"16:9\"") && seen[1].contains("\"numOutputs\":1") && seen[1].contains("\"prompt\":\"a wooden crate\""));
        assert!(seen[2].starts_with("GET /jobs/job_test "));
        assert!(seen[4].starts_with("GET /assets/asset_test "));
        assert!(seen[0].to_ascii_lowercase().contains("authorization: basic "));
        assert!(!seen[5].to_ascii_lowercase().contains("authorization"));
    }

    /// The estimate is Scenario's quote for the very request, by a dry run that submits nothing.
    #[test]
    fn the_estimate_is_a_dry_run_quote() {
        let (base, seen) = http::tests::serve(|_| vec![(269, json!({"creativeUnitsCost":19.5}).to_string())]);
        assert_eq!(quick(base).estimate(KEY, &gemini(), &wide()).unwrap(), Some(19.5), "whatever the listed typical price says");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert!(seen[0].starts_with("POST /generate/custom/model_google-gemini-3-1-flash?dryRun=api ") && seen[0].contains("\"aspectRatio\":\"16:9\""));
    }

    /// A job that says nothing of what it cost cost what it was quoted; one that says something no
    /// one could believe likewise.
    #[test]
    fn a_job_without_its_cost_cost_its_quote() {
        for billing in [json!({}), json!({"cuCost": -4}), json!({"cuCost": 1e300}), json!({"cuCost": "17"})] {
            let (base, _) = http::tests::serve(|base| {
                vec![
                    (269, json!({"creativeUnitsCost":19}).to_string()),
                    (200, json!({"job":{"jobId":"job_test"}}).to_string()),
                    (200, json!({"job":{"status":"success","billing":billing,"metadata":{"assetIds":["asset_test"]}}}).to_string()),
                    (200, json!({"asset":{"url":format!("{base}/media.png")}}).to_string()),
                    (200, "PNG fake".into()),
                ]
            });
            assert_eq!(image(&quick(base)).unwrap().cost, Some(19.0));
        }
    }

    /// Preserve the requested image, polygon count, and texturing options in mesh submission.
    #[test]
    fn mesh_request_honours_target_polygons_and_texture() {
        let (base, seen) = http::tests::serve(|base| {
            vec![
                (269, json!({"creativeUnitsCost":120}).to_string()),
                (200, json!({"job":{"jobId":"job_test"}}).to_string()),
                (200, json!({"job":{"status":"success","metadata":{"assetIds":["asset_test"]}}}).to_string()),
                (200, json!({"asset":{"url":format!("{base}/mesh.glb")}}).to_string()),
                (200, "glTF fake".into()),
            ]
        });
        let req = AssetRequest::Model3d { image_url: Some("https://example.com/input.png"), prompt: None, textured: true, faces: 3000 };
        let made = Scenario { base, ..Scenario::default() }.generate(KEY, &meshy(), &req).unwrap();
        assert_eq!((made.bytes.as_slice(), made.cost), (&b"glTF fake"[..], Some(120.0)));
        let seen = seen.lock().unwrap();
        assert!(seen[1].contains("\"image\":[\"https://example.com/input.png\"]"));
        assert!(seen[1].contains("\"targetPolycount\":3000") && seen[1].contains("\"shouldTexture\":true"));
    }

    /// Stop after preflight when a quote is invalid, already a job, or unconfirmed.
    #[test]
    fn unsafe_or_missing_quotes_never_submit_a_generation() {
        for quote in [
            json!({"creativeUnitsCost":-1}),
            json!({"creativeUnitsCost":1e300}),
            json!({"creativeUnitsCost":MAX_ASSET_CREDITS + 1.0}),
            json!({"creativeUnitsCost":"19"}),
            json!({}),
            json!({"creativeUnitsCost":1,"job":{"jobId":"unexpected"}}),
        ] {
            let (base, seen) = http::tests::serve(|_| vec![(269, quote.to_string()), (269, quote.to_string())]);
            let provider = Scenario { base, ..Scenario::default() };
            assert!(provider.estimate(KEY, &gemini(), &wide()).is_err(), "{quote}");
            let error = image(&provider).unwrap_err();
            assert!(!was_charged(&error), "{quote}");
            assert_eq!(seen.lock().unwrap().len(), 2, "{quote}: a quote each, and nothing submitted");
        }
        let (base, seen) = http::tests::serve(|_| vec![(200, json!({"creativeUnitsCost":19}).to_string())]);
        assert!(image(&Scenario { base, ..Scenario::default() }).is_err());
        assert_eq!(seen.lock().unwrap().len(), 1, "an ordinary success is not a confirmed dry run");
    }

    /// A quote of nothing is a free run: it goes ahead and costs nothing.
    #[test]
    fn a_free_quote_is_a_free_run() {
        let (base, seen) = http::tests::serve(|base| {
            vec![
                (269, json!({"creativeUnitsCost":0}).to_string()),
                (269, json!({"creativeUnitsCost":0}).to_string()),
                (200, json!({"job":{"jobId":"job_test"}}).to_string()),
                (200, json!({"job":{"status":"success","metadata":{"assetIds":["asset_test"]}}}).to_string()),
                (200, json!({"asset":{"url":format!("{base}/media.png")}}).to_string()),
                (200, "PNG fake".into()),
            ]
        });
        let provider = quick(base);
        assert_eq!(provider.estimate(KEY, &gemini(), &wide()).unwrap(), Some(0.0));
        assert_eq!(provider.generate_within(KEY, &gemini(), &wide(), Some(0.0)).unwrap().cost, Some(0.0));
        assert_eq!(seen.lock().unwrap().len(), 6);
    }

    /// A run is quoted again just before it is submitted; a quote above the one its limit was
    /// checked against stops it with nothing submitted, and one at or below it goes ahead.
    #[test]
    fn a_price_that_rose_since_it_was_checked_is_not_paid() {
        let (base, seen) = http::tests::serve(|_| vec![(269, json!({"creativeUnitsCost":21}).to_string())]);
        let error = quick(base).generate_within(KEY, &gemini(), &wide(), Some(19.0)).unwrap_err();
        assert_eq!(error.code, ErrorCode::OverLimit);
        assert!(error.message.contains("price rose since it was checked") && error.message.contains("21 credits"), "{}", error.message);
        assert!(!was_charged(&error), "nothing was submitted");
        assert_eq!(seen.lock().unwrap().len(), 1, "the quote only");
        for approved in [Some(19.0), Some(25.0), None] {
            let (base, seen) = http::tests::serve(|base| {
                vec![
                    (269, json!({"creativeUnitsCost":19}).to_string()),
                    (200, json!({"job":{"jobId":"job_test"}}).to_string()),
                    (200, json!({"job":{"status":"success","metadata":{"assetIds":["asset_test"]}}}).to_string()),
                    (200, json!({"asset":{"url":format!("{base}/media.png")}}).to_string()),
                    (200, "PNG fake".into()),
                ]
            });
            assert_eq!(quick(base).generate_within(KEY, &gemini(), &wide(), approved).unwrap().cost, Some(19.0), "{approved:?}");
            assert_eq!(seen.lock().unwrap().len(), 5);
        }
    }

    /// A request the model can't take, and a picture that can't be sent, are refused before anything is asked.
    #[test]
    fn a_request_that_cannot_be_sent_is_refused_before_anything_is_asked() {
        let provider = Scenario { base: "http://127.0.0.1:9".into(), ..Scenario::default() };
        let solid = |url| AssetRequest::Model3d { image_url: Some(url), prompt: None, textured: false, faces: 500 };
        for url in ["asset_../../x", "file:///etc/passwd", "http://example.com/x.png", "data:image/png,raw", "https://user:pw@example.com/x.png"] {
            let error = provider.generate(KEY, &meshy(), &solid(url)).unwrap_err();
            assert_eq!(error.code, ErrorCode::BadSettings, "{url}");
            assert!(!was_charged(&error));
        }
        let from_words = AssetRequest::Model3d { image_url: None, prompt: Some("a barrel"), textured: false, faces: 500 };
        assert_eq!(provider.estimate(KEY, &meshy(), &from_words).unwrap_err().code, ErrorCode::BadSettings, "the model needs a picture");
        assert_eq!(provider.estimate(KEY, &ModelDetail { id: "../x".into(), ..gemini() }, &wide()).unwrap_err().code, ErrorCode::BadSettings);
        let blank = AssetRequest::Image { prompt: " ", shape: Shape::Square };
        assert_eq!(provider.generate(KEY, &gemini(), &blank).unwrap_err().code, ErrorCode::BadSettings);
        for key in ["", "no-secret"] {
            assert_eq!(provider.generate(key, &gemini(), &wide()).unwrap_err().code, ErrorCode::BadSettings, "{key:?}");
        }
    }

    /// Reject failed jobs, unsafe asset identifiers, and expired deadlines without exposing remote text.
    #[test]
    fn failed_jobs_invalid_ids_and_deadlines_fail_without_leaking_body() {
        for job in [json!({"status":"failure","metadata":{"error":"test-secret"}}), json!({"status":"success","metadata":{"assetIds":["../../bad"]}})] {
            let (base, _) = http::tests::serve(|_| {
                vec![
                    (269, json!({"creativeUnitsCost":1}).to_string()),
                    (200, json!({"job":{"jobId":"job_test"}}).to_string()),
                    (200, json!({"job":job}).to_string()),
                ]
            });
            let error = image(&Scenario { base, ..Scenario::default() }).unwrap_err();
            assert!(!format!("{error:?}").contains("test-secret"));
            assert_eq!(charged_amount(&error), Some(1.0), "a submitted job reports its quoted charge, in credits");
        }
        let error = image(&Scenario { timeout: Duration::ZERO, ..Scenario::default() }).unwrap_err();
        assert!(!was_charged(&error), "nothing was submitted");
    }

    /// Failures after submission (unknown status, missing asset address, failed download) report the
    /// quoted charge; failures before submission do not.
    #[test]
    fn every_failure_after_submission_reports_the_quoted_charge() {
        // What the server answers after the job is submitted, given its own address.
        type AfterSubmit = fn(&str) -> Vec<(u16, String)>;
        let after_submit: Vec<AfterSubmit> = vec![
            |_| vec![(200, json!({"job":{"status":"mystery"}}).to_string())],
            |_| vec![(200, json!({"job":{"status":"success","metadata":{"assetIds":["asset_test"]}}}).to_string()), (200, json!({"asset":{}}).to_string())],
            |base| {
                vec![
                    (200, json!({"job":{"status":"success","metadata":{"assetIds":["asset_test"]}}}).to_string()),
                    (200, json!({"asset":{"url":format!("{base}/media.png")}}).to_string()),
                    (500, "gone".into()),
                ]
            },
        ];
        for answers in after_submit {
            let (base, seen) = http::tests::serve(|base| {
                let mut all = vec![(269, json!({"creativeUnitsCost":19}).to_string()), (200, json!({"job":{"jobId":"job_test"}}).to_string())];
                all.extend(answers(base));
                all
            });
            let error = image(&quick(base)).unwrap_err();
            assert_eq!(charged_amount(&error), Some(19.0));
            assert!(seen.lock().unwrap().len() >= 3);
        }
        let (base, _) = http::tests::serve(|_| vec![(269, json!({"creativeUnitsCost":19}).to_string()), (402, "no credit".into())]);
        let error = image(&Scenario { base, ..Scenario::default() }).unwrap_err();
        assert!(!was_charged(&error), "a refused submission is not charged");
        // An accepted submission whose answer is empty or not JSON is still charged.
        for answer in ["", "not json"] {
            let (base, _) = http::tests::serve(|_| vec![(269, json!({"creativeUnitsCost":19}).to_string()), (200, answer.into())]);
            let error = image(&Scenario { base, ..Scenario::default() }).unwrap_err();
            assert_eq!(error.code, ErrorCode::GarbledOutput);
            assert_eq!(charged_amount(&error), Some(19.0), "answer {answer:?}");
        }
        // An unreadable price check is not: nothing was submitted.
        let (base, seen) = http::tests::serve(|_| vec![(269, "not json".into())]);
        assert!(!was_charged(&image(&Scenario { base, ..Scenario::default() }).unwrap_err()));
        assert_eq!(seen.lock().unwrap().len(), 1);
    }
    /// Require both credential components and reject control characters during local validation.
    #[test]
    fn credentials_are_validated_before_a_precursor_can_be_paid_for() {
        let provider = Scenario::default();
        for key in ["", "test-key", ":secret", "key:", "key:secret\r\ninjected"] {
            assert!(provider.validate_key(key).is_err());
        }
        assert!(provider.validate_key("test-key:test-secret").is_ok());
        assert_eq!(provider.unit(), Unit::Credits);
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;
    use crate::assets::inputs::{FieldKind, MAX_CHOICES, MAX_FIELDS};
    use crate::assets::model::AssetOption;
    use crate::assets::tables::InputRoles;
    use crate::assets::{AssetRequest, Shape};
    use craft_anything_platform::sync::Mutex;
    use serde_json::json;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    // The API these tests stand in for, read on 2026-11-03:
    //
    // * `GET https://api.cloud.scenario.com/v1/models/public` lists the public models. Its page
    //   (https://docs.scenario.com/api/resources/models/methods/list) documents `pageSize` (default 100,
    //   at most 500) and `paginationToken`, and says the answer carries `nextPaginationToken`. It was
    //   called without any credential and answered 200 `{"models": [...], "nextPaginationToken": "..."}`
    //   (a token is base64 of `{"offset":N,"limit":L}`, and `paginationToken=<it>` gave the next page).
    //   A model has `id` (`model_...`), `name`, `type` (606 of 766 are `custom`; the others are LoRAs
    //   such as `flux.1-lora`), `privacy`, `status`, `capabilities` (`txt2img`, `img23d`, `txt23d`,
    //   `txt2audio`, ... 17 at most) and free-form `tags` (28 at most). Models that are retiring carry
    //   the tag `Deprecated`, `deprecated:<replacement>` and `sc:legacy`; speech, sound effects and music
    //   are told apart by tags only (`TTS`, `text-to-speech`, `SFX`, `sound-effects`, `Music`).
    // * `GET https://api.cloud.scenario.com/v1/models/{modelId}` needs a credential (it answered 401
    //   without one; `inputs` and `performanceStats` are absent from the public list). The page
    //   (https://docs.scenario.com/api/resources/models/methods/retrieve) documents the answer's model
    //   with `inputs` (each: `name`, `type` of `boolean`, `file`, `file_array`, `inputs_array`, `model`,
    //   `model_array`, `number`, `number_array`, `string`, `string_array`; `allowedValues`, `default`,
    //   `kind` of a file, `min`, `max`, and `required` as an object of `always`, `conditional`,
    //   `ifDefined`, `ifNotDefined`) and `performanceStats` (`variants[]` with `capability`, `variantKey`,
    //   `costPerAssetMinCU`, `costPerAssetP50CU`, `costPerAssetMaxCU`, ... and `default`, a variant key).
    //   That page's example holds placeholders only, and no credential was used here: the values below
    //   are invented to that shape, with input names the generate calls of this app have always sent.
    // * Its body for running a model is the inputs by `name`
    //   (https://docs.scenario.com/api/resources/generate/methods/run_model), and the page has no 404
    //   in its list of answers: a model that is gone is read from a 404 or a `deleted` status.

    const KEY: &str = "k-123:s-456";

    fn no_pause(_: &str) -> Duration {
        Duration::ZERO
    }

    // ---- fixtures ---------------------------------------------------------------------------

    /// One entry of the public list, with the fields that list gives.
    fn entry(id: &str, name: &str, capabilities: &[&str], tags: &[&str]) -> Value {
        json!({
            "id": id, "name": name, "type": "custom", "privacy": "public", "status": "trained",
            "shortDescription": "A model.", "capabilities": capabilities, "tags": tags
        })
    }

    fn picture(id: &str) -> Value {
        entry(id, "A picture model", &["txt2img", "img2img"], &["sc:third-party"])
    }

    /// The model `GET /v1/models/{id}` answers: its list entry with `inputs` and `performanceStats`.
    fn described(entry: &Value, inputs: Value, stats: Option<Value>) -> String {
        let mut model = entry.clone();
        model["inputs"] = inputs;
        if let Some(stats) = stats {
            model["performanceStats"] = stats;
        }
        json!({"model": model}).to_string()
    }

    fn prompt_input() -> Value {
        json!({"name": "prompt", "type": "string", "prompt": true, "required": {"always": true}})
    }

    fn prompt_only() -> Value {
        json!([prompt_input()])
    }

    /// The inputs of a picture model: a prompt, a shape, a count.
    fn picture_inputs() -> Value {
        json!([
            prompt_input(),
            {"name": "aspectRatio", "type": "string", "allowedValues": ["1:1", "16:9", "9:16"], "default": "1:1"},
            {"name": "numOutputs", "type": "number", "min": 1, "max": 4, "default": 1}
        ])
    }

    fn stats(variants: Value, default: Option<&str>) -> Value {
        let mut stats = json!({"variants": variants});
        if let Some(default) = default {
            stats["default"] = json!(default);
        }
        stats
    }

    fn variant(capability: &str, key: &str, p50: Value) -> Value {
        json!({"capability": capability, "variantKey": key, "costPerAssetMinCU": 1, "costPerAssetP50CU": p50, "costPerAssetMaxCU": 99})
    }

    /// The token that follows page `i`.
    fn token(i: usize) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(format!("{{\"offset\":{},\"limit\":500}}", i * 500))
    }

    /// Pages of the public list and the answer for each model, as the stand-in serves them.
    struct Library {
        pages: Vec<Vec<Value>>,
        models: HashMap<String, (u16, String)>,
    }

    impl Library {
        /// Every model described with only a prompt input and no statistics.
        fn new(pages: Vec<Vec<Value>>) -> Library {
            let models = pages
                .iter()
                .flatten()
                .filter_map(|entry| Some((entry.get("id")?.as_str()?.to_string(), (200, described(entry, prompt_only(), None)))))
                .collect();
            Library { pages, models }
        }

        /// What `GET /v1/models/{id}` answers instead.
        fn with(mut self, id: &str, status: u16, body: String) -> Library {
            self.models.insert(id.to_string(), (status, body));
            self
        }

        fn answer(&self, target: &str) -> (u16, String) {
            let path = target.split('?').next().unwrap_or_default();
            if path == "/v1/models/public" {
                let page = (0..self.pages.len()).find(|&i| {
                    if i == 0 { !target.contains("paginationToken=") } else { target.ends_with(&format!("paginationToken={}", http::encode(&token(i)))) }
                });
                let Some((i, models)) = page.and_then(|i| Some((i, self.pages.get(i)?))) else { return (400, "{}".into()) };
                let next = (i + 1 < self.pages.len()).then(|| token(i + 1));
                return (200, json!({"models": models, "nextPaginationToken": next}).to_string());
            }
            match path.strip_prefix("/v1/models/").and_then(|id| self.models.get(id)) {
                Some((status, body)) => (*status, body.clone()),
                None => (404, json!({"message": "SECRET-BODY"}).to_string()),
            }
        }
    }

    // ---- a stand-in server that answers each request on a thread of its own -------------------

    struct Stand {
        base: String,
        seen: Arc<Mutex<Vec<String>>>,
        /// The most requests that were being answered at one moment.
        busiest: Arc<AtomicUsize>,
        done: Arc<AtomicBool>,
    }

    impl Drop for Stand {
        fn drop(&mut self) {
            self.done.store(true, Ordering::SeqCst);
        }
    }

    impl Stand {
        fn requests(&self) -> Vec<String> {
            self.seen.lock().unwrap().clone()
        }

        /// The requests for a model, not the list.
        fn model_requests(&self) -> Vec<String> {
            self.requests().into_iter().filter(|r| !r.starts_with("GET /v1/models/public")).collect()
        }

        fn scenario(&self) -> Scenario {
            Scenario { base: format!("{}/v1", self.base), ..Scenario::default() }
        }
    }

    type Handler = dyn Fn(&str, &str) -> (u16, String) + Send + Sync;

    /// Serves `handler(target, lowercased head)` for any number of requests; `pause(target)` is how long
    /// each answer is held back.
    fn stand(pause: fn(&str) -> Duration, handler: impl Fn(&str, &str) -> (u16, String) + Send + Sync + 'static) -> Stand {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let busiest = Arc::new(AtomicUsize::new(0));
        let done = Arc::new(AtomicBool::new(false));
        let live = Arc::new(AtomicUsize::new(0));
        let handler: Arc<Handler> = Arc::new(handler);
        let (seen2, busiest2, done2) = (seen.clone(), busiest.clone(), done.clone());
        std::thread::spawn(move || {
            while !done2.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let (handler, seen, busiest, live) = (handler.clone(), seen2.clone(), busiest2.clone(), live.clone());
                        std::thread::spawn(move || answer(stream, pause, &*handler, &seen, &busiest, &live));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(2)),
                    Err(_) => break,
                }
            }
        });
        Stand { base, seen, busiest, done }
    }

    fn answer(mut stream: TcpStream, pause: fn(&str) -> Duration, handler: &Handler, seen: &Mutex<Vec<String>>, busiest: &AtomicUsize, live: &AtomicUsize) {
        // Accepted sockets inherit the listener's mode differently across platforms.
        stream.set_nonblocking(false).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut head = String::new();
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            head.push_str(&line);
            if line == "\r\n" {
                break;
            }
        }
        let target = head.split_whitespace().nth(1).unwrap_or_default().to_string();
        seen.lock().unwrap().push(head.clone());
        busiest.fetch_max(live.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
        std::thread::sleep(pause(&target));
        let (status, body) = handler(&target, &head.to_ascii_lowercase());
        live.fetch_sub(1, Ordering::SeqCst);
        let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    }

    fn serving(library: Library) -> Stand {
        stand(no_pause, move |target, _| library.answer(target))
    }

    fn ids(list: &ModelList) -> Vec<&str> {
        list.models.iter().map(|m| m.id.as_str()).collect()
    }

    fn found<'a>(list: &'a ModelList, id: &str) -> &'a ListedModel {
        list.models.iter().find(|m| m.id == id).unwrap_or_else(|| panic!("{id} is not in {:?}", ids(list)))
    }

    fn quoted(typical: Option<f64>) -> Pricing {
        Pricing::Before { unit: Unit::Credits, rate: Rate::Quoted { typical } }
    }

    // ---- which models, and which type -------------------------------------------------------

    #[test]
    fn capabilities_choose_the_type() {
        let page = vec![
            picture("model_pic"),
            entry("model_pic-and-more", "Pic", &["txt2img", "img2img", "video2img"], &[]),
            entry("model_from-photo", "Photo to 3D", &["img23d"], &["Image to 3D"]),
            entry("model_from-text", "Text to 3D", &["txt23d"], &[]),
            entry("model_retexture", "Retexture", &["3d23d"], &[]),
            entry("model_clip", "Clip", &["txt2video", "img2video"], &["Video"]),
            entry("model_edit-only", "Edit", &["img2img"], &[]),
            // Audio is told apart by tags, in any case.
            entry("model_voice-a", "Voice A", &["txt2audio"], &["TTS"]),
            entry("model_voice-b", "Voice B", &["txt2audio"], &["text-to-speech", "Audio"]),
            entry("model_fx-a", "Effects A", &["txt2audio"], &["SFX"]),
            entry("model_fx-b", "Effects B", &["txt2audio"], &["sound-effects"]),
            entry("model_tune-a", "Tune A", &["txt2audio"], &["Music"]),
            entry("model_tune-b", "Tune B", &["txt2audio"], &["music", "Text to Music"]),
            // Tags that name two types settle nothing, and neither do none: the known-models table does.
            entry("model_both", "Both", &["txt2audio"], &["TTS", "Music"]),
            entry("model_elevenlabs-tts-v3", "No tags", &["txt2audio"], &["Audio"]),
            entry("model_minimax-music-3-0", "Not settled", &["txt2audio"], &["Text to Music"]),
            entry("model_mystery", "Mystery", &["txt2audio"], &[]),
            // A tag that settles it wins over the table, which says speech for this id.
            entry("model_elevenlabs-tts-v4", "Contradicted", &["txt2audio"], &["Music"]),
            // Trained styles and the like are not generated through the custom endpoint.
            {
                let mut lora = picture("model_a-lora");
                lora["type"] = json!("flux.1-lora");
                lora
            },
        ];
        let stand = serving(Library::new(vec![page]));
        let scenario = stand.scenario();
        let of = |kind| -> Vec<String> {
            let list = scenario.list(Some(KEY), kind).unwrap();
            assert_eq!(list.error, None, "{kind:?}");
            assert!(list.models.iter().all(|m| m.provider == "scenario" && m.kinds.contains(&kind)), "{kind:?}");
            list.models.iter().map(|m| m.id.clone()).collect()
        };
        assert_eq!(of(AssetKind::Image), ["model_pic", "model_pic-and-more"]);
        assert_eq!(of(AssetKind::Model3d), ["model_from-photo", "model_from-text"]);
        assert_eq!(of(AssetKind::Speech), ["model_voice-a", "model_voice-b", "model_elevenlabs-tts-v3"]);
        assert_eq!(of(AssetKind::SoundEffect), ["model_fx-a", "model_fx-b"]);
        assert_eq!(of(AssetKind::Music), ["model_tune-a", "model_tune-b", "model_minimax-music-3-0", "model_elevenlabs-tts-v4"]);
        let list = scenario.list(Some(KEY), AssetKind::Image).unwrap();
        assert_eq!(found(&list, "model_pic").name, "A picture model");
        assert_eq!(found(&list, "model_pic").kinds, [AssetKind::Image]);
        // The public list is asked for pages of the most Scenario gives, and is not sent the key.
        let requests = stand.requests();
        assert!(requests[0].starts_with("GET /v1/models/public?pageSize=500 "), "{}", requests[0]);
        assert!(!requests[0].to_ascii_lowercase().contains("authorization"), "{}", requests[0]);
    }

    #[test]
    fn the_capability_table_serves_both_ways() {
        // Every type is listed from some capability, and a capability that settles its type says one.
        for kind in AssetKind::ALL {
            assert!(CAPABILITIES.iter().any(|(_, makes)| makes.contains(&kind)), "{kind:?}");
        }
        assert_eq!(kinds_of(&entry("model_x", "X", &["txt2img"], &[]), "model_x"), [AssetKind::Image]);
        assert_eq!(kinds_of(&entry("model_x", "X", &["txt2img", "img23d"], &[]), "model_x"), [AssetKind::Image, AssetKind::Model3d]);
        assert_eq!(kinds_of(&entry("model_x", "X", &["txt2audio"], &["TTS"]), "model_x"), [AssetKind::Speech]);
        assert!(kinds_of(&entry("model_x", "X", &["txt2audio"], &[]), "model_x").is_empty());
        // The table's kinds count only among the kinds the capability can make: this id makes pictures.
        assert!(kinds_of(&entry("model_google-gemini-3-1-flash", "X", &["txt2audio"], &[]), "model_google-gemini-3-1-flash").is_empty());
        assert!(kinds_of(&json!({"id": "model_x", "capabilities": "txt2img", "tags": 5}), "model_x").is_empty(), "lists of the wrong shape say nothing");
        // A price is for the capabilities of the kinds asked for, and of no kind at all when none is.
        assert!(can_make("txt2img", &[AssetKind::Image]) && can_make("txt2img", &[AssetKind::Model3d, AssetKind::Image]));
        assert!(!can_make("txt2img", &[AssetKind::Model3d]) && !can_make("img2img", &[AssetKind::Image]) && !can_make("unheard-of", &[AssetKind::Image]));
        assert!(!can_make("txt2img", &[]) && !can_make("txt2audio", &[]));
    }

    #[test]
    fn deprecated_models_are_hidden() {
        let page = vec![
            picture("model_fine"),
            entry("model_old-a", "A", &["txt2img"], &["Deprecated", "Image"]),
            entry("model_old-b", "B", &["txt2img"], &["deprecated:model_fine"]),
            entry("model_old-c", "C", &["txt2img"], &["sc:legacy"]),
            entry("model_old-d", "D", &["txt2img"], &["DEPRECATED:model_fine"]),
            entry("model_old-e", "E", &["txt2img"], &["  deprecated "]),
            // Words that only look like it.
            entry("model_not-old", "Not old", &["txt2img"], &["deprecation notes", "legacy", "sc:featured"]),
            // The list may be a few minutes old: the model's own answer can say it is retiring.
            entry("model_stale", "Stale", &["txt2img"], &[]),
        ];
        let stale = described(&entry("model_stale", "Stale", &["txt2img"], &["deprecated:model_fine"]), prompt_only(), None);
        let stand = serving(Library::new(vec![page]).with("model_stale", 200, stale));
        let list = stand.scenario().list(Some(KEY), AssetKind::Image).unwrap();
        assert_eq!(ids(&list), ["model_fine", "model_not-old"]);
        assert_eq!(list.error, None);
        // Nothing is asked about a model that is hidden.
        assert!(stand.model_requests().iter().all(|r| !r.contains("model_old-")), "{:?}", stand.model_requests());
        // Hidden without a key too.
        let page = vec![entry("model_google-gemini-3-1-flash", "Old Gemini", &["txt2img"], &["Deprecated"])];
        assert!(serving(Library::new(vec![page])).scenario().list(None, AssetKind::Image).unwrap().models.is_empty());
    }

    // ---- prices ------------------------------------------------------------------------------

    #[test]
    fn prices_are_credit_quotes() {
        let model = |id: &str, stats: Option<Value>| (picture(id), stats);
        let cases = vec![
            // The default variant of the capability this type needs, not the first one.
            model(
                "model_default",
                Some(stats(
                    json!([
                        variant("img2img", "img2img:1K", json!(7)),
                        variant("txt2img", "txt2img:2K", json!(38)),
                        variant("txt2img", "txt2img:1K", json!(19))
                    ]),
                    Some("txt2img:1K"),
                )),
            ),
            // No default named: the first variant of the capability.
            model(
                "model_first",
                Some(stats(json!([variant("img2img", "a", json!(7)), variant("txt2img", "b", json!(12.5)), variant("txt2img", "c", json!(40))]), None)),
            ),
            // A default that names the wrong capability's variant is not taken.
            model("model_other-default", Some(stats(json!([variant("txt2img", "a", json!(12)), variant("img2video", "v", json!(90))]), Some("v")))),
            model("model_no-stats", None),
            model("model_no-variants", Some(stats(json!([]), None))),
            model("model_no-p50", Some(stats(json!([{"capability": "txt2img", "variantKey": "a", "costPerAssetMinCU": 3}]), None))),
            model("model_wrong-capability", Some(stats(json!([variant("img2video", "v", json!(90))]), None))),
            model("model_text-p50", Some(stats(json!([variant("txt2img", "a", json!("19"))]), None))),
            model("model_odd-stats", Some(json!("nothing"))),
            // A variant with no capability named is as good as any.
            model("model_unnamed", Some(stats(json!([{"variantKey": "a", "costPerAssetP50CU": 4}]), None))),
            // A statistic that is negative, or above the most a limit can be, is no statistic: a
            // run is quoted by a dry run anyway, so the model is still tracked.
            model("model_negative", Some(stats(json!([variant("txt2img", "a", json!(-3))]), None))),
            model("model_huge", Some(stats(json!([variant("txt2img", "a", json!(1e300))]), None))),
            model("model_over", Some(stats(json!([variant("txt2img", "a", json!(MAX_ASSET_CREDITS + 1.0))]), None))),
            model("model_most", Some(stats(json!([variant("txt2img", "a", json!(MAX_ASSET_CREDITS))]), None))),
            model("model_free", Some(stats(json!([variant("txt2img", "a", json!(0))]), None))),
        ];
        let page: Vec<Value> = cases.iter().map(|(entry, _)| entry.clone()).collect();
        let mut library = Library::new(vec![page]);
        for (entry, stats) in &cases {
            let id = entry["id"].as_str().unwrap();
            library = library.with(id, 200, described(entry, picture_inputs(), stats.clone()));
        }
        let stand = serving(library);
        let list = stand.scenario().list(Some(KEY), AssetKind::Image).unwrap();
        let price = |id: &str| found(&list, id).pricing.clone();
        assert_eq!(price("model_default"), quoted(Some(19.0)));
        assert_eq!(price("model_first"), quoted(Some(12.5)));
        assert_eq!(price("model_other-default"), quoted(Some(12.0)));
        for unpriced in ["model_no-stats", "model_no-variants", "model_no-p50", "model_wrong-capability", "model_text-p50", "model_odd-stats"] {
            assert_eq!(price(unpriced), quoted(None), "{unpriced}");
        }
        assert_eq!(price("model_unnamed"), quoted(Some(4.0)));
        for garbage in ["model_negative", "model_huge", "model_over"] {
            assert_eq!(price(garbage), quoted(None), "{garbage}");
        }
        assert_eq!(price("model_most"), quoted(Some(MAX_ASSET_CREDITS)));
        assert_eq!(price("model_free"), quoted(Some(0.0)));
        // One model that makes pictures and 3D models is priced by the capability of the type asked for.
        let both = entry("model_both-kinds", "Both", &["txt2img", "img23d"], &[]);
        let both_stats = stats(json!([variant("txt2img", "a", json!(19)), variant("img23d", "b", json!(120))]), Some("a"));
        let stand_both = serving(Library::new(vec![vec![both.clone()]]).with("model_both-kinds", 200, described(&both, prompt_only(), Some(both_stats))));
        let of = |kind| stand_both.scenario().list(Some(KEY), kind).unwrap().models.remove(0).pricing;
        assert_eq!((of(AssetKind::Image), of(AssetKind::Model3d)), (quoted(Some(19.0)), quoted(Some(120.0))));
        // The model's own answer need not repeat the capabilities or tags the list gave: the price is
        // that of the type the list put it under.
        let bare = picture("model_bare-answer");
        let mut answer: Value =
            serde_json::from_str(&described(&bare, picture_inputs(), Some(stats(json!([variant("txt2img", "a", json!(23))]), None)))).unwrap();
        answer["model"].as_object_mut().unwrap().remove("capabilities");
        answer["model"].as_object_mut().unwrap().remove("tags");
        let stand_bare = serving(Library::new(vec![vec![bare]]).with("model_bare-answer", 200, answer.to_string()));
        assert_eq!(stand_bare.scenario().list(Some(KEY), AssetKind::Image).unwrap().models[0].pricing, quoted(Some(23.0)));
        // describe has only the answer: no capabilities, no kinds, so no variant is the price of anything.
        let detail = stand_bare.scenario().describe(KEY, "model_bare-answer").unwrap();
        assert_eq!((detail.kinds.as_slice(), detail.pricing), (&[][..], quoted(None)));
        // Unless the known-models table has it: that is what it is for.
        let known = entry("model_google-gemini-3-1-flash", "", &["txt2img"], &[]);
        let mut answer: Value = serde_json::from_str(&described(&known, prompt_only(), None)).unwrap();
        answer["model"].as_object_mut().unwrap().remove("capabilities");
        let stand_known = serving(Library::new(vec![vec![known]]).with("model_google-gemini-3-1-flash", 200, answer.to_string()));
        assert_eq!(stand_known.scenario().describe(KEY, "model_google-gemini-3-1-flash").unwrap().kinds, [AssetKind::Image]);
        assert_eq!(found(&list, "model_default").pricing.shown(AssetKind::Image), "~19 credits a picture");
        assert_eq!(found(&list, "model_no-stats").pricing.shown(AssetKind::Image), "quoted before each run");
        assert_eq!(list.error, None);
        // `describe` reads the same price, and the inputs beside it.
        let detail = stand.scenario().describe(KEY, "model_default").unwrap();
        assert_eq!(detail.pricing, quoted(Some(19.0)));
        assert_eq!((detail.id.as_str(), detail.kinds.as_slice()), ("model_default", &[AssetKind::Image][..]));
    }

    // ---- without a key -----------------------------------------------------------------------

    #[test]
    fn without_a_key_only_the_models_this_app_knows_are_listed() {
        let first = vec![picture("model_unknown"), entry("model_google-gemini-3-1-flash", "", &["txt2img"], &["featured"])];
        let second = vec![picture("model_also-unknown")];
        let stand = serving(Library::new(vec![first, second]));
        let list = stand.scenario().list(None, AssetKind::Image).unwrap();
        assert_eq!(ids(&list), ["model_google-gemini-3-1-flash"]);
        let gemini = found(&list, "model_google-gemini-3-1-flash");
        assert_eq!(gemini.name, "Scenario Gemini 3.1", "no name in the list: the table's");
        assert_eq!(gemini.pricing, quoted(None));
        assert!(gemini.ignores.is_empty(), "its inputs are unknown, so nothing is said to be ignored");
        assert!(list.error.as_deref().is_some_and(|e| e.contains("Scenario key")), "{:?}", list.error);
        // Two pages of the list and nothing else: inputs and prices need a key.
        let requests = stand.requests();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /v1/models/public?pageSize=500 "), "{}", requests[0]);
        let next = format!("GET /v1/models/public?pageSize=500&paginationToken={} ", http::encode(&token(1)));
        assert!(requests[1].starts_with(&next), "{}", requests[1]);
        assert!(next.contains("%3D"), "a token is escaped: {next}");
        assert!(requests.iter().all(|r| !r.to_ascii_lowercase().contains("authorization")));
        // Of another type, none is known.
        assert!(stand.scenario().list(None, AssetKind::Music).unwrap().models.is_empty());
        // The 3D model this app offers is Meshy 7.1: the Meshy 6 endpoint it replaced is deprecated.
        let meshy = vec![
            entry("model_meshy-7-1-img23d", "Meshy 7.1 - Image to 3D", &["img23d"], &[]),
            entry("model_meshy-img23d", "Meshy 6 Image-to-3D", &["img23d"], &["Deprecated", "deprecated:model_meshy-7-1-img23d", "sc:legacy"]),
        ];
        let list = serving(Library::new(vec![meshy])).scenario().list(None, AssetKind::Model3d).unwrap();
        assert_eq!(ids(&list), ["model_meshy-7-1-img23d"]);
    }

    // ---- describe: inputs, price, and models that are gone --------------------------------------

    #[test]
    fn a_model_is_described_with_its_inputs_and_price() {
        let entry = entry("model_pic", "Pic", &["txt2img"], &["Image"]);
        let body = described(&entry, picture_inputs(), Some(stats(json!([variant("txt2img", "txt2img:1K", json!(19))]), Some("txt2img:1K"))));
        let stand = serving(Library::new(vec![vec![entry]]).with("model_pic", 200, body));
        let scenario = stand.scenario();
        let detail = scenario.describe(&format!("  {KEY} "), "model_pic").unwrap();
        assert_eq!((detail.name.as_str(), &detail.kinds), ("Pic", &vec![AssetKind::Image]));
        assert_eq!(detail.pricing, quoted(Some(19.0)));
        let ratio = detail.fields.iter().find(|f| f.name == "aspectRatio").unwrap();
        assert_eq!(ratio.kind, FieldKind::Choice(vec!["1:1".into(), "16:9".into(), "9:16".into()]));
        assert!(ratio.has_default && !ratio.required);
        assert!(detail.fields.iter().any(|f| f.name == "prompt" && f.required && f.kind == FieldKind::Text));
        // Described again within ten minutes, with the same key: what Scenario said is used again, unasked.
        assert_eq!(scenario.describe(KEY, "model_pic").unwrap(), detail);
        let requests = stand.requests();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("GET /v1/models/model_pic "), "{}", requests[0]);
        use base64::Engine as _;
        let basic = base64::engine::general_purpose::STANDARD.encode(KEY).to_ascii_lowercase();
        assert!(requests[0].to_ascii_lowercase().contains(&format!("authorization: basic {basic}\r\n")), "the key, trimmed: {}", requests[0]);
    }

    #[test]
    fn a_removed_model_is_a_clear_error() {
        let gone = described(&entry("model_gone", "Gone", &["txt2img"], &[]), prompt_only(), None).replace("\"trained\"", "\"deleted\"");
        let stand = serving(Library::new(vec![vec![]]).with("model_gone", 200, gone));
        let scenario = stand.scenario();
        // A 404 (nothing by that id), a model Scenario still answers for but has deleted, and a model
        // the answer of which has nothing in it to read.
        for id in ["model_missing", "model_gone"] {
            let err = scenario.describe(KEY, id).unwrap_err();
            assert_eq!(err.code, ErrorCode::BadSettings, "{id}");
            assert!(err.message.contains(id) && !err.message.contains("SECRET-BODY"), "{}", err.message);
            let remediation = err.remediation.as_deref().unwrap();
            assert!(remediation.contains("Choose another model") && remediation.contains("Generated Assets"), "{remediation}");
        }
        let nothing = serving(Library::new(vec![vec![]]).with("model_empty", 200, "{}".into()));
        assert_eq!(nothing.scenario().describe(KEY, "model_empty").unwrap_err().code, ErrorCode::GarbledOutput);
    }

    /// The design: a chosen model that is deprecated at the provider fails with a typed error that
    /// names it and says to choose another, the same as one that has gone, and before anything else
    /// about it is read.
    #[test]
    fn a_model_being_retired_is_a_clear_error_too() {
        for tags in [json!(["Deprecated"]), json!(["deprecated:model_new"]), json!(["sc:legacy"])] {
            let mut retiring = picture("model_retiring");
            retiring["tags"] = tags.clone();
            // Its inputs cannot be read either: being retired is what is said.
            let body = described(&retiring, json!("not a list"), None);
            let stand = serving(Library::new(vec![vec![]]).with("model_retiring", 200, body));
            let err = stand.scenario().describe(KEY, "model_retiring").unwrap_err();
            assert_eq!(err.code, ErrorCode::BadSettings, "{tags}");
            assert!(err.message.contains("model_retiring") && err.message.contains("retired"), "{}", err.message);
            let remediation = err.remediation.as_deref().unwrap();
            assert!(remediation.contains("Choose another model") && remediation.contains("Generated Assets"), "{remediation}");
            assert_eq!(stand.requests().len(), 1);
        }
        // A model that is not marked is described.
        let stand = serving(Library::new(vec![vec![picture("model_active")]]));
        assert!(stand.scenario().describe(KEY, "model_active").is_ok());
    }

    #[test]
    fn a_key_or_model_id_that_cannot_be_sent_is_refused_before_anything_is_asked() {
        // Nothing listens at this address: a request would fail as a network error.
        let scenario = Scenario { base: "http://127.0.0.1:9/v1".into(), ..Scenario::default() };
        for key in ["", "  ", "no-colon", ":secret", "key:", "k\u{e9}y:secret", "k:s\nx"] {
            assert_eq!(scenario.list(Some(key), AssetKind::Image).unwrap_err().code, ErrorCode::BadSettings, "{key:?}");
            assert_eq!(scenario.describe(key, "model_pic").unwrap_err().code, ErrorCode::BadSettings, "{key:?}");
        }
        for model in ["", "../x", "a b", "a/b", "model_x?y=1", "model_x.y", "\u{e9}", &"a".repeat(201)] {
            let err = scenario.describe(KEY, model).unwrap_err();
            assert_eq!(err.code, ErrorCode::BadSettings, "{model:?}");
            assert!(err.remediation.is_some());
        }
    }

    // ---- the inputs of a model ---------------------------------------------------------------

    #[test]
    fn inputs_are_read_into_the_fields_the_app_can_fill() {
        let inputs = json!([
            {"name": "prompt", "type": "string", "prompt": true, "required": {"always": true}},
            {"name": "aspectRatio", "type": "string", "allowedValues": ["1:1", "16:9"], "default": "1:1"},
            {"name": "negativePrompt", "type": "string", "allowedValues": []},
            {"name": "steps", "type": "number", "min": 1, "max": 50, "default": 30, "required": {"always": false}},
            {"name": "guidanceScale", "type": "number"},
            {"name": "duration", "type": "number", "allowedValues": [5, 10]},
            {"name": "enablePbr", "type": "boolean", "default": null},
            {"name": "image", "type": "file", "kind": "image", "required": {"always": true}},
            {"name": "photo", "type": "file"},
            {"name": "sound", "type": "file", "kind": "audio", "required": {"always": true}},
            {"name": "references", "type": "file_array", "kind": "image"},
            {"name": "gallery", "type": "file_array"},
            {"name": "clips", "type": "file_array", "kind": "video"},
            {"name": "tags", "type": "string_array"},
            {"name": "weights", "type": "number_array"},
            {"name": "loras", "type": "model_array"},
            {"name": "base", "type": "model"},
            {"name": "layers", "type": "inputs_array", "inputs": [{"name": "inner", "type": "string"}]},
            {"name": "mask", "type": "file", "required": {"conditional": {"values": {"mode": "edit"}, "operator": "and"}}},
            {"name": "other", "type": "string", "required": {"ifDefined": {"mask": "Needed with a mask"}}},
            {"name": "third", "type": "string", "required": {"ifNotDefined": {"mask": "Needed without a mask"}}},
            {"name": "bare", "type": "string", "required": true},
            {"name": "mystery", "type": "hologram"},
            {"name": "prompt", "type": "boolean"}
        ]);
        let fields = scenario_fields(&inputs).unwrap();
        let field = |name: &str| fields.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no {name}")).clone();
        assert_eq!(fields.len(), 23, "a name given twice is read once");
        assert_eq!(field("prompt").kind, FieldKind::Text, "the first of two inputs of one name");
        let prompt = field("prompt");
        assert!(prompt.required && !prompt.has_default);
        let ratio = field("aspectRatio");
        assert_eq!(ratio.kind, FieldKind::Choice(vec!["1:1".into(), "16:9".into()]));
        assert!(!ratio.required && ratio.has_default);
        assert_eq!(field("negativePrompt").kind, FieldKind::Text, "an empty list of values limits nothing");
        let steps = field("steps");
        assert_eq!((steps.kind, steps.has_default, steps.required), (FieldKind::Number { min: Some(1.0), max: Some(50.0) }, true, false));
        assert_eq!(field("guidanceScale").kind, FieldKind::Number { min: None, max: None });
        assert_eq!(field("duration").kind, FieldKind::Other, "a number from a fixed set is not one this app can fill");
        assert_eq!((field("enablePbr").kind, field("enablePbr").has_default), (FieldKind::Bool, false), "a default of null is none");
        assert_eq!((field("image").kind, field("image").required), (FieldKind::Image, true));
        assert_eq!(field("photo").kind, FieldKind::Image, "a file is a picture unless it says otherwise");
        assert_eq!(field("sound").kind, FieldKind::Other);
        // A list of pictures is one this app can fill (with the one picture); a list of anything else is not.
        assert_eq!((field("references").kind, field("gallery").kind), (FieldKind::Images, FieldKind::Images));
        for other in ["clips", "tags", "weights", "loras", "base", "layers", "mystery"] {
            assert_eq!(field(other).kind, FieldKind::Other, "{other}");
        }
        // Required only when something else is, or is not, given: this app never gives those.
        for conditional in ["mask", "other", "third"] {
            assert!(!field(conditional).required, "{conditional}");
        }
        assert!(field("bare").required, "a plain `true` is read too");
        assert_eq!(scenario_fields(&json!([])).unwrap(), []);
    }

    #[test]
    fn inputs_that_cannot_be_read_are_refused_with_what_to_do() {
        let refuse = |inputs: Value| scenario_fields(&inputs).unwrap_err();
        for unreadable in [
            json!("inputs"),
            json!({"name": "prompt"}),
            json!(null),
            json!([{"type": "string"}]),
            json!([{"name": 5, "type": "string"}]),
            json!([{"name": "", "type": "string"}]),
            json!([7]),
        ] {
            let err = refuse(unreadable.clone());
            assert_eq!(err.code, ErrorCode::GarbledOutput, "{unreadable}");
            assert!(err.remediation.as_deref().is_some_and(|r| r.contains("Choose another model")));
        }
        let many: Vec<Value> = (0..=MAX_FIELDS).map(|i| json!({"name": format!("f{i}"), "type": "string"})).collect();
        assert_eq!(refuse(json!(many)).code, ErrorCode::OverLimit);
        let few: Vec<Value> = (0..MAX_FIELDS).map(|i| json!({"name": format!("f{i}"), "type": "string"})).collect();
        assert_eq!(scenario_fields(&json!(few)).unwrap().len(), MAX_FIELDS);
        let values: Vec<String> = (0..=MAX_CHOICES).map(|i| format!("v{i}")).collect();
        assert_eq!(refuse(json!([{"name": "size", "type": "string", "allowedValues": values}])).code, ErrorCode::OverLimit);
        let values: Vec<String> = (0..MAX_CHOICES).map(|i| format!("v{i}")).collect();
        assert!(scenario_fields(&json!([{"name": "size", "type": "string", "allowedValues": values}])).is_ok());
        let long = "n".repeat(129);
        let err = refuse(json!([{"name": long, "type": "string"}]));
        assert_eq!(err.code, ErrorCode::OverLimit);
        assert!(!err.message.contains("nnnn"), "{}", err.message);
        assert!(scenario_fields(&json!([{"name": "n".repeat(128), "type": "string"}])).is_ok());
        // Nesting is never followed: the inner inputs of a list are not read at all.
        let mut nested = json!({"name": "leaf", "type": "string"});
        for _ in 0..60 {
            nested = json!({"name": "layer", "type": "inputs_array", "inputs": [nested]});
        }
        assert_eq!(scenario_fields(&json!([nested])).unwrap().len(), 1);
    }

    /// The roles of Scenario's own input names (camelCase) let a request be laid out in its inputs.
    #[test]
    fn scenario_inputs_take_a_request() {
        use crate::assets::inputs::map_request;
        let roles = InputRoles::built_in();
        let fields = scenario_fields(&picture_inputs()).unwrap();
        let wide = AssetRequest::Image { prompt: "a crate", shape: Shape::Wide };
        let mapped = map_request(&fields, &wide, roles).unwrap();
        // One picture a run: the price of a run is the price of one.
        assert_eq!(Value::Object(mapped.body), json!({"prompt": "a crate", "aspectRatio": "16:9", "numOutputs": 1}));
        assert_eq!(mapped.ignored, []);
        assert_eq!(runnable(&fields, AssetKind::Image, roles), Ok(vec![]));

        let solid = scenario_fields(&json!([
            {"name": "image", "type": "file", "kind": "image", "required": {"always": true}},
            {"name": "targetPolycount", "type": "number", "min": 100, "max": 300000},
            {"name": "shouldTexture", "type": "boolean"}
        ]))
        .unwrap();
        let request = AssetRequest::Model3d { image_url: Some("asset_abc"), prompt: None, textured: true, faces: 3000 };
        let mapped = map_request(&solid, &request, roles).unwrap();
        // A face count is a whole number, though Scenario has only one kind of number.
        assert_eq!(Value::Object(mapped.body), json!({"image": "asset_abc", "targetPolycount": 3000, "shouldTexture": true}));
        assert_eq!(runnable(&solid, AssetKind::Model3d, roles), Ok(vec![]));

        // A model that takes its picture as a list, as Scenario's image-to-3D models may, is sent a list.
        let listed = scenario_fields(&json!([
            {"name": "image", "type": "file_array", "kind": "image", "required": {"always": true}},
            {"name": "targetPolycount", "type": "number"}
        ]))
        .unwrap();
        let mapped = map_request(&listed, &request, roles).unwrap();
        assert_eq!(Value::Object(mapped.body), json!({"image": ["asset_abc"], "targetPolycount": 3000}));
        assert_eq!(runnable(&listed, AssetKind::Model3d, roles), Ok(vec![AssetOption::Textured]));
    }

    #[test]
    fn a_model_this_app_cannot_fill_is_left_out_and_one_that_cannot_be_read_is_reported() {
        let page = vec![
            picture("model_fine"),
            picture("model_needs-mask"),
            picture("model_needs-list"),
            picture("model_no-inputs"),
            picture("model_removed"),
            picture("model_broken"),
        ];
        let needs = |inputs: Value| described(&picture("x"), inputs, None);
        let library = Library::new(vec![page])
            .with("model_needs-mask", 200, needs(json!([prompt_input(), {"name": "mask", "type": "file", "required": {"always": true}}])))
            .with(
                "model_needs-list",
                200,
                needs(json!([prompt_input(), {"name": "references", "type": "file_array", "kind": "image", "required": {"always": true}}])),
            )
            .with("model_no-inputs", 200, json!({"model": {"id": "model_no-inputs"}}).to_string())
            .with("model_removed", 404, "{}".into());
        let stand = serving(library.with("model_broken", 500, "oops".into()));
        let list = stand.scenario().list(Some(KEY), AssetKind::Image).unwrap();
        assert_eq!(ids(&list), ["model_fine"]);
        // A model that is gone or unreadable is left out quietly; one whose answer failed is counted.
        let error = list.error.as_deref().unwrap();
        assert!(error.contains("1 model") && error.contains("could not be read"), "{error}");
        assert!(!error.contains("SECRET-BODY") && !error.contains("oops"), "{error}");
        let stand = serving(Library::new(vec![vec![picture("model_gone")]]).with("model_gone", 404, "{}".into()));
        let list = stand.scenario().list(Some(KEY), AssetKind::Image).unwrap();
        assert_eq!((list.models.len(), list.error), (0, None), "a model that went between the list and its answer is not a failure");
    }

    /// One model's answer that is no use (too large, not JSON, no model in it) or that refuses this account
    /// that model is not a model to run, though this app knows its id: it is left out, and the rest of the
    /// list is read. A request that merely failed (the server's 500) says nothing of the model, so the
    /// table's word for it stands.
    #[test]
    fn what_one_models_answer_says_is_not_what_the_list_says() {
        const KNOWN: &str = "model_google-gemini-3-1-flash";
        let huge = "x".repeat(usize::try_from(http::MAX_JSON).unwrap() + 1);
        let cases = [
            ("too large", 200, huge, false),
            ("not json", 200, "not json".to_string(), false),
            ("no model", 200, "{}".to_string(), false),
            ("forbidden", 403, "{}".to_string(), false),
            // Any other refusal says this app asked wrongly or the model is not for it: not a passing failure.
            ("bad request", 400, "{}".to_string(), false),
            ("gone", 410, "{}".to_string(), false),
            ("unprocessable", 422, "{}".to_string(), false),
            ("teapot", 418, "{}".to_string(), false),
            // Asked not to follow a redirect, an answer that is one is not the model.
            ("moved", 301, "{}".to_string(), false),
            ("see other", 303, "{}".to_string(), false),
            ("not modified", 304, "{}".to_string(), false),
            // The network, a timeout, a request answered too early and a server's trouble may pass.
            ("request timeout", 408, "{}".to_string(), true),
            ("too early", 425, "{}".to_string(), true),
            ("server error", 500, "oops".to_string(), true),
            ("bad gateway", 502, "oops".to_string(), true),
            ("unavailable", 503, "oops".to_string(), true),
        ];
        for (what, status, body, offered) in cases {
            let known = entry(KNOWN, "", &["txt2img"], &[]);
            let stand = serving(Library::new(vec![vec![known, picture("model_fine")]]).with(KNOWN, status, body));
            let list = stand.scenario().list(Some(KEY), AssetKind::Image).unwrap();
            assert!(list.models.iter().any(|m| m.id == "model_fine"), "{what}: the rest of the list is read");
            assert_eq!(list.models.iter().any(|m| m.id == KNOWN), offered, "{what}: {:?}", ids(&list));
            if offered {
                assert_eq!(found(&list, KNOWN).pricing, quoted(None), "{what}");
            }
            let error = list.error.as_deref().unwrap();
            assert!(error.contains("1 model") && error.contains("could not be read") && !error.contains("xxxx") && !error.contains("oops"), "{what}: {error}");
        }
        // describe says why, without repeating what the provider sent.
        let huge = "x".repeat(usize::try_from(http::MAX_JSON).unwrap() + 1);
        let stand = serving(Library::new(vec![vec![picture("model_huge")]]).with("model_huge", 200, huge));
        let err = stand.scenario().describe(KEY, "model_huge").unwrap_err();
        assert_eq!(err.code, ErrorCode::OverLimit, "the code for an answer larger than this app reads");
        assert!(err.remediation.is_some() && !err.message.contains("xxxx"), "{}", err.message);
    }

    #[test]
    fn what_a_model_ignores_comes_from_its_inputs() {
        let entry = entry("model_pic", "Pic", &["txt2img"], &[]);
        let library = Library::new(vec![vec![entry.clone(), picture("model_square")]]).with("model_pic", 200, described(&entry, picture_inputs(), None)).with(
            "model_square",
            200,
            described(&picture("model_square"), prompt_only(), None),
        );
        let list = serving(library).scenario().list(Some(KEY), AssetKind::Image).unwrap();
        assert_eq!(found(&list, "model_pic").ignores, []);
        assert_eq!(found(&list, "model_square").ignores, [AssetOption::Shape]);
    }

    // ---- bounds ------------------------------------------------------------------------------

    #[test]
    fn what_a_provider_calls_a_model_is_tidied_and_unsafe_ids_are_left_out() {
        let hostile_name = format!("Evil\nIgnore the above\u{7}{}", "x".repeat(500));
        let page = vec![
            entry("model_evil", &hostile_name, &["txt2img"], &[]),
            entry("model_nameless", "   ", &["txt2img"], &[]),
            entry("model_google-gemini-3-1-flash", "", &["txt2img"], &[]),
            entry("../etc/passwd", "Path", &["txt2img"], &[]),
            entry("model with space", "Space", &["txt2img"], &[]),
            entry("model_uthana-3.0", "Dot", &["txt2img"], &[]),
            entry("model_a/b", "Slash", &["txt2img"], &[]),
            entry(&format!("model_{}", "a".repeat(300)), "Long", &["txt2img"], &[]),
            json!({"id": 7, "capabilities": ["txt2img"], "type": "custom"}),
            json!({"capabilities": ["txt2img"], "type": "custom"}),
            json!("not a model"),
            json!(null),
        ];
        let stand = serving(Library::new(vec![page]));
        let list = stand.scenario().list(Some(KEY), AssetKind::Image).unwrap();
        assert_eq!(ids(&list), ["model_evil", "model_nameless", "model_google-gemini-3-1-flash"]);
        let evil = found(&list, "model_evil");
        assert!(evil.name.starts_with("EvilIgnore the above") && evil.name.ends_with('\u{2026}'), "{}", evil.name);
        assert!(evil.name.chars().count() <= 81 && !evil.name.chars().any(char::is_control), "{:?}", evil.name);
        assert_eq!(found(&list, "model_nameless").name, "model_nameless", "no name shown: its id");
        assert_eq!(found(&list, "model_google-gemini-3-1-flash").name, "Scenario Gemini 3.1", "no name from Scenario: the table's");
        assert_eq!(stand.model_requests().len(), 3, "an unsafe id is never put in a URL");
    }

    #[test]
    fn a_page_is_read_only_up_to_its_bound() {
        // Only a model this app knows is listed without a key, so the known one marks the bound.
        let known = || entry("model_google-gemini-3-1-flash", "Gemini", &["txt2img"], &[]);
        for (at, listed) in [(MAX_PAGE_MODELS - 1, true), (MAX_PAGE_MODELS, false)] {
            let mut page: Vec<Value> = (0..at).map(|i| picture(&format!("model_p{i}"))).collect();
            page.push(known());
            page.extend((0..50).map(|i| picture(&format!("model_q{i}"))));
            let list = serving(Library::new(vec![page])).scenario().list(None, AssetKind::Image).unwrap();
            assert_eq!(list.models.len(), usize::from(listed), "the model at {at}");
        }
    }

    #[test]
    fn a_page_larger_than_the_limit_is_refused() {
        // One byte over what a page may be: the whole body is read, the page is not.
        let (base, _) = http::tests::serve(|_| vec![(200, "x".repeat(usize::try_from(http::MAX_LIST_PAGE).unwrap() + 1))]);
        let err = Scenario { base: format!("{base}/v1"), ..Scenario::default() }.list(None, AssetKind::Image).unwrap_err();
        assert_eq!(err.code, ErrorCode::OverLimit);
        assert!(!err.message.contains("xxxx"), "{}", err.message);
    }

    #[test]
    fn a_list_stops_at_the_page_limit_and_says_so() {
        // Every page names a model of its own and another page after it.
        let count = Arc::new(AtomicUsize::new(0));
        let stand = stand(no_pause, move |target, _| {
            if let Some(id) = target.strip_prefix("/v1/models/model_") {
                return (200, described(&picture(&format!("model_{id}")), prompt_only(), None));
            }
            let n = count.fetch_add(1, Ordering::SeqCst);
            (200, json!({"models": [picture(&format!("model_m{n}"))], "nextPaginationToken": token(1)}).to_string())
        });
        let list = stand.scenario().list(Some(KEY), AssetKind::Image).unwrap();
        let pages = stand.requests().iter().filter(|r| r.starts_with("GET /v1/models/public")).count();
        assert_eq!(pages, MAX_PAGES);
        assert_eq!(list.models.len(), MAX_PAGES);
        assert!(list.error.as_deref().is_some_and(|e| e.contains("incomplete") && e.contains(&MAX_PAGES.to_string())), "{:?}", list.error);
    }

    #[test]
    fn a_next_page_token_that_cannot_be_asked_for_ends_the_list_and_says_so() {
        let unusable = [json!("a".repeat(10_000)), json!("a".repeat(CURSOR_LENGTH + 1)), json!(5), json!({"offset": 1}), json!(["t"]), json!(true)];
        for next in unusable {
            let first = json!({"models": [picture("model_one")], "nextPaginationToken": next}).to_string();
            let stand = stand(no_pause, move |target, _| {
                if target.starts_with("/v1/models/public") { (200, first.clone()) } else { (200, described(&picture("model_one"), prompt_only(), None)) }
            });
            let list = stand.scenario().list(Some(KEY), AssetKind::Image).unwrap();
            assert_eq!(ids(&list), ["model_one"], "{next}");
            assert_eq!(stand.requests().iter().filter(|r| r.starts_with("GET /v1/models/public")).count(), 1, "no token, no second page");
            assert!(list.error.as_deref().is_some_and(|e| e.contains("incomplete") && e.contains("next page")), "{next}: {:?}", list.error);
        }
        // No token, or an empty one, is the last page. One of the longest size allowed is followed.
        for last in [json!(null), json!("")] {
            let answer = json!({"models": [picture("model_one")], "nextPaginationToken": last}).to_string();
            let stand = stand(no_pause, move |target, _| {
                (200, if target.starts_with("/v1/models/public") { answer.clone() } else { described(&picture("model_one"), prompt_only(), None) })
            });
            assert_eq!(stand.scenario().list(Some(KEY), AssetKind::Image).unwrap().error, None, "{last}");
        }
        let longest = "a".repeat(CURSOR_LENGTH);
        let first = json!({"models": [], "nextPaginationToken": longest}).to_string();
        let stand = stand(no_pause, move |target, _| (200, if target.contains("paginationToken") { json!({"models": []}).to_string() } else { first.clone() }));
        assert_eq!(stand.scenario().list(Some(KEY), AssetKind::Image).unwrap().error, None);
        assert!(stand.requests()[1].contains(&format!("&paginationToken={longest} ")), "{}", stand.requests()[1]);
    }

    #[test]
    fn a_rate_limit_part_way_returns_what_was_read_and_says_so() {
        let first = vec![entry("model_google-gemini-3-1-flash", "", &["txt2img"], &[])];
        let page = json!({"models": first, "nextPaginationToken": token(1)}).to_string();
        let stand =
            stand(
                no_pause,
                move |target, _| {
                    if target.contains("paginationToken") { (429, json!({"message": "SECRET-BODY"}).to_string()) } else { (200, page.clone()) }
                },
            );
        let list = stand.scenario().list(None, AssetKind::Image).unwrap();
        assert_eq!(ids(&list), ["model_google-gemini-3-1-flash"], "the pages read before the limit are kept");
        let error = list.error.as_deref().unwrap();
        assert!(error.contains("incomplete") && error.contains("rate limit"), "{error}");
        assert!(!error.contains("SECRET-BODY"), "a provider's words are not repeated: {error}");
        // With nothing read at all it is an error.
        let nothing = stand_with(429, "{}");
        assert_eq!(nothing.scenario().list(None, AssetKind::Image).unwrap_err().code, ErrorCode::OverLimit);
        assert_eq!(stand_with(200, "not json").scenario().list(None, AssetKind::Image).unwrap_err().code, ErrorCode::GarbledOutput);
        assert_eq!(stand_with(200, "{}").scenario().list(None, AssetKind::Image).unwrap_err().code, ErrorCode::GarbledOutput, "no `models`");
        assert_eq!(stand_with(500, "{}").scenario().list(None, AssetKind::Image).unwrap_err().code, ErrorCode::ProgramFailed);
        let unreachable = Scenario { base: "http://127.0.0.1:9/v1".into(), ..Scenario::default() };
        assert_eq!(unreachable.list(None, AssetKind::Image).unwrap_err().code, ErrorCode::EngineMissing);
    }

    fn stand_with(status: u16, body: &'static str) -> Stand {
        stand(no_pause, move |_, _| (status, body.to_string()))
    }

    #[test]
    fn a_key_the_provider_refuses_or_a_limit_it_reaches_stops_the_reading_and_keeps_what_is_known() {
        let everything_answers = |status: u16| {
            let mut page: Vec<Value> = (0..20).map(|i| picture(&format!("model_p{i}"))).collect();
            page.push(entry("model_google-gemini-3-1-flash", "", &["txt2img"], &[]));
            let mut library = Library::new(vec![page]);
            for id in library.models.keys().cloned().collect::<Vec<_>>() {
                library = library.with(&id, status, "{}".into());
            }
            serving(library)
        };
        for (status, said) in [(401, "credential"), (402, "limit"), (429, "limit")] {
            let stand = everything_answers(status);
            let list = stand.scenario().list(Some(KEY), AssetKind::Image).unwrap();
            assert!(stand.model_requests().len() <= 4, "{status}: the first refusal ends it: {}", stand.model_requests().len());
            assert_eq!(ids(&list), ["model_google-gemini-3-1-flash"], "{status}: the models this app knows are still offered");
            assert_eq!(found(&list, "model_google-gemini-3-1-flash").pricing, quoted(None));
            let error = list.error.as_deref().unwrap();
            assert!(error.contains("could not be read") && error.contains(said), "{status}: {error}");
        }
        // A 403 is this account being kept from one model, not from the rest: every model is asked for,
        // none is offered (not even the one this app knows), and each is counted.
        let stand = everything_answers(403);
        let list = stand.scenario().list(Some(KEY), AssetKind::Image).unwrap();
        assert_eq!(stand.model_requests().len(), 21);
        assert!(list.models.is_empty(), "{:?}", ids(&list));
        assert!(list.error.as_deref().is_some_and(|e| e.contains("21 models could not be read")), "{:?}", list.error);
    }

    #[test]
    fn inputs_are_read_four_models_at_a_time_and_at_most_three_hundred() {
        let mut page: Vec<Value> = (0..304).map(|i| picture(&format!("model_p{i}"))).collect();
        page.push(entry("model_google-gemini-3-1-flash", "", &["txt2img"], &[]));
        let library = Library::new(vec![page]);
        let stand = stand(
            |target| if target.starts_with("/v1/models/public") { Duration::ZERO } else { Duration::from_millis(15) },
            move |target, _| library.answer(target),
        );
        let list = stand.scenario().list(Some(KEY), AssetKind::Image).unwrap();
        assert_eq!(stand.model_requests().len(), 300);
        let busiest = stand.busiest.load(Ordering::SeqCst);
        assert!((2..=4).contains(&busiest), "{busiest} at once");
        // The models beyond the bound are not read: only the one this app knows is still listed.
        assert_eq!(list.models.len(), 300 + 1);
        assert_eq!(list.models.last().map(|m| m.id.as_str()), Some("model_google-gemini-3-1-flash"));
        assert_eq!(found(&list, "model_google-gemini-3-1-flash").pricing, quoted(None));
        let error = list.error.as_deref().unwrap();
        assert!(error.contains("incomplete") && error.contains("300"), "{error}");
        // Listed in the order Scenario gave.
        assert_eq!(ids(&list).first(), Some(&"model_p0"));
    }

    /// The deadline covers the whole call: the pages and the models' answers alike.
    #[test]
    fn a_list_that_takes_too_long_is_cut_short() {
        // The page arrives after the deadline, so no second page is asked for.
        let first = json!({"models": [entry("model_google-gemini-3-1-flash", "", &["txt2img"], &[])], "nextPaginationToken": token(1)}).to_string();
        let slow = stand(|_| Duration::from_millis(800), move |_, _| (200, first.clone()));
        let scenario = Scenario { list_deadline: Duration::from_millis(500), ..slow.scenario() };
        let list = scenario.list(None, AssetKind::Image).unwrap();
        assert_eq!(ids(&list), ["model_google-gemini-3-1-flash"], "what was read is kept");
        let error = list.error.as_deref().unwrap();
        assert!(error.contains("incomplete") && error.contains("cut short"), "{error}");
        assert_eq!(slow.requests().len(), 1);

        // The models' answers are slow: those not reached are not asked for.
        let page: Vec<Value> = (0..12).map(|i| picture(&format!("model_p{i}"))).collect();
        let library = Library::new(vec![page]);
        let slow = stand(
            |target| if target.starts_with("/v1/models/public") { Duration::ZERO } else { Duration::from_millis(700) },
            move |target, _| library.answer(target),
        );
        let scenario = Scenario { list_deadline: Duration::from_millis(400), ..slow.scenario() };
        let list = scenario.list(Some(KEY), AssetKind::Image).unwrap();
        assert_eq!(slow.model_requests().len(), 4, "one each, then the time was up");
        assert_eq!(list.models.len(), 4);
        let error = list.error.as_deref().unwrap();
        assert!(error.contains("incomplete") && error.contains("cut short"), "{error}");

        // Out of time before the first request: nothing was read, so it is an error.
        let none = Scenario { list_deadline: Duration::ZERO, base: "http://127.0.0.1:9/v1".into(), ..Scenario::default() };
        assert_eq!(none.list(Some(KEY), AssetKind::Image).unwrap_err().code, ErrorCode::TimedOut);
    }
}
