//! Generated pictures from MiniMax's open platform over plain HTTPS (`image-01`): the cheapest
//! picture provider in the asset table. One key (`Authorization: Bearer …`), paid per picture;
//! which models exist and what they cost comes from the asset table (`assets.toml`).

use super::{AssetProvider, AssetRequest, Generated, Shape, SizeBy, agent, download, unexpected};
use craft_anything_evidence::{ErrorCode, ToolError};
use serde_json::{Value, json};
use std::time::Duration;

/// The provider id the asset table names.
pub const ID: &str = "minimax";
const BASE: &str = "https://api.minimax.cn/v1";

/// MiniMax's open platform, as an [`AssetProvider`].
pub struct Minimax {
    base: String,
}

impl Default for Minimax {
    fn default() -> Minimax {
        Minimax { base: BASE.to_string() }
    }
}

impl Minimax {
    /// The provider answering at `base`, for a test's own server.
    pub fn at(base: impl Into<String>) -> Minimax {
        Minimax { base: base.into() }
    }

    fn post(&self, key: &str, path: &str, body: &Value) -> Result<Value, ToolError> {
        let url = format!("{}{path}", self.base.trim_end_matches('/'));
        let resp = agent(Duration::from_secs(10 * 60))
            .post(&url)
            .header("Authorization", &format!("Bearer {key}"))
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(|e| {
                ToolError::new(ErrorCode::EngineMissing, format!("Couldn't reach MiniMax's open platform: {e}"))
                    .with_remediation("Check your internet connection and try again.")
            })?;
        let status = resp.status().as_u16();
        let text = resp.into_body().read_to_string().unwrap_or_default();
        match status {
            200..=299 => {
                let v: Value =
                    serde_json::from_str(&text).map_err(|e| ToolError::new(ErrorCode::GarbledOutput, format!("MiniMax sent an unreadable answer: {e}")))?;
                // MiniMax reports its own errors in `base_resp`, sometimes with a 200 status.
                let code = v.pointer("/base_resp/status_code").and_then(Value::as_i64).unwrap_or(0);
                if code == 0 {
                    return Ok(v);
                }
                let said = v.pointer("/base_resp/status_msg").and_then(Value::as_str).unwrap_or("no reason given").to_string();
                Err(if code == 1004 {
                    ToolError::new(ErrorCode::BadSettings, "MiniMax didn't accept the key.")
                        .with_remediation("Check the MiniMax key in Settings → AI → Game assets.")
                } else if code == 1008 {
                    ToolError::new(ErrorCode::OverLimit, format!("The MiniMax account is out of credit: {said}"))
                        .with_remediation("Add credit at platform.minimax.cn, then try again.")
                } else {
                    ToolError::new(ErrorCode::EngineMissing, format!("MiniMax refused the request ({code}): {said}"))
                })
            }
            401 | 403 => Err(ToolError::new(ErrorCode::BadSettings, "MiniMax didn't accept the key.")
                .with_remediation("Check the MiniMax key in Settings → AI → Game assets.")),
            _ => Err(ToolError::new(ErrorCode::EngineMissing, format!("MiniMax answered {status}: {}", text.chars().take(300).collect::<String>()))),
        }
    }
}

impl AssetProvider for Minimax {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        "MiniMax"
    }

    fn keys_page(&self) -> Option<&'static str> {
        Some("https://platform.minimax.cn/user-center/access-key")
    }

    fn key_env(&self) -> Option<&'static str> {
        Some("MINIMAX_API_KEY")
    }

    fn generate(&self, key: &str, req: &AssetRequest) -> Result<Generated, ToolError> {
        let key = key.trim();
        match *req {
            AssetRequest::Image { model, prompt, shape } => {
                let ratio = minimax_ratio(model.size_by, shape).ok_or_else(|| {
                    ToolError::new(ErrorCode::BadSettings, format!("{} isn't a MiniMax picture model.", model.id))
                        .with_remediation("Point the asset table's entry at the provider that runs it.")
                })?;
                let body = json!({
                    "model": model.endpoint,
                    "prompt": prompt,
                    "aspect_ratio": ratio,
                    "response_format": "url",
                    "n": 1,
                    "aigc_watermark": false
                });
                let v = self.post(key, "/image_generation", &body)?;
                let url = v.pointer("/data/image_urls/0").and_then(Value::as_str).filter(|u| !u.is_empty());
                let Some(url) = url else {
                    let failed = v.pointer("/metadata/failed_count").and_then(Value::as_i64).unwrap_or(0);
                    if failed > 0 {
                        return Err(ToolError::new(ErrorCode::ProgramFailed, "MiniMax's safety check blocked that picture.")
                            .with_remediation("Describe it differently."));
                    }
                    return Err(unexpected("MiniMax", "picture", &v));
                };
                let bytes = download(url)?;
                Ok(Generated { url: url.to_string(), bytes, cost_usd: req.listed_cost() })
            }
            AssetRequest::Model3d { .. } => Err(ToolError::new(ErrorCode::NotSupported, "MiniMax makes pictures, not 3D models.")
                .with_remediation("Choose a 3D model generator from another provider in the asset table (fal.ai has one).")),
        }
    }
}

