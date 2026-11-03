use craft_anything_platform::serve::{StaticServer, serve_dir};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

const MAX_HEAD: usize = 16 * 1024;
const CONTENT: &str = "private-preview-content";

fn preview() -> (tempfile::TempDir, StaticServer) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), CONTENT).unwrap();
    let server = serve_dir(dir.path()).unwrap();
    (dir, server)
}

fn response(port: u16, request: &str, end_input: bool) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    if end_input {
        stream.shutdown(Shutdown::Write).unwrap();
    }
    let mut response = String::new();
    stream.take(4096).read_to_string(&mut response).unwrap();
    assert!(response.len() < 4096, "response exceeded the test's bounded read");
    response
}

#[test]
fn regression_preview_rejects_eof_before_header_terminator() {
    let (_dir, server) = preview();
    for method in ["GET", "HEAD"] {
        for suffix in ["", "Host: localhost\r\n", "X\n", "\r"] {
            let reply = response(server.port, &format!("{method} / HTTP/1.1\r\n{suffix}"), true);
            assert!(!reply.starts_with("HTTP/1.1 200"), "incomplete {method} headers served a file: {reply:?}");
            assert!(!reply.contains(CONTENT), "incomplete headers exposed the file: {reply:?}");
        }
    }
}

#[test]
fn regression_preview_rejects_a_header_truncated_at_the_read_cap() {
    let (_dir, server) = preview();
    for method in ["GET", "HEAD"] {
        let prefix = format!("{method} / HTTP/1.1\r\nHost: localhost\r\nX-Fill: ");
        let request = prefix.clone() + &"x".repeat(MAX_HEAD - prefix.len());
        // Keep the client open: exhausting Take must not manufacture a valid header ending.
        let reply = response(server.port, &request, false);
        assert!(!reply.starts_with("HTTP/1.1 200"), "a truncated {method} header served a file: {reply:?}");
        assert!(!reply.contains(CONTENT), "a truncated header exposed the file: {reply:?}");
    }
}

#[test]
fn regression_preview_preserves_complete_lf_and_crlf_headers_at_the_cap() {
    let (_dir, server) = preview();
    for method in ["GET", "HEAD"] {
        for ending in ["\n", "\r\n"] {
            let prefix = format!("{method} / HTTP/1.1{ending}Host: localhost{ending}X-Fill: ");
            let terminator = ending.repeat(2);
            let request = prefix.clone() + &"x".repeat(MAX_HEAD - prefix.len() - terminator.len()) + &terminator;
            let reply = response(server.port, &request, false);
            assert!(reply.starts_with("HTTP/1.1 200 "), "complete {method} headers were rejected: {reply:?}");
            assert_eq!(reply.contains(CONTENT), method == "GET", "{reply:?}");
        }
    }
}
