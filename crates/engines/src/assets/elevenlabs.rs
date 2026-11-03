//! ElevenLabs audio, using the caller's key and billed in credits. [`ElevenLabs::list`] and
//! [`ElevenLabs::describe`] say which models make each type of audio and what they cost in credits:
//! speech models come from the account's own list, sound effects and music are the two fixed
//! endpoints whose rates the app keeps (`known_models.toml`). A run sends the request laid out in
//! the endpoint's inputs ([`map_request`]) with the chosen model named.
use super::inputs::{FieldKind, InputField, Mapped, REMEDIATION, map_request, runnable};
use super::model::{AssetOption, ListedModel, MAX_ASSET_CREDITS, ModelDetail, ModelList, Pricing, Rate, Unit};
use super::tables::{InputRoles, KnownModel, KnownModels};
use super::{AssetKind, AssetProvider, AssetRequest, Generated, charged_or_unknown, estimate_from, http};
use craft_anything_evidence::{ErrorCode, ToolError};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::time::Duration;

pub const ID: &str = "elevenlabs";
/// What people call it.
const NAME: &str = "ElevenLabs";
pub struct ElevenLabs {
    base: String,
}
impl Default for ElevenLabs {
    /// Use the production ElevenLabs API endpoint.
    fn default() -> Self {
        Self { base: "https://api.elevenlabs.io/v1".into() }
    }
}

/// The lengths, in seconds, of a sound effect and of a track that ElevenLabs makes.
const SOUND_SECONDS: (f64, f64) = (0.5, 30.0);
const MUSIC_SECONDS: (f64, f64) = (3.0, 600.0);
/// The longest text spoken, and the longest description of a sound or a track.
const MAX_SPEECH: usize = 10_000;
const MAX_PROMPT: usize = 2000;
/// How long one generation may take.
const GENERATE_TIMEOUT: Duration = Duration::from_secs(600);
/// The header in which ElevenLabs says how many credits a clip of speech cost.
const CHARACTER_COST: &str = "character-cost";
/// The input of a speech request that ElevenLabs takes in the address rather than the body.
const VOICE_FIELD: &str = "voice_id";
/// The input every request names its model in.
const MODEL_FIELD: &str = "model_id";

/// Where a request for `req` goes, and its body: the request laid out in the endpoint's inputs, with
/// the chosen model named. Also the options the inputs leave unused. Lengths, texts and the voice
/// are checked against what ElevenLabs takes, and a model of another type is refused.
fn request(model: &ModelDetail, req: &AssetRequest) -> Result<Sent, ToolError> {
    if http::id(&model.id).is_err() {
        return Err(http::bad("That is not an id ElevenLabs gives its models.").with_remediation(REMEDIATION));
    }
    if !model.kinds.contains(&req.kind()) {
        return Err(http::bad("The chosen ElevenLabs model does not make this type of audio.").with_remediation(REMEDIATION));
    }
    let path = match *req {
        AssetRequest::Speech { text, voice_id } => {
            http::text(text, MAX_SPEECH)?;
            http::id(voice_id)?;
            format!("text-to-speech/{voice_id}?output_format=mp3_44100_128")
        }
        AssetRequest::SoundEffect { prompt, duration_seconds } => {
            http::text(prompt, MAX_PROMPT)?;
            http::duration(duration_seconds, SOUND_SECONDS.0, SOUND_SECONDS.1)?;
            "sound-generation?output_format=mp3_44100_128".to_string()
        }
        AssetRequest::Music { prompt, duration_seconds, .. } => {
            http::text(prompt, MAX_PROMPT)?;
            http::duration(duration_seconds, MUSIC_SECONDS.0, MUSIC_SECONDS.1)?;
            "music?output_format=mp3_44100_128".to_string()
        }
        AssetRequest::Image { .. } | AssetRequest::Model3d { .. } => return Err(http::bad("ElevenLabs makes speech, sound effects and music only.")),
    };
    let Mapped { mut body, ignored } = map_request(&model.fields, req, InputRoles::built_in())?;
    body.remove(VOICE_FIELD);
    body.insert(MODEL_FIELD.to_string(), json!(model.id));
    Ok(Sent { path, body, ignored })
}

/// A request as it is sent: where to, its body, and the options of the request it leaves unused.
struct Sent {
    path: String,
    body: Map<String, Value>,
    ignored: Vec<AssetOption>,
}

/// The credits ElevenLabs says a clip of speech cost: a number that can be believed.
fn character_cost(headers: &ureq::http::HeaderMap) -> Option<f64> {
    let credits: f64 = headers.get(CHARACTER_COST)?.to_str().ok()?.trim().parse().ok()?;
    (credits.is_finite() && (0.0..=MAX_ASSET_CREDITS).contains(&credits)).then_some(credits)
}

impl AssetProvider for ElevenLabs {
    /// Return the identifier a project's choice names.
    fn id(&self) -> &'static str {
        ID
    }
    /// Return the provider's display name.
    fn name(&self) -> &'static str {
        NAME
    }
    /// Name the environment variable used when no credential is stored.
    fn key_env(&self) -> Option<&'static str> {
        Some("ELEVENLABS_API_KEY")
    }
    /// Return the account page where the person can manage API credentials.
    fn keys_page(&self) -> Option<&'static str> {
        Some("https://elevenlabs.io/app/settings/api-keys")
    }
    fn unit(&self) -> Unit {
        Unit::Credits
    }
    fn list_models(&self, key: Option<&str>, kind: AssetKind) -> Result<ModelList, ToolError> {
        self.list(key, kind)
    }
    fn describe(&self, key: &str, model: &str) -> Result<ModelDetail, ToolError> {
        ElevenLabs::describe(self, key, model)
    }
    /// The model's credits for this request: characters times its rate for speech, seconds times
    /// the rate the app keeps for sound effects and music. A request ElevenLabs can't take is refused.
    fn estimate(&self, key: &str, model: &ModelDetail, req: &AssetRequest) -> Result<Option<f64>, ToolError> {
        self.validate_key(key)?;
        request(model, req)?;
        Ok(estimate_from(&model.pricing, req))
    }
    /// Send the request and return the MP3 it answers with. What it cost is ElevenLabs' own count of
    /// credits for speech, where it gives one, else the model's rate for the request.
    fn generate(&self, key: &str, model: &ModelDetail, req: &AssetRequest) -> Result<Generated, ToolError> {
        let key = http::key(key)?;
        let Sent { path, body, ignored } = request(model, req)?;
        let response = http::agent(GENERATE_TIMEOUT)
            .post(format!("{}/{path}", self.base))
            .header("xi-api-key", key)
            .header("Content-Type", "application/json")
            .send(Value::Object(body).to_string())
            .map_err(|_| http::network())?;
        http::status(response.status().as_u16())?;
        let counted = if req.kind() == AssetKind::Speech { character_cost(response.headers()) } else { None };
        let cost = counted.or_else(|| estimate_from(&model.pricing, req));
        // The request was accepted, so a failed body read still reports the charge.
        let bytes = http::bytes(response.into_body(), http::MAX_BYTES).map_err(|e| charged_or_unknown(e, cost))?;
        Ok(Generated { url: String::new(), bytes, cost, ignored })
    }
}

