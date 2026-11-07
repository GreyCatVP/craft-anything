//! DeepSeek Harness (dsh) sign-in and sign-out: the Harness's own DeepSeek account sign-in
//! (authorization code + PKCE, loopback redirect). The grant is written into the Harness's own
//! credential file in the shape the Harness itself uses; Craft Anything never keeps the token.

use craft_anything_evidence::{ErrorCode, ToolError};
use serde_json::{Value, json};
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Where the DeepSeek account lives.
pub const ORIGIN: &str = "https://platform.deepseek.com";
/// The record a sign-in stores, and the one that names this computer.
const RECORD: &str = "deepseek-account-platform/default";
const DEVICE: &str = "deepseek-account-platform/device";
const KEY_REF: &str = "DEEPSEEK_API_KEY";
/// The most of a response body we read.
const MAX_BODY: usize = 64 * 1024;
/// The most of the credential file we read.
const MAX_CREDENTIALS: u64 = 1 << 20;

/// Whether the Harness on this computer is signed in, and to what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignIn {
    SignedIn { account: Option<String> },
    SignedOut,
}

/// The Harness's own folder: DSH_HOME when it's set (the person's choice), else ~/.dsh.
pub fn home() -> Option<PathBuf> {
    let set = std::env::var_os("DSH_HOME").filter(|v| !v.is_empty()).map(PathBuf::from);
    set.or_else(|| dirs::home_dir().map(|h| h.join(".dsh")))
}

/// The credential file in a Harness folder.
fn credentials(home: &Path) -> PathBuf {
    home.join(".credentials.yaml")
}

/// Read the credential file, if it's a regular file small enough to read.
fn read_credentials(home: &Path) -> Option<String> {
    let file = credentials(home);
    let meta = std::fs::metadata(&file).ok()?;
    if !meta.is_file() || meta.len() > MAX_CREDENTIALS {
        return None;
    }
    std::fs::read(&file).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Whether the Harness on this computer is signed in to a DeepSeek account.
pub fn status(home: &Path) -> SignIn {
    let Some(text) = read_credentials(home) else { return SignIn::SignedOut };
    let mut section = String::new();
    let mut account = None;
    for line in text.lines().take(4096) {
        let trimmed = line.trim_end();
        let body = trimmed.trim_start();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        let top_level = !trimmed.starts_with(' ') && !trimmed.starts_with('\t');
        if top_level {
            section = trimmed.trim_end_matches(':').to_string();
            continue;
        }
        if section == "refs"
            && let Some((key, value)) = body.split_once(':')
            && key.trim() == KEY_REF
            && !value.trim().is_empty()
        {
            return SignIn::SignedIn { account: Some("DeepSeek API key".to_string()) };
        }
        if body.starts_with(&format!("{RECORD}:")) {
            account = Some("DeepSeek account".to_string());
        }
        if body.starts_with("issuer:")
            && let Some((_, value)) = body.split_once(':')
            && account.is_some()
            && !value.trim().is_empty()
        {
            account = Some(value.trim().trim_matches('"').to_string());
        }
    }
    match account {
        Some(account) => SignIn::SignedIn { account: Some(account) },
        None => SignIn::SignedOut,
    }
}

fn io_error(what: &str, e: std::io::Error) -> ToolError {
    ToolError::new(ErrorCode::BadSettings, format!("{what}: {e}"))
}

fn bad(message: impl Into<String>) -> ToolError {
    ToolError::new(ErrorCode::BadSettings, message)
}

/// Sign in to the DeepSeek account the Harness should use: start the account sign-in, open the
/// page it returns with `open_browser`, wait for the browser to come back to this computer, then
/// keep the Harness's own grant. Blocking, with a deadline: call it off the UI thread.
pub fn sign_in(home: &Path, origin: &str, open_browser: &dyn Fn(&str), deadline: Duration) -> Result<(), ToolError> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|e| io_error("Couldn't open the sign-in's return address on this computer", e))?;
    listener.set_nonblocking(true).map_err(|e| io_error("Couldn't set up the sign-in's return address", e))?;
    let port = listener.local_addr().map_err(|e| io_error("Couldn't read the sign-in's return address", e))?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/oauth/callback");

    let verifier = random_token(48);
    let challenge = base64url(&sha256(&verifier));
    let state = random_token(24);
    let device_id = existing_device_id(home).unwrap_or_else(random_uuid);

    let init = post_json(
        origin,
        "/auth-api/v0/dsh/auth_init",
        &json!({
            "code_challenge": challenge,
            "code_challenge_method": "S256",
            "state": state,
            "redirect_uri": redirect_uri,
            "locale": "zh_CN",
            "login_source": "desktop",
        }),
        &[],
    )?;
    let authorize_url = init.get("authorize_url").and_then(Value::as_str).unwrap_or_default().to_string();
    if !authorize_url_is_safe(&authorize_url, origin) {
        return Err(bad("The account sign-in answered with an address that isn't the DeepSeek page."));
    }
    open_browser(&authorize_url);

    let code = wait_for_code(&listener, &state, deadline)?;
    let exchanged = post_json(
        origin,
        "/auth-api/v0/dsh/auth_exchange",
        &json!({
            "code": code,
            "code_verifier": verifier,
            "redirect_uri": redirect_uri,
            "device_id": device_id,
            "device_model": device_model(),
            "os_version": os_version(),
        }),
        &[],
    )?;
    let token = exchanged.get("token").and_then(Value::as_str).unwrap_or_default().trim().to_string();
    if token.is_empty() || token.len() > 4096 {
        return Err(bad("The account sign-in finished without a token."));
    }
    let text = read_credentials(home).unwrap_or_default();
    let next = grant_yaml(&text, &token, &device_id, origin);
    std::fs::create_dir_all(home).map_err(|e| io_error("Couldn't make the Harness's folder", e))?;
    craft_anything_platform::fs::write_atomic_private(&credentials(home), next.as_bytes())?;
    Ok(())
}

