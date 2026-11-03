//! PlayStation 3 title files: `PARAM.SFO` (the title's name, id and version) and what kind of
//! executable an `EBOOT.BIN` or `.elf` is: a plain PowerPC ELF, an unencrypted development SELF,
//! or an encrypted retail or store SELF.
//!
//! Nothing here decrypts anything or holds any key. An encrypted executable is identified as
//! encrypted and left alone.

use crate::FormatError;
use crate::bytes::Reader;
use serde::Serialize;
use std::collections::BTreeMap;

/// A `PARAM.SFO` larger than this isn't one.
const MAX_SFO: usize = 1 << 20;
const MAX_SFO_ENTRIES: u32 = 512;

/// A `PARAM.SFO` value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum SfoValue {
    Text(String),
    Number(u32),
}

/// A parsed `PARAM.SFO`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sfo {
    pub values: BTreeMap<String, SfoValue>,
}

impl Sfo {
    pub fn text(&self, key: &str) -> Option<&str> {
        match self.values.get(key) {
            Some(SfoValue::Text(s)) => Some(s.as_str()),
            _ => None,
        }
    }
    pub fn title(&self) -> Option<&str> {
        self.text("TITLE")
    }
    /// `BLUS30001`-style id.
    pub fn title_id(&self) -> Option<&str> {
        self.text("TITLE_ID")
    }
    /// `DG` disc game, `HG` hard-disk game, `GD` game data, …
    pub fn category(&self) -> Option<&str> {
        self.text("CATEGORY")
    }
}

/// Parse a `PARAM.SFO`.
pub fn parse_sfo(data: &[u8]) -> Result<Sfo, FormatError> {
    if data.len() > MAX_SFO {
        return Err(FormatError::TooLarge(format!("a PARAM.SFO of {} bytes", data.len())));
    }
    if data.get(..4) != Some(b"\0PSF".as_slice()) {
        return Err(FormatError::Unsupported("not a PARAM.SFO (no PSF mark)".into()));
    }
    let r = Reader::new(data);
    let (Some(keys), Some(values), Some(count)) = (r.u32_le(8), r.u32_le(12), r.u32_le(16)) else {
        return Err(FormatError::Truncated("PARAM.SFO header".into()));
    };
    if count > MAX_SFO_ENTRIES {
        return Err(FormatError::TooLarge(format!("{count} PARAM.SFO entries")));
    }
    let mut out = BTreeMap::new();
    for i in 0..count as usize {
        let e = 20 + i * 16;
        let (Some(key_off), Some(fmt), Some(len), Some(off)) = (r.u16_le(e), r.u16_le(e + 2), r.u32_le(e + 4), r.u32_le(e + 12)) else {
            return Err(FormatError::Truncated("PARAM.SFO index".into()));
        };
        let Some(key) = (keys as usize).checked_add(usize::from(key_off)).and_then(|o| r.cstr(o, 64)) else {
            return Err(FormatError::Malformed("a PARAM.SFO key points outside the file".into()));
        };
        let at = (values as usize).checked_add(off as usize).ok_or_else(|| FormatError::Malformed("PARAM.SFO value".into()))?;
        let value = match fmt {
            0x0404 => r.u32_le(at).map(SfoValue::Number),
            _ => r.slice(at, len as usize).map(|b| {
                let b = b.split(|c| *c == 0).next().unwrap_or_default();
                SfoValue::Text(String::from_utf8_lossy(b).into_owned())
            }),
        };
        let value = value.ok_or_else(|| FormatError::Malformed(format!("PARAM.SFO value {key} points outside the file")))?;
        out.insert(key, value);
    }
    Ok(Sfo { values: out })
}

/// What kind of PS3 executable a file is, from its first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutableKind {
    /// A plain 64-bit big-endian PowerPC ELF: what a recompiler reads.
    PpuElf,
    /// A plain SPU ELF (a co-processor program, not the title's main executable).
    SpuElf,
    /// A development (fake-signed) SELF: the ELF inside is stored unencrypted.
    DevelopmentSelf,
    /// A retail SELF: the ELF inside is encrypted.
    RetailSelf,
    /// A store (NPDRM) SELF: encrypted and tied to a licence.
    StoreSelf,
    /// Some other file.
    Other,
}

impl ExecutableKind {
    /// Whether its code can be read as it is (no decryption needed).
    pub fn readable(self) -> bool {
        matches!(self, ExecutableKind::PpuElf | ExecutableKind::DevelopmentSelf)
    }
}

/// Bytes [`classify`] needs at most (the SCE header and its application info).
pub const CLASSIFY_LEN: usize = 0x200;

