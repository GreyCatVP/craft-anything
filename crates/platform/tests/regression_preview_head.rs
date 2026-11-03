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
fn regression_head_errors_have_no_body() {
    let dir = tempfile::tempdir().unwrap();
    let srv = serve_dir(dir.path()).unwrap();
    let mut s = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(format!("HEAD /missing.js HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n", srv.port).as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 404"));
    assert!(out.contains("Content-Length: 9\r\n"));
    assert!(out.ends_with("\r\n\r\n"), "{out}");
    assert!(get(srv.port, "/missing.js").ends_with("Not found"));
}
