//! `assets_models_list`: the models each asset provider offers for one asset type, with what each
//! costs, from the providers whose key the person saved. A list is kept in `asset-models.json` in
//! the config folder for a few minutes, so a picker that is opened again does not ask every
//! provider again, and a provider that cannot be reached still shows the list it gave last time.

use crate::key_tools::providers;
use crate::keys::{self, KeyStore};
use craft_anything_engine::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_engine::tools::asset_tools::type_words;
use craft_anything_engines::assets::model::ListedModel;
use craft_anything_engines::assets::{AssetKind, AssetProvider};
use craft_anything_evidence::ToolError;
use craft_anything_platform::fs::write_atomic;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

/// The cache file, in the config folder.
const FILE: &str = "asset-models.json";
/// The current `asset-models.json` format version: `{"version": 1, "lists": {"<provider>/<kind>": ...}}`.
/// Bumping it means adding a step that brings the older ones forward (see docs/file-formats.md).
const CACHE_VERSION: u32 = 1;
/// How long a list fetched earlier is used again, unless the caller says otherwise.
const DEFAULT_MAX_AGE_MINUTES: u32 = 10;
/// The most of the cache file that is read; a larger one is not read at all and is left as it is.
const MAX_CACHE_BYTES: u64 = 32 * 1024 * 1024;
/// After a failed attempt to refresh, a look that is not a refresh shows the list that is kept for
/// this long (a minute) before asking the provider again.
const RETRY_AFTER_FAILURE_MS: u64 = 60_000;
/// The longest reason a provider's failure is kept and returned.
const MAX_ERROR_CHARS: usize = 600;

/// Register the model-list tool.
pub fn specs() -> Vec<ToolSpec> {
    vec![ToolSpec {
        id: "assets_models_list",
        title: "List the models for an asset type",
        summary: "List the models each asset provider offers for one asset type (`kind`: image, model_3d, speech, sound_effect or music), with the price the provider gives and the options each ignores, from the providers whose key is saved (assets_key_set); the others are left out, and so is a provider that makes nothing of the type (its list is empty and has no error). Returns `providers`: each with its name, `unit` (usd or credits), `fetched_ms`, `models` and `error` (null, else why the provider could not be asked, in which case `models` is the last list it gave). Pass a model's `provider`, `id` (as `model`), `name` (as `model_name`) and `pricing` unchanged to project_assets_choose. Lists are kept in the config folder and used again for `max_age_minutes` (default 10); a failed refresh is not tried again for a minute unless `refresh` is true, which asks every provider again.",
        beginner: "Shows which models your services offer for pictures, 3D models, voices, sounds or music, and what each costs.",
        toolset: Toolset::Setup,
        effects: Effects::ACCESSES_NETWORK.union(Effects::WRITES_FILESYSTEM),
        requires: Requires::Nothing,
        input_schema: schema_of::<ListParams>,
        run: assets_models_list,
    }]
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    /// The asset type: image, model_3d, speech, sound_effect or music.
    pub kind: AssetKind,
    /// Ask the providers again even when a list fetched lately is kept.
    #[serde(default)]
    pub refresh: bool,
    /// How old a kept list may be and still be used, in minutes (default 10; 0 asks again).
    #[serde(default)]
    pub max_age_minutes: Option<u32>,
}

/// One provider's list for one asset type, as it is kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Cached {
    /// When `models` were fetched (milliseconds since 1970). After a failed refresh this stays the
    /// time of the last good list, not of the failure.
    fetched_ms: u64,
    models: Vec<ListedModel>,
    /// Why the provider could not be asked the last time, if it could not.
    error: Option<String>,
    /// When that last failed attempt was made. Until [`RETRY_AFTER_FAILURE_MS`] has passed, a look
    /// that is not a refresh shows this list and its error instead of asking again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failed_ms: Option<u64>,
}

impl Cached {
    /// The provider listed nothing for this type and said nothing was wrong: it does not make it.
    fn makes_nothing(&self) -> bool {
        self.models.is_empty() && self.error.is_none()
    }
}

/// The lists that are kept, by `<provider>/<kind>`.
#[derive(Debug, Default)]
struct Cache {
    lists: BTreeMap<String, Cached>,
    /// The file is one this build must not write: written by a newer Craft Anything, or too large
    /// to have been read. Its lists, as far as they are known, are still used.
    keep_as_is: bool,
}

impl Cache {
    /// The kept lists. A cache is only a cache: a file that is missing, damaged, or without a version
    /// holds nothing, and a list that cannot be read is left out.
    fn read(dir: &Path) -> Cache {
        let Ok(file) = std::fs::File::open(dir.join(FILE)) else { return Cache::default() };
        let mut bytes = Vec::new();
        if file.take(MAX_CACHE_BYTES + 1).read_to_end(&mut bytes).is_err() {
            return Cache::default();
        }
        if bytes.len() as u64 > MAX_CACHE_BYTES {
            return Cache { lists: BTreeMap::new(), keep_as_is: true };
        }
        let Ok(doc) = serde_json::from_slice::<Value>(&bytes) else { return Cache::default() };
        let Some(version) = doc.get("version").and_then(Value::as_u64).filter(|&v| v >= 1) else { return Cache::default() };
        let lists = doc
            .get("lists")
            .and_then(Value::as_object)
            .map(|lists| lists.iter().filter_map(|(slot, list)| Some((slot.clone(), Cached::deserialize(list).ok()?))).collect())
            .unwrap_or_default();
        Cache { lists, keep_as_is: version > u64::from(CACHE_VERSION) }
    }

