//! Bounded HTTP shared by the asset adapters. Never include remote bodies or URLs in errors.
use super::inputs::{REMEDIATION, clipped};
use craft_anything_evidence::{ErrorCode, ToolError};
use std::{
    io::Read,
    time::{Duration, Instant},
};

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
// Shared by the providers' model lists (`Fal::list`, `Scenario::list`, ...).

/// The longest model name kept, and the longest next-page cursor or token followed.
pub(super) const NAME_LENGTH: usize = 80;
pub(super) const CURSOR_LENGTH: usize = 200;

/// Whether `deadline` has passed. A list checks it before each request it starts.
pub(super) fn time_is_up(deadline: Instant) -> bool {
    Instant::now() >= deadline
}

/// An error as one sentence or two, for a list's `error`.
pub(super) fn words(e: &ToolError) -> String {
    format!("{} {}", e.message, e.remediation.as_deref().unwrap_or_default()).trim().to_string()
}

/// The list ran out of time. `provider` is the name its people know it by ("fal.ai", "Scenario").
pub(super) fn too_slow(provider: &str) -> ToolError {
    ToolError::new(ErrorCode::TimedOut, format!("Reading {provider}'s model list took too long, so it was cut short."))
        .with_remediation("Refresh the list to read the rest.")
}

/// The provider's model list could not be fetched at all (the connection failed or timed out).
pub(super) fn cannot_reach(provider: &str) -> ToolError {
    ToolError::new(ErrorCode::EngineMissing, format!("Couldn't reach {provider} to read its models."))
        .with_remediation("Check your internet connection and try again.")
}

/// The name to show for a model: the provider's, else the known-models table's, else its id. Each
/// is tidied (no control characters, at most [`NAME_LENGTH`] characters), and a blank one is skipped.
pub(super) fn shown_name(provider_name: Option<&str>, table_name: Option<&str>, id: &str) -> String {
    [provider_name, table_name]
        .into_iter()
        .flatten()
        .map(|name| clipped(name, NAME_LENGTH))
        .map(|name| name.trim().to_string())
        .find(|name| !name.is_empty())
        .unwrap_or_else(|| id.to_string())
}

/// A chosen model its provider no longer serves; its name is the provider's, so it is clipped.
pub(super) fn gone(provider: &str, model: &str) -> ToolError {
    ToolError::new(ErrorCode::BadSettings, format!("The model \"{}\" is no longer available at {provider}.", clipped(model, NAME_LENGTH)))
        .with_remediation(REMEDIATION)
}

