//! One entry read straight from a ZIP file without loading the file: only its end record, its
//! central directory and that entry's bytes are read. For archives too big to hold in memory just
//! to look at one small entry (a JAR's manifest).

use crate::bytes::Reader;
use crate::zip::{self, ZipError};
use std::io::{Read, Seek, SeekFrom};

/// Most central directory read; a bigger one is refused rather than loaded.
const MAX_DIRECTORY: u64 = 64 << 20;

/// The entry named `name` (not a folder), decompressed to at most `max_bytes` and checked against
/// its CRC-32. `None`: the archive has no such entry. Classic archives only: a ZIP64 one is
/// `Unsupported`.
pub fn read_named_entry(file: &mut (impl Read + Seek), name: &str, max_bytes: u64) -> Result<Option<Vec<u8>>, ZipError> {
    let len = file.seek(SeekFrom::End(0)).map_err(io)?;
    let tail_start = len.saturating_sub(zip::EOCD_SEARCH as u64);
    let tail = read_at(file, tail_start, len - tail_start)?;
    let eocd = zip::find_eocd(&tail).ok_or(ZipError::NotZip)?;
    let r = Reader::new(&tail);
    let field16 = |at| r.u16_le(eocd + at).ok_or(ZipError::Truncated("end record"));
    let field32 = |at| r.u32_le(eocd + at).ok_or(ZipError::Truncated("end record"));
    let (entries, size, offset) = (field16(10)?, field32(12)?, field32(16)?);
    if entries == 0xffff || size == 0xffff_ffff || offset == 0xffff_ffff {
        return Err(ZipError::Unsupported("ZIP64 archives read in parts".into()));
    }
    let (size, offset) = (u64::from(size), u64::from(offset));
    if size > MAX_DIRECTORY {
        return Err(ZipError::TooLarge(MAX_DIRECTORY));
    }
    // The directory ends where the end record starts; anything before its stated offset is a
    // prepended stub, as `zip::parse` allows.
    let start = (tail_start + eocd as u64).checked_sub(size).ok_or(ZipError::Corrupt("directory starts before the file".into()))?;
    let prefix = start.checked_sub(offset).ok_or(ZipError::Corrupt("directory sits before its stated offset".into()))?;
    // The directory and its end record alone, with the directory's offset made 0 to match.
    let mut directory = read_at(file, start, size)?;
    let mut end = tail.get(eocd..).ok_or(ZipError::Truncated("end record"))?.to_vec();
    end.get_mut(16..20).ok_or(ZipError::Truncated("end record"))?.copy_from_slice(&[0; 4]);
    directory.extend_from_slice(&end);
    let archive = zip::parse(&directory, usize::MAX)?;
    let Some(mut entry) = archive.entries.into_iter().find(|e| e.name == name && !e.is_dir) else { return Ok(None) };
    if entry.compressed_size > max_bytes.saturating_add(1 << 16) {
        return Err(ZipError::TooLarge(max_bytes));
    }
    let at = entry.local_header_offset.checked_add(prefix).ok_or(ZipError::Corrupt("local header offset".into()))?;
    let header = read_at(file, at, 30)?;
    let fields = Reader::new(&header);
    let name_len = u64::from(fields.u16_le(26).ok_or(ZipError::Truncated("local header"))?);
    let extra_len = u64::from(fields.u16_le(28).ok_or(ZipError::Truncated("local header"))?);
    let local = read_at(file, at, 30 + name_len + extra_len + entry.compressed_size)?;
    entry.local_header_offset = 0;
    zip::read_entry(&local, &entry, max_bytes).map(Some)
}

/// Exactly `len` bytes from `at`; a file that ends sooner is truncated.
fn read_at(file: &mut (impl Read + Seek), at: u64, len: u64) -> Result<Vec<u8>, ZipError> {
    file.seek(SeekFrom::Start(at)).map_err(io)?;
    let mut out = Vec::new();
    file.take(len).read_to_end(&mut out).map_err(io)?;
    if (out.len() as u64) < len {
        return Err(ZipError::Truncated("file"));
    }
    Ok(out)
}

fn io(e: std::io::Error) -> ZipError {
    ZipError::Corrupt(format!("couldn't read it: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn an_entry_is_read_from_the_directory_without_the_whole_file() {
        let manifest: &[u8] = b"Manifest-Version: 1.0\r\nMain-Class: app.Main\r\n\r\n";
        let filler = vec![7u8; 200_000];
        let archive = craft_anything_testkit::electron::zip_stored(&[("app/Big.class", &filler), ("META-INF/MANIFEST.MF", manifest), ("META-INF/", b"")]);
        let read = |bytes: Vec<u8>, name: &str| read_named_entry(&mut Cursor::new(bytes), name, 1 << 20);
        assert_eq!(read(archive.clone(), "META-INF/MANIFEST.MF").unwrap().as_deref(), Some(manifest));
        assert_eq!(read(archive.clone(), "META-INF/MISSING").unwrap(), None);
        assert_eq!(read(archive.clone(), "META-INF/").unwrap(), None, "a folder isn't an entry to read");

        let mut stubbed = b"#!/bin/sh\nexec java -jar \"$0\" \"$@\"\n".to_vec();
        stubbed.extend_from_slice(&archive);
        assert_eq!(read(stubbed, "META-INF/MANIFEST.MF").unwrap().as_deref(), Some(manifest), "a launcher stub before the archive");

        assert_eq!(read(b"not a zip".to_vec(), "META-INF/MANIFEST.MF"), Err(ZipError::NotZip));
        assert!(read(archive[..archive.len() - 30].to_vec(), "META-INF/MANIFEST.MF").is_err(), "a cut-off file");
        assert_eq!(read_named_entry(&mut Cursor::new(archive), "app/Big.class", 1000), Err(ZipError::TooLarge(1000)));
    }
}