    /// Keep the lists, unless the file is a newer one.
    fn save(&self, dir: &Path) -> Result<(), ToolError> {
        if self.keep_as_is {
            return Ok(());
        }
        #[derive(Serialize)]
        struct Out<'a> {
            version: u32,
            lists: &'a BTreeMap<String, Cached>,
        }
        let bytes = serde_json::to_vec(&Out { version: CACHE_VERSION, lists: &self.lists }).map_err(|e| ToolError::internal(e.to_string()))?;
        write_atomic(&dir.join(FILE), &bytes)
    }

    /// Keep the lists when `changed`. A cache that cannot be kept does not stop the listing; it is said.
    fn keep(&self, ctx: &Ctx<'_>, dir: &Path, changed: bool) {
        if changed && let Err(e) = self.save(dir) {
            ctx.report(None, format!("Couldn't keep the model lists for next time: {}", e.for_people()));
        }
    }
}

/// Whether `list` can be shown again without asking the provider. A list with no error is while it
/// is younger than `reuse_ms`. One that failed to refresh is for a minute after the failure (so a
/// screen opened again while offline does not wait out every timeout again) or `reuse_ms` if that
/// is shorter. A refresh (`reuse_ms` none) always asks, and so does a list dated in the future (the
/// clock moved) and one that says its provider makes nothing of the type.
fn reusable(list: &Cached, now: u64, reuse_ms: Option<u64>) -> bool {
    let within = |since: u64, limit: u64| now.checked_sub(since).is_some_and(|age| age < limit);
    let Some(max) = reuse_ms else { return false };
    if list.makes_nothing() {
        return false;
    }
    match (&list.error, list.failed_ms) {
        (None, _) => within(list.fetched_ms, max),
        (Some(_), Some(failed)) => within(failed, max.min(RETRY_AFTER_FAILURE_MS)),
        (Some(_), None) => false,
    }
}

/// Ask `provider` for its models for `kind`. When that fails (an error, or a list that only has an
/// error) and an earlier list is kept, the earlier list is what is returned, with the error.
fn fetch(provider: &dyn AssetProvider, key: &str, kind: AssetKind, kept: Option<Cached>, now: u64) -> Cached {
    let (models, error) = match provider.list_models(Some(key), kind) {
        Ok(list) => (list.models, list.error),
        Err(e) => (Vec::new(), Some(e.for_people())),
    };
    let Some(error) = error.map(|e| e.chars().take(MAX_ERROR_CHARS).collect::<String>()) else {
        return Cached { fetched_ms: now, models, error: None, failed_ms: None };
    };
    match kept.filter(|k| !k.models.is_empty()) {
        Some(kept) => Cached { fetched_ms: kept.fetched_ms, models: kept.models, error: Some(error), failed_ms: Some(now) },
        None => Cached { fetched_ms: now, models, error: Some(error), failed_ms: Some(now) },
    }
}

