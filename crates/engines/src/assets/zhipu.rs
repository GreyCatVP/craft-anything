//! Generated pictures from Zhipu's open platform (智谱开放平台, BigModel) over plain HTTPS:
//! CogView-4 and CogView-3-Flash. One key (`Authorization: Bearer …`), paid per picture; which
//! models exist and what they cost comes from the asset table (`assets.toml`).

use super::{AssetProvider, AssetRequest, Generated, Shape, SizeBy, agent, download, unexpected};
use craft_anything_evidence::{ErrorCode, ToolError};
use serde_json::{Value, json};
use std::time::Duration;

/// The provider id the asset table names.
pub const ID: &str = "zhipu";
const BASE: &str = "https://open.bigmodel.cn/api/paas/v4";

/// Zhipu's open platform, as an [`AssetProvider`].
pub struct Zhipu {
    base: String,
}

impl Default for Zhipu {
    fn default() -> Zhipu {
        Zhipu { base: BASE.to_string() }
    }
}

impl Zhipu {
    /// The provider answering at `base`, for a test's own server.
    pub fn at(base: impl Into<String>) -> Zhipu {
        Zhipu { base: base.into() }
    }

    fn post(&self, key: &str, path: &str, body: &Value) -> Result<Value, ToolError> {
        let url = format!("{}{path}", self.base.trim_end_matches('/'));
        let resp = agent(Duration::from_secs(10 * 60))
            .post(&url)
            .header("Authorization", &format!("Bearer {key}"))
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(|e| {
                ToolError::new(ErrorCode::EngineMissing, format!("Couldn't reach Zhipu's open platform: {e}"))
                    .with_remediation("Check your internet connection and try again.")
            })?;
        let status = resp.status().as_u16();
        let text = resp.into_body().read_to_string().unwrap_or_default();
        match status {
            200..=299 => serde_json::from_str(&text).map_err(|e| ToolError::new(ErrorCode::GarbledOutput, format!("Zhipu sent an unreadable answer: {e}"))),
            401 | 403 => Err(ToolError::new(ErrorCode::BadSettings, "Zhipu's open platform didn't accept the key.")
                .with_remediation("Check the Zhipu key in Settings → AI → Game assets.")),
            402 | 429 => Err(ToolError::new(ErrorCode::OverLimit, format!("Zhipu's open platform refused the request: {}", reason(&text)))
                .with_remediation("Check the account's balance and rate limits at open.bigmodel.cn, then try again.")),
            _ => Err(ToolError::new(ErrorCode::EngineMissing, format!("Zhipu's open platform answered {status}: {}", reason(&text)))),
        }
    }
}

impl AssetProvider for Zhipu {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        "Zhipu (智谱)"
    }

    fn keys_page(&self) -> Option<&'static str> {
        Some("https://open.bigmodel.cn/usercenter/apikeys")
    }

    fn key_env(&self) -> Option<&'static str> {
        Some("ZHIPU_API_KEY")
    }

    fn generate(&self, key: &str, req: &AssetRequest) -> Result<Generated, ToolError> {
        let key = key.trim();
        match *req {
            AssetRequest::Image { model, prompt, shape } => {
                let size = zhipu_size(model.size_by, shape).ok_or_else(|| {
                    ToolError::new(ErrorCode::BadSettings, format!("{} isn't a Zhipu picture model.", model.id))
                        .with_remediation("Point the asset table's entry at the provider that runs it.")
                })?;
                // `watermark_enabled` off: an asset of one's own game shouldn't carry a watermark.
                let body = json!({"model": model.endpoint, "prompt": prompt, "size": size, "quality": "standard", "watermark_enabled": false});
                let v = self.post(key, "/images/generations", &body)?;
                if let Some(why) = filtered(&v) {
                    return Err(ToolError::new(ErrorCode::ProgramFailed, format!("Zhipu's safety check blocked that picture: {why}"))
                        .with_remediation("Describe it differently."));
                }
                let url = v.pointer("/data/0/url").and_then(Value::as_str).filter(|u| !u.is_empty()).ok_or_else(|| unexpected("Zhipu", "picture", &v))?;
                let bytes = download(url)?;
                Ok(Generated { url: url.to_string(), bytes, cost_usd: req.listed_cost() })
            }
            AssetRequest::Model3d { .. } => Err(ToolError::new(ErrorCode::NotSupported, "Zhipu's open platform makes pictures, not 3D models.")
                .with_remediation("Choose a 3D model generator from another provider in the asset table (fal.ai has one).")),
        }
    }
}

