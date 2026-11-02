//! ElevenLabs audio, using the caller's key and published rate estimates.
use super::{AssetProvider, AssetRequest, Generated, http};
use craft_anything_evidence::ToolError;
use serde_json::json;
use std::time::Duration;

pub const ID: &str = "elevenlabs";
pub struct ElevenLabs {
    base: String,
}
impl Default for ElevenLabs {
    /// Use the production ElevenLabs API endpoint.
    fn default() -> Self {
        Self { base: "https://api.elevenlabs.io/v1".into() }
    }
}

/// Check modality-specific limits, identifiers, provider ownership, and a positive finite estimate.
fn validate(req: &AssetRequest) -> Result<(), ToolError> {
    let model = match *req {
        AssetRequest::Speech { model, text, voice_id } => {
            http::text(text, 10_000)?;
            http::id(voice_id)?;
            model
        }
        AssetRequest::SoundEffect { model, prompt, duration_seconds } => {
            http::text(prompt, 2000)?;
            http::duration(duration_seconds, 0.5, 30.0)?;
            model
        }
        AssetRequest::Music { model, prompt, duration_seconds, .. } => {
            http::text(prompt, 2000)?;
            http::duration(duration_seconds, 3.0, 600.0)?;
            model
        }
        _ => return Err(http::bad("ElevenLabs does not support this asset request.")),
    };
    http::id(&model.endpoint)?;
    if model.provider != ID || model.kind != req.kind() || !req.listed_cost().is_finite() || req.listed_cost() <= 0.0 {
        return Err(http::bad("The audio model or its rate does not match this request."));
    }
    Ok(())
}
impl AssetProvider for ElevenLabs {
    /// Return the catalogue identifier used to route requests to ElevenLabs.
    fn id(&self) -> &'static str {
        ID
    }
    /// Return the provider's display name.
    fn name(&self) -> &'static str {
        "ElevenLabs"
    }
    /// Name the environment variable used when no credential is stored.
    fn key_env(&self) -> Option<&'static str> {
        Some("ELEVENLABS_API_KEY")
    }
    /// Return the account page where the person can manage API credentials.
    fn keys_page(&self) -> Option<&'static str> {
        Some("https://elevenlabs.io/app/settings/api-keys")
    }
    /// Validate the audio request and return its catalogue estimate without network access.
    fn estimate(&self, req: &AssetRequest) -> Result<f64, ToolError> {
        validate(req)?;
        Ok(req.listed_cost())
    }
    /// Validate and submit the modality's payload, returning bounded MP3 bytes and an estimated cost.
    fn generate(&self, key: &str, req: &AssetRequest) -> Result<Generated, ToolError> {
        validate(req)?;
        self.validate_key(key)?;
        let key = http::key(key)?;
        let (path, content_type, body): (String, String, Vec<u8>) = match *req {
            AssetRequest::Speech { model, text, voice_id } => (
                format!("text-to-speech/{voice_id}?output_format=mp3_44100_128"),
                "application/json".into(),
                json!({"text":text,"model_id":model.endpoint}).to_string().into_bytes(),
            ),
            AssetRequest::SoundEffect { model, prompt, duration_seconds } => (
                "sound-generation?output_format=mp3_44100_128".into(),
                "application/json".into(),
                json!({"text":prompt,"duration_seconds":duration_seconds,"model_id":model.endpoint,"prompt_influence":0.3}).to_string().into_bytes(),
            ),
            AssetRequest::Music { model, prompt, duration_seconds, instrumental } => (
                "music?output_format=mp3_44100_128".into(),
                "application/json".into(),
                json!({"prompt":prompt,"music_length_ms":(duration_seconds*1000.0).ceil() as u64,"force_instrumental":instrumental,"model_id":model.endpoint})
                    .to_string()
                    .into_bytes(),
            ),
            _ => return Err(http::bad("ElevenLabs does not support this request.")),
        };
        let response = http::agent(Duration::from_secs(600))
            .post(format!("{}/{path}", self.base))
            .header("xi-api-key", key)
            .header("Content-Type", content_type)
            .send(body)
            .map_err(|_| http::network())?;
        http::status(response.status().as_u16())?;
        // The request was accepted, so a failed body read still reports the charge.
        let cost = req.listed_cost();
        let bytes = http::bytes(response.into_body(), http::MAX_BYTES).map_err(|e| super::charged(e, cost))?;
        Ok(Generated { url: String::new(), bytes, cost_usd: cost })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{AssetKind, Catalogue};
    use craft_anything_evidence::ErrorCode;

    /// Check each audio endpoint's payload, authentication, binary result, and catalogue estimate.
    #[test]
    fn speech_sound_and_music_use_distinct_wire_schemas_and_binary_results() {
        let (base, seen) = http::tests::serve(|_| vec![(200, "ID3 speech".into()), (200, "ID3 sound".into()), (200, "ID3 music".into())]);
        let provider = ElevenLabs { base };
        let c = Catalogue::built_in();
        let speech = AssetRequest::Speech { model: c.audio_model(AssetKind::Speech, None).unwrap(), text: "héllo", voice_id: "voice_test" };
        let sound = AssetRequest::SoundEffect { model: c.audio_model(AssetKind::SoundEffect, None).unwrap(), prompt: "wind", duration_seconds: 2.5 };
        let music =
            AssetRequest::Music { model: c.audio_model(AssetKind::Music, None).unwrap(), prompt: "gentle music", duration_seconds: 10.0, instrumental: true };
        assert_eq!(provider.generate("sk-test", &speech).unwrap().bytes, b"ID3 speech");
        assert_eq!(provider.generate("sk-test", &sound).unwrap().bytes, b"ID3 sound");
        assert_eq!(provider.generate("sk-test", &music).unwrap().bytes, b"ID3 music");
        assert_eq!(provider.estimate(&speech).unwrap(), 5.0 * 0.00008);
        assert_eq!(provider.estimate(&sound).unwrap(), 0.005);
        assert_eq!(provider.estimate(&music).unwrap(), 0.025);
        let seen = seen.lock().unwrap();
        assert!(seen[0].starts_with("POST /text-to-speech/voice_test?output_format=mp3_44100_128 "));
        assert!(seen[0].contains("eleven_multilingual_v2"));
        assert!(seen[1].starts_with("POST /sound-generation?output_format=mp3_44100_128 "));
        assert!(seen[1].contains("\"duration_seconds\":2.5"));
        assert!(seen[2].starts_with("POST /music?output_format=mp3_44100_128 "));
        assert!(seen[2].contains("\"music_length_ms\":10000") && seen[2].contains("\"force_instrumental\":true"));
        assert!(seen.iter().all(|s| s.to_ascii_lowercase().contains("xi-api-key: sk-test")));
    }
    /// Reject invalid audio inputs locally and classify HTTP errors without echoing remote secrets.
    #[test]
    fn invalid_inputs_and_remote_errors_never_expose_credentials() {
        let c = Catalogue::built_in();
        let model = c.audio_model(AssetKind::SoundEffect, None).unwrap();
        for duration in [f64::NAN, f64::INFINITY, -1.0, 0.0, 31.0] {
            assert!(ElevenLabs::default().estimate(&AssetRequest::SoundEffect { model, prompt: "wind", duration_seconds: duration }).is_err());
        }
        let speech = AssetRequest::Speech { model: c.audio_model(AssetKind::Speech, None).unwrap(), text: "hi", voice_id: "../bad" };
        assert!(ElevenLabs::default().generate("sk-test", &speech).is_err());
        let (base, _) = http::tests::serve(|_| {
            vec![(401, "sk-test echoed by remote".into()), (429, "sk-test echoed by remote".into()), (500, "sk-test echoed by remote".into())]
        });
        let provider = ElevenLabs { base };
        let req = AssetRequest::SoundEffect { model, prompt: "wind", duration_seconds: 1.0 };
        for code in [ErrorCode::BadSettings, ErrorCode::OverLimit, ErrorCode::ProgramFailed] {
            let error = provider.generate("sk-test", &req).unwrap_err();
            assert_eq!(error.code, code);
            assert!(!format!("{error:?}").contains("sk-test"));
            assert_eq!(crate::assets::charged_usd(&error), None, "a refused request is not charged");
        }
    }
    /// A body that fails after an accepted request still reports the request's charge.
    #[test]
    fn a_failed_body_after_an_accepted_request_reports_its_charge() {
        let model = Catalogue::built_in().audio_model(AssetKind::SoundEffect, None).unwrap();
        let req = AssetRequest::SoundEffect { model, prompt: "wind", duration_seconds: 2.0 };
        let (base, _) = http::tests::serve(|_| vec![(200, String::new())]);
        let error = ElevenLabs { base }.generate("sk-test", &req).unwrap_err();
        assert_eq!(error.code, ErrorCode::GarbledOutput);
        assert_eq!(crate::assets::charged_usd(&error), Some(req.listed_cost()));
    }
}
