//! Detects a file's type from its magic bytes (file signature), without parsing the full file.
//! Target-free: works on any file path or the active target.

use super::static_input::{self, Input};
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_evidence::{Confidence, Coverage, Environment, EvidenceBuilder, Limitation, Location, Subject, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// A known file signature: offset, bytes to match, and the type name.
#[derive(Debug, Clone)]
struct Signature {
    offset: usize,
    bytes: &'static [u8],
    file_type: &'static str,
    mime: &'static str,
    confidence: Confidence,
}

/// All known signatures, ordered so more specific/longer matches win.
const SIGNATURES: &[Signature] = &[
    // Images
    Signature { offset: 0, bytes: b"\x89PNG\r\n\x1a\n", file_type: "PNG image", mime: "image/png", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\xff\xd8\xff", file_type: "JPEG image", mime: "image/jpeg", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"GIF87a", file_type: "GIF image", mime: "image/gif", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"GIF89a", file_type: "GIF image", mime: "image/gif", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"RIFF", file_type: "WEBP image (or RIFF container)", mime: "image/webp", confidence: Confidence::Derived },
    Signature { offset: 0, bytes: b"BM", file_type: "BMP image", mime: "image/bmp", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x00\x00\x01\x00", file_type: "ICO image", mime: "image/x-icon", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x00\x00\x02\x00", file_type: "CUR image", mime: "image/x-icon", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"II\x2a\x00", file_type: "TIFF image (little-endian)", mime: "image/tiff", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"MM\x00\x2a", file_type: "TIFF image (big-endian)", mime: "image/tiff", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\xfd\xfb\x28", file_type: "HEIC image", mime: "image/heic", confidence: Confidence::Derived },

    // Documents
    Signature { offset: 0, bytes: b"%PDF", file_type: "PDF document", mime: "application/pdf", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x25\x50\x44\x46", file_type: "PDF document", mime: "application/pdf", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"PK\x03\x04", file_type: "ZIP archive (or DOCX/XLSX/PPTX/ODT/etc.)", mime: "application/zip", confidence: Confidence::Derived },
    Signature { offset: 0, bytes: b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1", file_type: "Compound file (legacy DOC/XLS/PPT)", mime: "application/msword", confidence: Confidence::Derived },
    Signature { offset: 0, bytes: b"\x7FELF", file_type: "ELF executable", mime: "application/x-executable", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"MZ", file_type: "PE executable (Windows .exe/.dll)", mime: "application/x-dosexec", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\xCA\xFE\xBA\xBE", file_type: "Mach-O universal binary / Java class", mime: "application/x-mach-binary", confidence: Confidence::Derived },
    Signature { offset: 0, bytes: b"\xFE\xED\xFA\xCE", file_type: "Mach-O executable (32-bit)", mime: "application/x-mach-binary", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\xFE\xED\xFA\xCF", file_type: "Mach-O executable (64-bit)", mime: "application/x-mach-binary", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\xCF\xFA\xED\xFE", file_type: "Mach-O executable (32-bit, big-endian)", mime: "application/x-mach-binary", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\xCF\xFA\xED\xFF", file_type: "Mach-O executable (64-bit, big-endian)", mime: "application/x-mach-binary", confidence: Confidence::Observed },

    // Archives
    Signature { offset: 0, bytes: b"\x1F\x8B\x08", file_type: "GZIP archive", mime: "application/gzip", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"BZh", file_type: "BZIP2 archive", mime: "application/x-bzip2", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\xFD\x7F\x5A\x58\x5A", file_type: "XZ archive", mime: "application/x-xz", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x28\xB5\x2F\xFD", file_type: "Zstandard archive", mime: "application/zstd", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x37\x7A\xBC\xAF\x27\x1C", file_type: "7-Zip archive", mime: "application/x-7z-compressed", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"Rar!\x1A\x07", file_type: "RAR archive", mime: "application/x-rar-compressed", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x52\x61\x72\x21\x1A\x07\x00", file_type: "RAR5 archive", mime: "application/x-rar-compressed", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x04\x22\x4D\x18", file_type: "LZ4 archive", mime: "application/x-lz4", confidence: Confidence::Derived },

    // Audio
    Signature { offset: 0, bytes: b"RIFF", file_type: "WAV audio (or RIFF container)", mime: "audio/wav", confidence: Confidence::Derived },
    Signature { offset: 0, bytes: b"\xFF\xFB", file_type: "MP3 audio", mime: "audio/mpeg", confidence: Confidence::Derived },
    Signature { offset: 0, bytes: b"\xFF\xF3", file_type: "MP3 audio", mime: "audio/mpeg", confidence: Confidence::Derived },
    Signature { offset: 0, bytes: b"\xFF\xF2", file_type: "MP3 audio", mime: "audio/mpeg", confidence: Confidence::Derived },
    Signature { offset: 0, bytes: b"ID3", file_type: "MP3 audio", mime: "audio/mpeg", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x42\x4F\x4F\x58", file_type: "Ogg media", mime: "application/ogg", confidence: Confidence::Derived },
    Signature { offset: 0, bytes: b"fLaC", file_type: "FLAC audio", mime: "audio/flac", confidence: Confidence::Observed },

    // Video
    Signature { offset: 0, bytes: b"\x00\x00\x00\x18\x66\x74\x79\x70", file_type: "MP4 video", mime: "video/mp4", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x00\x00\x00\x20\x66\x74\x79\x70", file_type: "MP4 video", mime: "video/mp4", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x00\x00\x00\x24\x66\x74\x79\x70", file_type: "MP4 video", mime: "video/mp4", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x00\x00\x00\x1C\x66\x74\x79\x70\x6D\x70\x34\x32", file_type: "MP4 video", mime: "video/mp4", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x1A\x45\xDF\xA3", file_type: "Matroska video (MKV/WebM)", mime: "video/x-matroska", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x4D\x54\x68\x64", file_type: "MIDI file", mime: "audio/midi", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x52\x49\x46\x46", file_type: "AVI video (or RIFF container)", mime: "video/x-msvideo", confidence: Confidence::Derived },

    // Executables and code
    Signature { offset: 0, bytes: b"\x7FELF", file_type: "ELF executable", mime: "application/x-executable", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"MZ", file_type: "PE executable (Windows .exe/.dll/.sys)", mime: "application/x-dosexec", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\xCA\xFE\xBA\xBE", file_type: "Java class file", mime: "application/java-vm", confidence: Confidence::Derived },
    Signature { offset: 0, bytes: b"PK\x03\x04", file_type: "Java JAR / Android APK (ZIP-based)", mime: "application/java-archive", confidence: Confidence::Derived },
    Signature { offset: 0, bytes: b"\x7F\x45\x4C\x46", file_type: "ELF shared object", mime: "application/x-sharedlib", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"\x00\x61\x73\x6D", file_type: "WebAssembly module", mime: "application/wasm", confidence: Confidence::Observed },

    // Text / script
    Signature { offset: 0, bytes: b"#!/", file_type: "Shell script", mime: "text/x-shellscript", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"<?xml", file_type: "XML document", mime: "application/xml", confidence: Confidence::Observed },
    Signature { offset: 0, bytes: b"<", file_type: "HTML or XML document", mime: "text/html", confidence: Confidence::Inferred },
    Signature { offset: 0, bytes: b"{", file_type: "JSON or text", mime: "application/json", confidence: Confidence::Inferred },
]

/// Detect file type from first bytes.
fn detect(data: &[u8]) -> Vec<(String, String, Confidence)> {
    let mut results: Vec<(String, String, Confidence)> = Vec::new();
    for sig in SIGNATURES {
        if sig.offset + sig.bytes.len() <= data.len() {
            if data[sig.offset..sig.offset + sig.bytes.len()] == sig.bytes[..] {
                results.push((sig.file_type.to_string(), sig.mime.to_string(), sig.confidence));
            }
        }
    }
    // Sort by confidence descending (Certain > Likely > Possible > Unlikely)
    results.sort_by(|a, b| b.2.cmp(&a.2));
    results
}

/// Is the data printable text?
fn looks_like_text(data: &[u8]) -> bool {
    if data.is_empty() {
        return false;
    }
    let printable = data.iter().take(data.len().min(4096)).filter(|b| {
        matches!(b, 0x09 | 0x0A | 0x0D | 0x20..=0x7E)
    }).count();
    printable as f64 / data.len().min(4096) as f64 > 0.85
}

pub fn specs() -> Vec<ToolSpec> {
    let e = Effects::EVIDENCE;
    vec![
        spec(
            "file_type_detect",
            "File type detector",
            "Inspects a file's first bytes (magic bytes / file signature) and reports the likely file type and MIME type, with confidence. No parsing, no execution—safe on any file.",
            "Tell me what kind of file this is, from its signature.",
            Toolset::Formats,
            e,
            Requires::Nothing,
            schema_of::<DetectParams>,
            file_type_detect,
        ),
    ]
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DetectParams {
    /// Absolute path of the file to inspect; the active target when omitted.
    #[serde(default)]
    pub path: Option<String>,
}

fn file_type_detect(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: DetectParams = parse_params(params)?;

    let input: Input = static_input::load(ctx, p.path.as_deref(), 4096)?;
    let data = &input.bytes;

    let hits = detect(data);

    let (file_type, mime, confidence) = if let Some((ft, mt, conf)) = hits.into_iter().next() {
        (ft, mt, conf)
    } else if looks_like_text(data) {
        ("Plain text".to_string(), "text/plain".to_string(), Confidence::Derived)
    } else {
        ("Unknown binary data".to_string(), "application/octet-stream".to_string(), Confidence::Inferred)
    };

    let subject = Subject {
        kind: "file".into(),
        sha256: None,
        name: Some(input.path.display().to_string()),
    };
    let result = json!({
        "file_type": file_type,
        "mime_type": mime,
        "bytes_inspected": data.len(),
        "signatures_checked": SIGNATURES.len(),
    });
    let summary = format!("{file_type} ({mime})");

    let evidence = EvidenceBuilder::new(
        "file_type_detect",
        subject,
        Environment::builtin(true),
    )
    .params(params)
    .result(result.clone())
    .summary(summary)
    .confidence(confidence)
    .coverage(Coverage {
        status: craft_anything_evidence::CoverageStatus::Partial,
        examined: Some(data.len() as u64),
        total: None,
    })
    .limitation(Limitation::new(
        "magic-bytes-only",
        "magic-byte detection only inspects the first few kilobytes; container formats (ZIP, RIFF) may hide inner types",
    ))
    .location(Location::new("file", input.path.display().to_string()))
    .build()?;

    Ok(ToolOutput::evidence(evidence))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_png() {
        let data = b"\x89PNG\r\n\x1a\nrest of file";
        let hits = detect(data);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].0, "PNG image");
    }

    #[test]
    fn detects_elf() {
        let data = b"\x7FELF\x02\x01\x01";
        let hits = detect(data);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].0, "ELF executable");
    }

    #[test]
    fn detects_zip() {
        let data = b"PK\x03\x04\x14\x00";
        let hits = detect(data);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].0, "ZIP archive (or DOCX/XLSX/PPTX/ODT/etc.)");
    }

    #[test]
    fn unknown_returns_unknown() {
        let data = b"\x00\x01\x02\x03\x04\x05";
        let hits = detect(data);
        assert!(hits.is_empty());
    }

    #[test]
    fn text_detection() {
        let data = b"Hello, world!\nThis is text.";
        assert!(looks_like_text(data));
    }

    #[test]
    fn binary_not_text() {
        let data = b"\x00\x01\x02\x03\xFF\xFE\xFD";
        assert!(!looks_like_text(data));
    }
}