/// Sign out: ask the account to forget this sign-in (best effort), then remove the Harness's own
/// grant. A computer that isn't signed in is already signed out.
pub fn sign_out(home: &Path, origin: &str) -> Result<(), ToolError> {
    let text = read_credentials(home).unwrap_or_default();
    if let Some(token) = record_field(&text, RECORD, "token") {
        let _ = post_json(origin, "/auth-api/v0/users/logout", &json!({}), &[("x-dsh-auth-token", token.as_str())]);
    }
    if text.is_empty() {
        return Ok(());
    }
    let next = remove_record(&text, RECORD);
    if next != text {
        craft_anything_platform::fs::write_atomic_private(&credentials(home), next.as_bytes())?;
    }
    Ok(())
}

/// Whether the Harness's sign-in page is the one we asked for.
fn authorize_url_is_safe(url: &str, origin: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else { return false };
    if scheme != "https" && !(scheme == "http" && origin.starts_with("http://")) {
        return false;
    }
    let host = rest.split('/').next().unwrap_or_default().split(':').next().unwrap_or_default();
    let origin_host = origin.split_once("://").map(|(_, r)| r.split('/').next().unwrap_or_default().split(':').next().unwrap_or_default()).unwrap_or_default();
    if host != origin_host || host.is_empty() {
        return false;
    }
    url.contains("/dsh/authorize")
}

/// Wait on the loopback listener for the browser's return, comparing the state.
fn wait_for_code(listener: &TcpListener, state: &str, deadline: Duration) -> Result<String, ToolError> {
    let start = Instant::now();
    while start.elapsed() < deadline {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let request = read_request(&mut stream);
                let target = request.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or_default().to_string();
                let (code, got_state) = callback_values(&target);
                let ok = got_state.as_deref().is_some_and(|got| same_secret(got, state));
                let body = if ok {
                    "<html><body>Signed in to DeepSeek. You can close this tab.</body></html>"
                } else {
                    "<html><body>The sign-in didn't match. Try again from Craft Anything.</body></html>"
                };
                let _ = respond(&mut stream, if ok { "200 OK" } else { "400 Bad Request" }, "text/html; charset=utf-8", body);
                if ok {
                    return code.ok_or_else(|| bad("The sign-in came back without a code."));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(80));
            }
            Err(e) => return Err(io_error("The sign-in's return address failed", e)),
        }
    }
    Err(ToolError::new(ErrorCode::TimedOut, "The sign-in timed out before the browser came back."))
}