/// What kind of executable `head` (the file's first [`CLASSIFY_LEN`] bytes, or fewer) starts.
pub fn classify(head: &[u8]) -> ExecutableKind {
    let r = Reader::new(head);
    if head.get(..4) == Some(b"\x7fELF".as_slice()) {
        // 64-bit, big-endian, machine 21 (PPC64) or 23 (SPU, 32-bit).
        return match (r.u8(4), r.u8(5), r.u16_be(0x12)) {
            (Some(2), Some(2), Some(21)) => ExecutableKind::PpuElf,
            (Some(1), Some(2), Some(23)) => ExecutableKind::SpuElf,
            _ => ExecutableKind::Other,
        };
    }
    if head.get(..4) != Some(b"SCE\0".as_slice()) || r.u16_be(10) != Some(1) {
        return ExecutableKind::Other;
    }
    // key_revision 0x8000 marks a development (fake-signed) SELF, whose contents aren't encrypted.
    if r.u16_be(8) == Some(0x8000) {
        return ExecutableKind::DevelopmentSelf;
    }
    // The extended header's application info says whether it's a store (NPDRM) title: type 8.
    let npdrm = r.u64_be(0x28).and_then(|o| usize::try_from(o).ok()).and_then(|o| o.checked_add(12)).and_then(|o| r.u32_be(o)) == Some(8);
    if npdrm { ExecutableKind::StoreSelf } else { ExecutableKind::RetailSelf }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sfo(entries: &[(&str, Result<&str, u32>)]) -> Vec<u8> {
        let (mut keys, mut vals, mut index) = (Vec::new(), Vec::new(), Vec::new());
        for (k, v) in entries {
            let ko = keys.len() as u16;
            keys.extend(k.as_bytes());
            keys.push(0);
            let vo = vals.len() as u32;
            let (fmt, bytes) = match v {
                Ok(s) => (0x0204u16, [s.as_bytes(), &[0]].concat()),
                Err(n) => (0x0404, n.to_le_bytes().to_vec()),
            };
            index.extend(ko.to_le_bytes());
            index.extend(fmt.to_le_bytes());
            index.extend((bytes.len() as u32).to_le_bytes());
            index.extend((bytes.len() as u32).to_le_bytes());
            index.extend(vo.to_le_bytes());
            vals.extend(bytes);
        }
        let key_start = 20 + index.len() as u32;
        let val_start = key_start + keys.len() as u32;
        let mut d = b"\0PSF".to_vec();
        d.extend(0x0101u32.to_le_bytes());
        d.extend(key_start.to_le_bytes());
        d.extend(val_start.to_le_bytes());
        d.extend((entries.len() as u32).to_le_bytes());
        d.extend(index);
        d.extend(keys);
        d.extend(vals);
        d
    }

    #[test]
    fn reads_param_sfo() {
        let d = sfo(&[("TITLE", Ok("Sample Game")), ("TITLE_ID", Ok("BLUS00000")), ("CATEGORY", Ok("DG")), ("PARENTAL_LEVEL", Err(3))]);
        let s = parse_sfo(&d).unwrap();
        assert_eq!(s.title(), Some("Sample Game"));
        assert_eq!(s.title_id(), Some("BLUS00000"));
        assert_eq!(s.category(), Some("DG"));
        assert_eq!(s.values.get("PARENTAL_LEVEL"), Some(&SfoValue::Number(3)));
    }

    #[test]
    fn hostile_sfo_is_an_error() {
        assert!(parse_sfo(b"\0PSF").is_err());
        let mut d = sfo(&[("TITLE", Ok("x"))]);
        d[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_sfo(&d).is_err());
        let mut d = sfo(&[("TITLE", Ok("x"))]);
        d[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_sfo(&d).is_err());
        for cut in 0..sfo(&[("TITLE", Ok("abc"))]).len() {
            let _ = parse_sfo(&sfo(&[("TITLE", Ok("abc"))])[..cut]);
        }
    }

    #[test]
    fn classifies_executables() {
        let mut elf = vec![0u8; 64];
        elf[..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2;
        elf[5] = 2;
        elf[0x12..0x14].copy_from_slice(&21u16.to_be_bytes());
        assert_eq!(classify(&elf), ExecutableKind::PpuElf);
        elf[4] = 1;
        elf[0x12..0x14].copy_from_slice(&23u16.to_be_bytes());
        assert_eq!(classify(&elf), ExecutableKind::SpuElf);
        elf[0x12..0x14].copy_from_slice(&62u16.to_be_bytes());
        assert_eq!(classify(&elf), ExecutableKind::Other);

        let mut s = vec![0u8; 0x100];
        s[..4].copy_from_slice(b"SCE\0");
        s[4..8].copy_from_slice(&2u32.to_be_bytes());
        s[10..12].copy_from_slice(&1u16.to_be_bytes());
        s[0x28..0x30].copy_from_slice(&0x70u64.to_be_bytes());
        s[0x70 + 12..0x70 + 16].copy_from_slice(&4u32.to_be_bytes());
        assert_eq!(classify(&s), ExecutableKind::RetailSelf);
        s[0x70 + 12..0x70 + 16].copy_from_slice(&8u32.to_be_bytes());
        assert_eq!(classify(&s), ExecutableKind::StoreSelf);
        s[8..10].copy_from_slice(&0x8000u16.to_be_bytes());
        assert_eq!(classify(&s), ExecutableKind::DevelopmentSelf);
        assert!(ExecutableKind::DevelopmentSelf.readable());
        assert!(!ExecutableKind::RetailSelf.readable());
        // An application-info offset far past the end.
        s[8..10].copy_from_slice(&0u16.to_be_bytes());
        s[0x28..0x30].copy_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(classify(&s), ExecutableKind::RetailSelf);
        assert_eq!(classify(b"SC"), ExecutableKind::Other);
    }
}
