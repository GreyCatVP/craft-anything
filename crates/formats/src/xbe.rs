//! Original Xbox executables (`.xbe`): the image header, the title's certificate, the sections and
//! the XDK library versions the title was linked against. Only header bytes are read; nothing is
//! decrypted or verified (the signature is reported as present, never checked).
//!
//! Every address in the header is a virtual address inside the header block, which is loaded at
//! `base`; the file offset of such an address is `address - base`.

use crate::FormatError;
use crate::bytes::Reader;
use serde::Serialize;

/// "XBEH".
pub const MAGIC: &[u8; 4] = b"XBEH";
/// Header block read at most (real titles keep it to a few KiB).
const MAX_HEADERS: u32 = 16 << 20;
/// Sections and library entries read at most.
const MAX_SECTIONS: u32 = 1024;
const MAX_LIBRARIES: u32 = 256;

/// One XDK library the title was linked against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Library {
    /// Up to 8 characters, e.g. `XAPILIB`, `D3D8`, `DSOUND`.
    pub name: String,
    pub major: u16,
    pub minor: u16,
    /// The XDK build number (e.g. 5849), which picks the signatures a recompiler matches against.
    pub build: u16,
    pub qfe: u16,
    pub debug: bool,
}

/// One section of the image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Section {
    pub name: String,
    pub virtual_address: u32,
    pub virtual_size: u32,
    pub raw_offset: u32,
    pub raw_size: u32,
    pub executable: bool,
    pub writable: bool,
}

/// What an XBE's header says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Xbe {
    pub base: u32,
    pub image_size: u32,
    /// The certificate's title id as the usual 8 hex digits (`4D530004`).
    pub title_id: String,
    /// The two publisher letters and the number most people know a title by (`MS-004`).
    pub title_code: String,
    pub title_name: String,
    /// Regions the title is allowed in: `north_america`, `japan`, `rest_of_world`, `manufacturing`.
    pub regions: Vec<&'static str>,
    pub disc_number: u32,
    pub version: u32,
    pub sections: Vec<Section>,
    pub libraries: Vec<Library>,
}

impl Xbe {
    /// The highest XDK build among the libraries (what a signature set is chosen by).
    pub fn xdk_build(&self) -> Option<u16> {
        self.libraries.iter().map(|l| l.build).max()
    }
}

/// Whether `data` starts like an XBE.
pub fn is_xbe(data: &[u8]) -> bool {
    data.get(..4) == Some(MAGIC.as_slice())
}

/// Parse an XBE from its first bytes. `data` must hold the whole header block (`size_of_headers`,
/// at offset 0x108); the rest of the file isn't needed.
pub fn parse(data: &[u8]) -> Result<Xbe, FormatError> {
    if !is_xbe(data) {
        return Err(FormatError::Unsupported("not an Xbox executable (no XBEH mark)".into()));
    }
    let r = Reader::new(data);
    let u32_at = |off: usize, what: &str| r.u32_le(off).ok_or_else(|| FormatError::Truncated(what.into()));
    let base = u32_at(0x104, "image header")?;
    let headers = u32_at(0x108, "image header")?;
    if headers > MAX_HEADERS {
        return Err(FormatError::TooLarge(format!("a header block of {headers} bytes")));
    }
    let image_size = u32_at(0x10C, "image header")?;
    let cert_addr = u32_at(0x118, "image header")?;
    let section_count = u32_at(0x11C, "image header")?;
    let section_addr = u32_at(0x120, "image header")?;
    let library_count = u32_at(0x160, "image header")?;
    let library_addr = u32_at(0x164, "image header")?;
    // A header address as a file offset inside the header block.
    let at = |addr: u32, what: &str| -> Result<usize, FormatError> {
        let off = addr.checked_sub(base).filter(|o| *o < headers).ok_or_else(|| FormatError::Malformed(format!("{what} points outside the header")))?;
        Ok(off as usize)
    };

    let cert = at(cert_addr, "the certificate")?;
    let title = r.u32_le(cert + 8).ok_or_else(|| FormatError::Truncated("certificate".into()))?;
    let mut name = Vec::new();
    for i in 0..40 {
        match r.u16_le(cert + 0x0C + i * 2) {
            Some(0) | None => break,
            Some(c) => name.push(c),
        }
    }
    let region_bits = r.u32_le(cert + 0xA0).unwrap_or(0);
    let regions = [(1u32, "north_america"), (2, "japan"), (4, "rest_of_world"), (0x8000_0000, "manufacturing")]
        .into_iter()
        .filter(|(bit, _)| region_bits & bit != 0)
        .map(|(_, n)| n)
        .collect();
    let publisher: String = [(title >> 24) as u8, (title >> 16) as u8].iter().map(|b| if b.is_ascii_graphic() { *b as char } else { '?' }).collect();

    let mut sections = Vec::new();
    if section_count > MAX_SECTIONS {
        return Err(FormatError::TooLarge(format!("{section_count} sections")));
    }
    if section_count > 0 {
        let first = at(section_addr, "the section table")?;
        for i in 0..section_count as usize {
            let s = first
                .checked_add(i.checked_mul(56).ok_or_else(|| FormatError::Malformed("section table".into()))?)
                .ok_or_else(|| FormatError::Malformed("section table".into()))?;
            let field = |n: usize| r.u32_le(s + n * 4).ok_or_else(|| FormatError::Truncated("section table".into()));
            let flags = field(0)?;
            let name = at(field(5)?, "a section name").ok().and_then(|o| r.cstr(o, 64)).unwrap_or_default();
            sections.push(Section {
                name,
                virtual_address: field(1)?,
                virtual_size: field(2)?,
                raw_offset: field(3)?,
                raw_size: field(4)?,
                writable: flags & 1 != 0,
                executable: flags & 4 != 0,
            });
        }
    }

    let mut libraries = Vec::new();
    if library_count > MAX_LIBRARIES {
        return Err(FormatError::TooLarge(format!("{library_count} library versions")));
    }
    if library_count > 0 {
        let first = at(library_addr, "the library versions")?;
        for i in 0..library_count as usize {
            let l = first + i * 16;
            let raw = r.slice(l, 8).ok_or_else(|| FormatError::Truncated("library versions".into()))?;
            let name: String = raw.iter().take_while(|b| **b != 0).map(|b| if b.is_ascii_graphic() { *b as char } else { '?' }).collect();
            let word = |n: usize| r.u16_le(l + 8 + n * 2).ok_or_else(|| FormatError::Truncated("library versions".into()));
            let flags = word(3)?;
            libraries.push(Library { name, major: word(0)?, minor: word(1)?, build: word(2)?, qfe: flags & 0x1FFF, debug: flags & 0x8000 != 0 });
        }
    }

    Ok(Xbe {
        base,
        image_size,
        title_id: format!("{title:08X}"),
        title_code: format!("{publisher}-{:03}", title & 0xFFFF),
        title_name: String::from_utf16_lossy(&name),
        regions,
        disc_number: r.u32_le(cert + 0xA8).unwrap_or(0),
        version: r.u32_le(cert + 0xAC).unwrap_or(0),
        sections,
        libraries,
    })
}