/// `code` and `state` from a request target such as `/oauth/callback?code=...&state=...`.
fn callback_values(target: &str) -> (Option<String>, Option<String>) {
    let query = target.split_once('?').map(|(_, q)| q).unwrap_or_default();
    let mut code = None;
    let mut state = None;
    for pair in query.split('&').take(32) {
        let Some((key, value)) = pair.split_once('=') else { continue };
        let value = percent_decode(value);
        match key {
            "code" => code = Some(value),
            "state" => state = Some(value),
            _ => {}
        }
    }
    (code, state)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn same_secret(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn read_request(stream: &mut TcpStream) -> String {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    let mut header_end = None;
    while buf.len() < 16 * 1024 {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    header_end = Some(at);
                    break;
                }
            }
            Err(_) => break,
        }
    }
    // Read the whole body too, so the caller can answer without the client still writing to a
    // socket we're about to close (which would reset the connection).
    if let Some(at) = header_end {
        let head = String::from_utf8_lossy(&buf[..at]).to_ascii_lowercase();
        let want: usize =
            head.lines().find_map(|line| line.trim_start().strip_prefix("content-length:")).and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
        let mut have = buf.len().saturating_sub(at + 4);
        while have < want && buf.len() < 64 * 1024 {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    have = buf.len().saturating_sub(at + 4);
                }
                Err(_) => break,
            }
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) -> std::io::Result<()> {
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    stream.write_all(response.as_bytes())?;
    stream.flush()
}

/// One JSON POST to the account service, reading the `{code, data:{biz_code, biz_data}}` envelope.
fn post_json(origin: &str, path: &str, body: &Value, headers: &[(&str, &str)]) -> Result<Value, ToolError> {
    let url = format!("{}{path}", origin.trim_end_matches('/'));
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(30)))
        .build()
        .new_agent();
    let mut request = agent
        .post(&url)
        .header("content-type", "application/json")
        .header("x-client-platform", client_platform())
        .header("x-client-version", env!("CARGO_PKG_VERSION"))
        .header("x-client-locale", "zh_CN")
        .header("x-client-timezone-offset", "0");
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    let response =
        request.send(body.to_string()).map_err(|e| ToolError::new(ErrorCode::EngineMissing, format!("Couldn't reach the DeepSeek account service: {e}")))?;
    let status = response.status().as_u16();
    let mut text = String::new();
    let mut reader = response.into_body().into_reader().take(MAX_BODY as u64);
    let _ = reader.read_to_string(&mut text);
    if !(200..300).contains(&status) {
        return Err(ToolError::new(
            ErrorCode::EngineMissing,
            format!("The DeepSeek account service answered {status}: {}", text.chars().take(200).collect::<String>()),
        ));
    }
    let value: Value = serde_json::from_str(text.trim())
        .map_err(|_| ToolError::new(ErrorCode::EngineMissing, "The DeepSeek account service answered something that isn't a result."))?;
    let code = value.get("code").and_then(Value::as_i64).unwrap_or(0);
    let data = value.get("data").cloned().unwrap_or(Value::Null);
    let biz_code = data.get("biz_code").and_then(Value::as_i64).unwrap_or(0);
    if code != 0 || biz_code != 0 {
        let message = value.get("message").and_then(Value::as_str).or_else(|| data.get("biz_message").and_then(Value::as_str)).unwrap_or("the service refused");
        return Err(ToolError::new(ErrorCode::EngineMissing, format!("The DeepSeek account service refused: {message}")));
    }
    Ok(data.get("biz_data").cloned().unwrap_or(Value::Null))
}

fn client_platform() -> &'static str {
    if cfg!(windows) {
        "desktop-win"
    } else if cfg!(target_os = "macos") {
        "desktop-mac"
    } else {
        "web"
    }
}

fn device_model() -> String {
    format!("{} {}", std::env::consts::OS, std::env::consts::ARCH)
}

fn os_version() -> String {
    std::env::consts::OS.to_string()
}

