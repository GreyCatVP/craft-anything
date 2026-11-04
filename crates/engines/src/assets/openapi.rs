//! The input fields of a model, read from the OpenAPI document fal publishes for it
//! (`expand=openapi-3.0` on its model list).
//!
//! The document is the provider's, so it is read with bounds: a `$ref` or `allOf` chain is
//! followed [`MAX_DEPTH`] steps and no more, the whole document costs at most [`MAX_STEPS`]
//! steps (a schema that refers to others many times over cannot multiply its way past that), and
//! a model with more than [`MAX_FIELDS`] inputs, a choice with more than [`MAX_CHOICES`] values,
//! or an input named by more than [`MAX_NAME`] characters is refused rather than cut short.
//! Nothing is copied out of the document except the field names and choice values that are kept.
//! A reference may only point inside the document.

use super::inputs::{FieldKind, InputField, MAX_CHOICES, MAX_FIELDS, REMEDIATION};
use craft_anything_evidence::{ErrorCode, ToolError};
use serde_json::{Map, Value};
use std::collections::HashSet;

/// How many `$ref` and `allOf` steps lie between a schema and what it finally says.
const MAX_DEPTH: usize = 8;
/// How many schemas are looked at for one model. A real model needs a few hundred.
const MAX_STEPS: usize = 5_000;
/// The longest input name kept; real names are a few words.
const MAX_NAME: usize = 128;

/// The inputs of the model the document describes: every property of the body its endpoint takes,
/// in the kinds this app can fill (anything else is [`FieldKind::Other`]).
///
/// A property is required when the schema lists it, and has a default when the schema gives one.
/// "Or null" (`anyOf` with a null, or a `type` list with `"null"`) makes a property optional, not
/// a different kind; of several kinds a property may be, a choice of named values wins (a picture
/// size may be a width and height or a name), then the first kind that is a plain value.
pub fn input_fields(doc: &Value) -> Result<Vec<InputField>, ToolError> {
    let body = request_schema(doc).ok_or_else(garbled)?;
    let mut reader = Reader { doc, steps: 0 };
    let input = reader.see(body, 0)?;
    // The schema's own properties come first, so what it says wins over what it builds on.
    let mut properties: Vec<(&str, &Value)> = Vec::new();
    let mut named: HashSet<&str> = HashSet::new();
    let mut required: HashSet<&str> = HashSet::new();
    for part in &input.0 {
        if let Some(own) = part.get("properties").and_then(Value::as_object) {
            if own.len() > MAX_FIELDS {
                return Err(too_big());
            }
            for (name, schema) in own {
                if named.insert(name.as_str()) {
                    if properties.len() >= MAX_FIELDS {
                        return Err(too_big());
                    }
                    properties.push((name, schema));
                }
            }
        }
        if let Some(list) = part.get("required").and_then(Value::as_array) {
            required.extend(list.iter().filter_map(Value::as_str));
        }
    }
    let mut fields = Vec::with_capacity(properties.len());
    for (name, schema) in properties {
        if name.len() > MAX_NAME {
            return Err(too_big());
        }
        if name.is_empty() {
            continue;
        }
        let seen = reader.see(schema, 1)?;
        let kind = reader.kind(&seen, 1)?;
        fields.push(InputField { name: name.to_string(), kind, required: required.contains(name), has_default: seen.get("default").is_some() });
    }
    Ok(fields)
}

/// The schema of the JSON body a model's endpoint takes. The path is not the endpoint id (Tripo's
/// carries a `fal-ai/` prefix its id lacks), so it is the path without a `{placeholder}`, which
/// the status and cancel paths all have, that takes a POST body.
fn request_schema(doc: &Value) -> Option<&Value> {
    doc.get("paths")?
        .as_object()?
        .iter()
        .filter(|(path, _)| !path.contains('{'))
        .find_map(|(_, item)| item.pointer("/post/requestBody/content/application~1json/schema"))
}

/// A schema seen through its references: the schemas that together say what it is, its own first.
struct Seen<'a>(Vec<&'a Map<String, Value>>);

impl<'a> Seen<'a> {
    /// What the first of them that has the key says.
    fn get(&self, key: &str) -> Option<&'a Value> {
        self.0.iter().find_map(|schema| schema.get(key))
    }
}