fn assets_models_list(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ListParams = parse_params(params)?;
    let all = providers(ctx);
    // A fork or a test can put its own key store in the session; the person's is the system's.
    let store = ctx.session.ext.get::<KeyStore>().map_or_else(KeyStore::system, |k| k.as_ref().clone());
    let dir = ctx.session.config_dir();
    let mut cache = Cache::read(&dir);
    let now = craft_anything_engine::inbox::now_ms();
    let reuse_ms = (!p.refresh).then(|| u64::from(p.max_age_minutes.unwrap_or(DEFAULT_MAX_AGE_MINUTES)).saturating_mul(60_000));
    let (many, _) = type_words(p.kind);
    let mut changed = false;
    let mut listed = Vec::new();
    for provider in all.ids().into_iter().filter_map(|id| all.get(id)) {
        // What was asked of the providers before a cancel is kept.
        if ctx.cancel.is_cancelled() {
            cache.keep(ctx, &dir, changed);
            return Err(ToolError::cancelled());
        }
        // Only the providers whose key the person saved are asked.
        let Some(key) = keys::asset_key(&store, provider.as_ref()) else { continue };
        let slot = format!("{}/{}", provider.id(), p.kind.id());
        let kept = cache.lists.get(&slot).cloned();
        let list = match kept {
            Some(kept) if reusable(&kept, now, reuse_ms) => kept,
            kept => {
                ctx.report(None, format!("Asking {} which models make {many}…", provider.name()));
                let list = fetch(provider.as_ref(), &key, p.kind, kept, now);
                // A provider that lists nothing and has no complaint does not make this type: it is
                // not shown, and nothing is kept of it.
                if list.makes_nothing() {
                    changed |= cache.lists.remove(&slot).is_some();
                    continue;
                }
                cache.lists.insert(slot, list.clone());
                changed = true;
                list
            }
        };
        listed.push(json!({"provider": provider.id(), "name": provider.name(), "unit": provider.unit(), "fetched_ms": list.fetched_ms, "models": list.models, "error": list.error}));
    }
    cache.keep(ctx, &dir, changed);
    Ok(ToolOutput::Value(json!({"kind": p.kind, "providers": listed})))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{KeyStore, asset_key_name};
    use craft_anything_engine::tool::Progress;
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    use craft_anything_engine::{CallOptions, Caller, CancelToken, Engine};
    use craft_anything_engines::assets::model::{AssetOption, ListedModel, ModelDetail, ModelList, Pricing, Rate, Unit};
    use craft_anything_engines::assets::{AssetKind, AssetProvider, AssetProviders, AssetRequest, Generated};
    use craft_anything_evidence::ErrorCode;
    use craft_anything_platform::sync::Mutex;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// What a stand-in provider answers when asked to list.
    type Answer = Result<ModelList, ToolError>;

    /// A provider that lists what the test tells it to and counts the times it was asked.
    struct Standin {
        id: &'static str,
        name: &'static str,
        unit: Unit,
        answer: Arc<Mutex<Answer>>,
        asked: Arc<AtomicUsize>,
        /// Cancelled when the provider is asked, as a person pressing Stop during the call would.
        cancel: Arc<Mutex<Option<CancelToken>>>,
    }

    impl AssetProvider for Standin {
        fn id(&self) -> &'static str {
            self.id
        }
        fn name(&self) -> &'static str {
            self.name
        }
        fn unit(&self) -> Unit {
            self.unit
        }
        fn list_models(&self, key: Option<&str>, _kind: AssetKind) -> Result<ModelList, ToolError> {
            assert_eq!(key, Some("sk-test"), "a provider is only asked with the key the person saved");
            self.asked.fetch_add(1, Ordering::SeqCst);
            if let Some(token) = &*self.cancel.lock().unwrap() {
                token.cancel();
            }
            self.answer.lock().unwrap().clone()
        }
        fn describe(&self, _key: &str, _model: &str) -> Result<ModelDetail, ToolError> {
            Err(ToolError::invalid("not here"))
        }
        fn generate(&self, _key: &str, _model: &ModelDetail, _req: &AssetRequest) -> Result<Generated, ToolError> {
            Err(ToolError::invalid("not here"))
        }
    }

    fn model(provider: &str, id: &str, pricing: Pricing) -> ListedModel {
        ListedModel {
            provider: provider.into(),
            id: id.into(),
            name: format!("Model {id}"),
            kinds: vec![AssetKind::Image],
            pricing,
            ignores: vec![AssetOption::Shape],
        }
    }

    fn per_picture(x: f64) -> Pricing {
        Pricing::Before { unit: Unit::Usd, rate: Rate::PerRequest(x) }
    }

    fn listed(models: Vec<ListedModel>) -> Answer {
        Ok(ModelList { models, error: None })
    }

    /// A stand-in provider with its controls.
    struct Handle {
        answer: Arc<Mutex<Answer>>,
        asked: Arc<AtomicUsize>,
        cancel: Arc<Mutex<Option<CancelToken>>>,
    }

    impl Handle {
        fn answer(&self, a: Answer) {
            *self.answer.lock().unwrap() = a;
        }
        fn asked(&self) -> usize {
            self.asked.load(Ordering::SeqCst)
        }
        fn cancel_when_asked(&self, token: CancelToken) {
            *self.cancel.lock().unwrap() = Some(token);
        }
    }

    fn standin(id: &'static str, name: &'static str, unit: Unit, first: Answer) -> (Arc<dyn AssetProvider>, Handle) {
        let (answer, asked, cancel) = (Arc::new(Mutex::new(first)), Arc::new(AtomicUsize::new(0)), Arc::new(Mutex::new(None)));
        (Arc::new(Standin { id, name, unit, answer: answer.clone(), asked: asked.clone(), cancel: cancel.clone() }), Handle { answer, asked, cancel })
    }

    /// An engine whose registry holds only the given stand-ins, whose key store is a file in its
    /// own folder (nothing here reads the keychain or the environment), and whose config folder is
    /// where the cache goes. `keyed` names the providers that have a saved key.
    struct Fixture {
        engine: Arc<Engine>,
        dir: PathBuf,
    }

    impl Fixture {
        fn new(providers: Vec<Arc<dyn AssetProvider>>, keyed: &[&str]) -> Fixture {
            let mut session = craft_anything_engine::testing::session();
            let dir = session.config_dir();
            let keys = KeyStore::file_only(dir.join("ai-keys.toml"));
            for id in keyed {
                keys.set(&asset_key_name(id), "sk-test").unwrap();
            }
            // The built-in providers are replaced by inert ones (the environment could hold a real
            // key for them, and no test may reach a real provider).
            let mut registry = AssetProviders::built_in();
            for id in registry.ids() {
                if !providers.iter().any(|p| p.id() == id) {
                    registry.register(standin(id, "Unused", Unit::Usd, listed(Vec::new())).0);
                }
            }
            for p in providers {
                registry.register(p);
            }
            session.ext.insert(Arc::new(registry));
            session.ext.insert(Arc::new(keys));
            Fixture { engine: Engine::shared(session, specs()), dir }
        }

        fn call(&self, params: Value) -> Result<Value, ToolError> {
            self.call_with(params, CallOptions::new(Caller::Cli))
        }

        fn call_with(&self, params: Value, opts: CallOptions<'_>) -> Result<Value, ToolError> {
            self.engine.call("assets_models_list", params, opts).map(|o| o.to_json()["result"].clone())
        }

        fn cache_path(&self) -> PathBuf {
            self.dir.join("asset-models.json")
        }

        fn cache(&self) -> Value {
            serde_json::from_slice(&std::fs::read(self.cache_path()).unwrap()).unwrap()
        }
    }

    /// The provider entries of a result, by provider id.
    fn entry<'a>(result: &'a Value, provider: &str) -> &'a Value {
        result["providers"].as_array().unwrap().iter().find(|p| p["provider"] == provider).unwrap_or_else(|| panic!("{provider} is not in {result}"))
    }

    fn ids(entry: &Value) -> Vec<String> {
        entry["models"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap().to_string()).collect()
    }

    #[test]
    fn a_failed_refresh_keeps_the_cached_list_and_reports_the_error() {
        let two = vec![model("standin", "m1", per_picture(0.04)), model("standin", "m2", Pricing::After { unit: Unit::Usd })];
        let (p, handle) = standin("standin", "Stand-in", Unit::Usd, listed(two.clone()));
        let f = Fixture::new(vec![p], &["standin"]);
        let first = f.call(json!({"kind": "image"})).unwrap();
        assert_eq!((ids(entry(&first, "standin")), &entry(&first, "standin")["error"]), (vec!["m1".to_string(), "m2".to_string()], &json!(null)));
        let fetched = entry(&first, "standin")["fetched_ms"].as_u64().unwrap();
        assert_eq!(handle.asked(), 1);

        // The provider fails now, in each way a provider can: an error, and a list that only has an error.
        let failures: [Answer; 2] = [
            Err(ToolError::new(ErrorCode::EngineMissing, "Couldn't reach Stand-in to read its models.")
                .with_remediation("Check your internet connection and try again.")),
            Ok(ModelList { models: Vec::new(), error: Some("Stand-in said no.".into()) }),
        ];
        for (n, failure) in failures.into_iter().enumerate() {
            handle.answer(failure);
            let again = f.call(json!({"kind": "image", "refresh": true})).unwrap();
            let e = entry(&again, "standin");
            assert_eq!(ids(e), ["m1", "m2"], "the list from before is kept");
            assert_eq!(e["models"], serde_json::to_value(&two).unwrap(), "with its prices");
            assert_eq!(e["fetched_ms"].as_u64().unwrap(), fetched, "it says when that list was fetched, not when the refresh failed");
            let error = e["error"].as_str().unwrap_or_else(|| panic!("no error in {e}"));
            assert!(error.contains(["Couldn't reach Stand-in", "Stand-in said no."][n]), "{error}");
            assert!(n != 0 || error.contains("Check your internet connection"), "the way out comes with it: {error}");
            // What is kept on disk is that list too, so a restart does not lose it.
            let kept = &f.cache()["lists"]["standin/image"];
            assert_eq!((kept["models"].as_array().unwrap().len(), kept["fetched_ms"].as_u64().unwrap()), (2, fetched));
            assert!(kept["error"].is_string());
        }

        // The error is not held against the provider once it answers again.
        handle.answer(listed(vec![model("standin", "m3", per_picture(0.1))]));
        let healed = f.call(json!({"kind": "image", "refresh": true})).unwrap();
        assert_eq!((ids(entry(&healed, "standin")), &entry(&healed, "standin")["error"]), (vec!["m3".to_string()], &json!(null)));
        assert_eq!(f.cache()["lists"]["standin/image"]["error"], json!(null));
    }

    #[test]
    fn a_failure_with_nothing_kept_still_reports_the_error_and_whatever_the_provider_gave() {
        let (p, handle) = standin("standin", "Stand-in", Unit::Usd, Err(ToolError::new(ErrorCode::EngineMissing, "Couldn't reach Stand-in.")));
        let f = Fixture::new(vec![p], &["standin"]);
        let r = f.call(json!({"kind": "image"})).unwrap();
        let e = entry(&r, "standin");
        assert!(ids(e).is_empty() && e["error"].as_str().unwrap().contains("Couldn't reach Stand-in"), "{e}");
        // Never fresh, so a refresh (or, after a minute, any look) asks again.
        handle.answer(listed(vec![model("standin", "m1", per_picture(0.04))]));
        assert_eq!(ids(entry(&f.call(json!({"kind": "image", "refresh": true})).unwrap(), "standin")), ["m1"]);
        assert_eq!(handle.asked(), 2);

        // A list that came with an error and some models (a provider's fallback) is shown with the error.
        handle.answer(Ok(ModelList { models: vec![model("standin", "known", per_picture(0.5))], error: Some("Only the known models are shown.".into()) }));
        let other = f.call(json!({"kind": "model_3d"})).unwrap();
        assert_eq!(
            (ids(entry(&other, "standin")), entry(&other, "standin")["error"].as_str()),
            (vec!["known".to_string()], Some("Only the known models are shown."))
        );
    }

    #[test]
    fn a_fresh_cache_is_used_without_a_request() {
        let (p, handle) = standin("standin", "Stand-in", Unit::Usd, listed(vec![model("standin", "m1", per_picture(0.04))]));
        let f = Fixture::new(vec![p], &["standin"]);
        let first = f.call(json!({"kind": "image"})).unwrap();
        assert_eq!(handle.asked(), 1);
        // The file is the contract between versions and with the pickers: its exact shape.
        assert_eq!(
            f.cache(),
            json!({"version": 1, "lists": {"standin/image": {
                "fetched_ms": entry(&first, "standin")["fetched_ms"],
                "models": [{"provider": "standin", "id": "m1", "name": "Model m1", "kinds": ["image"],
                            "pricing": {"when": "before", "unit": "usd", "rate": {"per_request": 0.04}}, "ignores": ["shape"]}],
                "error": null
            }}})
        );

        handle.answer(listed(vec![model("standin", "changed", per_picture(9.0))]));
        let second = f.call(json!({"kind": "image"})).unwrap();
        assert_eq!(handle.asked(), 1, "a list fetched a moment ago is not asked for again");
        assert_eq!(second, first, "and it is returned as it was");
        // Another type is another list.
        f.call(json!({"kind": "speech"})).unwrap();
        assert_eq!(handle.asked(), 2);

        // refresh and a shorter allowed age both ask again.
        f.call(json!({"kind": "image", "refresh": true})).unwrap();
        assert_eq!(handle.asked(), 3);
        assert_eq!(ids(entry(&f.call(json!({"kind": "image", "max_age_minutes": 0})).unwrap(), "standin")), ["changed"]);
        assert_eq!(handle.asked(), 4, "no age is fresh enough for 0 minutes");
        f.call(json!({"kind": "image", "max_age_minutes": 60})).unwrap();
        assert_eq!(handle.asked(), 4);
    }

    #[test]
    fn an_old_list_is_asked_for_again_and_a_new_process_reads_the_kept_one() {
        let (p, handle) = standin("standin", "Stand-in", Unit::Usd, listed(vec![model("standin", "fresh", per_picture(0.04))]));
        let f = Fixture::new(vec![p], &["standin"]);
        let day = 24 * 60 * 60 * 1000;
        let now = craft_anything_engine::inbox::now_ms();
        let seed = |fetched_ms: u64| {
            let old = serde_json::to_value(vec![model("standin", "old", per_picture(0.01))]).unwrap();
            std::fs::write(
                f.cache_path(),
                json!({"version": 1, "lists": {"standin/image": {"fetched_ms": fetched_ms, "models": old, "error": null}}}).to_string(),
            )
            .unwrap();
        };
        // Kept a moment ago, in an earlier run: used.
        seed(now - 60_000);
        assert_eq!(ids(entry(&f.call(json!({"kind": "image"})).unwrap(), "standin")), ["old"]);
        assert_eq!(handle.asked(), 0);
        // Kept a day ago: asked for again, and what comes back is kept.
        seed(now - day);
        assert_eq!(ids(entry(&f.call(json!({"kind": "image"})).unwrap(), "standin")), ["fresh"]);
        assert_eq!(handle.asked(), 1);
        // Said to be fetched in the future (the clock moved): not trusted as fresh.
        seed(now + day);
        assert_eq!(ids(entry(&f.call(json!({"kind": "image"})).unwrap(), "standin")), ["fresh"]);
        assert_eq!(handle.asked(), 2);
    }

    #[test]
    fn providers_without_a_key_are_left_out() {
        let (a, ha) = standin("keyed", "Keyed", Unit::Usd, listed(vec![model("keyed", "k1", per_picture(0.04))]));
        let (b, hb) = standin("keyless", "Keyless", Unit::Credits, listed(vec![model("keyless", "x1", per_picture(0.04))]));
        let f = Fixture::new(vec![a, b], &["keyed"]);
        let r = f.call(json!({"kind": "image"})).unwrap();
        assert_eq!(r["kind"], "image");
        let providers = r["providers"].as_array().unwrap();
        assert_eq!(providers.len(), 1, "{r}");
        let e = entry(&r, "keyed");
        assert_eq!((e["name"].as_str(), e["unit"].as_str(), ids(e)), (Some("Keyed"), Some("usd"), vec!["k1".to_string()]));
        assert_eq!((ha.asked(), hb.asked()), (1, 0), "a provider with no key is not asked");
        assert!(f.cache()["lists"].get("keyless/image").is_none());

        // Once its key is saved it appears, with its own unit.
        let keys = KeyStore::file_only(f.dir.join("ai-keys.toml"));
        keys.set(&asset_key_name("keyless"), "sk-test").unwrap();
        let r = f.call(json!({"kind": "image"})).unwrap();
        assert_eq!((r["providers"].as_array().unwrap().len(), entry(&r, "keyless")["unit"].as_str()), (2, Some("credits")));
        // And with no key at all, there is nothing to list.
        let none = Fixture::new(Vec::new(), &[]);
        assert_eq!(none.call(json!({"kind": "music"})).unwrap()["providers"], json!([]));
    }

    #[test]
    fn a_newer_cache_is_read_and_never_written() {
        let (p, handle) = standin("standin", "Stand-in", Unit::Usd, listed(vec![model("standin", "fresh", per_picture(0.04))]));
        let f = Fixture::new(vec![p], &["standin"]);
        let kept = serde_json::to_value(vec![model("standin", "kept", per_picture(0.01))]).unwrap();
        let newer = json!({"version": 2, "future": true, "lists": {"standin/image": {"fetched_ms": craft_anything_engine::inbox::now_ms(), "models": kept, "error": null, "more": 1}}});
        let bytes = newer.to_string();
        std::fs::write(f.cache_path(), &bytes).unwrap();
        assert_eq!(ids(entry(&f.call(json!({"kind": "image"})).unwrap(), "standin")), ["kept"], "read as far as it is known");
        assert_eq!(handle.asked(), 0);
        // A list it does not hold is fetched and shown, but the file stays as the newer build wrote it.
        assert_eq!(ids(entry(&f.call(json!({"kind": "speech"})).unwrap(), "standin")), ["fresh"]);
        assert_eq!(ids(entry(&f.call(json!({"kind": "image", "refresh": true})).unwrap(), "standin")), ["fresh"]);
        assert_eq!(std::fs::read_to_string(f.cache_path()).unwrap(), bytes);
        assert_eq!(handle.asked(), 2);
    }

    #[test]
    fn a_damaged_cache_is_only_a_cache() {
        let (p, handle) = standin("standin", "Stand-in", Unit::Usd, listed(vec![model("standin", "m1", per_picture(0.04))]));
        let f = Fixture::new(vec![p], &["standin"]);
        for bad in [
            "not json",
            "[]",
            "{}",
            r#"{"version": 1, "lists": 4}"#,
            r#"{"version": "one"}"#,
            r#"{"version": 1, "lists": {"standin/image": {"fetched_ms": "soon"}}}"#,
        ] {
            std::fs::write(f.cache_path(), bad).unwrap();
            let r = f.call(json!({"kind": "image", "max_age_minutes": 0})).unwrap();
            assert_eq!(ids(entry(&r, "standin")), ["m1"], "{bad}");
            assert_eq!(f.cache()["version"], 1, "{bad}: it is replaced by a good one");
        }
        assert_eq!(handle.asked(), 6);
    }

    #[test]
    fn a_cache_that_cannot_be_written_does_not_stop_the_list_and_is_said() {
        let (p, _) = standin("standin", "Stand-in", Unit::Usd, listed(vec![model("standin", "m1", per_picture(0.04))]));
        let f = Fixture::new(vec![p], &["standin"]);
        // A folder where the file belongs.
        std::fs::create_dir_all(f.cache_path()).unwrap();
        let said: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = said.clone();
        let progress = move |p: Progress| sink.lock().unwrap().push(p.message);
        let r = f.call_with(json!({"kind": "image"}), CallOptions::new(Caller::Cli).with_progress(&progress)).unwrap();
        assert_eq!(ids(entry(&r, "standin")), ["m1"]);
        let said = said.lock().unwrap().clone();
        assert!(said.iter().any(|m| m.starts_with("Asking Stand-in which models make pictures")), "{said:?}");
        assert!(said.iter().any(|m| m.starts_with("Couldn't keep the model lists for next time:")), "{said:?}");
        // When it can be written, nothing of the kind is said.
        let (q, _) = standin("other", "Other", Unit::Usd, listed(vec![model("other", "m1", per_picture(0.04))]));
        let g = Fixture::new(vec![q], &["other"]);
        let said: Arc<Mutex<Vec<String>>> = Arc::default();
        let sink = said.clone();
        let progress = move |p: Progress| sink.lock().unwrap().push(p.message);
        g.call_with(json!({"kind": "image"}), CallOptions::new(Caller::Cli).with_progress(&progress)).unwrap();
        assert!(!said.lock().unwrap().iter().any(|m| m.contains("Couldn't keep")));
    }

    #[test]
    fn a_provider_that_makes_nothing_of_a_type_is_left_out() {
        let (maker, _) = standin("maker", "Maker", Unit::Usd, listed(vec![model("maker", "m1", per_picture(0.04))]));
        let (none, none_asked) = standin("none", "Makes none", Unit::Usd, listed(Vec::new()));
        let broken_answer = Ok(ModelList { models: Vec::new(), error: Some("Broken said no.".into()) });
        let (broken, _) = standin("broken", "Broken", Unit::Usd, broken_answer);
        let f = Fixture::new(vec![maker, none, broken], &["maker", "none", "broken"]);
        let r = f.call(json!({"kind": "image"})).unwrap();
        let shown: Vec<&str> = r["providers"].as_array().unwrap().iter().filter_map(|p| p["provider"].as_str()).collect();
        assert_eq!(shown, ["maker", "broken"], "an empty list with no error is the provider saying it does not make this type: {r}");
        assert_eq!(
            (ids(entry(&r, "broken")), entry(&r, "broken")["error"].as_str()),
            (Vec::<String>::new(), Some("Broken said no.")),
            "an empty list with an error stays, with it"
        );
        let lists = f.cache()["lists"].clone();
        assert!(lists.get("none/image").is_none() && lists.get("maker/image").is_some() && lists.get("broken/image").is_some(), "{lists}");
        // Nothing is kept of it, so it is asked each time (it is not a network call for the providers that do this).
        f.call(json!({"kind": "image"})).unwrap();
        assert_eq!(none_asked.asked(), 2);

        // An empty list kept as if it were a list (by a build that did) is not shown and is dropped.
        let now = craft_anything_engine::inbox::now_ms();
        std::fs::write(f.cache_path(), json!({"version": 1, "lists": {"none/image": {"fetched_ms": now, "models": [], "error": null}}}).to_string()).unwrap();
        let again = f.call(json!({"kind": "image"})).unwrap();
        assert_eq!(again["providers"].as_array().unwrap().len(), 2, "{again}");
        assert!(f.cache()["lists"].get("none/image").is_none());
        assert_eq!(none_asked.asked(), 3, "it was asked again rather than believed");
    }

    #[test]
    fn after_a_failed_refresh_the_next_look_does_not_ask_again_for_a_minute() {
        let kept = vec![model("standin", "m1", per_picture(0.04))];
        let (p, handle) = standin("standin", "Stand-in", Unit::Usd, listed(kept));
        let f = Fixture::new(vec![p], &["standin"]);
        f.call(json!({"kind": "image"})).unwrap();
        handle.answer(Err(ToolError::new(ErrorCode::TimedOut, "Stand-in took too long.")));
        let failed = f.call(json!({"kind": "image", "refresh": true})).unwrap();
        assert_eq!((handle.asked(), ids(entry(&failed, "standin"))), (2, vec!["m1".to_string()]));
        assert!(entry(&failed, "standin")["error"].is_string());

        // Opened again at once, still offline: the same answer, with no wait for the provider.
        assert_eq!(f.call(json!({"kind": "image"})).unwrap(), failed);
        assert_eq!(f.call(json!({"kind": "image", "max_age_minutes": 60})).unwrap(), failed);
        assert_eq!(handle.asked(), 2);
        // Even after the provider is back, until the minute is over; a refresh always asks.
        handle.answer(listed(vec![model("standin", "m2", per_picture(0.1))]));
        assert_eq!(f.call(json!({"kind": "image"})).unwrap(), failed);
        assert_eq!(handle.asked(), 2);
        // A caller that wants nothing older than "now" is not held to the window.
        assert_eq!(ids(entry(&f.call(json!({"kind": "image", "max_age_minutes": 0})).unwrap(), "standin")), ["m2"]);
        assert_eq!(handle.asked(), 3);

        // A minute later it asks again.
        handle.answer(Err(ToolError::new(ErrorCode::TimedOut, "Stand-in took too long.")));
        f.call(json!({"kind": "image", "refresh": true})).unwrap();
        assert_eq!(handle.asked(), 4);
        let mut file = f.cache();
        let old = craft_anything_engine::inbox::now_ms() - 120_000;
        file["lists"]["standin/image"]["failed_ms"] = json!(old);
        std::fs::write(f.cache_path(), file.to_string()).unwrap();
        handle.answer(listed(vec![model("standin", "m3", per_picture(0.2))]));
        assert_eq!(ids(entry(&f.call(json!({"kind": "image"})).unwrap(), "standin")), ["m3"]);
        assert_eq!(handle.asked(), 5);
        assert!(f.cache()["lists"]["standin/image"].get("failed_ms").is_none(), "a good list carries no failure");

        // The same for a provider with nothing kept, and for a list that came with an error.
        let (q, other) = standin("other", "Other", Unit::Usd, Err(ToolError::new(ErrorCode::TimedOut, "Other took too long.")));
        let g = Fixture::new(vec![q], &["other"]);
        let first = g.call(json!({"kind": "image"})).unwrap();
        assert_eq!(g.call(json!({"kind": "image"})).unwrap(), first);
        assert_eq!(other.asked(), 1);
    }

    #[test]
    fn a_cancel_between_providers_keeps_what_was_fetched() {
        let (a, first) = standin("first", "First", Unit::Usd, listed(vec![model("first", "a1", per_picture(0.04))]));
        let (b, second) = standin("second", "Second", Unit::Usd, listed(vec![model("second", "b1", per_picture(0.05))]));
        let token = CancelToken::default();
        first.cancel_when_asked(token.clone());
        let f = Fixture::new(vec![a, b], &["first", "second"]);
        let err = f.call_with(json!({"kind": "image"}), CallOptions::new(Caller::Cli).with_cancel(token)).expect_err("cancelled");
        assert_eq!(err.code, ErrorCode::Cancelled);
        assert_eq!((first.asked(), second.asked()), (1, 0), "the next provider is not asked once the call is cancelled");
        let lists = f.cache()["lists"].clone();
        assert_eq!(ids(&lists["first/image"]), ["a1"], "what the first provider listed is kept");
        assert!(lists.get("second/image").is_none());
        // Looking again uses what was kept and asks only the provider that was not asked.
        let r = f.call(json!({"kind": "image"})).unwrap();
        assert_eq!((ids(entry(&r, "first")), ids(entry(&r, "second"))), (vec!["a1".to_string()], vec!["b1".to_string()]));
        assert_eq!((first.asked(), second.asked()), (1, 1));
    }

    #[test]
    fn a_providers_reason_is_cut_to_its_cap() {
        let cap = 600;
        // Cut by characters, not bytes.
        let (p, handle) = standin("standin", "Stand-in", Unit::Usd, Err(ToolError::new(ErrorCode::EngineMissing, "\u{e9}".repeat(900))));
        let f = Fixture::new(vec![p], &["standin"]);
        let r = f.call(json!({"kind": "image"})).unwrap();
        assert_eq!(entry(&r, "standin")["error"].as_str().unwrap().chars().count(), cap);
        assert_eq!(f.cache()["lists"]["standin/image"]["error"].as_str().unwrap().chars().count(), cap, "what is kept is cut too");
        // A list's own reason as well, and a short one is left as it is.
        handle.answer(Ok(ModelList { models: Vec::new(), error: Some("x".repeat(5000)) }));
        let r = f.call(json!({"kind": "image", "refresh": true})).unwrap();
        assert_eq!(entry(&r, "standin")["error"].as_str().unwrap().len(), cap);
        handle.answer(Ok(ModelList { models: Vec::new(), error: Some("Short.".into()) }));
        let r = f.call(json!({"kind": "image", "refresh": true})).unwrap();
        assert_eq!(entry(&r, "standin")["error"], "Short.");
    }

    #[test]
    fn what_a_listed_model_says_about_its_price_is_what_project_assets_choose_takes() {
        let usd = |rate| Pricing::Before { unit: Unit::Usd, rate };
        let credits = |rate| Pricing::Before { unit: Unit::Credits, rate };
        let dollars = [
            per_picture(0.04),
            usd(Rate::PerMegapixel(0.003)),
            usd(Rate::PerSecond(0.0125)),
            usd(Rate::PerCharacter(0.00002)),
            usd(Rate::Quoted { typical: None }),
            usd(Rate::Quoted { typical: Some(0.2) }),
            Pricing::After { unit: Unit::Usd },
            Pricing::Unknown,
        ];
        let in_credits = [
            credits(Rate::PerRequest(20.0)),
            credits(Rate::PerMegapixel(7.5)),
            credits(Rate::PerSecond(15.0)),
            credits(Rate::PerCharacter(0.5)),
            credits(Rate::Quoted { typical: None }),
            credits(Rate::Quoted { typical: Some(35.0) }),
            Pricing::After { unit: Unit::Credits },
            Pricing::Unknown,
        ];
        let models = |provider: &str, prices: &[Pricing]| -> Vec<ListedModel> {
            prices.iter().enumerate().map(|(n, p)| model(provider, &format!("m{n}"), p.clone())).collect()
        };
        let (a, _) = standin("dollars", "Dollars", Unit::Usd, listed(models("dollars", &dollars)));
        let (b, _) = standin("credits", "Credits", Unit::Credits, listed(models("credits", &in_credits)));
        let f = Fixture::new(vec![a, b], &["dollars", "credits"]);
        let dir = tempfile::tempdir().unwrap();
        let ws = Workspace::create(dir.path(), "Game", "A game", TargetRef::None).unwrap();
        let result = f.call(json!({"kind": "image"})).unwrap();
        let mut chosen_count = 0;
        for provider in ["dollars", "credits"] {
            let unit = entry(&result, provider)["unit"].clone();
            for listed in entry(&result, provider)["models"].as_array().unwrap() {
                // The model exactly as the list showed it: provider, id, name and price.
                let params = json!({"project": ws.root, "kind": "image", "provider": listed["provider"], "model": listed["id"], "model_name": listed["name"], "pricing": listed["pricing"]});
                let chosen = f.engine.call("project_assets_choose", params, CallOptions::new(Caller::Cli)).map(|o| o.to_json()["result"].clone());
                let chosen = chosen.unwrap_or_else(|e| panic!("{listed}: {e:?}"));
                assert_eq!((&chosen["types"]["image"]["provider"], &chosen["types"]["image"]["model"]), (&listed["provider"], &listed["id"]));
                let stored = Workspace::open(&ws.root).unwrap().manifest.assets.types[&AssetKind::Image].choice.clone();
                assert_eq!(serde_json::to_value(&stored.pricing).unwrap(), listed["pricing"], "{listed}");
                assert_eq!(serde_json::to_value(stored.unit).unwrap(), unit, "{listed}");
                assert_eq!((stored.model.as_str(), stored.model_name.as_str()), (listed["id"].as_str().unwrap(), listed["name"].as_str().unwrap()));
                chosen_count += 1;
            }
        }
        assert_eq!(chosen_count, dollars.len() + in_credits.len());
    }

    #[test]
    fn the_tool_is_one_of_the_ai_layers_and_runs_in_clean_room() {
        assert_eq!(crate::tools().iter().filter(|t| t.id == "assets_models_list").count(), 1);
        assert!(craft_anything_engine::clean_room::allows("assets_models_list"), "it reads no target");
    }

    #[test]
    fn the_params_are_checked() {
        let (p, handle) = standin("standin", "Stand-in", Unit::Usd, listed(Vec::new()));
        let f = Fixture::new(vec![p], &["standin"]);
        for bad in [
            json!({}),
            json!({"kind": "video"}),
            json!({"kind": "image", "surprise": 1}),
            json!({"kind": "image", "max_age_minutes": -1}),
            json!({"kind": "image", "refresh": "yes"}),
        ] {
            let err = f.call(bad.clone()).expect_err("refused");
            assert_eq!(err.code, ErrorCode::BadInput, "{bad}");
        }
        assert_eq!(handle.asked(), 0, "nothing is asked of a provider for params that are refused");
    }
}
