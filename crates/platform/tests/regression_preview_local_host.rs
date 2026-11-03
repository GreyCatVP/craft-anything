use craft_anything_platform::serve::serve_dir;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;
fn get(port: u16, path: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    out
}

#[test]
fn regression_preview_refuses_foreign_hosts_and_origins() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "local-only-content").unwrap();
    let srv = serve_dir(dir.path()).unwrap();
    for extra in [
        "Host: attacker.example\r\n".to_string(),
        "Host: localhost.attacker.example\r\n".to_string(),
        format!("Host: localhost:{}\r\nOrigin: https://attacker.example\r\n", srv.port),
        format!("Host: localhost:{}\r\nHost: attacker.example\r\n", srv.port),
        format!("Host: localhost:{}\r\nOrigin: http://localhost:3000\r\nOrigin: https://attacker.example\r\n", srv.port),
    ] {
        let mut s = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        s.write_all(format!("GET / HTTP/1.1\r\n{extra}\r\n").as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 400") || out.starts_with("HTTP/1.1 403"), "{out}");
        assert!(!out.contains("local-only-content"));
    }
    assert!(get(srv.port, "/").contains("local-only-content"));
}

#[test]
fn regression_preview_accepts_a_local_origin() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "local-only-content").unwrap();
    let srv = serve_dir(dir.path()).unwrap();
    let mut stream = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(format!("GET / HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: http://localhost:3000\r\n\r\n", srv.port).as_bytes()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    assert!(headers.starts_with("HTTP/1.1 200 "), "{headers}");
    assert_eq!(body, "local-only-content");
}
