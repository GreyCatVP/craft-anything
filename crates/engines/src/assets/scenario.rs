//! Scenario asynchronous image and mesh generation with a mandatory cost preflight.
use super::{AssetProvider, AssetRequest, Generated, Shape, http};
use craft_anything_evidence::{ErrorCode, ToolError};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub const ID: &str = "scenario";
/// Starter's $15 / 1500 CU. This is a budgeting conversion, not an invoice.
const USD_PER_CU: f64 = 0.01;
pub struct Scenario {
    base: String,
    timeout: Duration,
    poll_interval: Duration,
}
impl Default for Scenario {
    fn default() -> Self {
        Self { base: "https://api.cloud.scenario.com/v1".into(), timeout: Duration::from_secs(600), poll_interval: Duration::from_secs(2) }
    }
}

fn request<'a>(req: &'a AssetRequest<'_>) -> Result<(&'a str, Value), ToolError> {
    let (model, body) = match *req {
        AssetRequest::Image { model, prompt, shape } => {
            http::text(prompt, 10_000)?;
            let ratio = match shape {
                Shape::Square => "1:1",
                Shape::Wide => "16:9",
                Shape::Tall => "9:16",
            };
            (model.endpoint.as_str(), json!({"prompt":prompt,"aspectRatio":ratio,"resolution":"1K","numOutputs":1}))
        }
        AssetRequest::Model3d { model, image_url, textured, faces } => {
            if !model.faces().contains(&faces) {
                return Err(http::bad("The requested polygon count is outside this model's range."));
            }
            if image_url.starts_with("asset_") {
                http::id(image_url)?;
            } else if image_url.starts_with("data:image/") {
                if image_url.len() > (28 << 20) || !image_url.contains(";base64,") {
                    return Err(http::bad("The Scenario input image is too large or malformed."));
                }
            } else {
                http::media_url(image_url)?;
            }
            (
                model.endpoint.as_str(),
                json!({"image":[image_url],"shouldTexture":textured,"shouldRemesh":true,"targetPolycount":faces,"topology":"triangle","enablePbr":textured,"textureResolution":"2k"}),
            )
        }
        _ => return Err(http::bad("Scenario does not support this asset request.")),
    };
    http::id(model)?;
    if req.provider() != ID || !req.listed_cost().is_finite() || req.listed_cost() <= 0.0 {
        return Err(http::bad("The Scenario model needs a positive spending ceiling."));
    }
    Ok((model, body))
}

