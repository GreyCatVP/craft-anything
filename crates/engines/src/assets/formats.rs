//! What kind of file a provider's answer is, read from its first bytes. A generated asset is saved
//! under the extension of what it really is: a model asked for a PNG may send a JPEG, and one asked
//! for MP3 audio may send WAV, so the bytes are looked at rather than trusted to match the name.

/// A file format a generated asset can come back in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileType {
    /// The extension it is saved with (lower case, no dot).
    pub ext: &'static str,
    /// What people call it.
    pub name: &'static str,
    /// Its media type, for a picture sent on inline.
    pub media: &'static str,
}

const PNG: FileType = FileType { ext: "png", name: "PNG", media: "image/png" };
const JPEG: FileType = FileType { ext: "jpg", name: "JPEG", media: "image/jpeg" };
const WEBP: FileType = FileType { ext: "webp", name: "WebP", media: "image/webp" };
const GIF: FileType = FileType { ext: "gif", name: "GIF", media: "image/gif" };
const MP3: FileType = FileType { ext: "mp3", name: "MP3", media: "audio/mpeg" };
const WAV: FileType = FileType { ext: "wav", name: "WAV", media: "audio/wav" };
const OGG: FileType = FileType { ext: "ogg", name: "Ogg", media: "audio/ogg" };
const FLAC: FileType = FileType { ext: "flac", name: "FLAC", media: "audio/flac" };
const AAC: FileType = FileType { ext: "aac", name: "AAC", media: "audio/aac" };
const M4A: FileType = FileType { ext: "m4a", name: "M4A", media: "audio/mp4" };
const MP4: FileType = FileType { ext: "mp4", name: "MP4", media: "video/mp4" };
const GLB: FileType = FileType { ext: "glb", name: "GLB", media: "model/gltf-binary" };

/// Every format [`sniff`] can tell.
const ALL: [FileType; 12] = [PNG, JPEG, WEBP, GIF, MP3, WAV, OGG, FLAC, AAC, M4A, MP4, GLB];

/// The format of `bytes`, or `None` when they are none of the formats a generation comes back in.
pub fn sniff(bytes: &[u8]) -> Option<FileType> {
    let at = |from: usize, magic: &[u8]| bytes.get(from..from.saturating_add(magic.len())) == Some(magic);
    if at(0, b"\x89PNG\r\n\x1a\n") {
        return Some(PNG);
    }
    if at(0, b"\xff\xd8\xff") {
        return Some(JPEG);
    }
    if at(0, b"RIFF") && at(8, b"WEBP") {
        return Some(WEBP);
    }
    if at(0, b"RIFF") && at(8, b"WAVE") {
        return Some(WAV);
    }
    if at(0, b"GIF87a") || at(0, b"GIF89a") {
        return Some(GIF);
    }
    if at(0, b"OggS") {
        return Some(OGG);
    }
    if at(0, b"fLaC") {
        return Some(FLAC);
    }
    if at(0, b"glTF") {
        return Some(GLB);
    }
    if at(4, b"ftyp") {
        return Some(if at(8, b"M4A ") { M4A } else { MP4 });
    }
    if at(0, b"ID3") {
        return Some(MP3);
    }
    // A frame's sync word: eleven bits set. MPEG audio names a layer (I to III) and a version that
    // isn't the reserved one; AAC's ADTS frames have layer 00.
    match (bytes.first(), bytes.get(1)) {
        (Some(0xff), Some(&b)) if b & 0xf6 == 0xf0 => Some(AAC),
        (Some(0xff), Some(&b)) if b & 0xe0 == 0xe0 && b & 0x06 != 0 && b & 0x18 != 0x08 => Some(MP3),
        _ => None,
    }
}

/// What people call the format saved with extension `ext` ("PNG" for `png`), else the extension in
/// capitals.
pub fn name_of(ext: &str) -> String {
    ALL.iter().find(|f| same_extension(f.ext, ext)).map_or_else(|| ext.to_ascii_uppercase(), |f| f.name.to_string())
}

/// Whether two extensions name the same format: case aside, and `jpg` and `jpeg` alike.
pub fn same_extension(a: &str, b: &str) -> bool {
    let plain = |e: &str| {
        let e = e.trim_start_matches('.').to_ascii_lowercase();
        if e == "jpeg" { "jpg".to_string() } else { e }
    };
    plain(a) == plain(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_format_is_told_by_its_first_bytes() {
        let cases: [(&[u8], Option<&str>); 17] = [
            (b"\x89PNG\r\n\x1a\nrest", Some("png")),
            (b"\xff\xd8\xff\xe0JFIF", Some("jpg")),
            (b"RIFF\x24\x00\x00\x00WEBPVP8 ", Some("webp")),
            (b"RIFF\x24\x00\x00\x00WAVEfmt ", Some("wav")),
            (b"GIF89a....", Some("gif")),
            (b"OggS\x00\x02", Some("ogg")),
            (b"fLaC\x00\x00", Some("flac")),
            (b"ID3\x04\x00", Some("mp3")),
            // An MPEG-1 layer III frame with no tag before it.
            (b"\xff\xfb\x90\x64", Some("mp3")),
            // An MPEG-2 layer III frame.
            (b"\xff\xf3\x90\x64", Some("mp3")),
            // AAC in ADTS frames.
            (b"\xff\xf1\x50\x80", Some("aac")),
            (b"\x00\x00\x00\x20ftypM4A \x00", Some("m4a")),
            (b"\x00\x00\x00\x20ftypisom\x00", Some("mp4")),
            (b"glTF\x02\x00\x00\x00", Some("glb")),
            // Not any of them: text, an error page, too short, nothing.
            (b"{\"error\":\"no\"}", None),
            (b"\xff", None),
            (b"", None),
        ];
        for (bytes, ext) in cases {
            assert_eq!(sniff(bytes).map(|f| f.ext), ext, "{bytes:?}");
        }
        // A RIFF file of another kind, and a sync word with a reserved version, are neither.
        assert_eq!(sniff(b"RIFF\x24\x00\x00\x00AVI LIST"), None);
        assert_eq!(sniff(b"\xff\xeb\x90\x64"), None);
    }

    #[test]
    fn extensions_compare_as_formats() {
        assert!(same_extension("jpg", "JPEG") && same_extension(".PNG", "png") && !same_extension("png", "webp"));
        assert_eq!((name_of("png"), name_of("MP3"), name_of("jpeg"), name_of("xyz")), ("PNG".into(), "MP3".into(), "JPEG".into(), "XYZ".into()));
    }
}