struct Reader<'a> {
    doc: &'a Value,
    steps: usize,
}

impl<'a> Reader<'a> {
    /// `schema` with its `$ref` and `allOf` followed, `depth` steps from where the reading began.
    fn see(&mut self, schema: &'a Value, depth: usize) -> Result<Seen<'a>, ToolError> {
        self.steps += 1;
        if depth > MAX_DEPTH || self.steps > MAX_STEPS {
            return Err(too_big());
        }
        let Some(own) = schema.as_object() else { return Ok(Seen(Vec::new())) };
        let mut parts = vec![own];
        if let Some(target) = own.get("$ref").and_then(Value::as_str) {
            let target = self.lookup(target)?;
            parts.extend(self.see(target, depth + 1)?.0);
        }
        if let Some(all) = own.get("allOf").and_then(Value::as_array) {
            for part in all {
                parts.extend(self.see(part, depth + 1)?.0);
            }
        }
        Ok(Seen(parts))
    }

    /// What a `$ref` points at. Only a place in this document is followed.
    fn lookup(&self, target: &str) -> Result<&'a Value, ToolError> {
        let doc: &'a Value = self.doc;
        target.strip_prefix('#').and_then(|pointer| doc.pointer(pointer)).ok_or_else(garbled)
    }

    fn kind(&mut self, seen: &Seen<'a>, depth: usize) -> Result<FieldKind, ToolError> {
        if let Some(values) = seen.get("enum").and_then(Value::as_array) {
            if values.len() > MAX_CHOICES {
                return Err(too_big());
            }
            // A value that may also be null is still a choice of the others.
            let words: Vec<String> = values.iter().filter(|v| !v.is_null()).filter_map(|v| v.as_str().map(str::to_string)).collect();
            let kept = values.iter().filter(|v| !v.is_null()).count();
            return Ok(if !words.is_empty() && words.len() == kept { FieldKind::Choice(words) } else { FieldKind::Other });
        }
        if let Some(options) = seen.get("anyOf").or_else(|| seen.get("oneOf")).and_then(Value::as_array) {
            let mut best = FieldKind::Other;
            for option in options {
                let option = self.see(option, depth + 1)?;
                match self.kind(&option, depth + 1)? {
                    FieldKind::Other => {}
                    named @ FieldKind::Choice(_) => return Ok(named),
                    plain if best == FieldKind::Other => best = plain,
                    _ => {}
                }
            }
            return Ok(best);
        }
        Ok(match type_of(seen) {
            Some("string") => FieldKind::Text,
            Some("integer") => {
                FieldKind::Integer { min: whole_limit(seen, "minimum", "exclusiveMinimum"), max: whole_limit(seen, "maximum", "exclusiveMaximum") }
            }
            Some("number") => FieldKind::Number { min: limit(seen, "minimum", "exclusiveMinimum"), max: limit(seen, "maximum", "exclusiveMaximum") },
            Some("boolean") => FieldKind::Bool,
            _ => FieldKind::Other,
        })
    }
}

/// The schema's `type`; of a list of types ("string" and "null"), the first that is not "null".
fn type_of<'a>(seen: &Seen<'a>) -> Option<&'a str> {
    match seen.get("type")? {
        Value::String(one) => Some(one.as_str()),
        Value::Array(many) => many.iter().filter_map(Value::as_str).find(|t| *t != "null"),
        _ => None,
    }
}

/// A numeric limit: the inclusive one if the schema gives it, else the exclusive one (OpenAPI 3.1
/// and Pydantic write it as a number). A limit that is not a finite number is no limit, and the
/// 3.0 spelling (`exclusiveMinimum: true` beside `minimum`) leaves `minimum` as it was.
fn limit(seen: &Seen, inclusive: &str, exclusive: &str) -> Option<f64> {
    let number = |key: &str| seen.get(key).and_then(Value::as_f64).filter(|x| x.is_finite());
    number(inclusive).or_else(|| number(exclusive))
}