/// How Zhipu is told a picture's shape: the pixel size, `WxH`.
fn zhipu_size(by: SizeBy, shape: Shape) -> Option<&'static str> {
    match by {
        SizeBy::Pixels => Some(match shape {
            Shape::Square => "1024x1024",
            Shape::Wide => "1344x768",
            Shape::Tall => "768x1344",
        }),
        SizeBy::ImageSize | SizeBy::AspectRatio => None,
    }
}

/// The safety filter's own words, when it blocked the picture.
fn filtered(v: &Value) -> Option<String> {
    let said = v.get("content_filter")?;
    match said {
        Value::Null => None,
        Value::String(s) if s.trim().is_empty() => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(true) => Some("it was refused".to_string()),
        other => Some(other.to_string().chars().take(200).collect()),
    }
}

/// The reason in Zhipu's error answer (`error.message`), or a short version of it.
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
        let (base, seen) = serve(vec![(200, json!({"data": [{"url": url}]}).to_string())]);
        let model = table().image_model(Some("cogview-4")).unwrap();
        let req = AssetRequest::Image { model, prompt: "a mossy stone", shape: Shape::Wide };
        let made = Zhipu::at(&base).generate("sk-test", &req).unwrap();
        assert_eq!(made.bytes, b"PNG");
        assert_eq!(made.url, url);
        assert!((made.cost_usd - model.cost(Shape::Wide)).abs() < 1e-12);
        let sent = seen.lock().unwrap().join("\n").to_ascii_lowercase();
        assert!(sent.contains("authorization: bearer sk-test"), "{sent}");
        assert!(sent.contains("cogview-4-250304"), "{sent}");
        assert!(sent.contains("1344x768"), "a wide picture is asked for at the wide size: {sent}");
        assert!(sent.contains("mossy stone"), "{sent}");
    }

    #[test]
    fn a_refused_key_names_the_setting_and_a_blocked_picture_says_so() {
        let (base, _seen) = serve(vec![(401, json!({"error": {"message": "bad key"}}).to_string())]);
        let model = table().image_model(Some("cogview-4")).unwrap();
        let req = AssetRequest::Image { model, prompt: "x", shape: Shape::Square };
        let err = Zhipu::at(&base).generate("sk-test", &req).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadSettings);
        assert!(err.remediation.is_some());
        let (base, _) = serve(vec![(200, json!({"data": [{"url": "http://127.0.0.1:9/x"}], "content_filter": "blocked"}).to_string())]);
        let err = Zhipu::at(&base).generate("sk-test", &req).unwrap_err();
        assert_eq!(err.code, ErrorCode::ProgramFailed);
        assert!(err.message.contains("blocked"), "{}", err.message);
    }

    #[test]
    fn a_3d_model_request_is_refused_with_what_to_use_instead() {
        let model = table().model_3d(None).unwrap();
        let req = AssetRequest::Model3d { model, image_url: "http://127.0.0.1:9/a.png", textured: false, faces: 500 };
        let err = Zhipu::at("http://127.0.0.1:9").generate("sk-test", &req).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotSupported);
        assert!(err.remediation.is_some_and(|r| r.contains("fal")));
    }

    #[test]
    fn zhipu_sizes_come_from_the_shape() {
        assert_eq!(zhipu_size(SizeBy::Pixels, Shape::Square), Some("1024x1024"));
        assert_eq!(zhipu_size(SizeBy::Pixels, Shape::Wide), Some("1344x768"));
        assert_eq!(zhipu_size(SizeBy::Pixels, Shape::Tall), Some("768x1344"));
        assert_eq!(zhipu_size(SizeBy::ImageSize, Shape::Square), None);
    }

    #[test]
    fn the_table_has_a_zhipu_model_at_its_published_price() {
        let c = table();
        let cog = c.image_model(Some("cogview-4")).unwrap();
        assert_eq!((cog.provider.as_str(), cog.endpoint.as_str(), cog.size_by), (ID, "cogview-4-250304", SizeBy::Pixels));
        assert!(cog.cost(Shape::Square) > 0.0);
        let flash = c.image_model(Some("cogview-3-flash")).unwrap();
        assert_eq!(flash.cost(Shape::Wide), 0.0);
        assert!(AssetProviders::built_in().get(ID).is_some(), "the registry knows Zhipu");
    }
}
