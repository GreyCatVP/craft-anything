//! Original Xbox executables (`.xbe`): the image header, the title's certificate, the sections and
//! the XDK library versions the title was linked against. Only header bytes are read; nothing is
//! decrypted or verified (the signature is reported as present, never checked).
//!
//! Every address in the header is a virtual address inside the header block, which is loaded at
//! `base`; the file offset of such an address is `address - base`.
//!
//! The entry point and the kernel thunk table's address are stored XORed with a constant, one pair
//! for retail and one for debug builds (address encoding, not encryption); [`parse`] takes the pair
//! whose entry point lands inside the image. [`binary`] gives the shared executable model, so the
//! native analysis tools read an XBE like any other program.

use crate::FormatError;
use crate::binary::{Arch, Binary, Format, Import, ImportSlot};
use crate::bytes::Reader;
use serde::Serialize;

/// "XBEH".
pub const MAGIC: &[u8; 4] = b"XBEH";
/// Header block read at most (real titles keep it to a few KiB).
const MAX_HEADERS: u32 = 16 << 20;
/// Sections and library entries read at most.
const MAX_SECTIONS: u32 = 1024;
const MAX_LIBRARIES: u32 = 256;
/// Kernel imports read at most (the kernel exports fewer than 400).
const MAX_KERNEL_IMPORTS: usize = 1024;
/// (entry point, kernel thunk) XOR keys: retail, then debug.
const KEYS: [(u32, u32); 2] = [(0xA8FC_57AB, 0x5B6D_40B6), (0x9485_9D4B, 0xEFB1_F152)];

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
    /// Bytes of the header block, loaded at `base`.
    pub headers: u32,
    /// The decoded entry point, when one of the key pairs puts it inside the image.
    pub entry: Option<u32>,
    /// The decoded address of the kernel thunk table (the kernel imports, by ordinal).
    pub kernel_thunk: Option<u32>,
    /// `retail` or `debug`: which key pair decoded the entry point.
    pub keys: Option<&'static str>,
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

    let in_image = |va: u32| va.checked_sub(base).is_some_and(|o| o < image_size);
    let (entry_raw, thunk_raw) = (r.u32_le(0x128).unwrap_or(0), r.u32_le(0x158).unwrap_or(0));
    let found = KEYS.iter().zip(["retail", "debug"]).find(|((e, _), _)| in_image(entry_raw ^ e));
    let (entry, kernel_thunk, keys) = match found {
        Some(((e, t), kind)) => (Some(entry_raw ^ e), Some(thunk_raw ^ t).filter(|t| in_image(*t)), Some(kind)),
        None => (None, None, None),
    };

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
        headers,
        entry,
        kernel_thunk,
        keys,
    })
}

/// The file's bytes at a virtual address, at most `n`, from the section (or header block) that
/// holds it.
pub fn read<'a>(x: &Xbe, data: &'a [u8], va: u32, n: usize) -> Option<&'a [u8]> {
    let (start, avail) = if let Some(o) = va.checked_sub(x.base).filter(|o| *o < x.headers) {
        (o, x.headers - o)
    } else {
        let s = x.sections.iter().find(|s| va.checked_sub(s.virtual_address).is_some_and(|o| o < s.raw_size))?;
        let o = va - s.virtual_address;
        (s.raw_offset.checked_add(o)?, s.raw_size - o)
    };
    let start = start as usize;
    data.get(start..start.checked_add(n.min(avail as usize))?.min(data.len()))
}

/// The kernel imports: (thunk slot address, ordinal), in table order.
pub fn kernel_imports(x: &Xbe, data: &[u8]) -> Vec<(u32, u32)> {
    let Some(table) = x.kernel_thunk else { return Vec::new() };
    let mut out = Vec::new();
    for i in 0..MAX_KERNEL_IMPORTS as u32 {
        let Some(slot) = table.checked_add(i * 4) else { break };
        let Some(w) = read(x, data, slot, 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes) else { break };
        if w == 0 {
            break;
        }
        out.push((slot, w & 0x7FFF_FFFF));
    }
    out
}

