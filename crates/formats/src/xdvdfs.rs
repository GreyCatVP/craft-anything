//! Original Xbox disc images (XDVDFS): find the game partition, list its files and say where each
//! file's bytes are, so the caller can stream them out. Works on a whole-disc dump (the game
//! partition sits at a fixed offset for each disc generation) or an image of the game partition
//! alone. Reads go through [`ReadAt`], so a multi-gigabyte image is never loaded; nothing here
//! does I/O itself.
//!
//! Xbox discs carry no encryption on the game partition; nothing is decrypted or bypassed.

use crate::FormatError;
use serde::Serialize;

pub const SECTOR: u64 = 2048;
/// The volume descriptor's sector inside the partition.
const VOLUME_SECTOR: u64 = 32;
pub const MAGIC: &[u8; 20] = b"MICROSOFT*XBOX*MEDIA";
/// Where the game partition starts: an image of the partition alone, then whole-disc dumps of the
/// disc generations (XGD1, XGD2, XGD2 with a larger video partition, XGD3).
pub const PARTITION_OFFSETS: &[u64] = &[0, 0x1830_0000, 0x0FD9_0000, 0x89D8_0000, 0x0208_0000];
/// One directory table read at most.
const MAX_DIR: u32 = 16 << 20;
/// Entries listed and folders deep, at most.
pub const MAX_ENTRIES: usize = 200_000;
const MAX_DEPTH: usize = 32;

/// Reads bytes at an absolute offset of the image. Returns `None` past the end or on a read error.
pub trait ReadAt {
    fn read_at(&mut self, offset: u64, len: usize) -> Option<Vec<u8>>;
}

impl ReadAt for &[u8] {
    fn read_at(&mut self, offset: u64, len: usize) -> Option<Vec<u8>> {
        let start = usize::try_from(offset).ok()?;
        self.get(start..start.checked_add(len)?).map(<[u8]>::to_vec)
    }
}

/// The game partition of an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Volume {
    /// Byte offset of the partition in the image.
    pub partition: u64,
    pub root_sector: u32,
    pub root_size: u32,
}

/// One file or folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// Path inside the partition, `/`-separated, without a leading slash.
    pub path: String,
    pub dir: bool,
    pub size: u32,
    /// Absolute byte offset of the file's data in the image.
    pub offset: u64,
}

/// What [`list`] found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// A limit (entries, depth, a damaged or cyclic table) stopped the walk early.
    pub truncated: bool,
}