/// A random string of URL-safe characters, from the process's own random hash keys and the clock.
fn random_token(len: usize) -> String {
    use std::hash::{BuildHasher, Hasher as _};
    let mut out = String::with_capacity(len);
    let state = std::collections::hash_map::RandomState::new();
    let mut counter: u64 = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    while out.len() < len {
        let mut hasher = state.build_hasher();
        hasher.write_u64(counter);
        counter = counter.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let value = hasher.finish();
        for byte in value.to_le_bytes() {
            if out.len() >= len {
                break;
            }
            let c = (byte % 64) as usize;
            let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            out.push(alphabet[c] as char);
        }
    }
    out
}

fn random_uuid() -> String {
    let raw = random_token(32);
    let hex: String = raw.bytes().map(|b| format!("{:02x}", b % 16)).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

/// SHA-256, from the workspace's own hash (its hex is turned back into bytes here).
fn sha256(input: &str) -> Vec<u8> {
    let hex = craft_anything_evidence::sha256_hex(input.as_bytes());
    let bytes = hex.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i + 1 < bytes.len() {
        if let Ok(pair) = std::str::from_utf8(&bytes[i..i + 2])
            && let Ok(byte) = u8::from_str_radix(pair, 16)
        {
            out.push(byte);
        }
        i += 2;
    }
    out
}

/// base64url without padding, as PKCE wants it.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    let mut i = 0;
    while i < bytes.len() {
        let b0 = bytes[i] as u32;
        let b1 = bytes.get(i + 1).map(|b| *b as u32).unwrap_or(0);
        let b2 = bytes.get(i + 2).map(|b| *b as u32).unwrap_or(0);
        out.push(ALPHABET[((b0 >> 2) & 63) as usize] as char);
        out.push(ALPHABET[(((b0 << 4) | (b1 >> 4)) & 63) as usize] as char);
        if i + 1 < bytes.len() {
            out.push(ALPHABET[(((b1 << 2) | (b2 >> 6)) & 63) as usize] as char);
        }
        if i + 2 < bytes.len() {
            out.push(ALPHABET[(b2 & 63) as usize] as char);
        }
        i += 3;
    }
    out
}

/// The value of `field` inside the credential record `record`, when it's a plain value.
fn record_field(text: &str, record: &str, field: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines().take(4096) {
        let body = line.trim_start();
        if !line.starts_with(' ') && !line.starts_with('\t') {
            inside = false;
            continue;
        }
        if body.starts_with(&format!("{record}:")) {
            inside = true;
            continue;
        }
        if !inside {
            continue;
        }
        if !line.starts_with("    ") {
            inside = false;
            continue;
        }
        if let Some((key, value)) = body.split_once(':')
            && key.trim() == field
        {
            let value = value.trim().trim_matches('"').to_string();
            return (!value.is_empty()).then_some(value);
        }
    }
    None
}

/// The device id the Harness already uses on this computer, so a sign-in keeps it.
fn existing_device_id(home: &Path) -> Option<String> {
    let text = read_credentials(home)?;
    record_field(&text, DEVICE, "id")
}

/// The credential file with the account grant written in, every other line kept verbatim.
pub(crate) fn grant_yaml(text: &str, token: &str, device: &str, issuer: &str) -> String {
    let without = remove_record(text, RECORD);
    let without = remove_record(&without, DEVICE);
    let mut out = without.trim_end().to_string();
    if !out.contains("version:") {
        out = format!("version: 1\n{out}");
    }
    let block = [
        format!("  {DEVICE}:"),
        "    kind: grant".to_string(),
        "    payload:".to_string(),
        format!("      id: {device}"),
        format!("  {RECORD}:"),
        "    kind: grant".to_string(),
        "    payload:".to_string(),
        "      version: 1".to_string(),
        format!("      token: {token}"),
        format!("      issuer: {issuer}"),
    ]
    .join("\n");
    if let Some(at) = records_section_start(&out) {
        let mut next = String::new();
        let mut inserted = false;
        for (i, line) in out.lines().enumerate() {
            if i == at + 1 {
                next.push_str(&block);
                next.push('\n');
                inserted = true;
            }
            next.push_str(line);
            next.push('\n');
        }
        if !inserted {
            next.push_str(&block);
            next.push('\n');
        }
        next
    } else {
        format!("{out}\nrecords:\n{block}\n")
    }
}

