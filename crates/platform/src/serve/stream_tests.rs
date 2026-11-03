use super::send_file;
use std::cell::Cell;
use std::io::{Read, Write};
use std::rc::Rc;

const READ_CHUNK: usize = 4096;
const MAX_READ_AHEAD: u64 = 32 * 1024;

struct Source {
    available: u64,
    consumed: Rc<Cell<u64>>,
    written: Rc<Cell<u64>>,
}

impl Read for Source {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let consumed = self.consumed.get();
        let count = buffer.len().min(READ_CHUNK).min((self.available - consumed) as usize);
        if consumed + count as u64 - self.written.get() > MAX_READ_AHEAD {
            return Err(std::io::Error::other("body must make output progress before consuming the entire source"));
        }
        buffer[..count].fill(b'x');
        self.consumed.set(consumed + count as u64);
        Ok(count)
    }
}

struct Response {
    headers: Vec<u8>,
    consumed: Rc<Cell<u64>>,
    written: Rc<Cell<u64>>,
    first_body_write_after: Option<u64>,
}

impl Write for Response {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.headers.is_empty() {
            assert!(bytes.starts_with(b"HTTP/1.1 200 OK\r\n"));
            assert!(bytes.ends_with(b"\r\n\r\n"));
            self.headers.extend_from_slice(bytes);
            return Ok(bytes.len());
        }
        // Partial writes exercise the copy loop without retaining the body in the test sink.
        let count = bytes.len().min(733);
        assert!(bytes[..count].iter().all(|&byte| byte == b'x'));
        self.first_body_write_after.get_or_insert(self.consumed.get());
        self.written.set(self.written.get() + count as u64);
        Ok(count)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn file_body_makes_progress_with_bounded_read_ahead() {
    let len = 8 * 1024 * 1024;
    let consumed = Rc::new(Cell::new(0));
    let written = Rc::new(Cell::new(0));
    let source = Source {
        // Bytes beyond the metadata length must not become part of this response.
        available: len + READ_CHUNK as u64,
        consumed: consumed.clone(),
        written: written.clone(),
    };
    let mut response = Response { headers: Vec::new(), consumed: consumed.clone(), written: written.clone(), first_body_write_after: None };

    send_file(&mut response, source, len, "application/octet-stream", true).unwrap();

    assert_eq!(consumed.get(), len);
    assert_eq!(written.get(), len);
    assert!(response.first_body_write_after.unwrap() <= MAX_READ_AHEAD);
    let headers = String::from_utf8(response.headers).unwrap();
    assert!(headers.contains(&format!("Content-Length: {len}\r\n")));
    assert!(headers.contains("Content-Type: application/octet-stream\r\n"));
}

#[test]
fn head_response_never_reads_the_source() {
    struct RefuseReads(Rc<Cell<usize>>);
    impl Read for RefuseReads {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            self.0.set(self.0.get() + 1);
            Err(std::io::Error::other("HEAD must not read file contents"))
        }
    }

    let reads = Rc::new(Cell::new(0));
    let mut response = Response { headers: Vec::new(), consumed: Rc::new(Cell::new(0)), written: Rc::new(Cell::new(0)), first_body_write_after: None };
    send_file(&mut response, RefuseReads(reads.clone()), 8 * 1024 * 1024, "text/plain", false).unwrap();

    assert_eq!(reads.get(), 0);
    assert_eq!(response.written.get(), 0);
    assert_eq!(response.first_body_write_after, None);
    let headers = String::from_utf8(response.headers).unwrap();
    assert!(headers.contains("Content-Length: 8388608\r\n"));
    assert!(headers.contains("Content-Type: text/plain\r\n"));
}