// ---------------------------------------------------------------------------------------------
// Listing: the models ElevenLabs makes audio with, and what they cost in credits.
// ---------------------------------------------------------------------------------------------

/// The most models of the account's list that are read.
const MAX_MODELS: usize = 200;
/// How long the one request for the list may take.
const LIST_TIMEOUT: Duration = Duration::from_secs(60);

/// Said beside a speech list made without a key.
const NO_KEY: &str = "Add an ElevenLabs key in Settings \u{2192} AI \u{2192} Game assets to list the speech models your account can use and what each \
                      costs; sound effects and music need no list.";

impl ElevenLabs {
    /// The models ElevenLabs makes `kind` with, and what each costs in credits.
    ///
    /// Speech models are the account's: `GET /models` with the key (which needs the `models_read`
    /// permission), the models that make speech and need no alpha access, each priced by its
    /// `character_cost_multiplier` in credits a character. Without a key the list is empty and
    /// `error` says to add one. Sound effects and music have one model each, fixed in
    /// `known_models.toml` with the credit rate the app keeps for it, and are listed with or without a
    /// key and without a request. ElevenLabs makes no pictures or 3D models: the list is empty.
    ///
    /// A request that fails is an error (nothing was read); only the first [`MAX_MODELS`] models of a
    /// longer list are read, and `error` says so.
    pub fn list(&self, key: Option<&str>, kind: AssetKind) -> Result<ModelList, ToolError> {
        match kind {
            AssetKind::SoundEffect | AssetKind::Music => Ok(ModelList { models: fixed_models(kind), error: None }),
            AssetKind::Image | AssetKind::Model3d => Ok(ModelList { models: Vec::new(), error: None }),
            AssetKind::Speech => {
                let Some(key) = key else { return Ok(ModelList { models: Vec::new(), error: Some(NO_KEY.to_string()) }) };
                // The fixed speech fields can be run (a test says so), so this is always a list of models.
                let Some(ignores) = ignored(AssetKind::Speech) else { return Ok(ModelList { models: Vec::new(), error: None }) };
                let mut models = self.account(http::key(key)?)?;
                let cut = models.len() > MAX_MODELS;
                models.truncate(MAX_MODELS);
                let mut listed: HashSet<&str> = HashSet::new();
                let models = models
                    .iter()
                    .filter_map(|entry| {
                        let id = speech_id(entry)?;
                        listed.insert(id).then(|| speech_model(entry, id, ignores.clone()))
                    })
                    .collect();
                let error = cut.then(|| format!("The list is incomplete: only the first {MAX_MODELS} models were read."));
                Ok(ModelList { models, error })
            }
        }
    }

    /// One model with the inputs a request for it carries and its price. A sound effect or music
    /// model is one of the fixed two and needs no request; a speech model is looked up in the
    /// account's list, and one it no longer holds (or that makes no speech, or needs alpha access) is a
    /// [`ErrorCode::BadSettings`] error that names it. What the list said of a speech model is used
    /// again for ten minutes ([`super::described`]).
    pub fn describe(&self, key: &str, model: &str) -> Result<ModelDetail, ToolError> {
        let key = http::key(key)?;
        if http::id(model).is_err() {
            return Err(http::bad("That is not an id ElevenLabs gives its models.").with_remediation(REMEDIATION));
        }
        if let Some(fixed) = fixed_model(model) {
            return Ok(ModelDetail {
                id: fixed.id.clone(),
                name: fixed.name.clone(),
                kinds: fixed.kinds.clone(),
                fields: fixed.kinds.first().map_or_else(Vec::new, |&kind| fixed_fields(kind)),
                pricing: per_second(fixed),
                unit_price: None,
            });
        }
        super::described(&format!("{ID} {}", self.base), key, model, || self.speech_detail(key, model))
    }

    /// A speech model as the account's list has it, asked of ElevenLabs.
    fn speech_detail(&self, key: &str, model: &str) -> Result<ModelDetail, ToolError> {
        let models = self.account(key)?;
        let entry = models.iter().find(|entry| speech_id(entry) == Some(model)).ok_or_else(|| http::gone(NAME, model))?;
        Ok(ModelDetail {
            id: model.to_string(),
            name: name_of(entry, model),
            kinds: vec![AssetKind::Speech],
            fields: fixed_fields(AssetKind::Speech),
            pricing: speech_pricing(entry),
            unit_price: None,
        })
    }

    /// The models of the account's list as ElevenLabs answered, as many as the size limit of an answer
    /// lets through. The key goes only to ElevenLabs' API.
    fn account(&self, key: &str) -> Result<Vec<Value>, ToolError> {
        let url = format!("{}/models", self.base);
        let response = http::get(|| http::agent(LIST_TIMEOUT).get(&url).header("xi-api-key", key).call()).map_err(|_| http::cannot_reach(NAME))?;
        let status = response.status().as_u16();
        // A key without `models_read` is answered 404 (`workspace_not_found`), a key that is wrong 401.
        if matches!(status, 401 | 403 | 404) {
            return Err(refused());
        }
        http::status(status)?;
        match http::json(response.into_body())? {
            Value::Array(models) => Ok(models),
            _ => Err(http::garbled("ElevenLabs' model list could not be read.")),
        }
    }
}