/// The shared executable model of a whole XBE file: the header block and the sections, the
/// decoded entry point, the kernel imports as named slots (`xboxkrnl_<ordinal>`), and the XDK
/// libraries.
pub fn binary(data: &[u8]) -> Result<Binary, FormatError> {
    let x = parse(data)?;
    let mut b = Binary::raw(Arch::X86, u64::from(x.base), 0, 0);
    b.format = Format::Xbe;
    b.arch_name = "x86".into();
    b.entry = x.entry.map(u64::from);
    b.sections = std::iter::once(crate::binary::Section {
        name: "headers".into(),
        segment: None,
        addr: u64::from(x.base),
        size: u64::from(x.headers),
        file_offset: Some(0),
        file_size: u64::from(x.headers).min(data.len() as u64),
        read: true,
        write: false,
        exec: false,
        kind: "rodata",
        entropy: None,
        alloc: true,
    })
    .chain(x.sections.iter().map(|s| {
        let in_file = u64::from(s.raw_offset) < data.len() as u64;
        crate::binary::Section {
            name: s.name.clone(),
            segment: None,
            addr: u64::from(s.virtual_address),
            size: u64::from(s.virtual_size),
            file_offset: in_file.then_some(u64::from(s.raw_offset)),
            file_size: if in_file { u64::from(s.raw_size).min(data.len() as u64 - u64::from(s.raw_offset)) } else { 0 },
            read: true,
            write: s.writable,
            exec: s.executable,
            kind: if s.executable {
                "code"
            } else if s.writable {
                "data"
            } else {
                "rodata"
            },
            entropy: None,
            alloc: true,
        }
    }))
    .collect();
    for (slot, ordinal) in kernel_imports(&x, data) {
        let name = format!("xboxkrnl_{ordinal}");
        b.imports.push(Import { name: name.clone(), library: Some("xboxkrnl.exe".into()), ordinal: u16::try_from(ordinal).ok(), slot: Some(u64::from(slot)) });
        b.import_slots.push(ImportSlot { addr: u64::from(slot), name, library: Some("xboxkrnl.exe".into()), kind: "slot" });
    }
    if !b.imports.is_empty() {
        b.libraries.push("xboxkrnl.exe".into());
    }
    b.libraries.extend(x.libraries.iter().map(|l| format!("{} {}.{}.{}", l.name, l.major, l.minor, l.build)));
    b.info.insert("title".into(), x.title_name.clone());
    b.info.insert("title_id".into(), x.title_id.clone());
    b.info.insert("title_code".into(), x.title_code.clone());
    if let Some(build) = x.xdk_build() {
        b.info.insert("xdk_build".into(), build.to_string());
    }
    if let Some(k) = x.keys {
        b.info.insert("build_kind".into(), k.into());
    } else {
        b.warnings.push("the entry point decodes outside the image with both the retail and the debug key".into());
    }
    b.details = serde_json::to_value(&x).unwrap_or_default();
    Ok(b)
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
    fn decodes_entry_and_kernel_imports_into_the_shared_model() {
        let mut d = sample("Sample Game", &[("XAPILIB", 5849)]);
        let (ek, tk) = KEYS[0];
        d[0x128..0x12C].copy_from_slice(&(0x11000u32 ^ ek).to_le_bytes());
        d[0x158..0x15C].copy_from_slice(&(0x10900u32 ^ tk).to_le_bytes());
        for (i, w) in [0x8000_0001u32, 0x8000_0102, 0].iter().enumerate() {
            d[0x900 + i * 4..0x904 + i * 4].copy_from_slice(&w.to_le_bytes());
        }
        let x = parse(&d).unwrap();
        assert_eq!((x.entry, x.kernel_thunk, x.keys), (Some(0x11000), Some(0x10900), Some("retail")));
        assert_eq!(kernel_imports(&x, &d), vec![(0x10900, 1), (0x10904, 0x102)]);
        let b = crate::parse_binary(&d).unwrap();
        assert_eq!(b.format, Format::Xbe);
        assert_eq!(b.arch, Arch::X86);
        assert_eq!(b.entry, Some(0x11000));
        assert_eq!(b.import_slots.iter().map(|s| (s.addr, s.name.as_str())).collect::<Vec<_>>(), [(0x10900, "xboxkrnl_1"), (0x10904, "xboxkrnl_258")]);
        assert_eq!(b.sections.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["headers", ".text"]);
        // .text lies past the end of this header-only file: mapped, not file-backed
        assert_eq!(b.sections[1].file_offset, None);
        assert!(b.libraries.contains(&"XAPILIB 1.0.5849".to_string()));
        assert_eq!(b.info.get("title").map(String::as_str), Some("Sample Game"));
        // neither key lands inside the image: no entry, a warning, no panic
        d[0x128..0x12C].copy_from_slice(&0u32.to_le_bytes());
        let b = binary(&d).unwrap();
        assert_eq!(b.entry, None);
        assert!(b.imports.is_empty() && !b.warnings.is_empty());
    }

    #[test]
    fn a_thunk_table_running_off_the_file_stops() {
        let mut d = sample("x", &[]);
        let (ek, tk) = KEYS[0];
        d[0x128..0x12C].copy_from_slice(&(0x10800u32 ^ ek).to_le_bytes());
        d[0x158..0x15C].copy_from_slice(&(0x10FFCu32 ^ tk).to_le_bytes());
        d[0xFFC..0x1000].copy_from_slice(&0x8000_0005u32.to_le_bytes());
        let x = parse(&d).unwrap();
        assert_eq!(kernel_imports(&x, &d), vec![(0x10FFC, 5)]);
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
