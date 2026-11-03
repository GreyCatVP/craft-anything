//! Bounded HTTP shared by the asset adapters. Never include remote bodies or URLs in errors.
use craft_anything_evidence::{ErrorCode, ToolError};
use std::{io::Read, time::Duration};

pub(super) const MAX_BYTES: u64 = 200 << 20;
pub(super) const MAX_JSON: u64 = 2 << 20;
/// The largest page of a model list that is read. A page carries each model's OpenAPI document (a
/// page of ten is about 100 KB), which a model with long lists of values can make several times
/// larger; this keeps room for that and still stops a server that never ends.
pub(super) const MAX_LIST_PAGE: u64 = 16 << 20;

/// Build a client with a request deadline, redirects disabled, and explicit status handling.
pub(super) fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .user_agent("CraftAnything-dev")
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_global(Some(timeout))
        .build()
        .new_agent()
}

/// Classify a locally supplied message as invalid provider settings.
pub(super) fn bad(message: &str) -> ToolError {
    ToolError::new(ErrorCode::BadSettings, message)
}
/// Classify a locally supplied message as an unreadable provider response.
pub(super) fn garbled(message: &str) -> ToolError {
    ToolError::new(ErrorCode::GarbledOutput, message)
}
/// Report a transport failure without exposing request details or implying no charge occurred.
pub(super) fn network() -> ToolError {
    ToolError::new(ErrorCode::EngineMissing, "The asset service could not be reached or its request timed out.")
        .with_remediation("Check your connection and provider account before retrying; an accepted job may already have been charged.")
}
/// `text` for a query string: everything but letters, digits, `-_.~` and `/` as `%XX`.
pub(super) fn encode(text: &str) -> String {
    use std::fmt::Write;
    text.bytes().fold(String::with_capacity(text.len()), |mut out, b| {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
        out
    })
}
/// Accept only bounded, nonempty identifiers safe to embed in provider URL paths.
pub(super) fn id(value: &str) -> Result<(), ToolError> {
    if value.is_empty() || value.len() > 200 || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        return Err(bad("The provider model, voice, job or asset identifier is invalid."));
    }
    Ok(())
}
/// Trim a credential and reject empty, oversized, non-ASCII, or control-character values.
pub(super) fn key(value: &str) -> Result<&str, ToolError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 4096 || !value.is_ascii() || value.chars().any(char::is_control) {
        return Err(bad("Enter a valid asset provider credential in Settings."));
    }
    Ok(value)
}
/// Require nonblank text within the model's limit measured in Unicode scalar values.
pub(super) fn text(value: &str, limit: usize) -> Result<(), ToolError> {
    if value.trim().is_empty() || value.chars().count() > limit {
        return Err(bad("The asset text is empty or exceeds this model's length limit."));
    }
    Ok(())
}
/// Require a finite duration within the model's inclusive range.
pub(super) fn duration(value: f64, min: f64, max: f64) -> Result<(), ToolError> {
    if !value.is_finite() || value < min || value > max {
        return Err(bad("The requested audio duration is outside this model's supported range."));
    }
    Ok(())
}
/// Accept successful statuses and map provider failures without reading remote error bodies.
pub(super) fn status(status: u16) -> Result<(), ToolError> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(bad("The asset provider refused the credential or this account does not have access to the model.")
            .with_remediation("Check the key, its permissions and the provider subscription in Settings.")),
        402 | 429 => Err(ToolError::new(ErrorCode::OverLimit, "The asset provider's credit or rate limit was reached.")
            .with_remediation("Check the provider's balance or retry after its rate limit resets.")),
        _ => Err(ToolError::new(ErrorCode::ProgramFailed, format!("The asset provider returned HTTP {status}."))),
    }
}
/// Read a nonempty body, stopping after one byte beyond the size limit to detect overflow.
pub(super) fn bytes(body: ureq::Body, limit: u64) -> Result<Vec<u8>, ToolError> {
    let mut bytes = Vec::new();
    body.into_reader().take(limit + 1).read_to_end(&mut bytes).map_err(|_| network())?;
    if bytes.len() as u64 > limit {
        return Err(ToolError::new(ErrorCode::OverLimit, "The asset provider's response is too large."));
    }
    if bytes.is_empty() {
        return Err(garbled("The asset provider returned an empty file."));
    }
    Ok(bytes)
}
/// Parse a size-bounded JSON response without including its contents in parse errors.
pub(super) fn json(body: ureq::Body) -> Result<serde_json::Value, ToolError> {
    json_up_to(body, MAX_JSON)
}
/// [`json`] with a size limit of the caller's choosing, for the few answers that are meant to be
/// larger than [`MAX_JSON`] (a page of a model list: [`MAX_LIST_PAGE`]).
pub(super) fn json_up_to(body: ureq::Body, limit: u64) -> Result<serde_json::Value, ToolError> {
    serde_json::from_slice(&bytes(body, limit)?).map_err(|_| garbled("The asset provider returned unreadable JSON."))
}
/// Require HTTPS without embedded credentials, allowing loopback HTTP only in tests.
pub(super) fn media_url(url: &str) -> Result<(), ToolError> {
    let uri: ureq::http::Uri = url.parse().map_err(|_| garbled("The asset provider returned an invalid download address."))?;
    let https = uri.scheme_str() == Some("https");
    let local_test = cfg!(test) && uri.scheme_str() == Some("http") && uri.host() == Some("127.0.0.1");
    if (!https && !local_test) || uri.authority().is_none_or(|a| a.as_str().contains('@')) {
        return Err(garbled("Asset downloads require an HTTPS address without embedded credentials."));
    }
    Ok(())
}
/// Download bounded media from a validated address without forwarding provider credentials.
pub(super) fn download(url: &str, timeout: Duration) -> Result<Vec<u8>, ToolError> {
    media_url(url)?;
    // A separate agent and no auth headers: credentials never reach the media host.
    let response = agent(timeout).get(url).call().map_err(|_| network())?;
    status(response.status().as_u16())?;
    bytes(response.into_body(), MAX_BYTES)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use craft_anything_platform::sync::Mutex;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        sync::Arc,
        time::Instant,
    };

    /// Serve a finite response sequence on loopback and record request headers and payloads.
    pub fn serve(answers: impl FnOnce(&str) -> Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        serve_after(Duration::ZERO, answers)
    }

    /// [`serve`], with each answer held back for `delay` after its request has been read.
    pub fn serve_after(delay: Duration, answers: impl FnOnce(&str) -> Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let answers = answers(&base);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            for (status, body) in answers {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(start.elapsed() < Duration::from_secs(10), "local request was never sent");
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(e) => panic!("local accept failed: {e}"),
                    }
                };
                // Accepted sockets inherit the listener's mode differently across platforms.
                stream.set_nonblocking(false).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                    head.push_str(&line);
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut payload = vec![0; len];
                reader.read_exact(&mut payload).unwrap();
                recorded.lock().unwrap().push(format!("{head}{}", String::from_utf8_lossy(&payload)));
                std::thread::sleep(delay);
                write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        (base, seen)
    }

    #[test]
    fn query_values_are_escaped() {
        assert_eq!(encode("fal-ai/flux/dev"), "fal-ai/flux/dev");
        assert_eq!(encode("Mg=="), "Mg%3D%3D");
        assert_eq!(encode("a b&c=d#e+f%"), "a%20b%26c%3Dd%23e%2Bf%25");
        assert_eq!(encode("\u{e9}"), "%C3%A9");
    }

    /// Reject unsafe identifiers and download schemes while allowing signed HTTPS media URLs.
    #[test]
    fn identifiers_and_urls_cannot_inject_paths_or_credentials() {
        for value in ["", "../jobs", "x?token=y", "x/y", "a\r\nb"] {
            assert!(id(value).is_err());
        }
        for value in ["file:///etc/passwd", "http://example.com/file", "https://user:password@example.com/file"] {
            assert!(media_url(value).is_err());
        }
        assert!(media_url("https://cdn.example.com/file?signature=fake").is_ok());
    }
    /// Enforce both the body-size ceiling and a deadline when a server stalls.
    #[test]
    fn response_size_and_wall_clock_are_bounded() {
        let (base, _) = serve(|_| vec![(200, "too long".into())]);
        let response = agent(Duration::from_secs(2)).get(base).call().unwrap();
        assert_eq!(bytes(response.into_body(), 3).unwrap_err().code, ErrorCode::OverLimit);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let thread = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(100));
        });
        assert!(agent(Duration::from_millis(30)).get(url).call().is_err());
        thread.join().unwrap();
    }
}