/// ElevenLabs would not list its models for this key.
fn refused() -> ToolError {
    ToolError::new(ErrorCode::BadSettings, "ElevenLabs did not let this key read its list of models.").with_remediation(
        "Check the key in Settings \u{2192} AI \u{2192} Game assets. A restricted key needs the models_read permission (turn it on for the key at \
         elevenlabs.io/app/settings/api-keys); without it ElevenLabs answers that the workspace was not found.",
    )
}

/// The id of a model of the account's list that this app can make speech with: one that does text
/// to speech, needs no alpha access, and has an id that is safe in a request. A model that requires
/// alpha access is left out because a request for it fails for an ordinary account.
fn speech_id(entry: &Value) -> Option<&str> {
    let is = |field: &str| entry.get(field).and_then(Value::as_bool) == Some(true);
    if !is("can_do_text_to_speech") || is("requires_alpha_access") {
        return None;
    }
    entry.get("model_id")?.as_str().filter(|id| http::id(id).is_ok())
}

fn speech_model(entry: &Value, id: &str, ignores: Vec<AssetOption>) -> ListedModel {
    ListedModel { provider: ID.into(), id: id.to_string(), name: name_of(entry, id), kinds: vec![AssetKind::Speech], pricing: speech_pricing(entry), ignores }
}

/// The name to show: ElevenLabs', else the known-models table's, else the id.
fn name_of(entry: &Value, id: &str) -> String {
    http::shown_name(entry.get("name").and_then(Value::as_str), KnownModels::built_in().name(ID, id), id)
}

/// What a clip costs: its characters times the model's `character_cost_multiplier`, in credits.
fn speech_pricing(entry: &Value) -> Pricing {
    credits(entry.pointer("/model_rates/character_cost_multiplier").and_then(Value::as_f64), Rate::PerCharacter)
}

/// What a fixed model costs: the credits a second that `known_models.toml` keeps for it.
fn per_second(model: &KnownModel) -> Pricing {
    credits(model.credits_per_second, Rate::PerSecond)
}

/// A rate in credits. A number that is not finite, is negative or is above the most a limit can be is
/// no rate (it would skew every limit), and the price is [`Pricing::Unknown`].
fn credits(rate: Option<f64>, per: fn(f64) -> Rate) -> Pricing {
    match rate.filter(|rate| rate.is_finite() && (0.0..=MAX_ASSET_CREDITS).contains(rate)) {
        Some(rate) => Pricing::Before { unit: Unit::Credits, rate: per(rate) },
        None => Pricing::Unknown,
    }
}

// Both fixed models are deprecated; their replacements and removal dates are in `known_models.toml`.

/// The fixed models of an audio type: the one endpoint ElevenLabs has for it, no list names it.
fn fixed_models(kind: AssetKind) -> Vec<ListedModel> {
    let Some(ignores) = ignored(kind) else { return Vec::new() };
    KnownModels::built_in()
        .fixed(ID, kind)
        .into_iter()
        .map(|model| ListedModel {
            provider: ID.into(),
            id: model.id.clone(),
            name: model.name.clone(),
            kinds: model.kinds.clone(),
            pricing: per_second(model),
            ignores: ignores.clone(),
        })
        .collect()
}

/// The options of a request of this kind that the fixed fields leave unused, as for any other
/// provider's model; `None` if a request could not be run with them (a model is not offered then).
fn ignored(kind: AssetKind) -> Option<Vec<AssetOption>> {
    runnable(&fixed_fields(kind), kind, InputRoles::built_in()).ok()
}

/// The fixed model of this id, if it is one.
fn fixed_model(id: &str) -> Option<&'static KnownModel> {
    [AssetKind::SoundEffect, AssetKind::Music].into_iter().find_map(|kind| KnownModels::built_in().fixed(ID, kind).into_iter().find(|model| model.id == id))
}

