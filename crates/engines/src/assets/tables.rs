//! Two small tables behind the models providers list live: which asset types a model makes when
//! the provider's list doesn't say (`known_models.toml`), and what the input fields of a model
//! mean to us (`input_roles.toml`). Both are built in and typed; a table that doesn't parse is
//! logged and then behaves as an empty one, and a test keeps the shipped tables parsing.

use super::{AssetKind, Shape};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

const KNOWN_MODELS_TOML: &str = include_str!("known_models.toml");
const INPUT_ROLES_TOML: &str = include_str!("input_roles.toml");

/// The only version of either table this build reads.
const VERSION: u32 = 1;

/// A model whose asset types, or fixed credit rate, this app knows without asking the provider.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KnownModel {
    /// The provider's id (`fal`, `scenario`, `elevenlabs`).
    pub provider: String,
    /// The model's id at that provider, the one its live list uses.
    pub id: String,
    /// Shown to the person when the provider's list gives no name.
    pub name: String,
    pub kinds: Vec<AssetKind>,
    /// Credits a second of audio costs, for the models whose rate the provider doesn't publish.
    pub credits_per_second: Option<f64>,
    /// The provider's only endpoint for its types, in no live list: offered without asking.
    #[serde(default)]
    pub fixed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct KnownModelsFile {
    version: u32,
    #[serde(default)]
    model: Vec<KnownModel>,
}

/// The asset types of models the providers' lists leave unsaid (`known_models.toml`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KnownModels {
    models: Vec<KnownModel>,
}

impl KnownModels {
    /// The built-in table. One that doesn't parse (a broken edit) is logged and acts as an empty
    /// table; a test keeps it parsing.
    pub fn built_in() -> &'static KnownModels {
        static TABLE: OnceLock<KnownModels> = OnceLock::new();
        TABLE.get_or_init(|| KnownModels::from_text_or_empty(KNOWN_MODELS_TOML))
    }

    /// The table in `text`, or an empty one (after a warning) when the text doesn't parse.
    fn from_text_or_empty(text: &str) -> KnownModels {
        KnownModels::parse(text).unwrap_or_else(|e| {
            log::warn!("known_models.toml is unreadable ({e}); no models are known in advance");
            KnownModels::default()
        })
    }

    /// A table from TOML text. An unknown asset type or field, another version, a model with no
    /// id or no type, a rate that isn't a positive number, or the same model twice is an error.
    pub fn parse(text: &str) -> Result<KnownModels, String> {
        let file: KnownModelsFile = toml::from_str(text).map_err(|e| e.to_string())?;
        if file.version != VERSION {
            return Err(format!("version {} is not supported (this build reads {VERSION})", file.version));
        }
        let mut seen = HashSet::new();
        for m in &file.model {
            let who = format!("{} {}", m.provider, m.id);
            if m.provider.trim().is_empty() || m.id.trim().is_empty() {
                return Err(format!("a model has no provider or id ({who:?})"));
            }
            if m.kinds.is_empty() {
                return Err(format!("{who} has no asset type"));
            }
            if m.credits_per_second.is_some_and(|rate| !rate.is_finite() || rate <= 0.0) {
                return Err(format!("{who} has a credit rate that is not a positive number"));
            }
            if !seen.insert((m.provider.as_str(), m.id.as_str())) {
                return Err(format!("{who} is listed twice"));
            }
        }
        Ok(KnownModels { models: file.model })
    }

    fn find(&self, provider: &str, model: &str) -> Option<&KnownModel> {
        self.models.iter().find(|m| m.provider == provider && m.id == model)
    }

    /// The name this table gives the model, or `None` when it doesn't know it.
    pub fn name(&self, provider: &str, model: &str) -> Option<&str> {
        self.find(provider, model).map(|m| m.name.as_str())
    }

    /// The asset types the model makes, or `None` when this table doesn't know it.
    pub fn kinds(&self, provider: &str, model: &str) -> Option<&[AssetKind]> {
        self.find(provider, model).map(|m| m.kinds.as_slice())
    }

    /// The provider's fixed models for a type, which no live list names.
    pub fn fixed(&self, provider: &str, kind: AssetKind) -> Vec<&KnownModel> {
        self.models.iter().filter(|m| m.fixed && m.provider == provider && m.kinds.contains(&kind)).collect()
    }

    /// The credit rate this app keeps for a model, or `None` when it keeps none.
    pub fn credits_per_second(&self, provider: &str, model: &str) -> Option<f64> {
        self.find(provider, model).and_then(|m| m.credits_per_second)
    }
}