fn remaining(start: Instant, timeout: Duration) -> Result<Duration, ToolError> {
    timeout.checked_sub(start.elapsed()).filter(|v| !v.is_zero()).ok_or_else(|| {
        ToolError::new(ErrorCode::ProgramFailed, "Scenario generation timed out. The remote job may still finish and incur a charge.")
            .with_remediation("Check the job in Scenario before generating again.")
    })
}
impl Scenario {
    fn call(&self, auth: &str, path: &str, body: Option<&Value>, timeout: Duration) -> Result<Value, ToolError> {
        let agent = http::agent(timeout);
        let url = format!("{}/{path}", self.base);
        let response = if let Some(body) = body {
            agent.post(url).header("Authorization", auth).header("Content-Type", "application/json").send(body.to_string())
        } else {
            agent.get(url).header("Authorization", auth).call()
        }
        .map_err(|_| http::network())?;
        http::status(response.status().as_u16())?;
        http::json(response.into_body())
    }
}
impl AssetProvider for Scenario {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Scenario"
    }
    fn key_env(&self) -> Option<&'static str> {
        Some("SCENARIO_API_KEY")
    }
    fn keys_page(&self) -> Option<&'static str> {
        Some("https://app.scenario.com")
    }
    fn key_hint(&self) -> &'static str {
        "API key:API secret"
    }
    fn estimate(&self, req: &AssetRequest) -> Result<f64, ToolError> {
        request(req)?;
        Ok(req.listed_cost())
    }
    fn generate(&self, key: &str, req: &AssetRequest) -> Result<Generated, ToolError> {
        let (model, body) = request(req)?;
        let key = http::key(key)?;
        if !key.split_once(':').is_some_and(|(k, s)| !k.is_empty() && !s.is_empty()) {
            return Err(http::bad("Enter the Scenario credential as API key:API secret."));
        }
        use base64::Engine as _;
        let auth = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(key));
        let start = Instant::now();
        let path = format!("generate/custom/{model}");
        let quote = self.call(&auth, &format!("{path}?dryRun=true"), Some(&body), remaining(start, self.timeout)?)?;
        // Fail closed if a server ignores dryRun: never follow it with another paid request.
        if quote.get("job").is_some() || quote.get("jobId").is_some() {
            return Err(http::garbled("Scenario returned a job during the price check; check your account before trying again."));
        }
        let cost = quote
            .get("creativeUnitsCost")
            .and_then(Value::as_f64)
            .filter(|c| c.is_finite() && *c > 0.0)
            .ok_or_else(|| http::garbled("Scenario did not provide a valid cost quote; generation was not started."))?
            * USD_PER_CU;
        if !cost.is_finite() || cost > req.listed_cost() {
            return Err(ToolError::new(ErrorCode::OverLimit, "Scenario's current quote exceeds this model's spending ceiling; generation was not started."));
        }
        let submitted = self.call(&auth, &path, Some(&body), remaining(start, self.timeout)?)?;
        let job_id = submitted.pointer("/job/jobId").and_then(Value::as_str).ok_or_else(|| http::garbled("Scenario returned no job identifier."))?;
        http::id(job_id)?;
        loop {
            let job = self.call(&auth, &format!("jobs/{job_id}"), None, remaining(start, self.timeout)?)?;
            match job.pointer("/job/status").and_then(Value::as_str) {
                Some("success") => {
                    let asset_id =
                        job.pointer("/job/metadata/assetIds/0").and_then(Value::as_str).ok_or_else(|| http::garbled("Scenario completed without an asset."))?;
                    http::id(asset_id)?;
                    let asset = self.call(&auth, &format!("assets/{asset_id}"), None, remaining(start, self.timeout)?)?;
                    let url = asset
                        .pointer("/asset/url")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| http::garbled("Scenario returned no asset download address."))?
                        .to_string();
                    let bytes = http::download(&url, remaining(start, self.timeout)?)?;
                    return Ok(Generated { url, bytes, cost_usd: cost });
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::Catalogue;

    fn image(provider: &Scenario) -> Result<Generated, ToolError> {
        provider.generate(
            "test-key:test-secret",
            &AssetRequest::Image {
                model: Catalogue::built_in().image_model(Some("scenario-gemini-3-1")).unwrap(),
                prompt: "a wooden crate",
                shape: Shape::Wide,
            },
        )
    }
    #[test]
    fn price_submit_poll_resolve_and_download_keep_auth_off_media_requests() {
        let (base, seen) = http::tests::serve(|base| {
            vec![
                (269, json!({"creativeUnitsCost":19}).to_string()),
                (200, json!({"job":{"jobId":"job_test"}}).to_string()),
                (200, json!({"job":{"status":"in-progress"}}).to_string()),
                (200, json!({"job":{"status":"success","metadata":{"assetIds":["asset_test"]}}}).to_string()),
                (200, json!({"asset":{"url":format!("{base}/media.png")}}).to_string()),
                (200, "PNG fake".into()),
            ]
        });
        let provider = Scenario { base, poll_interval: Duration::ZERO, ..Scenario::default() };
        let result = image(&provider).unwrap();
        assert_eq!(result.bytes, b"PNG fake");
        assert_eq!(result.cost_usd, 0.19);
        let seen = seen.lock().unwrap();
        assert!(seen[0].starts_with("POST /generate/custom/model_google-gemini-3-1-flash?dryRun=true "));
        assert!(seen[1].starts_with("POST /generate/custom/model_google-gemini-3-1-flash "));
        assert!(seen[1].contains("\"aspectRatio\":\"16:9\"") && seen[1].contains("\"numOutputs\":1"));
        assert!(seen[2].starts_with("GET /jobs/job_test "));
        assert!(seen[4].starts_with("GET /assets/asset_test "));
        assert!(seen[0].to_ascii_lowercase().contains("authorization: basic "));
        assert!(!seen[5].to_ascii_lowercase().contains("authorization"));
    }
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
        let req = AssetRequest::Model3d {
            model: Catalogue::built_in().model_3d(Some("scenario-meshy")).unwrap(),
            image_url: "https://example.com/input.png",
            textured: true,
            faces: 3000,
        };
        let provider = Scenario { base, ..Scenario::default() };
        assert_eq!(provider.generate("test-key:test-secret", &req).unwrap().bytes, b"glTF fake");
        let seen = seen.lock().unwrap();
        assert!(seen[1].contains("\"image\":[\"https://example.com/input.png\"]"));
        assert!(seen[1].contains("\"targetPolycount\":3000") && seen[1].contains("\"shouldTexture\":true"));
    }
    #[test]
    fn unsafe_or_missing_quotes_never_submit_a_generation() {
        for quote in [
            json!({"creativeUnitsCost":101}),
            json!({"creativeUnitsCost":0}),
            json!({"creativeUnitsCost":-1}),
            json!({}),
            json!({"creativeUnitsCost":1,"job":{"jobId":"unexpected"}}),
        ] {
            let (base, seen) = http::tests::serve(|_| vec![(269, quote.to_string())]);
            assert!(image(&Scenario { base, ..Scenario::default() }).is_err());
            assert_eq!(seen.lock().unwrap().len(), 1);
        }
    }
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
        }
        assert!(image(&Scenario { timeout: Duration::ZERO, ..Scenario::default() }).is_err());
    }
}
