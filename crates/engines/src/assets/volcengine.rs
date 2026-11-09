//! Generated pictures from Volcengine's Ark (火山方舟) over plain HTTPS: the Seedream models.
//! One key (`Authorization: Bearer …`), paid per picture; which models exist and what they cost
//! comes from the asset table (`assets.toml`).

use super::{AssetProvider, AssetRequest, Generated, Shape, SizeBy, agent, download, unexpected};
use craft_anything_evidence::{ErrorCode, ToolError};
use serde_json::{Value, json};
use std::time::Duration;

/// The provider id the asset table names.
pub const ID: &str = "volcengine";
const BASE: &str = "https://ark.cn-beijing.volces.com/api/v3";

/// Volcengine Ark, as an [`AssetProvider`].
pub struct Volcengine {
    base: String,
}

impl Default for Volcengine {
    fn default() -> Volcengine {
        Volcengine { base: BASE.to_string() }
    }
}

impl Volcengine {
    /// The provider answering at `base`, for a test's own server.
    pub fn at(base: impl Into<String>) -> Volcengine {
        Volcengine { base: base.into() }
    }

    fn post(&self, key: &str, path: &str, body: &Value) -> Result<Value, ToolError> {
        let url = format!("{}{path}", self.base.trim_end_matches('/'));
        let resp = agent(Duration::from_secs(10 * 60))
            .post(&url)
            .header("Authorization", &format!("Bearer {key}"))
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(|e| {
                ToolError::new(ErrorCode::EngineMissing, format!("Couldn't reach Volcengine's Ark: {e}"))
                    .with_remediation("Check your internet connection and try again.")
            })?;
        let status = resp.status().as_u16();
        let text = resp.into_body().read_to_string().unwrap_or_default();
        match status {
            200..=299 => {
                serde_json::from_str(&text).map_err(|e| ToolError::new(ErrorCode::GarbledOutput, format!("Volcengine sent an unreadable answer: {e}")))
            }
            401 | 403 => Err(ToolError::new(ErrorCode::BadSettings, "Volcengine didn't accept the key.")
                .with_remediation("Check the Ark API key in Settings → AI → Game assets, and that the model is opened for it.")),
            429 => Err(ToolError::new(ErrorCode::OverLimit, format!("Volcengine throttled the request: {}", reason(&text)))
                .with_remediation("Wait a moment, or raise the model's rate limit in the Ark console.")),
            400 => Err(ToolError::new(ErrorCode::BadSettings, format!("Volcengine refused the request: {}", reason(&text)))),
            _ => Err(ToolError::new(ErrorCode::EngineMissing, format!("Volcengine answered {status}: {}", reason(&text)))),
        }
    }
}

impl AssetProvider for Volcengine {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        "Volcengine (火山方舟)"
    }

    fn keys_page(&self) -> Option<&'static str> {
        Some("https://console.volcengine.com/ark")
    }

    fn key_env(&self) -> Option<&'static str> {
        Some("ARK_API_KEY")
    }

    fn generate(&self, key: &str, req: &AssetRequest) -> Result<Generated, ToolError> {
        let key = key.trim();
        match *req {
            AssetRequest::Image { model, prompt, shape } => {
                let size = volcengine_size(model.size_by, shape).ok_or_else(|| {
                    ToolError::new(ErrorCode::BadSettings, format!("{} isn't a Volcengine picture model.", model.id))
                        .with_remediation("Point the asset table's entry at the provider that runs it.")
                })?;
                let body = json!({
                    "model": model.endpoint,
                    "prompt": prompt,
                    "size": size,
                    "response_format": "url",
                    "watermark": false
                });
                let v = self.post(key, "/images/generations", &body)?;
                let url = v.pointer("/data/0/url").and_then(Value::as_str).filter(|u| !u.is_empty()).ok_or_else(|| unexpected("Volcengine", "picture", &v))?;
                let bytes = download(url)?;
                Ok(Generated { url: url.to_string(), bytes, cost_usd: req.listed_cost() })
            }
            AssetRequest::Model3d { .. } => Err(ToolError::new(ErrorCode::NotSupported, "Volcengine's Ark makes pictures here, not 3D models.")
                .with_remediation("Choose a 3D model generator from another provider in the asset table (fal.ai has one).")),
        }
    }
}