/// What an input field of a model is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// The text describing what to make.
    Prompt,
    /// A picture to start from.
    Image,
    /// The picture's shape, as a named size or an aspect ratio.
    Shape,
    Width,
    Height,
    /// The number of faces asked of a 3D model.
    Faces,
    /// Whether a 3D model has textures.
    Textured,
    /// How long a sound or a track lasts, in seconds.
    Duration,
    /// Music without vocals.
    Instrumental,
    /// The voice speech is read in.
    Voice,
}

impl Role {
    /// Every role. A new variant must be added here as well as to the matches on `Role`, which
    /// the compiler checks; a test requires `input_roles.toml` to give each one a field name.
    pub const ALL: [Role; 10] =
        [Role::Prompt, Role::Image, Role::Shape, Role::Width, Role::Height, Role::Faces, Role::Textured, Role::Duration, Role::Instrumental, Role::Voice];
}

/// The field names of each role, as written in `input_roles.toml`.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct RoleNames {
    prompt: Vec<String>,
    image: Vec<String>,
    shape: Vec<String>,
    width: Vec<String>,
    height: Vec<String>,
    faces: Vec<String>,
    textured: Vec<String>,
    duration: Vec<String>,
    instrumental: Vec<String>,
    voice: Vec<String>,
}

impl RoleNames {
    fn by_role(&self) -> [(Role, &[String]); 10] {
        [
            (Role::Prompt, &self.prompt),
            (Role::Image, &self.image),
            (Role::Shape, &self.shape),
            (Role::Width, &self.width),
            (Role::Height, &self.height),
            (Role::Faces, &self.faces),
            (Role::Textured, &self.textured),
            (Role::Duration, &self.duration),
            (Role::Instrumental, &self.instrumental),
            (Role::Voice, &self.voice),
        ]
    }
}

/// The values of a `shape` field for each picture shape, best first.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
struct ShapeValues {
    square: Vec<String>,
    wide: Vec<String>,
    tall: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputRolesFile {
    version: u32,
    #[serde(default)]
    roles: RoleNames,
    #[serde(default)]
    shape_values: ShapeValues,
}

/// What the input fields of the providers' models mean to us (`input_roles.toml`).
#[derive(Debug, Default)]
pub struct InputRoles {
    roles: HashMap<String, Role>,
    shapes: ShapeValues,
}

impl InputRoles {
    /// The built-in table. One that doesn't parse (a broken edit) is logged and acts as an empty
    /// table, under which no field has a role; a test keeps it parsing.
    pub fn built_in() -> &'static InputRoles {
        static TABLE: OnceLock<InputRoles> = OnceLock::new();
        TABLE.get_or_init(|| InputRoles::from_text_or_empty(INPUT_ROLES_TOML))
    }

    /// The table in `text`, or an empty one (after a warning) when the text doesn't parse.
    fn from_text_or_empty(text: &str) -> InputRoles {
        InputRoles::parse(text).unwrap_or_else(|e| {
            log::warn!("input_roles.toml is unreadable ({e}); no model input has a known role");
            InputRoles::default()
        })
    }

    /// A table from TOML text. An unknown role or field, another version, an empty name, or a
    /// name (or shape value) listed twice, whether under one role or two, is an error.
    pub fn parse(text: &str) -> Result<InputRoles, String> {
        let file: InputRolesFile = toml::from_str(text).map_err(|e| e.to_string())?;
        if file.version != VERSION {
            return Err(format!("version {} is not supported (this build reads {VERSION})", file.version));
        }
        let mut roles = HashMap::new();
        for (role, names) in file.roles.by_role() {
            for name in names {
                if name.trim().is_empty() {
                    return Err(format!("{role:?} has an empty field name"));
                }
                if let Some(other) = roles.insert(name.clone(), role) {
                    return Err(format!("the field {name:?} is listed for {other:?} and for {role:?}"));
                }
            }
        }
        let mut seen = HashSet::new();
        let shapes = &file.shape_values;
        for value in shapes.square.iter().chain(&shapes.wide).chain(&shapes.tall) {
            if value.trim().is_empty() || !seen.insert(value.as_str()) {
                return Err(format!("the shape value {value:?} is empty or listed twice"));
            }
        }
        Ok(InputRoles { roles, shapes: file.shape_values })
    }