/// A chosen model its provider has marked as one it is retiring.
pub(super) fn retiring(provider: &str, model: &str) -> ToolError {
    ToolError::new(ErrorCode::BadSettings, format!("The model \"{}\" is being retired at {provider}.", clipped(model, NAME_LENGTH)))
        .with_remediation(REMEDIATION)
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
/// Send a GET, and once more if the system interrupted the first try (`EINTR`, which a busy
/// machine's signals can cause). A GET starts no paid work, so asking again is safe: never use
/// this for a POST, which may start a generation that is charged twice.
pub(super) fn get<T>(send: impl Fn() -> Result<T, ureq::Error>) -> Result<T, ureq::Error> {
    match send() {
        Err(ureq::Error::Io(e)) if e.kind() == std::io::ErrorKind::Interrupted => send(),
        other => other,
    }
}

/// Download bounded media from a validated address without forwarding provider credentials.
pub(super) fn download(url: &str, timeout: Duration) -> Result<Vec<u8>, ToolError> {
    media_url(url)?;
    // A separate agent and no auth headers: credentials never reach the media host.
    let response = get(|| agent(timeout).get(url).call()).map_err(|_| network())?;
    status(response.status().as_u16())?;
    bytes(response.into_body(), MAX_BYTES)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use craft_anything_platform::sync::Mutex;
    use std::{
        io::{BufRead, BufReader, Write},
        net::{TcpListener, TcpStream},
        sync::Arc,
        time::Instant,
    };

    /// Serve a finite response sequence on loopback and record request headers and payloads.
    pub fn serve(answers: impl FnOnce(&str) -> Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        serve_after(Duration::ZERO, answers)
    }

    /// [`serve`], with each answer held back for `delay` after its request has been read.
    pub fn serve_after(delay: Duration, answers: impl FnOnce(&str) -> Vec<(u16, String)>) -> (String, Arc<Mutex<Vec<String>>>) {
        stand(delay, |base| answers(base).into_iter().map(|(status, body)| (status, body, Vec::new())).collect())
    }

    /// [`serve`], with headers of its own on each answer (name, value).
    pub fn serve_with_headers(answers: impl FnOnce(&str) -> Vec<(u16, String, Vec<(String, String)>)>) -> (String, Arc<Mutex<Vec<String>>>) {
        stand(Duration::ZERO, answers)
    }

    fn stand(delay: Duration, answers: impl FnOnce(&str) -> Vec<(u16, String, Vec<(String, String)>)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let answers = answers(&base);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            for (status, body, headers) in answers {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((s, _)) => break s,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(start.elapsed() < Duration::from_secs(10), "local request was never sent");
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        Err(e) => panic!("local accept failed: {e}"),
                    }
                };
                recorded.lock().unwrap().push(request_of(&stream));
                std::thread::sleep(delay);
                // The client may be gone before the body is written (it does not read the body of an
                // error answer). That ends nothing here: a panic would kill the thread and with it every
                // answer still to come, which made tests that ask twice fail now and then.
                let extra: String = headers.iter().map(|(name, value)| format!("{name}: {value}\r\n")).collect();
                let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n{body}", body.len());
            }
        });
        (base, seen)
    }

    /// The request on a newly accepted stream: its head and its body.
    fn request_of(stream: &TcpStream) -> String {
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
        format!("{head}{}", String::from_utf8_lossy(&payload))
    }

    /// A stand-in that answers each request by what it asks for (`answer(target)`, the request's
    /// path and query), as many as come in ten seconds. A request sent again gets the same answer,
    /// so a test of many requests doesn't depend on none of them being sent twice: one the system
    /// interrupted after the stand-in had read it is sent again ([`get`]).
    pub fn serve_by(answer: impl Fn(&str) -> (u16, String) + Send + 'static) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        std::thread::spawn(move || {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(10) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = request_of(&stream);
                        let target = request.split_whitespace().nth(1).unwrap_or_default().to_string();
                        recorded.lock().unwrap().push(request);
                        let (status, body) = answer(&target);
                        let _ = write!(stream, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(1)),
                    Err(_) => break,
                }
            }
        });
        (base, seen)
    }

    #[test]
    fn the_messages_of_a_list_name_the_provider_and_tidy_the_model() {
        let hostile = format!("evil\nmodel{}", "x".repeat(200));
        for (error, said) in [(gone("Acme", &hostile), "no longer available at Acme."), (retiring("Acme", &hostile), "being retired at Acme.")] {
            assert_eq!(error.code, ErrorCode::BadSettings);
            assert!(error.message.ends_with(said) && error.message.contains("evilmodelxxx"), "{}", error.message);
            assert!(!error.message.chars().any(char::is_control) && error.message.chars().count() < 160, "{}", error.message);
            assert_eq!(error.remediation.as_deref(), Some(REMEDIATION));
        }
        let slow = too_slow("Acme");
        assert_eq!(slow.code, ErrorCode::TimedOut);
        assert_eq!(words(&slow), "Reading Acme's model list took too long, so it was cut short. Refresh the list to read the rest.");
        assert_eq!(words(&ToolError::new(ErrorCode::BadSettings, "No remedy.")), "No remedy.");
        assert!(!time_is_up(Instant::now() + Duration::from_secs(60)) && time_is_up(Instant::now()));
    }

    #[test]
    fn a_model_is_named_by_its_provider_then_the_table_then_its_id() {
        assert_eq!(shown_name(Some("  Flux  "), Some("Table"), "id"), "Flux");
        assert_eq!(shown_name(Some(" \n "), Some("Table"), "id"), "Table", "a blank name is skipped");
        assert_eq!(shown_name(None, None, "id"), "id");
        assert_eq!(shown_name(Some("ev\nil"), None, "id"), "evil");
        let long = shown_name(Some(&"x".repeat(500)), None, "id");
        assert_eq!(long.chars().count(), NAME_LENGTH + 1, "cut short, with an ellipsis");
    }

    #[test]
    fn an_unreachable_provider_is_named() {
        let error = cannot_reach("Acme");
        assert_eq!((error.code, error.message.as_str()), (ErrorCode::EngineMissing, "Couldn't reach Acme to read its models."));
        assert!(error.remediation.is_some());
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
    /// A GET interrupted by the system is sent once more; any other failure, or a second
    /// interruption, is the answer.
    #[test]
    fn an_interrupted_get_is_sent_once_more() {
        let counted = std::sync::atomic::AtomicUsize::new(0);
        let tries = &counted;
        let interrupted_then = |then: Result<u8, ureq::Error>| {
            let then = std::cell::RefCell::new(Some(then));
            move || -> Result<u8, ureq::Error> {
                if tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    return Err(ureq::Error::Io(std::io::Error::from(std::io::ErrorKind::Interrupted)));
                }
                then.borrow_mut().take().unwrap_or(Err(ureq::Error::Timeout(ureq::Timeout::Global)))
            }
        };
        assert_eq!(get(interrupted_then(Ok(7))).unwrap(), 7);
        assert_eq!(tries.swap(0, std::sync::atomic::Ordering::SeqCst), 2);
        let twice = get(interrupted_then(Err(ureq::Error::Io(std::io::Error::from(std::io::ErrorKind::Interrupted)))));
        assert!(matches!(twice, Err(ureq::Error::Io(e)) if e.kind() == std::io::ErrorKind::Interrupted));
        assert_eq!(tries.swap(0, std::sync::atomic::Ordering::SeqCst), 2, "only once more");
        let refused = get(|| -> Result<u8, ureq::Error> {
            tries.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(ureq::Error::Io(std::io::Error::from(std::io::ErrorKind::ConnectionRefused)))
        });
        assert!(refused.is_err());
        assert_eq!(tries.load(std::sync::atomic::Ordering::SeqCst), 1, "another failure is not tried again");
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