/// Find the game partition by its volume descriptor (the mark at the start and end of it).
pub fn open(src: &mut dyn ReadAt) -> Result<Volume, FormatError> {
    for &partition in PARTITION_OFFSETS {
        let Some(at) = partition.checked_add(VOLUME_SECTOR * SECTOR) else { continue };
        let Some(v) = src.read_at(at, SECTOR as usize) else { continue };
        if v.get(..20) != Some(MAGIC.as_slice()) || v.get(0x7EC..0x800) != Some(MAGIC.as_slice()) {
            continue;
        }
        let word = |o: usize| v.get(o..o + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
        let (Some(root_sector), Some(root_size)) = (word(20), word(24)) else { continue };
        return Ok(Volume { partition, root_sector, root_size });
    }
    Err(FormatError::Unsupported("no Xbox game partition found (not an Xbox disc image, or a damaged one)".into()))
}

/// List every file and folder of the partition, folders before what's in them.
pub fn list(src: &mut dyn ReadAt, vol: &Volume) -> Listing {
    let mut out = Listing { entries: Vec::new(), truncated: false };
    let mut seen_tables = std::collections::HashSet::new();
    walk(src, vol, vol.root_sector, vol.root_size, "", 0, &mut out, &mut seen_tables);
    out
}

#[allow(clippy::too_many_arguments)]
fn walk(src: &mut dyn ReadAt, vol: &Volume, sector: u32, size: u32, prefix: &str, depth: usize, out: &mut Listing, seen: &mut std::collections::HashSet<u32>) {
    if size == 0 {
        return;
    }
    if depth > MAX_DEPTH || size > MAX_DIR || !seen.insert(sector) {
        out.truncated = true;
        return;
    }
    let Some(table) = vol.partition.checked_add(u64::from(sector) * SECTOR).and_then(|at| src.read_at(at, size as usize)) else {
        out.truncated = true;
        return;
    };
    // The table is a binary tree; each node names its left and right children by offset / 4.
    let mut stack = vec![0usize];
    let mut visited = std::collections::HashSet::new();
    let mut subdirs = Vec::new();
    while let Some(off) = stack.pop() {
        if out.entries.len() >= MAX_ENTRIES {
            out.truncated = true;
            return;
        }
        if !visited.insert(off) {
            out.truncated = true;
            continue;
        }
        let Some(head) = table.get(off..off + 14) else {
            out.truncated = true;
            continue;
        };
        let left = u16::from_le_bytes([head[0], head[1]]);
        let right = u16::from_le_bytes([head[2], head[3]]);
        if left == 0xFFFF && right == 0xFFFF {
            continue; // padding: an empty folder
        }
        let start = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
        let len = u32::from_le_bytes([head[8], head[9], head[10], head[11]]);
        let attrs = head[12];
        let name_len = usize::from(head[13]);
        let Some(name) = table.get(off + 14..off + 14 + name_len) else {
            out.truncated = true;
            continue;
        };
        let name: String = name.iter().map(|b| if *b == b'/' || *b == b'\\' || *b < 0x20 { '_' } else { *b as char }).collect();
        if name.is_empty() || name == "." || name == ".." {
            out.truncated = true;
        } else {
            let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let dir = attrs & 0x10 != 0;
            let offset = vol.partition.saturating_add(u64::from(start).saturating_mul(SECTOR));
            if dir {
                subdirs.push((start, len, path.clone()));
            }
            out.entries.push(Entry { path, dir, size: len, offset });
        }
        for child in [right, left] {
            if child != 0 && child != 0xFFFF {
                stack.push(usize::from(child) * 4);
            }
        }
    }
    for (start, len, path) in subdirs {
        walk(src, vol, start, len, &path, depth + 1, out, seen);
    }
}

/// Find an entry by path, ignoring case (Xbox paths are case-insensitive).
pub fn find<'a>(listing: &'a Listing, path: &str) -> Option<&'a Entry> {
    listing.entries.iter().find(|e| e.path.eq_ignore_ascii_case(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One directory entry, padded to 4 bytes.
    fn entry(left: u16, right: u16, sector: u32, size: u32, dir: bool, name: &str) -> Vec<u8> {
        let mut e = Vec::new();
        e.extend(left.to_le_bytes());
        e.extend(right.to_le_bytes());
        e.extend(sector.to_le_bytes());
        e.extend(size.to_le_bytes());
        e.push(if dir { 0x10 } else { 0x20 });
        e.push(name.len() as u8);
        e.extend(name.as_bytes());
        while e.len() % 4 != 0 {
            e.push(0xFF);
        }
        e
    }

    /// A partition at `partition`: root with default.xbe and media/, media/ with intro.bik.
    fn image(partition: usize) -> Vec<u8> {
        let mut d = vec![0u8; partition + 50 * 2048];
        let vd = partition + 32 * 2048;
        d[vd..vd + 20].copy_from_slice(MAGIC);
        d[vd + 20..vd + 24].copy_from_slice(&40u32.to_le_bytes());
        d[vd + 24..vd + 28].copy_from_slice(&2048u32.to_le_bytes());
        d[vd + 0x7EC..vd + 0x800].copy_from_slice(MAGIC);
        let a = entry(0, 0, 44, 5, false, "default.xbe"); // 28 bytes: right child at 28/4 = 7
        let mut root = entry(0, 7, 44, 5, false, "default.xbe");
        assert_eq!(a.len(), 28);
        root.extend(entry(0, 0, 42, 2048, true, "media"));
        root.resize(2048, 0xFF);
        d[partition + 40 * 2048..partition + 41 * 2048].copy_from_slice(&root);
        let mut media = entry(0, 0, 45, 3, false, "intro.bik");
        media.resize(2048, 0xFF);
        d[partition + 42 * 2048..partition + 43 * 2048].copy_from_slice(&media);
        d[partition + 44 * 2048..partition + 44 * 2048 + 5].copy_from_slice(b"XBEH!");
        d
    }

    #[test]
    fn lists_a_partition_image_and_a_whole_disc() {
        for partition in [0usize, 0x0208_0000] {
            let d = image(partition);
            let mut src: &[u8] = &d;
            let vol = open(&mut src).unwrap();
            assert_eq!(vol.partition, partition as u64);
            let l = list(&mut src, &vol);
            assert!(!l.truncated, "{l:?}");
            let paths: Vec<_> = l.entries.iter().map(|e| e.path.as_str()).collect();
            assert_eq!(paths, ["default.xbe", "media", "media/intro.bik"]);
            let xbe = find(&l, "DEFAULT.XBE").unwrap();
            assert_eq!(src.read_at(xbe.offset, xbe.size as usize).unwrap(), b"XBEH!");
        }
    }

    #[test]
    fn hostile_tables_stop_without_crashing() {
        assert!(open(&mut (&b"nothing"[..])).is_err());
        let mut d = image(0);
        // The second root entry names itself as its right child: a cycle in the tree.
        d[40 * 2048 + 28 + 2..40 * 2048 + 28 + 4].copy_from_slice(&7u16.to_le_bytes());
        // media/ points back at the root table: a cycle between folders.
        d[40 * 2048 + 28 + 4..40 * 2048 + 28 + 8].copy_from_slice(&40u32.to_le_bytes());
        let mut src: &[u8] = &d;
        let vol = open(&mut src).unwrap();
        let l = list(&mut src, &vol);
        assert!(l.truncated);
        assert!(l.entries.len() < 10);
        // A root table past the end of the image.
        let mut d = image(0);
        d[32 * 2048 + 20..32 * 2048 + 24].copy_from_slice(&0x00FF_FFFFu32.to_le_bytes());
        let mut src: &[u8] = &d;
        let vol = open(&mut src).unwrap();
        assert!(list(&mut src, &vol).truncated);
    }
}