    /// What the field of this name is for, or `None` when it has no role we can fill.
    pub fn role_of(&self, field: &str) -> Option<Role> {
        self.roles.get(field).copied()
    }

    /// The value for `shape` that a `shape` field allows: the first of the shape's values, best
    /// first, that is among `allowed`. `None` when the field has no value for that shape.
    pub fn shape_value(&self, shape: Shape, allowed: &[String]) -> Option<String> {
        let values = match shape {
            Shape::Square => &self.shapes.square,
            Shape::Wide => &self.shapes.wide,
            Shape::Tall => &self.shapes.tall,
        };
        values.iter().find(|v| allowed.contains(v)).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn the_tables_load_and_answer() {
        // The shipped text must parse; `built_in()` would hide a failure behind an empty table.
        let known = KnownModels::parse(KNOWN_MODELS_TOML).unwrap();
        assert_eq!(&known, KnownModels::built_in(), "the built-in table is the one in the file");
        assert!(InputRoles::parse(INPUT_ROLES_TOML).is_ok());

        assert_eq!(known.kinds("elevenlabs", "eleven_text_to_sound_v2"), Some(&[AssetKind::SoundEffect][..]));
        assert_eq!(known.kinds("elevenlabs", "music_v1"), Some(&[AssetKind::Music][..]));
        let rate = known.credits_per_second("elevenlabs", "eleven_text_to_sound_v2");
        assert!(rate.is_some_and(|x| x > 0.0), "{rate:?}");
        let rate = known.credits_per_second("elevenlabs", "music_v1");
        assert!(rate.is_some_and(|x| x > 0.0), "{rate:?}");

        let roles = InputRoles::built_in();
        assert_eq!(roles.role_of("image_size"), Some(Role::Shape));
        assert_eq!(roles.shape_value(Shape::Wide, &strings(&["square_hd", "landscape_16_9"])), Some("landscape_16_9".into()));
        assert_eq!(roles.shape_value(Shape::Tall, &strings(&["square"])), None);
    }

    #[test]
    fn known_models_are_found_by_provider_and_id() {
        let known = KnownModels::built_in();
        // Today's models, under the provider's own id.
        assert_eq!(known.kinds("fal", "fal-ai/flux/dev"), Some(&[AssetKind::Image][..]));
        assert_eq!(known.kinds("fal", "tripo3d/p1/image-to-3d"), Some(&[AssetKind::Model3d][..]));
        assert_eq!(known.kinds("scenario", "model_meshy-7-1-img23d"), Some(&[AssetKind::Model3d][..]));
        // Scenario has deprecated the Meshy 6 endpoint that table named before; its replacement is the model.
        assert_eq!(known.kinds("scenario", "model_meshy-img23d"), None);
        assert_eq!(known.kinds("elevenlabs", "eleven_multilingual_v2"), Some(&[AssetKind::Speech][..]));
        // Audio models the providers can't tell apart.
        assert_eq!(known.kinds("fal", "fal-ai/kokoro/french"), Some(&[AssetKind::Speech][..]));
        assert_eq!(known.kinds("scenario", "model_meta-musicgen"), Some(&[AssetKind::Music][..]));
        assert_eq!(known.name("fal", "fal-ai/flux/dev"), Some("FLUX.1 [dev]"));
        assert_eq!(known.name("fal", "no-such-model"), None);
        // The id is the provider's, and a provider's models are its own.
        assert_eq!(known.kinds("scenario", "eleven_text_to_sound_v2"), None);
        assert_eq!(known.kinds("elevenlabs", "no-such-model"), None);
        // Only the models with a rate of their own have one.
        assert_eq!(known.credits_per_second("fal", "fal-ai/flux/dev"), None);
        assert_eq!(known.credits_per_second("elevenlabs", "no-such-model"), None);
    }

    #[test]
    fn fixed_models_are_the_providers_only_endpoints_and_all_have_a_rate() {
        let known = KnownModels::built_in();
        let ids = |provider, kind| known.fixed(provider, kind).into_iter().map(|m| m.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids("elevenlabs", AssetKind::SoundEffect), ["eleven_text_to_sound_v2"]);
        assert_eq!(ids("elevenlabs", AssetKind::Music), ["music_v1"]);
        assert!(ids("elevenlabs", AssetKind::Speech).is_empty(), "speech models come from the live list");
        assert!(ids("fal", AssetKind::Music).is_empty());
        assert!(known.models.iter().filter(|m| m.fixed).all(|m| m.credits_per_second.is_some()), "a fixed model without a rate is not tracked");
    }

    #[test]
    fn a_broken_table_is_a_build_error() {
        let unknown_kind = "version = 1\n[[model]]\nprovider = \"fal\"\nid = \"x\"\nname = \"X\"\nkinds = [\"video\"]\n";
        let err = KnownModels::parse(unknown_kind).unwrap_err();
        assert!(err.contains("video"), "{err}");
        let misspelt = "version = 1\n[[model]]\nprovider = \"fal\"\nid = \"x\"\nname = \"X\"\nkinds = [\"image\"]\ncredit_per_second = 1.0\n";
        assert!(KnownModels::parse(misspelt).is_err(), "a misspelt field is not ignored");
        assert!(KnownModels::parse("not toml at all [").is_err());

        let unknown_role = INPUT_ROLES_TOML.replace("[roles]\n", "[roles]\nsmell = [\"nose\"]\n");
        assert!(InputRoles::parse(&unknown_role).is_err());
    }

    #[test]
    fn a_table_that_contradicts_itself_is_an_error() {
        let model = |extra: &str| format!("version = 1\n[[model]]\nprovider = \"fal\"\nid = \"x\"\nname = \"X\"\nkinds = [\"image\"]\n{extra}");
        assert!(KnownModels::parse(&model("")).is_ok());
        for rate in ["0.0", "-1.0", "nan", "inf"] {
            assert!(KnownModels::parse(&model(&format!("credits_per_second = {rate}\n"))).is_err(), "rate {rate}");
        }
        assert!(KnownModels::parse(&model("").replace("version = 1", "version = 2")).is_err());
        assert!(KnownModels::parse(&model("").replace("kinds = [\"image\"]", "kinds = []")).is_err());
        assert!(KnownModels::parse(&model("").replace("id = \"x\"", "id = \"\"")).is_err());
        let twice = format!("{}{}", model(""), model("").replace("version = 1\n", ""));
        assert!(KnownModels::parse(&twice).unwrap_err().contains("twice"));

        assert!(InputRoles::parse(&INPUT_ROLES_TOML.replace("version = 1", "version = 2")).is_err());
        // A field name can't have two roles, and a shape value can't mean two shapes.
        let two_roles = INPUT_ROLES_TOML.replace("width = [\"width\"]", "width = [\"width\", \"prompt\"]");
        assert!(InputRoles::parse(&two_roles).unwrap_err().contains("prompt"));
        let two_shapes = INPUT_ROLES_TOML.replace("wide = [\"landscape_16_9\"", "wide = [\"square\", \"landscape_16_9\"");
        assert!(InputRoles::parse(&two_shapes).unwrap_err().contains("square"));
    }

    #[test]
    fn a_table_that_does_not_parse_acts_as_empty() {
        // What `built_in()` does with a file that doesn't parse: it must answer, not panic.
        let broken = [
            "not toml at all [",
            "version = 2\n",
            "version = 1\n[[model]]\nprovider = \"fal\"\nid = \"x\"\nname = \"X\"\nkinds = [\"video\"]\n",
            "version = 1\n[roles]\nprompt = [\"\"]\n",
            "",
        ];
        for text in broken {
            // Each is really broken for both parsers, so the empty table is the fallback and not the text.
            assert!(InputRoles::parse(text).is_err(), "{text:?}");
            assert!(KnownModels::parse(text).is_err(), "{text:?}");
            let roles = InputRoles::from_text_or_empty(text);
            assert_eq!(roles.role_of("prompt"), None, "{text:?}");
            assert_eq!(roles.shape_value(Shape::Square, &strings(&["square_hd"])), None, "{text:?}");
            let known = KnownModels::from_text_or_empty(text);
            assert_eq!(known, KnownModels::default(), "{text:?}");
            assert_eq!(known.kinds("fal", "fal-ai/flux/dev"), None);
            assert!(known.fixed("elevenlabs", AssetKind::Music).is_empty());
        }
        // Text that does parse is read as it is, not thrown away.
        assert_eq!(&KnownModels::from_text_or_empty(KNOWN_MODELS_TOML), KnownModels::built_in());
        assert_eq!(InputRoles::from_text_or_empty(INPUT_ROLES_TOML).role_of("prompt"), Some(Role::Prompt));
        // And the default is what `built_in()` would hold.
        assert_eq!(InputRoles::default().role_of("prompt"), None);
        assert_eq!(KnownModels::default().kinds("fal", "fal-ai/flux/dev"), None);
    }

    #[test]
    fn every_role_has_a_field_name() {
        // An exhaustive match on purpose: a new `Role` does not compile until it has a name here,
        // and the table must map that name back to it.
        fn example_field(role: Role) -> &'static str {
            match role {
                Role::Prompt => "prompt",
                Role::Image => "image_url",
                Role::Shape => "image_size",
                Role::Width => "width",
                Role::Height => "height",
                Role::Faces => "target_face_count",
                Role::Textured => "should_texture",
                Role::Duration => "seconds_total",
                Role::Instrumental => "force_instrumental",
                Role::Voice => "voice_id",
            }
        }
        let table = InputRoles::built_in();
        for role in Role::ALL {
            assert_eq!(table.role_of(example_field(role)), Some(role), "{role:?} has no field name in input_roles.toml");
        }
        // `ALL` lists each role once, and the table parser's own list covers the same ten.
        let distinct: HashSet<Role> = Role::ALL.into_iter().collect();
        assert_eq!(distinct.len(), Role::ALL.len());
        let parsed: HashSet<Role> = RoleNames::default().by_role().into_iter().map(|(role, _)| role).collect();
        assert_eq!(parsed, distinct, "the roles input_roles.toml can name are not the roles there are");
    }

