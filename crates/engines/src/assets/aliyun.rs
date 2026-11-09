//! Generated pictures from Alibaba Cloud's Model Studio (阿里云百炼) over plain HTTPS:
//! Z-Image-Turbo and the Wanxiang (万相) models it runs. One key (`Authorization: Bearer …`),
//! paid per picture; which models exist and what they cost comes from the asset table
//! (`assets.toml`).

use super::{AssetProvider, AssetRequest, Generated, Shape, SizeBy, agent, download, unexpected};
use craft_anything_evidence::{ErrorCode, ToolError};
use serde_json::{Value, json};
use std::time::Duration;

/// The provider id the asset table names.
pub const ID: &str = "aliyun";
const BASE: &str = "https://dashscope.aliyuncs.com/api/v1";

/// Alibaba Cloud Model Studio, as an [`AssetProvider`].
pub struct Aliyun {
    base: String,
}

impl Default for Aliyun {
    fn default() -> Aliyun {
        Aliyun { base: BASE.to_string() }
    }
}

impl Aliyun {
    /// The provider answering at `base`, for a test's own server.
    pub fn at(base: impl Into<String>) -> Aliyun {
        Aliyun { base: base.into() }
    }

    fn post(&self, key: &str, path: &str, body: &Value) -> Result<Value, ToolError> {
        let url = format!("{}{path}", self.base.trim_end_matches('/'));
        let resp = agent(Duration::from_secs(10 * 60))
            .post(&url)
            .header("Authorization", &format!("Bearer {key}"))
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(|e| {
                ToolError::new(ErrorCode::EngineMissing, format!("Couldn't reach Alibaba Cloud: {e}"))
                    .with_remediation("Check your internet connection and try again.")
            })?;
        let status = resp.status().as_u16();
        let text = resp.into_body().read_to_string().unwrap_or_default();
        match status {
            200..=299 => {
                serde_json::from_str(&text).map_err(|e| ToolError::new(ErrorCode::GarbledOutput, format!("Alibaba Cloud sent an unreadable answer: {e}")))
            }
            401 | 403 => Err(ToolError::new(ErrorCode::BadSettings, "Alibaba Cloud didn't accept the key.")
                .with_remediation("Check the DashScope key in Settings → AI → Game assets.")),
            429 => Err(ToolError::new(ErrorCode::OverLimit, format!("Alibaba Cloud throttled the request: {}", reason(&text)))
                .with_remediation("Wait a moment, or raise the model's rate limit in the Bailian console.")),
            400 => Err(ToolError::new(ErrorCode::BadSettings, format!("Alibaba Cloud refused the request: {}", reason(&text)))),
            _ => Err(ToolError::new(ErrorCode::EngineMissing, format!("Alibaba Cloud answered {status}: {}", reason(&text)))),
        }
    }
}

impl AssetProvider for Aliyun {
    fn id(&self) -> &'static str {
        ID
    }

    fn name(&self) -> &'static str {
        "Alibaba Cloud (阿里云百炼)"
    }

    fn keys_page(&self) -> Option<&'static str> {
        Some("https://bailian.console.aliyun.com/")
    }

    fn key_env(&self) -> Option<&'static str> {
        Some("DASHSCOPE_API_KEY")
    }

    fn generate(&self, key: &str, req: &AssetRequest) -> Result<Generated, ToolError> {
        let key = key.trim();
        match *req {
            AssetRequest::Image { model, prompt, shape } => {
                let size = aliyun_size(model.size_by, shape).ok_or_else(|| {
                    ToolError::new(ErrorCode::BadSettings, format!("{} isn't an Alibaba Cloud picture model.", model.id))
                        .with_remediation("Point the asset table's entry at the provider that runs it.")
                })?;
                let body = json!({
                    "model": model.endpoint,
                    "input": {"messages": [{"role": "user", "content": [{"text": prompt}]}]},
                    "parameters": {"size": size, "n": 1, "prompt_extend": false, "watermark": false}
                });
                let v = self.post(key, "/services/aigc/multimodal-generation/generation", &body)?;
                let url = image_url(&v).ok_or_else(|| unexpected("Alibaba Cloud", "picture", &v))?;
                let bytes = download(url)?;
                Ok(Generated { url: url.to_string(), bytes, cost_usd: req.listed_cost() })
            }
            AssetRequest::Model3d { .. } => Err(ToolError::new(ErrorCode::NotSupported, "Alibaba Cloud's picture models make pictures, not 3D models.")
                .with_remediation("Choose a 3D model generator from another provider in the asset table (fal.ai has one).")),
        }
    }
}