/// The inputs of a request to ElevenLabs' endpoint for `kind`, which are the same for every model
/// of it: what this app sends. A length the request leaves out is one ElevenLabs chooses, and a
/// sound whose length is chosen costs 200 credits instead of the rate a second, so `duration_seconds`
/// is a field the request always fills. The length of a track is in milliseconds (the `duration_ms`
/// role). The model is not a field a request fills: ElevenLabs' own request adds the model chosen.
fn fixed_fields(kind: AssetKind) -> Vec<InputField> {
    let field = |name: &str, kind: FieldKind, required: bool, has_default: bool| InputField { name: name.into(), kind, required, has_default };
    // The request always names the model; ElevenLabs has one of its own for a request that does not.
    let model_id = field("model_id", FieldKind::Text, false, true);
    match kind {
        AssetKind::Speech => vec![field("text", FieldKind::Text, true, false), field("voice_id", FieldKind::Text, true, false), model_id],
        AssetKind::SoundEffect => vec![
            field("text", FieldKind::Text, true, false),
            field("duration_seconds", FieldKind::Number { min: Some(SOUND_SECONDS.0), max: Some(SOUND_SECONDS.1) }, false, true),
            model_id,
        ],
        AssetKind::Music => vec![
            field("prompt", FieldKind::Text, true, false),
            field(
                "music_length_ms",
                FieldKind::Integer { min: Some((MUSIC_SECONDS.0 * 1000.0) as i64), max: Some((MUSIC_SECONDS.1 * 1000.0) as i64) },
                false,
                true,
            ),
            field("force_instrumental", FieldKind::Bool, false, true),
            model_id,
        ],
        AssetKind::Image | AssetKind::Model3d => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{AssetKind, charged_amount, was_charged};
    use craft_anything_evidence::ErrorCode;

    /// A speech model of the account's list, at `multiplier` credits a character.
    fn speech_model(pricing: Pricing) -> ModelDetail {
        ModelDetail {
            id: "eleven_multilingual_v2".into(),
            name: "Eleven Multilingual v2".into(),
            kinds: vec![AssetKind::Speech],
            fields: fixed_fields(AssetKind::Speech),
            pricing,
            unit_price: None,
        }
    }

    fn per_character(credits: f64) -> Pricing {
        Pricing::Before { unit: Unit::Credits, rate: Rate::PerCharacter(credits) }
    }

    /// A fixed model, as described without a request.
    fn fixed(id: &str) -> ModelDetail {
        ElevenLabs { base: "http://127.0.0.1:9".into() }.describe("sk-test", id).unwrap()
    }

    fn sound(seconds: f64) -> AssetRequest<'static> {
        AssetRequest::SoundEffect { prompt: "wind", duration_seconds: seconds }
    }

    /// Check each audio endpoint's payload, authentication, binary result, and what it cost in credits.
    #[test]
    fn speech_sound_and_music_use_distinct_wire_schemas_and_binary_results() {
        let (base, seen) = http::tests::serve_with_headers(|_| {
            vec![
                (200, "ID3 speech".into(), vec![(CHARACTER_COST.to_string(), "7".to_string())]),
                (200, "ID3 sound".into(), vec![]),
                (200, "ID3 music".into(), vec![(CHARACTER_COST.to_string(), "1".to_string())]),
            ]
        });
        let provider = ElevenLabs { base };
        let speech = AssetRequest::Speech { text: "héllo", voice_id: "voice_test" };
        let music = AssetRequest::Music { prompt: "gentle music", duration_seconds: 10.0, instrumental: true };
        let (talk, wind, tune) = (speech_model(per_character(1.0)), fixed("eleven_text_to_sound_v2"), fixed("music_v1"));
        assert_eq!(provider.estimate("sk-test", &talk, &speech).unwrap(), Some(5.0), "five characters at a credit each");
        assert_eq!(provider.estimate("sk-test", &wind, &sound(2.5)).unwrap(), Some(100.0), "40 credits a second");
        assert_eq!(provider.estimate("sk-test", &tune, &music).unwrap(), Some(150.0), "15 credits a second");
        let made = provider.generate("sk-test", &talk, &speech).unwrap();
        assert_eq!((made.bytes.as_slice(), made.cost), (&b"ID3 speech"[..], Some(7.0)), "ElevenLabs' own count of credits");
        let made = provider.generate("sk-test", &wind, &sound(2.5)).unwrap();
        assert_eq!((made.bytes.as_slice(), made.cost), (&b"ID3 sound"[..], Some(100.0)));
        let made = provider.generate("sk-test", &tune, &music).unwrap();
        assert_eq!((made.bytes.as_slice(), made.cost), (&b"ID3 music"[..], Some(150.0)), "a count of characters says nothing of a track");
        assert!(made.ignored.is_empty());
        let seen = seen.lock().unwrap();
        let body = |i: usize| -> Value { serde_json::from_str(seen[i].split("\r\n\r\n").nth(1).unwrap()).unwrap() };
        assert!(seen[0].starts_with("POST /text-to-speech/voice_test?output_format=mp3_44100_128 "));
        assert_eq!(body(0), json!({"text": "héllo", "model_id": "eleven_multilingual_v2"}), "the voice is in the address");
        assert!(seen[1].starts_with("POST /sound-generation?output_format=mp3_44100_128 "));
        assert_eq!(body(1), json!({"text": "wind", "duration_seconds": 2.5, "model_id": "eleven_text_to_sound_v2"}));
        assert!(seen[2].starts_with("POST /music?output_format=mp3_44100_128 "));
        assert_eq!(body(2), json!({"prompt": "gentle music", "music_length_ms": 10000, "force_instrumental": true, "model_id": "music_v1"}));
        assert!(seen.iter().all(|s| s.to_ascii_lowercase().contains("xi-api-key: sk-test")));
    }

    /// Without ElevenLabs' count, speech costs its characters at the model's rate; with no rate
    /// either, nobody knows.
    #[test]
    fn speech_without_a_count_costs_its_characters() {
        let (base, _) = http::tests::serve_with_headers(|_| {
            vec![(200, "ID3".into(), vec![]), (200, "ID3".into(), vec![(CHARACTER_COST.to_string(), "lots".to_string())]), (200, "ID3".into(), vec![])]
        });
        let provider = ElevenLabs { base };
        let speech = AssetRequest::Speech { text: "hello", voice_id: "voice_test" };
        assert_eq!(provider.generate("sk-test", &speech_model(per_character(0.5)), &speech).unwrap().cost, Some(2.5));
        assert_eq!(provider.generate("sk-test", &speech_model(per_character(0.5)), &speech).unwrap().cost, Some(2.5), "a count that is no number");
        assert_eq!(provider.generate("sk-test", &speech_model(Pricing::Unknown), &speech).unwrap().cost, None);
    }

    /// Reject invalid audio inputs locally and classify HTTP errors without echoing remote secrets.
    #[test]
    fn invalid_inputs_and_remote_errors_never_expose_credentials() {
        let wind = fixed("eleven_text_to_sound_v2");
        for duration in [f64::NAN, f64::INFINITY, -1.0, 0.0, 31.0] {
            assert!(ElevenLabs::default().estimate("sk-test", &wind, &sound(duration)).is_err());
        }
        let speech = AssetRequest::Speech { text: "hi", voice_id: "../bad" };
        assert!(ElevenLabs::default().generate("sk-test", &speech_model(per_character(1.0)), &speech).is_err());
        let (base, _) = http::tests::serve(|_| {
            vec![(401, "sk-test echoed by remote".into()), (429, "sk-test echoed by remote".into()), (500, "sk-test echoed by remote".into())]
        });
        let provider = ElevenLabs { base };
        for code in [ErrorCode::BadSettings, ErrorCode::OverLimit, ErrorCode::ProgramFailed] {
            let error = provider.generate("sk-test", &wind, &sound(1.0)).unwrap_err();
            assert_eq!(error.code, code);
            assert!(!format!("{error:?}").contains("sk-test"));
            assert!(!was_charged(&error), "a refused request is not charged");
        }
    }

    /// A model of another type, or a request for a picture, is refused before anything is sent.
    #[test]
    fn a_model_of_another_type_is_refused_before_anything_is_sent() {
        let provider = ElevenLabs { base: "http://127.0.0.1:9".into() };
        let music = AssetRequest::Music { prompt: "calm", duration_seconds: 10.0, instrumental: false };
        let picture = AssetRequest::Image { prompt: "a crate", shape: crate::assets::Shape::Square };
        for (model, req) in [(fixed("eleven_text_to_sound_v2"), music), (fixed("music_v1"), sound(2.0)), (fixed("music_v1"), picture)] {
            let error = provider.generate("sk-test", &model, &req).unwrap_err();
            assert_eq!(error.code, ErrorCode::BadSettings, "{} for {:?}", model.id, req.kind());
            assert!(!was_charged(&error));
            assert!(provider.estimate("sk-test", &model, &req).is_err());
        }
    }

    /// A body that fails after an accepted request still reports the request's charge.
    #[test]
    fn a_failed_body_after_an_accepted_request_reports_its_charge() {
        let (base, _) = http::tests::serve(|_| vec![(200, String::new()), (200, String::new())]);
        let provider = ElevenLabs { base };
        let error = provider.generate("sk-test", &fixed("eleven_text_to_sound_v2"), &sound(2.0)).unwrap_err();
        assert_eq!(error.code, ErrorCode::GarbledOutput);
        assert_eq!(charged_amount(&error), Some(80.0));
        // A clip whose cost nobody can say is still a charge, of unknown amount.
        let speech = AssetRequest::Speech { text: "hello", voice_id: "voice_test" };
        let error = provider.generate("sk-test", &speech_model(Pricing::Unknown), &speech).unwrap_err();
        assert!(was_charged(&error) && charged_amount(&error).is_none());
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;
    use crate::assets::inputs::{FieldKind, map_request, runnable};
    use crate::assets::model::{ListedModel, MAX_ASSET_CREDITS, ModelDetail, ModelList, Pricing, Rate, Unit};
    use crate::assets::tables::{InputRoles, KnownModels};
    use crate::assets::{AssetKind, AssetRequest};
    use craft_anything_evidence::ErrorCode;
    use craft_anything_platform::sync::Mutex;
    use serde_json::{Value, json};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    // The API these tests stand in for, read on 2026-11-03 (https://elevenlabs.io/docs/api-reference/models/list):
    // `GET /v1/models` answers a JSON array of models. A model has `model_id`, `name`,
    // `can_do_text_to_speech`, `requires_alpha_access`, `maximum_text_length_per_request` and
    // `model_rates.character_cost_multiplier` (credits a character). The key needs the `models_read`
    // permission; a key without it was reported to get 404 `workspace_not_found`, not 401. The fixtures
    // are invented to that shape: no request was made with a key.

    const KEY: &str = "sk-test";

    fn model(id: &str, name: &str, multiplier: Value) -> Value {
        json!({
            "model_id": id,
            "name": name,
            "can_do_text_to_speech": true,
            "requires_alpha_access": false,
            "maximum_text_length_per_request": 10000,
            "model_rates": {"character_cost_multiplier": multiplier}
        })
    }

    /// ElevenLabs answering the one request for the list with `models`.
    fn account(models: Value) -> (ElevenLabs, Arc<Mutex<Vec<String>>>) {
        answering(vec![(200, models.to_string())])
    }

    fn answering(answers: Vec<(u16, String)>) -> (ElevenLabs, Arc<Mutex<Vec<String>>>) {
        let (base, seen) = http::tests::serve(move |_| answers);
        (ElevenLabs { base }, seen)
    }

    /// An ElevenLabs whose server counts the connections made to it and closes each at once: a request
    /// that should not have been made is counted, and fails without waiting.
    struct Silent {
        provider: ElevenLabs,
        connections: Arc<AtomicUsize>,
        done: Arc<AtomicBool>,
    }

    impl Silent {
        fn new() -> Silent {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let (connections, done) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicBool::new(false)));
            let (counted, stop) = (connections.clone(), done.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        // The stream is dropped, which closes it.
                        Ok(_) => {
                            counted.fetch_add(1, Ordering::SeqCst);
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(2)),
                        Err(_) => break,
                    }
                }
            });
            Silent { provider: ElevenLabs { base }, connections, done }
        }

        fn connections(&self) -> usize {
            self.connections.load(Ordering::SeqCst)
        }
    }

    impl Drop for Silent {
        fn drop(&mut self) {
            self.done.store(true, Ordering::SeqCst);
        }
    }

    /// An address that nothing listens on: a port that was free a moment ago.
    fn nothing_listens() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    }

    fn ids(list: &ModelList) -> Vec<&str> {
        list.models.iter().map(|m| m.id.as_str()).collect()
    }

    fn credits(rate: Rate) -> Pricing {
        Pricing::Before { unit: Unit::Credits, rate }
    }

    #[test]
    fn speech_models_come_from_the_account() {
        let (provider, seen) = account(json!([
            model("eleven_multilingual_v2", "Eleven Multilingual v2", json!(1.0)),
            model("eleven_flash_v2_5", "Eleven Flash v2.5", json!(0.5)),
            // Not speech: a model that only converts one voice into another.
            json!({"model_id": "eleven_english_sts_v2", "name": "Eleven English STS v2", "can_do_text_to_speech": false,
                   "model_rates": {"character_cost_multiplier": 1.0}}),
            // Speech, but only for accounts with alpha access.
            json!({"model_id": "eleven_alpha", "name": "Alpha", "can_do_text_to_speech": true, "requires_alpha_access": true,
                   "model_rates": {"character_cost_multiplier": 1.0}}),
        ]));
        let list = provider.list(Some(KEY), AssetKind::Speech).unwrap();
        assert_eq!(list.error, None);
        assert_eq!(ids(&list), ["eleven_multilingual_v2", "eleven_flash_v2_5"]);
        assert_eq!(
            list.models[1],
            ListedModel {
                provider: "elevenlabs".into(),
                id: "eleven_flash_v2_5".into(),
                name: "Eleven Flash v2.5".into(),
                kinds: vec![AssetKind::Speech],
                pricing: credits(Rate::PerCharacter(0.5)),
                ignores: Vec::new(),
            }
        );
        assert_eq!(list.models[0].pricing, credits(Rate::PerCharacter(1.0)));
        assert_eq!(list.models[0].pricing.shown(AssetKind::Speech), "1 credit a character");
        assert_eq!(list.models[1].pricing.shown(AssetKind::Speech), "0.5 credits a character");
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "one request: the account's list");
        assert!(seen[0].starts_with("GET /models "), "{}", seen[0]);
        assert!(seen[0].to_ascii_lowercase().contains("xi-api-key: sk-test"));
    }

    /// What the account's list says is untrusted: odd entries, ids, names and rates are tidied or
    /// left out, and a rate nobody could believe is no price.
    #[test]
    fn what_the_account_says_is_read_with_care() {
        let hostile = format!("Evil\nmodel{}", "x".repeat(300));
        let (provider, _) = account(json!([
            model("eleven_multilingual_v2", "", json!(1.0)),
            model("a_negative_rate", "Negative", json!(-1.0)),
            model("a_huge_rate", "Huge", json!(1e300)),
            model("just_over", "Over", json!(MAX_ASSET_CREDITS + 1.0)),
            model("the_most", "Most", json!(MAX_ASSET_CREDITS)),
            model("a_free_one", "Free", json!(0.0)),
            model("a_text_rate", "Text", json!("1")),
            model("a_null_rate", "Null", Value::Null),
            model("hostile_name", &hostile, json!(2.0)),
            model("../escapes", "Escapes", json!(1.0)),
            model("has a space", "Space", json!(1.0)),
            model("", "No id", json!(1.0)),
            model("eleven_multilingual_v2", "Again", json!(9.0)),
            json!({"model_id": "no_rates", "name": "No rates", "can_do_text_to_speech": true}),
            json!({"model_id": "text_flag", "name": "Text flag", "can_do_text_to_speech": "true", "model_rates": {"character_cost_multiplier": 1.0}}),
            json!({"name": "No model id", "can_do_text_to_speech": true}),
            json!({"model_id": 7, "can_do_text_to_speech": true}),
            json!("not a model"),
            Value::Null,
        ]));
        let list = provider.list(Some(KEY), AssetKind::Speech).unwrap();
        assert_eq!(
            ids(&list),
            [
                "eleven_multilingual_v2",
                "a_negative_rate",
                "a_huge_rate",
                "just_over",
                "the_most",
                "a_free_one",
                "a_text_rate",
                "a_null_rate",
                "hostile_name",
                "no_rates"
            ]
        );
        let by_id = |id: &str| list.models.iter().find(|m| m.id == id).unwrap();
        // A blank name is the table's, and the first of two models with one id is the one kept.
        assert_eq!(by_id("eleven_multilingual_v2").name, "ElevenLabs Multilingual v2");
        assert_eq!(by_id("eleven_multilingual_v2").pricing, credits(Rate::PerCharacter(1.0)));
        for unbelievable in ["a_negative_rate", "a_huge_rate", "just_over", "a_text_rate", "a_null_rate", "no_rates"] {
            assert_eq!(by_id(unbelievable).pricing, Pricing::Unknown, "{unbelievable}");
        }
        assert_eq!(by_id("the_most").pricing, credits(Rate::PerCharacter(MAX_ASSET_CREDITS)), "the most is still a price");
        assert_eq!(by_id("a_free_one").pricing, credits(Rate::PerCharacter(0.0)), "free is a price");
        let name = &by_id("hostile_name").name;
        assert!(name.starts_with("Evilmodelxxx") && !name.chars().any(char::is_control) && name.chars().count() <= 81, "{name:?}");
    }

    #[test]
    fn sound_and_music_come_from_the_table() {
        let silent = Silent::new();
        let provider = &silent.provider;
        let known = KnownModels::built_in();
        for (kind, id) in [(AssetKind::SoundEffect, "eleven_text_to_sound_v2"), (AssetKind::Music, "music_v1")] {
            // With a key and without one: nothing is asked of ElevenLabs.
            for key in [Some(KEY), None] {
                let list = provider.list(key, kind).unwrap();
                assert_eq!(list.error, None, "{kind:?}");
                assert_eq!(ids(&list), [id]);
                let listed = &list.models[0];
                let rate = known.credits_per_second("elevenlabs", id).unwrap();
                assert_eq!(listed.pricing, credits(Rate::PerSecond(rate)));
                assert_eq!((listed.provider.as_str(), listed.kinds.as_slice()), ("elevenlabs", &[kind][..]));
                // What the model ignores is what its fields leave of the request: nothing.
                let fields = fixed_fields(kind);
                assert_eq!(runnable(&fields, kind, InputRoles::built_in()), Ok(Vec::new()), "{kind:?}");
                assert!(listed.ignores.is_empty(), "{kind:?}: {:?}", listed.ignores);
                assert_eq!(Some(listed.name.as_str()), known.name("elevenlabs", id));
            }
        }
        let sound = provider.list(None, AssetKind::SoundEffect).unwrap();
        assert_eq!(sound.models[0].pricing.shown(AssetKind::SoundEffect), "40 credits a second");
        let music = provider.list(None, AssetKind::Music).unwrap();
        assert_eq!(music.models[0].pricing.shown(AssetKind::Music), "15 credits a second");
        // ElevenLabs makes no pictures or 3D models.
        for kind in [AssetKind::Image, AssetKind::Model3d] {
            let list = provider.list(Some(KEY), kind).unwrap();
            assert_eq!((list.models.len(), list.error), (0, None), "{kind:?}");
        }
        assert_eq!(silent.connections(), 0, "no request was made");
    }

    #[test]
    fn no_key_lists_only_the_fixed_models() {
        let silent = Silent::new();
        let provider = &silent.provider;
        let speech = provider.list(None, AssetKind::Speech).unwrap();
        assert!(speech.models.is_empty());
        let said = speech.error.unwrap();
        assert!(said.starts_with("Add an ElevenLabs key"), "{said}");
        for kind in [AssetKind::SoundEffect, AssetKind::Music] {
            let list = provider.list(None, kind).unwrap();
            assert_eq!(list.models.len(), 1, "{kind:?} is still listed");
        }
        assert_eq!(silent.connections(), 0, "no request was made");
        // A key that cannot be sent is refused before anything is asked.
        for key in ["", "  ", "k\u{e9}y", "k\ney"] {
            assert_eq!(provider.list(Some(key), AssetKind::Speech).unwrap_err().code, ErrorCode::BadSettings, "{key:?}");
        }
        assert_eq!(silent.connections(), 0);
    }

    #[test]
    fn a_restricted_key_says_what_permission_is_missing() {
        for status in [404, 401, 403] {
            // The answer's own words never reach the person.
            let body = json!({"detail": {"status": "workspace_not_found", "message": "SECRET-BODY sk-test"}}).to_string();
            let (provider, seen) = answering(vec![(status, body.clone()), (status, body)]);
            let listed = provider.list(Some(KEY), AssetKind::Speech).unwrap_err();
            let described = provider.describe(KEY, "eleven_multilingual_v2").unwrap_err();
            for error in [listed, described] {
                assert_eq!(error.code, ErrorCode::BadSettings, "{status}");
                let said = http::words(&error);
                assert!(said.contains("models_read"), "{status}: {said}");
                assert!(!said.contains("SECRET-BODY") && !said.contains(KEY) && !said.contains("workspace_not_found"), "{status}: {said}");
            }
            assert_eq!(seen.lock().unwrap().len(), 2);
        }
        // Sound effects and music need no list, so such a key lists them as well as any.
        let (provider, _) = answering(vec![(404, "{}".into())]);
        assert_eq!(provider.list(Some(KEY), AssetKind::Music).unwrap().models.len(), 1);
    }

    #[test]
    fn a_list_that_fails_says_why_without_repeating_the_answer() {
        let secret = "SECRET-BODY sk-test".to_string();
        let huge = "x".repeat(usize::try_from(http::MAX_JSON).unwrap() + 1);
        let cases = [
            (429, secret.clone(), ErrorCode::OverLimit),
            (500, secret.clone(), ErrorCode::ProgramFailed),
            (200, "not json".to_string(), ErrorCode::GarbledOutput),
            (200, json!({"models": []}).to_string(), ErrorCode::GarbledOutput),
            (200, huge, ErrorCode::OverLimit),
        ];
        for (status, body, code) in cases {
            // An answer is served once, so each request has a server of its own.
            let (provider, _) = answering(vec![(status, body)]);
            let error = provider.list(Some(KEY), AssetKind::Speech).unwrap_err();
            assert_eq!(error.code, code, "{status}: {error:?}");
            let said = format!("{error:?}");
            assert!(!said.contains("SECRET-BODY") && !said.contains(KEY) && !said.contains("xxxx"), "{status}: {said}");
        }
        // A model is looked up in the same list, so it fails the same way.
        let (provider, _) = answering(vec![(500, secret)]);
        let error = provider.describe(KEY, "eleven_multilingual_v2").unwrap_err();
        assert_eq!(error.code, ErrorCode::ProgramFailed);
        assert!(!format!("{error:?}").contains("SECRET-BODY"));
        // Nothing listens: the connection fails.
        let nobody = ElevenLabs { base: nothing_listens() };
        let error = nobody.list(Some(KEY), AssetKind::Speech).unwrap_err();
        assert_eq!(error.code, ErrorCode::EngineMissing);
        assert!(error.message.contains("ElevenLabs") && error.remediation.is_some());
    }

    #[test]
    fn at_most_two_hundred_models_are_read() {
        let many: Vec<Value> = (0..250).map(|i| model(&format!("model_{i}"), "Speech", json!(1.0))).collect();
        let (provider, _) = account(Value::Array(many));
        let list = provider.list(Some(KEY), AssetKind::Speech).unwrap();
        assert_eq!(list.models.len(), 200);
        assert_eq!(ids(&list).last(), Some(&"model_199"));
        let said = list.error.unwrap();
        assert!(said.contains("incomplete") && said.contains("200"), "{said}");
        // A model past the 200th is still a model: describe does not stop reading where the list does.
        let many: Vec<Value> = (0..250).map(|i| model(&format!("model_{i}"), "Speech", json!(1.0))).collect();
        let (provider, _) = account(Value::Array(many));
        let detail = provider.describe(KEY, "model_249").unwrap();
        assert_eq!((detail.id.as_str(), detail.pricing), ("model_249", credits(Rate::PerCharacter(1.0))));
        // Exactly the bound is not incomplete.
        let exact: Vec<Value> = (0..200).map(|i| model(&format!("model_{i}"), "Speech", json!(1.0))).collect();
        let (provider, _) = account(Value::Array(exact));
        let list = provider.list(Some(KEY), AssetKind::Speech).unwrap();
        assert_eq!((list.models.len(), list.error), (200, None));
    }

    #[test]
    fn a_speech_model_is_described_with_its_fixed_fields_and_price() {
        let (provider, seen) = account(json!([
            model("eleven_flash_v2_5", "Eleven Flash v2.5", json!(0.5)),
            model("eleven_multilingual_v2", "Eleven Multilingual v2", json!(1.0)),
        ]));
        let detail = provider.describe(KEY, "eleven_flash_v2_5").unwrap();
        assert_eq!((detail.id.as_str(), detail.name.as_str()), ("eleven_flash_v2_5", "Eleven Flash v2.5"));
        assert_eq!(detail.kinds, [AssetKind::Speech]);
        assert_eq!(detail.pricing, credits(Rate::PerCharacter(0.5)));
        let names: Vec<&str> = detail.fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["text", "voice_id", "model_id"]);
        let (text, voice, model_id) = (&detail.fields[0], &detail.fields[1], &detail.fields[2]);
        assert!(text.required && voice.required, "a clip needs words and a voice");
        assert!(!model_id.required && model_id.has_default, "the model is named by the request, ElevenLabs has its own otherwise");
        assert!(detail.fields.iter().all(|f| f.kind == FieldKind::Text));
        assert!(seen.lock().unwrap()[0].starts_with("GET /models "));

        // The fields take a speech request, and a clip without a voice is refused.
        let roles = InputRoles::built_in();
        let mapped = map_request(&detail.fields, &AssetRequest::Speech { text: "hello", voice_id: "voice_1" }, roles).unwrap();
        assert_eq!(Value::Object(mapped.body), json!({"text": "hello", "voice_id": "voice_1"}));
        assert!(mapped.ignored.is_empty());
        assert!(map_request(&detail.fields, &AssetRequest::Speech { text: "hello", voice_id: "" }, roles).is_err());
    }

    #[test]
    fn sound_and_music_are_described_without_asking() {
        let silent = Silent::new();
        let provider = &silent.provider;
        let roles = InputRoles::built_in();

        let sound = provider.describe(KEY, "eleven_text_to_sound_v2").unwrap();
        assert_eq!((sound.id.as_str(), sound.name.as_str()), ("eleven_text_to_sound_v2", "ElevenLabs Sound Effects v2"));
        assert_eq!(sound.kinds, [AssetKind::SoundEffect]);
        assert_eq!(sound.pricing, credits(Rate::PerSecond(40.0)));
        // The rate is for a sound whose length is given, so a sound is always asked for with one.
        let wind = AssetRequest::SoundEffect { prompt: "wind", duration_seconds: 2.5 };
        let mapped = map_request(&sound.fields, &wind, roles).unwrap();
        assert_eq!(Value::Object(mapped.body), json!({"text": "wind", "duration_seconds": 2.5}));
        assert!(mapped.ignored.is_empty());
        let long = map_request(&sound.fields, &AssetRequest::SoundEffect { prompt: "wind", duration_seconds: 99.0 }, roles).unwrap();
        assert_eq!(long.body.get("duration_seconds").and_then(Value::as_f64), Some(30.0), "no longer than ElevenLabs makes");

        let music = provider.describe(KEY, "music_v1").unwrap();
        assert_eq!((music.id.as_str(), music.name.as_str()), ("music_v1", "ElevenLabs Music v1"));
        assert_eq!(music.kinds, [AssetKind::Music]);
        assert_eq!(music.pricing, credits(Rate::PerSecond(15.0)));
        // The length goes in milliseconds, as a whole number, and nothing about the request is ignored.
        let calm = AssetRequest::Music { prompt: "calm", duration_seconds: 12.3, instrumental: true };
        let mapped = map_request(&music.fields, &calm, roles).unwrap();
        assert_eq!(Value::Object(mapped.body.clone()), json!({"prompt": "calm", "music_length_ms": 12300, "force_instrumental": true}));
        assert!(mapped.body.get("music_length_ms").is_some_and(Value::is_i64), "{:?}", mapped.body);
        assert!(mapped.ignored.is_empty(), "{:?}", mapped.ignored);
        // A track is no shorter or longer than ElevenLabs makes.
        for (seconds, ms) in [(1.0, 3_000), (9_999.0, 600_000)] {
            let track = AssetRequest::Music { prompt: "calm", duration_seconds: seconds, instrumental: false };
            let mapped = map_request(&music.fields, &track, roles).unwrap();
            assert_eq!(mapped.body.get("music_length_ms"), Some(&json!(ms)), "{seconds}");
        }
        assert_eq!(silent.connections(), 0, "no request was made");
    }

    /// The model is not something a request says: ElevenLabs' own request adds the one the project chose,
    /// so no field is filled with it.
    #[test]
    fn a_mapped_request_never_names_the_model() {
        let (provider, _) = account(json!([model("eleven_flash_v2_5", "Flash", json!(0.5))]));
        let roles = InputRoles::built_in();
        let speech = provider.describe(KEY, "eleven_flash_v2_5").unwrap();
        let sound = describe_fixed("eleven_text_to_sound_v2");
        let music = describe_fixed("music_v1");
        let requests = [
            (&speech, AssetRequest::Speech { text: "hello", voice_id: "voice_1" }),
            (&sound, AssetRequest::SoundEffect { prompt: "wind", duration_seconds: 2.5 }),
            (&music, AssetRequest::Music { prompt: "calm", duration_seconds: 10.0, instrumental: false }),
        ];
        for (detail, request) in requests {
            let model_id = detail.fields.iter().find(|f| f.name == "model_id").unwrap();
            assert!(!model_id.required && model_id.has_default, "{}: the request never has to fill it", detail.id);
            let mapped = map_request(&detail.fields, &request, roles).unwrap();
            assert!(!mapped.body.contains_key("model_id"), "{}: {:?}", detail.id, mapped.body);
            assert!(mapped.ignored.is_empty(), "{}: {:?}", detail.id, mapped.ignored);
        }
    }

    /// A fixed model's detail (no request is made for it).
    fn describe_fixed(id: &str) -> ModelDetail {
        let silent = Silent::new();
        let detail = silent.provider.describe(KEY, id).unwrap();
        assert_eq!(silent.connections(), 0, "no request was made for {id}");
        detail
    }

    #[test]
    fn a_model_that_is_not_there_is_a_clear_error() {
        let page = || {
            json!([
                model("eleven_flash_v2_5", "Flash", json!(0.5)),
                json!({"model_id": "eleven_english_sts_v2", "name": "STS", "can_do_text_to_speech": false}),
                json!({"model_id": "eleven_alpha", "name": "Alpha", "can_do_text_to_speech": true, "requires_alpha_access": true}),
            ])
        };
        // Gone from the list, in it but not speech, in it but alpha only: none is a model to run.
        for id in ["eleven_gone", "eleven_english_sts_v2", "eleven_alpha"] {
            let (provider, _) = account(page());
            let error = provider.describe(KEY, id).unwrap_err();
            assert_eq!(error.code, ErrorCode::BadSettings, "{id}");
            assert!(error.message.contains(id) && error.message.contains("no longer available at ElevenLabs"), "{}", error.message);
            assert_eq!(error.remediation.as_deref(), Some(crate::assets::inputs::REMEDIATION));
        }
        let (provider, _) = account(page());
        assert_eq!(provider.describe(KEY, "eleven_flash_v2_5").unwrap().name, "Flash");
    }

    #[test]
    fn an_id_or_key_that_cannot_be_sent_is_refused_before_anything_is_asked() {
        let silent = Silent::new();
        let provider = &silent.provider;
        for id in ["", "../models", "a b", "eleven\n", "x?y=z", &"a".repeat(201)] {
            let error = provider.describe(KEY, id).unwrap_err();
            assert_eq!(error.code, ErrorCode::BadSettings, "{id:?}");
            assert!(error.remediation.is_some() && !error.message.contains('\n'), "{id:?}");
        }
        for key in ["", "  ", "k\u{e9}y", "k\ney"] {
            assert_eq!(provider.describe(key, "music_v1").unwrap_err().code, ErrorCode::BadSettings, "{key:?}");
            assert_eq!(provider.describe(key, "eleven_flash_v2_5").unwrap_err().code, ErrorCode::BadSettings, "{key:?}");
        }
        assert_eq!(silent.connections(), 0);
    }
}
