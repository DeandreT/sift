//! Message body decoding heuristics: gzip sniff → UTF-8 → JSON/XML detection
//! → binary. Raw bytes are always preserved for save/resubmit fidelity.

use std::io::Read as _;

/// What the decoder concluded about a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyFormat {
    Json,
    Xml,
    Text,
    Binary,
    Empty,
    /// AMQP value/sequence body (not raw bytes); text is a rendered preview.
    AmqpValue,
}

impl BodyFormat {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Xml => "XML",
            Self::Text => "text",
            Self::Binary => "binary",
            Self::Empty => "empty",
            Self::AmqpValue => "AMQP value",
        }
    }
}

/// A decoded message body: original bytes plus a textual rendering when the
/// content is text-like.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedBody {
    pub format: BodyFormat,
    /// Present for text-like formats; JSON is pretty-printed.
    pub text: Option<String>,
    /// The original (pre-gzip-decompression) bytes.
    pub bytes: Vec<u8>,
    /// True when the payload was gzip-compressed and `text` reflects the
    /// decompressed content.
    pub gzipped: bool,
}

/// Cap for gzip decompression so a malicious payload can't balloon memory.
const MAX_DECOMPRESSED: u64 = 64 * 1024 * 1024;

impl DecodedBody {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            format: BodyFormat::Empty,
            text: None,
            bytes: Vec::new(),
            gzipped: false,
        }
    }

    /// Wrap a rendered AMQP value/sequence body (no raw bytes available).
    #[must_use]
    pub fn amqp_value(rendered: String) -> Self {
        Self {
            format: BodyFormat::AmqpValue,
            text: Some(rendered),
            bytes: Vec::new(),
            gzipped: false,
        }
    }

    /// Size of the raw payload in bytes.
    #[must_use]
    pub fn size(&self) -> usize {
        self.bytes.len()
    }
}

/// Interpret `text` as base64 and decode it, then classify the result like
/// any other body (JSON/XML/text/binary). Accepts standard and URL-safe
/// alphabets and ignores surrounding whitespace/newlines.
pub fn decode_base64(text: &str) -> Result<DecodedBody, String> {
    use base64::Engine as _;

    let trimmed: String = text.split_whitespace().collect();
    if trimmed.is_empty() {
        return Err("nothing to decode".to_owned());
    }
    let engines = [
        base64::engine::general_purpose::STANDARD,
        base64::engine::general_purpose::URL_SAFE,
    ];
    for engine in engines {
        if let Ok(bytes) = engine.decode(trimmed.as_bytes()) {
            return Ok(decode(bytes));
        }
    }
    Err("not valid base64".to_owned())
}

/// Decode raw `Data`-section bytes.
#[must_use]
pub fn decode(bytes: Vec<u8>) -> DecodedBody {
    if bytes.is_empty() {
        return DecodedBody::empty();
    }

    // Gzip magic-number sniff; also covers senders that mark compression via
    // a Content-Encoding application property.
    if bytes.len() > 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        let mut decoder = flate2::read::GzDecoder::new(&bytes[..]).take(MAX_DECOMPRESSED);
        let mut inflated = Vec::new();
        if decoder.read_to_end(&mut inflated).is_ok() {
            let (format, text) = classify(&inflated);
            return DecodedBody {
                format,
                text,
                bytes,
                gzipped: true,
            };
        }
    }

    let (format, text) = classify(&bytes);
    DecodedBody {
        format,
        text,
        bytes,
        gzipped: false,
    }
}

/// Classify a byte payload as JSON / XML / plain text / binary.
fn classify(bytes: &[u8]) -> (BodyFormat, Option<String>) {
    let Ok(text) = std::str::from_utf8(strip_bom(bytes)) else {
        return (BodyFormat::Binary, None);
    };
    if looks_binary(text) {
        return (BodyFormat::Binary, None);
    }

    let trimmed = text.trim_start();
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
    {
        let pretty = serde_json::to_string_pretty(&value).unwrap_or_else(|_| text.to_owned());
        return (BodyFormat::Json, Some(pretty));
    }
    if trimmed.starts_with('<') {
        return (BodyFormat::Xml, Some(text.to_owned()));
    }
    (BodyFormat::Text, Some(text.to_owned()))
}