/// The index of the top-level `records:` line, if the file has one.
fn records_section_start(text: &str) -> Option<usize> {
    text.lines().position(|line| line.trim_end() == "records:" && !line.starts_with(' ') && !line.starts_with('\t'))
}

/// The credential file without the record `record` (its whole indented block), other lines kept.
pub(crate) fn remove_record(text: &str, record: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut skipping: Option<usize> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let body = line.trim_start();
        if let Some(level) = skipping {
            if line.trim().is_empty() || indent > level {
                continue;
            }
            skipping = None;
        }
        if indent == 2 && body.starts_with(&format!("{record}:")) {
            skipping = Some(indent);
            continue;
        }
        out.push(line);
    }
    let mut joined = out.join("\n");
    if text.ends_with('\n') && !joined.is_empty() {
        joined.push('\n');
    }
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn temp_home() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        (dir, home)
    }

    #[test]
    fn a_computer_without_a_file_is_signed_out() {
        let (_d, home) = temp_home();
        assert_eq!(status(&home), SignIn::SignedOut);
    }

    #[test]
    fn a_key_reference_and_a_grant_are_both_signed_in() {
        let (_d, home) = temp_home();
        std::fs::write(home.join(".credentials.yaml"), "version: 1\nrefs:\n  DEEPSEEK_API_KEY: sk-test\n").unwrap();
        assert!(matches!(status(&home), SignIn::SignedIn { .. }));
        std::fs::write(home.join(".credentials.yaml"), grant_yaml("version: 1\nrefs:\n  OTHER: kept\n", "tok", "dev-1", "https://platform.deepseek.com"))
            .unwrap();
        assert_eq!(status(&home), SignIn::SignedIn { account: Some("https://platform.deepseek.com".into()) });
    }

    #[test]
    fn the_grant_is_written_and_other_records_are_kept() {
        let before =
            "version: 1\nrefs:\n  ZAI_API_KEY: kept\nrecords:\n  client-connection/browser-session:\n    kind: secret\n    payload:\n      secret: s\n";
        let after = grant_yaml(before, "tok-123", "dev-9", "https://platform.deepseek.com");
        assert!(after.contains("ZAI_API_KEY: kept"), "{after}");
        assert!(after.contains("client-connection/browser-session:"), "{after}");
        assert!(after.contains(RECORD), "{after}");
        assert!(after.contains("token: tok-123"), "{after}");
        assert!(after.contains("id: dev-9"), "{after}");
        assert_eq!(record_field(&after, RECORD, "token").as_deref(), Some("tok-123"));
    }

    #[test]
    fn signing_out_removes_only_the_grant() {
        let before = grant_yaml("version: 1\nrefs:\n  ZAI_API_KEY: kept\n", "tok", "dev-1", "issuer-x");
        let after = remove_record(&before, RECORD);
        assert!(!after.contains(RECORD), "{after}");
        assert!(after.contains(DEVICE), "the device record stays: {after}");
        assert!(after.contains("ZAI_API_KEY: kept"), "{after}");
    }

    #[test]
    fn the_pkce_challenge_matches_the_known_example() {
        // RFC 7636 Appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(base64url(&sha256(verifier)), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn an_unrelated_service_is_refused_as_the_sign_in_page() {
        assert!(authorize_url_is_safe("https://platform.deepseek.com/dsh/authorize?x=1", ORIGIN));
        assert!(!authorize_url_is_safe("https://evil.example/dsh/authorize", ORIGIN));
        assert!(!authorize_url_is_safe("http://platform.deepseek.com/dsh/authorize", ORIGIN));
        assert!(!authorize_url_is_safe("https://platform.deepseek.com/other", ORIGIN));
    }

    #[test]
    fn the_state_is_compared_exactly() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd"));
        assert!(!same_secret("ab", "abc"));
    }

    #[test]
    fn a_callback_target_gives_up_its_code_and_state() {
        let (code, state) = callback_values("/oauth/callback?code=a%2Bb&state=xyz");
        assert_eq!(code.as_deref(), Some("a+b"));
        assert_eq!(state.as_deref(), Some("xyz"));
    }

    struct Fake {
        origin: String,
        stop: Arc<AtomicBool>,
    }

    impl Fake {
        fn url(&self) -> &str {
            &self.origin
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    /// A stand-in for the account service: answers `init`, `exchange` and `logout`.
    fn fake_service(exchange_token: &'static str, refuse: bool) -> Fake {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = std::thread::spawn(move || {
            let _ = listener.set_nonblocking(false);
            for _ in 0..8 {
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                let Ok((mut stream, _)) = listener.accept() else { break };
                let request = read_request(&mut stream);
                let target = request.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or_default().to_string();
                let body = request.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
                let answer = if target.contains("auth_init") {
                    let state =
                        serde_json::from_str::<Value>(&body).ok().and_then(|v| v.get("state").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
                    let redirect = serde_json::from_str::<Value>(&body)
                        .ok()
                        .and_then(|v| v.get("redirect_uri").and_then(Value::as_str).map(str::to_string))
                        .unwrap_or_default();
                    json!({ "code": 0, "data": { "biz_code": 0, "biz_data": { "authorize_url": format!("http://127.0.0.1:{}/dsh/authorize?state={}&redirect={}", addr.port(), state, redirect) } } }).to_string()
                } else if target.contains("auth_exchange") {
                    if refuse {
                        json!({ "code": 1, "message": "not today" }).to_string()
                    } else {
                        json!({ "code": 0, "data": { "biz_code": 0, "biz_data": { "token": exchange_token } } }).to_string()
                    }
                } else {
                    json!({ "code": 0, "data": { "biz_code": 0, "biz_data": {} } }).to_string()
                };
                let _ = respond(&mut stream, "200 OK", "application/json", &answer);
            }
        });
        drop(handle);
        Fake { origin: format!("http://127.0.0.1:{}", addr.port()), stop }
    }

    fn browser() -> impl Fn(&str) {
        move |url: &str| {
            let url = url.to_string();
            std::thread::spawn(move || {
                let (_, after) = url.split_once("/dsh/authorize?").unwrap_or(("", ""));
                let state = after.split('&').find_map(|p| p.strip_prefix("state=")).unwrap_or_default().to_string();
                let redirect = after.split('&').find_map(|p| p.strip_prefix("redirect=")).unwrap_or_default().to_string();
                let target = format!("{redirect}?code=fake-code&state={state}");
                let hostport = target.trim_start_matches("http://").split('/').next().unwrap_or_default().to_string();
                if let Ok(mut stream) = TcpStream::connect(&hostport) {
                    let _ =
                        stream.write_all(format!("GET {} HTTP/1.1\r\nHost: {}\r\n\r\n", &target[format!("http://{hostport}").len()..], hostport).as_bytes());
                    let _ = stream.flush();
                }
            });
        }
    }

    #[test]
    fn signing_in_writes_the_grant_into_the_harness_file() {
        let (_d, home) = temp_home();
        let fake = fake_service("tok-live-test", false);
        sign_in(&home, fake.url(), &browser(), Duration::from_secs(10)).unwrap();
        let text = std::fs::read_to_string(home.join(".credentials.yaml")).unwrap();
        assert!(text.contains("tok-live-test"), "{text}");
        assert!(matches!(status(&home), SignIn::SignedIn { .. }));
    }

    #[test]
    fn a_refused_exchange_is_reported_and_writes_nothing() {
        let (_d, home) = temp_home();
        let fake = fake_service("unused", true);
        let err = sign_in(&home, fake.url(), &browser(), Duration::from_secs(10)).unwrap_err();
        assert!(err.message.contains("refused"), "{}", err.message);
        assert!(!home.join(".credentials.yaml").exists());
    }

    #[test]
    fn signing_out_clears_locally_even_when_the_service_is_down() {
        let (_d, home) = temp_home();
        std::fs::write(home.join(".credentials.yaml"), grant_yaml("version: 1\n", "tok", "dev-1", "issuer")).unwrap();
        // Nothing listens there: the remote call fails, the local sign-out still happens.
        sign_out(&home, "http://127.0.0.1:9").unwrap();
        let text = std::fs::read_to_string(home.join(".credentials.yaml")).unwrap();
        assert!(!text.contains(RECORD), "{text}");
        assert_eq!(status(&home), SignIn::SignedOut);
    }
}