/// [`limit`] for a whole number: an exclusive limit moves one step inside, and a limit with a
/// fraction is taken inside as well, so that a value held to it still meets the limit.
fn whole_limit(seen: &Seen, inclusive: &str, exclusive: &str) -> Option<i64> {
    let lower = inclusive == "minimum";
    let bound = |key: &str, exact: bool| {
        let x = seen.get(key).and_then(Value::as_f64).filter(|x| x.is_finite())?;
        // `as` saturates, so a huge limit is the largest whole number rather than a panic.
        let inside = match (lower, exact) {
            (true, true) => x.ceil(),
            (false, true) => x.floor(),
            (true, false) => x.floor() + 1.0,
            (false, false) => x.ceil() - 1.0,
        };
        Some(inside as i64)
    };
    bound(inclusive, true).or_else(|| bound(exclusive, false))
}

fn garbled() -> ToolError {
    ToolError::new(ErrorCode::GarbledOutput, "The description fal.ai gives of this model's inputs could not be read.").with_remediation(REMEDIATION)
}

fn too_big() -> ToolError {
    ToolError::new(ErrorCode::OverLimit, "The description fal.ai gives of this model's inputs is larger than this app reads.").with_remediation(REMEDIATION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::inputs::{FieldKind, MAX_CHOICES, MAX_FIELDS};
    use serde_json::json;

    // Trimmed copies of what `GET https://api.fal.ai/v1/models?endpoint_id=<id>&expand=openapi-3.0`
    // answered on 2026-11-03 (https://fal.ai/docs/platform-apis/v1/models), for `fal-ai/flux/schnell`,
    // `tripo3d/p1/image-to-3d` and `fal-ai/kokoro/american-english`. The document's path is not the
    // endpoint id (Tripo's carries a `fal-ai/` prefix), so the input is found by its POST body.

    fn flux_schnell() -> Value {
        json!({
            "openapi": "3.0.4",
            "paths": {
                "/fal-ai/flux/schnell": {"post": {"requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/FluxSchnellInput"}}}}}},
                "/fal-ai/flux/schnell/requests/{request_id}": {"get": {"responses": {}}},
                "/fal-ai/flux/schnell/requests/{request_id}/cancel": {"put": {"responses": {}}}
            },
            "components": {"schemas": {
                "FluxSchnellInput": {
                    "title": "SchnellTextToImageInput",
                    "type": "object",
                    "x-fal-order-properties": ["num_inference_steps", "prompt", "image_size", "seed", "num_images", "output_format", "enable_safety_checker"],
                    "properties": {
                        "num_inference_steps": {"type": "integer", "minimum": 1, "maximum": 12, "default": 4},
                        "prompt": {"type": "string", "title": "Prompt", "description": "The prompt to generate an image from."},
                        "image_size": {
                            "title": "Image Size",
                            "anyOf": [
                                {"$ref": "#/components/schemas/ImageSize"},
                                {"type": "string", "enum": ["square_hd", "square", "portrait_4_3", "portrait_16_9", "landscape_4_3", "landscape_16_9"]}
                            ],
                            "default": "landscape_4_3"
                        },
                        "seed": {"title": "Seed", "anyOf": [{"type": "integer"}, {"type": "null"}]},
                        "num_images": {"type": "integer", "minimum": 1, "maximum": 4, "default": 1},
                        "output_format": {"type": "string", "enum": ["jpeg", "png"], "default": "jpeg"},
                        "enable_safety_checker": {"type": "boolean", "default": true}
                    },
                    "required": ["prompt"]
                },
                "ImageSize": {
                    "title": "ImageSize",
                    "type": "object",
                    "properties": {
                        "height": {"type": "integer", "exclusiveMinimum": 0, "maximum": 14142, "default": 512},
                        "width": {"type": "integer", "exclusiveMinimum": 0, "maximum": 14142, "default": 512}
                    }
                }
            }}
        })
    }

    fn tripo() -> Value {
        json!({
            "openapi": "3.0.4",
            "paths": {
                "/fal-ai/tripo3d/p1/image-to-3d": {"post": {"requestBody": {"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/Tripo3dP1ImageTo3dInput"}}}}}},
                "/fal-ai/tripo3d/p1/image-to-3d/requests/{request_id}/status": {"get": {"responses": {}}}
            },
            "components": {"schemas": {"Tripo3dP1ImageTo3dInput": {
                "type": "object",
                "properties": {
                    "model_seed": {"anyOf": [{"type": "integer"}, {"type": "null"}]},
                    "face_limit": {"anyOf": [{"type": "integer", "minimum": 48, "maximum": 20000}, {"type": "null"}]},
                    "texture": {"type": "boolean", "default": true},
                    "image_url": {"type": "string"},
                    "enable_safety_checker": {"type": "boolean", "default": true}
                },
                "required": ["image_url"]
            }}}
        })
    }

    fn kokoro() -> Value {
        json!({
            "openapi": "3.0.4",
            "paths": {"/fal-ai/kokoro/american-english": {"post": {"requestBody": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/KokoroInput"}}}}}}},
            "components": {"schemas": {"KokoroInput": {
                "type": "object",
                "properties": {
                    "voice": {"type": "string", "enum": ["af_heart", "af_alloy", "am_adam"], "default": "af_heart"},
                    "speed": {"type": "number", "minimum": 0.1, "maximum": 5.0, "default": 1.0},
                    "prompt": {"type": "string", "default": ""}
                }
            }}}
        })
    }

    /// A document whose input is `input`, with `extra` components beside it.
    fn document(input: Value, extra: Value) -> Value {
        let mut schemas = json!({"Input": input});
        if let (Some(into), Some(from)) = (schemas.as_object_mut(), extra.as_object()) {
            into.extend(from.clone());
        }
        json!({
            "openapi": "3.0.4",
            "paths": {"/x/y": {"post": {"requestBody": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Input"}}}}}}},
            "components": {"schemas": schemas}
        })
    }

    fn fields(doc: &Value) -> Vec<InputField> {
        input_fields(doc).unwrap()
    }

    fn named<'a>(fields: &'a [InputField], name: &str) -> &'a InputField {
        fields.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no field {name} in {fields:?}"))
    }

    fn strings(values: &[&str]) -> FieldKind {
        FieldKind::Choice(values.iter().map(|v| v.to_string()).collect())
    }

    #[test]
    fn a_picture_models_inputs_are_read_with_their_kinds_limits_and_defaults() {
        let f = fields(&flux_schnell());
        assert_eq!(f.len(), 7);
        let prompt = named(&f, "prompt");
        assert_eq!((&prompt.kind, prompt.required, prompt.has_default), (&FieldKind::Text, true, false));
        // A named size or a width and height: the named sizes are what the app can choose from.
        let size = named(&f, "image_size");
        assert_eq!(size.kind, strings(&["square_hd", "square", "portrait_4_3", "portrait_16_9", "landscape_4_3", "landscape_16_9"]));
        assert_eq!((size.required, size.has_default), (false, true));
        assert_eq!(named(&f, "num_images").kind, FieldKind::Integer { min: Some(1), max: Some(4) });
        // "or nothing" makes an input optional, not a different kind.
        let seed = named(&f, "seed");
        assert_eq!((&seed.kind, seed.required, seed.has_default), (&FieldKind::Integer { min: None, max: None }, false, false));
        assert_eq!(named(&f, "output_format").kind, strings(&["jpeg", "png"]));
        assert_eq!(named(&f, "enable_safety_checker").kind, FieldKind::Bool);
    }

    #[test]
    fn the_input_is_found_by_its_post_body_not_by_the_endpoint_id() {
        let f = fields(&tripo());
        assert_eq!(named(&f, "face_limit").kind, FieldKind::Integer { min: Some(48), max: Some(20000) });
        let image = named(&f, "image_url");
        assert_eq!((&image.kind, image.required), (&FieldKind::Text, true));
        assert_eq!(named(&f, "texture").kind, FieldKind::Bool);
        assert!(named(&f, "texture").has_default && !named(&f, "model_seed").has_default);
        // A number with a decimal limit; and a field with a default of "" still has a default.
        let f = fields(&kokoro());
        assert_eq!(named(&f, "speed").kind, FieldKind::Number { min: Some(0.1), max: Some(5.0) });
        assert!(named(&f, "prompt").has_default && !named(&f, "prompt").required);
        assert_eq!(named(&f, "voice").kind, strings(&["af_heart", "af_alloy", "am_adam"]));
    }

    #[test]
    fn a_document_without_an_input_is_unreadable() {
        for doc in [
            json!({}),
            json!({"error": {"code": "expansion_failed", "message": "x"}}),
            json!({"paths": {"/x/requests/{id}": {"post": {"requestBody": {}}}}}),
            json!("text"),
        ] {
            let err = input_fields(&doc).unwrap_err();
            assert_eq!(err.code, ErrorCode::GarbledOutput, "{doc}");
        }
    }

    #[test]
    fn references_and_wrappers_are_followed() {
        let input = json!({
            "type": "object",
            "allOf": [{"$ref": "#/components/schemas/Base"}],
            "properties": {
                // A named enum, as a reference and as a wrapper that also carries the default.
                "style": {"$ref": "#/components/schemas/Style"},
                "mood": {"allOf": [{"$ref": "#/components/schemas/Style"}], "default": "calm"},
                // A reference to a plain number type.
                "scale": {"$ref": "#/components/schemas/Scale"},
                "prompt": {"type": "string"}
            },
            "required": ["prompt", "extra"]
        });
        let extra = json!({
            "Base": {"type": "object", "properties": {"extra": {"type": "string"}, "prompt": {"type": "integer"}}, "required": ["extra"]},
            "Style": {"type": "string", "enum": ["calm", "wild"]},
            "Scale": {"type": "number", "minimum": 0, "maximum": 2}
        });
        let f = fields(&document(input, extra));
        assert_eq!(f.len(), 5, "{f:?}");
        assert_eq!(named(&f, "style").kind, strings(&["calm", "wild"]));
        let mood = named(&f, "mood");
        assert_eq!((&mood.kind, mood.has_default), (&strings(&["calm", "wild"]), true));
        assert_eq!(named(&f, "scale").kind, FieldKind::Number { min: Some(0.0), max: Some(2.0) });
        // A field is what the schema itself says, ahead of the one it builds on; required adds up.
        assert_eq!(named(&f, "prompt").kind, FieldKind::Text);
        assert!(named(&f, "extra").required && named(&f, "prompt").required);
    }

    #[test]
    fn what_this_app_cannot_fill_is_other_and_a_choice_of_numbers_is_too() {
        let input = json!({"properties": {
            "loras": {"type": "array", "items": {"type": "object"}},
            "image_urls": {"type": "array", "items": {"type": "string"}, "default": []},
            "mask": {"type": "object", "properties": {"x": {"type": "integer"}}},
            "steps": {"type": "integer", "enum": [4, 8]},
            "mystery": {},
            "either": {"anyOf": [{"type": "array", "items": {"type": "string"}}, {"type": "null"}]},
            "odd": {"type": ["string", "null"]},
            "count": {"type": ["integer", "null"], "minimum": 1}
        }, "required": ["loras"]});
        let f = fields(&document(input, json!({})));
        for name in ["loras", "image_urls", "mask", "steps", "mystery", "either"] {
            assert_eq!(named(&f, name).kind, FieldKind::Other, "{name}");
        }
        assert!(named(&f, "loras").required && named(&f, "image_urls").has_default);
        // The "type": [..., "null"] spelling some documents use.
        assert_eq!(named(&f, "odd").kind, FieldKind::Text);
        assert_eq!(named(&f, "count").kind, FieldKind::Integer { min: Some(1), max: None });
    }

    #[test]
    fn exclusive_limits_and_odd_numbers_become_limits_a_value_can_meet() {
        let input = json!({"properties": {
            "n": {"type": "integer", "exclusiveMinimum": 0, "maximum": 20.0},
            "x": {"type": "number", "exclusiveMinimum": 0.5},
            "legacy": {"type": "integer", "minimum": 1, "exclusiveMinimum": true},
            "wild": {"type": "number", "minimum": "low", "maximum": null}
        }});
        let f = fields(&document(input, json!({})));
        assert_eq!(named(&f, "n").kind, FieldKind::Integer { min: Some(1), max: Some(20) });
        assert_eq!(named(&f, "x").kind, FieldKind::Number { min: Some(0.5), max: None });
        assert_eq!(named(&f, "legacy").kind, FieldKind::Integer { min: Some(1), max: None });
        assert_eq!(named(&f, "wild").kind, FieldKind::Number { min: None, max: None });
    }

    /// Review Focus 3: a document is the provider's, and may be hostile or simply enormous.
    #[test]
    fn a_ref_cycle_or_huge_schema_is_not_runnable() {
        let refused = |doc: Value| {
            let err = input_fields(&doc).unwrap_err();
            assert_eq!(err.code, ErrorCode::OverLimit, "{err:?}");
            assert!(!err.message.is_empty() && err.remediation.is_some());
        };
        // A reference to itself, and a chain longer than the references that are followed.
        refused(document(json!({"properties": {"a": {"$ref": "#/components/schemas/A"}}}), json!({"A": {"$ref": "#/components/schemas/A"}})));
        refused(document(json!({"$ref": "#/components/schemas/Input"}), json!({})));
        let chain: serde_json::Map<String, Value> = (0..12).map(|i| (format!("S{i}"), json!({"$ref": format!("#/components/schemas/S{}", i + 1)}))).collect();
        refused(document(json!({"properties": {"a": {"$ref": "#/components/schemas/S0"}}}), Value::Object(chain)));
        // A chain within the limit is fine.
        let short: serde_json::Map<String, Value> = (0..5)
            .map(|i| (format!("S{i}"), if i == 4 { json!({"type": "boolean"}) } else { json!({"$ref": format!("#/components/schemas/S{}", i + 1)}) }))
            .collect();
        assert_eq!(
            named(&fields(&document(json!({"properties": {"a": {"$ref": "#/components/schemas/S0"}}}), Value::Object(short))), "a").kind,
            FieldKind::Bool
        );
        // More inputs than are read, or more values in a choice than are read.
        let many: serde_json::Map<String, Value> = (0..=MAX_FIELDS).map(|i| (format!("f{i}"), json!({"type": "string"}))).collect();
        refused(document(json!({"properties": many}), json!({})));
        let enough: serde_json::Map<String, Value> = (0..MAX_FIELDS).map(|i| (format!("f{i}"), json!({"type": "string"}))).collect();
        assert_eq!(fields(&document(json!({"properties": enough}), json!({}))).len(), MAX_FIELDS);
        let values: Vec<String> = (0..=MAX_CHOICES).map(|i| format!("v{i}")).collect();
        refused(document(json!({"properties": {"voice": {"type": "string", "enum": values}}}), json!({})));
        let values: Vec<String> = (0..MAX_CHOICES).map(|i| format!("v{i}")).collect();
        assert_eq!(
            named(&fields(&document(json!({"properties": {"voice": {"type": "string", "enum": values}}}), json!({}))), "voice").kind,
            FieldKind::Choice((0..MAX_CHOICES).map(|i| format!("v{i}")).collect())
        );
        // Many small steps that add up: each of three schemas refers to the next `width` times, so
        // reading it all takes `width` to the third power steps, far more than the document has
        // schemas, though no chain is deeper than the references that are followed.
        let fan = |width: usize| {
            let mut schemas = serde_json::Map::new();
            for level in 0..3 {
                let next = json!({"$ref": format!("#/components/schemas/F{}", level + 1)});
                schemas.insert(format!("F{level}"), json!({"allOf": vec![next; width]}));
            }
            schemas.insert("F3".into(), json!({"type": "string"}));
            document(json!({"properties": {"a": {"$ref": "#/components/schemas/F0"}}}), Value::Object(schemas))
        };
        assert_eq!(named(&fields(&fan(3)), "a").kind, FieldKind::Text, "the same shape, narrower, is read in full: the width is what is refused");
        refused(fan(60));
        // A reference to somewhere else than this document, or to nothing, is unreadable, not a crash.
        for target in ["https://example.invalid/schema.json", "#/components/schemas/Missing", "#/paths/~1x~1y/post/missing", "no-hash"] {
            let err = input_fields(&document(json!({"properties": {"a": {"$ref": target}}}), json!({}))).unwrap_err();
            assert_eq!(err.code, ErrorCode::GarbledOutput, "{target}");
        }
    }

    #[test]
    fn a_name_that_is_not_a_plain_name_is_refused() {
        let long = "n".repeat(500);
        let err = input_fields(&document(json!({"properties": {long: {"type": "string"}}}), json!({}))).unwrap_err();
        assert_eq!(err.code, ErrorCode::OverLimit);
        // Not a single remote word in the message.
        assert!(!err.message.contains("nnnn"), "{}", err.message);
    }
}