/// How MiniMax is told a picture's shape: the aspect ratio.
fn minimax_ratio(by: SizeBy, shape: Shape) -> Option<&'static str> {
    match by {
        SizeBy::AspectRatio => Some(match shape {
            Shape::Square => "1:1",
            Shape::Wide => "16:9",
            Shape::Tall => "9:16",
        }),
        SizeBy::ImageSize | SizeBy::Pixels => None,
    }
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
    fn a_picture_is_asked_for_by_ratio_and_downloaded() {
        use base64::Engine as _;
        let png = base64::engine::general_purpose::STANDARD.encode(b"PNG");
        let url = format!("data:image/png;base64,{png}");
        let (base, seen) = serve(vec![(200, json!({"data": {"image_urls": [url]}, "metadata": {"success_count": 1, "failed_count": 0}, "base_resp": {"status_code": 0, "status_msg": "success"}}).to_string())]);
        let model = table().image_model(Some("minimax-image-01")).unwrap();
        let req = AssetRequest::Image { model, prompt: "a mossy stone", shape: Shape::Wide };
        let made = Minimax::at(&base).generate("sk-test", &req).unwrap();
        assert_eq!(made.bytes, b"PNG");
        assert_eq!(made.url, url);
        assert!((made.cost_usd - model.cost(Shape::Wide)).abs() < 1e-12);
        let sent = seen.lock().unwrap().join("\n").to_ascii_lowercase();
        assert!(sent.contains("authorization: bearer sk-test"), "{sent}");
        assert!(sent.contains("image-01"), "{sent}");
        assert!(sent.contains("16:9"), "a wide picture is asked for at 16:9: {sent}");
        assert!(sent.contains("mossy stone"), "{sent}");
    }

    #[test]
    fn a_refused_key_names_the_setting_and_a_blocked_picture_says_so() {
        let key_err = json!({"base_resp": {"status_code": 1004, "status_msg": "invalid api key"}}).to_string();
        let (base, _) = serve(vec![(200, key_err)]);
        let model = table().image_model(Some("minimax-image-01")).unwrap();
        let req = AssetRequest::Image { model, prompt: "x", shape: Shape::Square };
        let err = Minimax::at(&base).generate("sk-test", &req).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadSettings);
        assert!(err.remediation.is_some());
        let blocked =
            json!({"data": {"image_urls": []}, "metadata": {"success_count": 0, "failed_count": 1}, "base_resp": {"status_code": 0, "status_msg": "success"}})
                .to_string();
        let (base, _) = serve(vec![(200, blocked)]);
        let err = Minimax::at(&base).generate("sk-test", &req).unwrap_err();
        assert_eq!(err.code, ErrorCode::ProgramFailed);
    }

    #[test]
    fn a_3d_model_request_is_refused_with_what_to_use_instead() {
        let model = table().model_3d(None).unwrap();
        let req = AssetRequest::Model3d { model, image_url: "http://127.0.0.1:9/a.png", textured: false, faces: 500 };
        let err = Minimax::at("http://127.0.0.1:9").generate("sk-test", &req).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotSupported);
        assert!(err.remediation.is_some_and(|r| r.contains("fal")));
    }

    #[test]
    fn the_table_has_a_minimax_model_at_its_published_price() {
        let c = table();
        let m = c.image_model(Some("minimax-image-01")).unwrap();
        assert_eq!((m.provider.as_str(), m.endpoint.as_str(), m.size_by), (ID, "image-01", SizeBy::AspectRatio));
        assert!(m.cost(Shape::Square) > 0.0);
        assert!(AssetProviders::built_in().get(ID).is_some(), "the registry knows MiniMax");
    }
}