/// How many bytes to read from the start of a file to [`parse`] it, or `None` if `head` (at least
/// 0x10C bytes) isn't an XBE.
pub fn header_len(head: &[u8]) -> Option<u32> {
    if !is_xbe(head) {
        return None;
    }
    Reader::new(head).u32_le(0x108).filter(|n| *n <= MAX_HEADERS)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small synthetic XBE header: one section, two libraries, a certificate.
    pub fn sample(title_name: &str, builds: &[(&str, u16)]) -> Vec<u8> {
        let base = 0x10000u32;
        let mut d = vec![0u8; 0x1000];
        d[..4].copy_from_slice(MAGIC);
        let put = |d: &mut Vec<u8>, off: usize, v: u32| d[off..off + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, 0x104, base);
        put(&mut d, 0x108, 0x1000);
        put(&mut d, 0x10C, 0x20000);
        put(&mut d, 0x118, base + 0x200);
        put(&mut d, 0x11C, 1);
        put(&mut d, 0x120, base + 0x400);
        put(&mut d, 0x160, builds.len() as u32);
        put(&mut d, 0x164, base + 0x600);
        // certificate
        put(&mut d, 0x208, 0x4D53_0004);
        for (i, c) in title_name.encode_utf16().take(40).enumerate() {
            d[0x20C + i * 2..0x20E + i * 2].copy_from_slice(&c.to_le_bytes());
        }
        put(&mut d, 0x2A0, 1 | 4);
        // section ".text"
        put(&mut d, 0x400, 4 | 2);
        put(&mut d, 0x404, 0x11000);
        put(&mut d, 0x408, 0x8000);
        put(&mut d, 0x40C, 0x1000);
        put(&mut d, 0x410, 0x8000);
        put(&mut d, 0x414, base + 0x800);
        d[0x800..0x805].copy_from_slice(b".text");
        for (i, (name, build)) in builds.iter().enumerate() {
            let l = 0x600 + i * 16;
            d[l..l + name.len()].copy_from_slice(name.as_bytes());
            d[l + 8..l + 10].copy_from_slice(&1u16.to_le_bytes());
            d[l + 12..l + 14].copy_from_slice(&build.to_le_bytes());
            d[l + 14..l + 16].copy_from_slice(&0x4001u16.to_le_bytes());
        }
        d
    }

    #[test]
    fn reads_title_sections_and_libraries() {
        let d = sample("Sample Game", &[("XAPILIB", 5849), ("D3D8", 5849)]);
        assert_eq!(header_len(&d), Some(0x1000));
        let x = parse(&d).unwrap();
        assert_eq!(x.title_name, "Sample Game");
        assert_eq!(x.title_id, "4D530004");
        assert_eq!(x.title_code, "MS-004");
        assert_eq!(x.regions, vec!["north_america", "rest_of_world"]);
        assert_eq!(x.sections.len(), 1);
        assert_eq!(x.sections[0].name, ".text");
        assert!(x.sections[0].executable);
        assert_eq!(x.libraries[1].name, "D3D8");
        assert_eq!(x.libraries[1].qfe, 1);
        assert_eq!(x.xdk_build(), Some(5849));
    }

    #[test]
    fn hostile_headers_are_errors_not_crashes() {
        assert!(parse(b"MZ").is_err());
        assert!(parse(b"XBEH").is_err());
        let mut d = sample("x", &[("D3D8", 4627)]);
        d[0x118..0x11C].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        assert!(parse(&d).is_err());
        let mut d = sample("x", &[("D3D8", 4627)]);
        d[0x160..0x164].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse(&d).is_err());
        let mut d = sample("x", &[]);
        d[0x11C..0x120].copy_from_slice(&200u32.to_le_bytes());
        assert!(parse(&d).is_err());
        for cut in [0x110, 0x300, 0x500, 0x700] {
            let d = sample("x", &[("D3D8", 5849)]);
            let _ = parse(&d[..cut]);
        }
    }
}