/// How Volcengine is told a picture's shape: the pixel size, `WxH`.
fn volcengine_size(by: SizeBy, shape: Shape) -> Option<&'static str> {
    match by {
        SizeBy::Pixels => Some(match shape {
            Shape::Square => "1024x1024",
            Shape::Wide => "1536x1024",
            Shape::Tall => "1024x1536",
        }),
        SizeBy::ImageSize | SizeBy::AspectRatio => None,
    }
}

/// The reason in Volcengine's error answer (`error.message`), shortened.
fn reason(text: &str) -> String {
    let said = serde_json::from_str::<Value>(text).ok().and_then(|v| {
        v.pointer("/error/message").and_then(Value::as_str).map(str::to_string).or_else(|| v.get("message").and_then(Value::as_str).map(str::to_string))
    });
    said.unwrap_or_else(|| text.chars().take(300).collect()).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assets::{AssetProviders, Catalogue};
    use craft_anything_platform::sync::Mutex;
    use std::io::{BufRead, BufReader, Read, Write};
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

    fn table() -> &'static Catalogue {
        Catalogue::built_in()
    }

    #[test]
    fn a_picture_is_asked_for_by_size_and_downloaded() {
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD.encode(b"PNG");
        let url = format!("data:image/png;base64,{png}");
        let (base, seen) = serve(vec![(
            200,
            json!({"model": "doubao-seedream-4-0-250828", "created": 1, "data": [{"url": url, "size": "1536x1024"}], "usage": {"generated_images": 1}})
                .to_string(),
        )]);
        let model = table().image_model(Some("seedream-4")).unwrap();
        let req = AssetRequest::Image { model, prompt: "a mossy stone", shape: Shape::Wide };
        let made = Volcengine::at(&base).generate("sk-test", &req).unwrap();
        assert_eq!(made.bytes, b"PNG");
        assert_eq!(made.url, url);
        assert!((made.cost_usd - model.cost(Shape::Wide)).abs() < 1e-12);
        let sent = seen.lock().unwrap().join("\n").to_ascii_lowercase();
        assert!(sent.contains("authorization: bearer sk-test"), "{sent}");
        assert!(sent.contains("doubao-seedream-4-0-250828"), "{sent}");
        assert!(sent.contains("1536x1024"), "a wide picture is asked for at 1536x1024: {sent}");
        assert!(sent.contains("mossy stone"), "{sent}");
    }

    #[test]
    fn a_refused_key_names_the_setting_and_a_bad_request_says_why() {
        let model = table().image_model(Some("seedream-4")).unwrap();
        let req = AssetRequest::Image { model, prompt: "x", shape: Shape::Square };
        let (base, _) = serve(vec![(401, json!({"error": {"code": "AuthenticationError", "message": "bad key"}}).to_string())]);
        let err = Volcengine::at(&base).generate("sk-test", &req).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadSettings);
        let (base, _) = serve(vec![(400, json!({"error": {"code": "InvalidParameter", "message": "size is wrong"}}).to_string())]);
        let err = Volcengine::at(&base).generate("sk-test", &req).unwrap_err();
        assert!(err.message.contains("size is wrong"), "{}", err.message);
    }

    #[test]
    fn a_3d_model_request_is_refused_with_what_to_use_instead() {
        let model = table().model_3d(None).unwrap();
        let req = AssetRequest::Model3d { model, image_url: "http://127.0.0.1:9/a.png", textured: false, faces: 500 };
        let err = Volcengine::at("http://127.0.0.1:9").generate("sk-test", &req).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotSupported);
    }

    #[test]
    fn the_table_has_a_seedream_model_at_its_price() {
        let c = table();
        let m = c.image_model(Some("seedream-4")).unwrap();
        assert_eq!((m.provider.as_str(), m.endpoint.as_str(), m.size_by), (ID, "doubao-seedream-4-0-250828", SizeBy::Pixels));
        assert!(m.cost(Shape::Square) > 0.0);
        assert!(AssetProviders::built_in().get(ID).is_some(), "the registry knows Volcengine");
    }
}
