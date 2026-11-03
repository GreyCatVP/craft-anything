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
fn regression_large_preview_file_and_head_keep_exact_lengths() {
    use std::io::{Seek, SeekFrom};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.bin");
    let mut file = std::fs::File::create(&path).unwrap();
    let len = 8 * 1024 * 1024;
    file.set_len(len).unwrap();
    file.seek(SeekFrom::End(-4)).unwrap();
    file.write_all(b"tail").unwrap();
    drop(file);
    let srv = serve_dir(dir.path()).unwrap();
    let get = get(srv.port, "/large.bin");
    let (headers, body) = get.split_once("\r\n\r\n").unwrap();
    assert!(headers.contains(&format!("Content-Length: {len}")));
    assert_eq!(body.len() as u64, len);
    assert!(body.ends_with("tail"));
    let mut s = TcpStream::connect(("127.0.0.1", srv.port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    s.write_all(format!("HEAD /large.bin HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n", srv.port).as_bytes()).unwrap();
    let mut head = String::new();
    s.read_to_string(&mut head).unwrap();
    assert!(head.contains(&format!("Content-Length: {len}")) && head.ends_with("\r\n\r\n"));
}