/// How Alibaba Cloud is told a picture's shape: the pixel size, `W*H`.
fn aliyun_size(by: SizeBy, shape: Shape) -> Option<&'static str> {
    match by {
        SizeBy::Pixels => Some(match shape {
            Shape::Square => "1024*1024",
            Shape::Wide => "1280*720",
            Shape::Tall => "720*1280",
        }),
        SizeBy::ImageSize | SizeBy::AspectRatio => None,
    }
}

/// The picture's address in the answer (`output.choices[0].message.content[].image`).
fn image_url(v: &Value) -> Option<&str> {
    v.pointer("/output/choices/0/message/content")?.as_array()?.iter().find_map(|part| part.get("image").and_then(Value::as_str)).filter(|u| !u.is_empty())
}

/// The reason in Alibaba Cloud's error answer (`message`, or `code`), shortened.
fn reason(text: &str) -> String {
    let said = serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_string).or_else(|| v.get("code").and_then(Value::as_str).map(str::to_string)));
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
        let (base, seen) =
            serve(vec![(200, json!({"output": {"choices": [{"message": {"content": [{"image": url}]}}]}, "usage": {"image_count": 1}}).to_string())]);
        let model = table().image_model(Some("z-image-turbo")).unwrap();
        let req = AssetRequest::Image { model, prompt: "a mossy stone", shape: Shape::Wide };
        let made = Aliyun::at(&base).generate("sk-test", &req).unwrap();
        assert_eq!(made.bytes, b"PNG");
        assert_eq!(made.url, url);
        assert!((made.cost_usd - model.cost(Shape::Wide)).abs() < 1e-12);
        let sent = seen.lock().unwrap().join("\n").to_ascii_lowercase();
        assert!(sent.contains("authorization: bearer sk-test"), "{sent}");
        assert!(sent.contains("z-image-turbo"), "{sent}");
        assert!(sent.contains("1280*720"), "a wide picture is asked for at 1280*720: {sent}");
        assert!(sent.contains("mossy stone"), "{sent}");
    }

    #[test]
    fn a_refused_key_names_the_setting_and_a_bad_request_says_why() {
        let model = table().image_model(Some("z-image-turbo")).unwrap();
        let req = AssetRequest::Image { model, prompt: "x", shape: Shape::Square };
        let (base, _) = serve(vec![(401, json!({"code": "InvalidApiKey", "message": "bad key"}).to_string())]);
        let err = Aliyun::at(&base).generate("sk-test", &req).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadSettings);
        let (base, _) = serve(vec![(400, json!({"code": "InvalidParameter", "message": "size is wrong"}).to_string())]);
        let err = Aliyun::at(&base).generate("sk-test", &req).unwrap_err();
        assert!(err.message.contains("size is wrong"), "{}", err.message);
    }

    #[test]
    fn a_3d_model_request_is_refused_with_what_to_use_instead() {
        let model = table().model_3d(None).unwrap();
        let req = AssetRequest::Model3d { model, image_url: "http://127.0.0.1:9/a.png", textured: false, faces: 500 };
        let err = Aliyun::at("http://127.0.0.1:9").generate("sk-test", &req).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotSupported);
    }

    #[test]
    fn the_table_has_an_aliyun_model_at_its_published_price() {
        let c = table();
        let m = c.image_model(Some("z-image-turbo")).unwrap();
        assert_eq!((m.provider.as_str(), m.endpoint.as_str(), m.size_by), (ID, "z-image-turbo", SizeBy::Pixels));
        assert!(m.cost(Shape::Square) > 0.0);
        assert!(AssetProviders::built_in().get(ID).is_some(), "the registry knows Alibaba Cloud");
    }
}