fn strip_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes)
}

/// Valid UTF-8 can still be binary in practice (e.g. WCF binary XML that
/// happens to decode); control-character density is the tiebreaker.
fn looks_binary(text: &str) -> bool {
    let control = text
        .chars()
        .take(512)
        .filter(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
        .count();
    control > 2
}

/// Render a classic hex dump (offset, hex bytes, ASCII) for binary bodies.
#[must_use]
pub fn hex_dump(bytes: &[u8], max_bytes: usize) -> String {
    use std::fmt::Write as _;

    let shown = &bytes[..bytes.len().min(max_bytes)];
    let mut out = String::with_capacity(shown.len() * 4);
    for (i, chunk) in shown.chunks(16).enumerate() {
        let _ = write!(out, "{:08x}  ", i * 16);
        for j in 0..16 {
            match chunk.get(j) {
                Some(b) => {
                    let _ = write!(out, "{b:02x} ");
                }
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push(' ');
        for b in chunk {
            out.push(if (0x20..0x7f).contains(b) {
                *b as char
            } else {
                '.'
            });
        }
        out.push('\n');
    }
    if bytes.len() > max_bytes {
        let _ = write!(out, "… {} more bytes", bytes.len() - max_bytes);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_body() {
        assert_eq!(decode(Vec::new()).format, BodyFormat::Empty);
    }

    #[test]
    fn json_is_detected_and_pretty_printed() {
        let decoded = decode(br#"{"order":42,"items":["a","b"]}"#.to_vec());
        assert_eq!(decoded.format, BodyFormat::Json);
        let text = decoded.text.unwrap();
        assert!(text.contains("\"order\": 42"));
        assert!(text.contains('\n'));
    }

    #[test]
    fn invalid_json_starting_with_brace_falls_back_to_text() {
        let decoded = decode(b"{not json at all".to_vec());
        assert_eq!(decoded.format, BodyFormat::Text);
    }

    #[test]
    fn xml_is_detected() {
        let decoded = decode(b"<order id=\"42\"><item/></order>".to_vec());
        assert_eq!(decoded.format, BodyFormat::Xml);
    }

    #[test]
    fn bom_is_stripped() {
        let decoded = decode(b"\xef\xbb\xbf{\"a\":1}".to_vec());
        assert_eq!(decoded.format, BodyFormat::Json);
    }

    #[test]
    fn binary_is_detected() {
        let decoded = decode(vec![0x00, 0x01, 0x02, 0xff, 0xfe, 0x40]);
        assert_eq!(decoded.format, BodyFormat::Binary);
        assert!(decoded.text.is_none());
    }

    #[test]
    fn gzip_payload_is_inflated_and_classified() {
        use std::io::Write as _;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(br#"{"compressed":true}"#).unwrap();
        let gz = encoder.finish().unwrap();

        let decoded = decode(gz.clone());
        assert!(decoded.gzipped);
        assert_eq!(decoded.format, BodyFormat::Json);
        // Raw bytes stay compressed for fidelity.
        assert_eq!(decoded.bytes, gz);
    }

    #[test]
    fn base64_decodes_and_reclassifies() {
        // base64 of {"a":1}
        let decoded = decode_base64("eyJhIjoxfQ==").unwrap();
        assert_eq!(decoded.format, BodyFormat::Json);
        assert!(decoded.text.unwrap().contains("\"a\": 1"));
    }

    #[test]
    fn base64_tolerates_whitespace_and_rejects_garbage() {
        assert!(decode_base64("eyJhIjox\n  fQ==").is_ok());
        assert!(decode_base64("not base64!!!").is_err());
        assert!(decode_base64("   ").is_err());
    }

    #[test]
    fn hex_dump_shape() {
        let dump = hex_dump(&[0x48, 0x65, 0x6c, 0x6c, 0x6f, 0x00], 1024);
        assert!(dump.starts_with("00000000  48 65 6c 6c 6f 00"));
        assert!(dump.contains("Hello."));
    }

    #[test]
    fn hex_dump_truncates() {
        let dump = hex_dump(&[0u8; 64], 32);
        assert!(dump.contains("… 32 more bytes"));
    }
}