    #[test]
    fn every_role_has_names_and_the_best_shape_value_wins() {
        let roles = InputRoles::built_in();
        for (field, role) in [
            ("prompt", Role::Prompt),
            ("text", Role::Prompt),
            ("image_url", Role::Image),
            // fal's Hunyuan 3D models name their picture `input_image_url`.
            ("input_image_url", Role::Image),
            ("aspect_ratio", Role::Shape),
            // Scenario writes its input names in camelCase: the ones its generate calls have always
            // sent (`aspectRatio`, `targetPolycount`, `shouldTexture`, `enablePbr`) and the
            // face count its 3D guide names (`targetFaceNum`, https://docs.scenario.com/get-started/generation/3d-model-generation/).
            ("aspectRatio", Role::Shape),
            ("targetPolycount", Role::Faces),
            ("targetFaceNum", Role::Faces),
            ("shouldTexture", Role::Textured),
            ("enablePbr", Role::Textured),
            ("width", Role::Width),
            ("height", Role::Height),
            ("target_face_count", Role::Faces),
            ("polycount", Role::Faces),
            ("should_texture", Role::Textured),
            ("enable_pbr", Role::Textured),
            ("seconds_total", Role::Duration),
            ("force_instrumental", Role::Instrumental),
            ("voice_id", Role::Voice),
        ] {
            assert_eq!(roles.role_of(field), Some(role), "{field}");
        }
        assert_eq!(roles.role_of("music_length_ms"), None, "milliseconds are not seconds");
        assert_eq!(roles.role_of("Prompt"), None, "field names are exact");
        // The first value in the table's order that the field allows, not the first it lists.
        assert_eq!(roles.shape_value(Shape::Square, &strings(&["1:1", "square", "square_hd"])), Some("square_hd".into()));
        assert_eq!(roles.shape_value(Shape::Square, &strings(&["1:1", "16:9"])), Some("1:1".into()));
        assert_eq!(roles.shape_value(Shape::Wide, &strings(&["16:9", "9:16"])), Some("16:9".into()));
        assert_eq!(roles.shape_value(Shape::Tall, &strings(&["9:16", "portrait_16_9"])), Some("portrait_16_9".into()));
        assert_eq!(roles.shape_value(Shape::Wide, &[]), None);
    }
}
