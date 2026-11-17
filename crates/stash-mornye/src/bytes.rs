//! Bytes as add-ons give and take them (spec §2.14; Lokal `morneye_host.dart` 146-201).

use base64::Engine;
use base64::alphabet;
use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use serde_json::{Value, json};

/// Reads base64 without its padding (the caller strips it), forgiving stray low bits as Dart's decoder does.
const LENIENT: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::RequireNone)
        .with_decode_allow_trailing_bits(true),
);

/// The bytes `value` stands for: `{__bytes: [...]}` or a JSON array whatever `encoding`; else a string read as
/// `base64` (whitespace removed, padding made right), `base64url`, `hex` (strict: `Invalid hex payload`), `utf8` /
/// `utf-8`, or `binary` / `latin1` / `raw` / `bytes` (each UTF-16 unit & 0xff). Others: `Unsupported binary
/// encoding`.
pub fn decode(value: &Value, encoding: &str) -> Result<Vec<u8>, String> {
    if let Some(list) = value.get("__bytes").and_then(Value::as_array) {
        return byte_list(list);
    }
    if let Value::Array(list) = value {
        return byte_list(list);
    }
    // Dart's `'${value ?? ''}'`.
    let text = match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    match encoding {
        "base64" => {
            let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            decode_base64(&compact)
        }
        "base64url" => decode_base64(&text),
        "hex" => decode_hex(&text),
        "utf8" | "utf-8" => Ok(text.into_bytes()),
        "binary" | "latin1" | "raw" | "bytes" => {
            Ok(text.encode_utf16().map(|unit| unit as u8).collect())
        }
        _ => Err("Unsupported binary encoding".into()),
    }
}

/// `bytes` as an add-on asked for them: `bytes` / `raw` → `{__bytes: [...], __buffer: true}`; `hex` (lowercase);
/// `base64url` (with `=` padding); `utf8`; `binary` / `latin1` (char codes); anything else standard base64.
pub fn encoded(bytes: &[u8], encoding: &str) -> Value {
    match encoding {
        "bytes" | "raw" => json!({ "__bytes": bytes, "__buffer": true }),
        "hex" => Value::String(crate::hex(bytes)),
        "base64url" => Value::String(URL_SAFE.encode(bytes)),
        "utf8" | "utf-8" => Value::String(String::from_utf8_lossy(bytes).into_owned()),
        "binary" | "latin1" => Value::String(bytes.iter().map(|&b| char::from(b)).collect()),
        _ => Value::String(STANDARD.encode(bytes)),
    }
}

/// `{__bytes: [...]}`: what the prelude turns into a `Uint8Array`.
pub fn js_bytes(bytes: &[u8]) -> Value {
    json!({ "__bytes": bytes })
}

/// Standard base64.
pub fn base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// base64url without padding.
pub fn base64url_nopad(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// A JSON list of byte values (each taken modulo 256, as `Uint8List.fromList` does).
fn byte_list(list: &[Value]) -> Result<Vec<u8>, String> {
    list.iter()
        .map(|item| {
            item.as_i64()
                .or_else(|| item.as_f64().map(|f| f as i64))
                .map(|n| n as u8)
                .ok_or_else(|| "Invalid byte array".to_string())
        })
        .collect()
}

/// Either base64 alphabet, padded or not (Dart's `base64.normalize` then `decode`).
fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    let text = text.replace("%3D", "=").replace("%3d", "=");
    let standard: String = text
        .trim_end_matches('=')
        .chars()
        .map(|c| match c {
            '-' => '+',
            '_' => '/',
            other => other,
        })
        .collect();
    LENIENT
        .decode(standard.as_bytes())
        .map_err(|_| "Invalid base64 payload".to_string())
}

fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) || !bytes.iter().all(u8::is_ascii_hexdigit) {
        return Err("Invalid hex payload".into());
    }
    bytes
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| "Invalid hex payload".to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_lists_win_over_the_encoding() {
        assert_eq!(decode(&json!([1, 2, 255]), "hex").unwrap(), [1, 2, 255]);
        assert_eq!(
            decode(&json!({ "__bytes": [7, 8] }), "utf8").unwrap(),
            [7, 8]
        );
        // Taken modulo 256, as a Uint8List does.
        assert_eq!(
            decode(&json!([256, -1, 3.9]), "base64").unwrap(),
            [0, 255, 3]
        );
        assert!(decode(&json!(["x"]), "base64").is_err());
    }

    #[test]
    fn base64_is_forgiving_like_darts() {
        assert_eq!(decode(&json!("aGVs bG8=\n"), "base64").unwrap(), b"hello");
        // Padding left out, or the URL-safe alphabet.
        assert_eq!(decode(&json!("aGVsbG8"), "base64").unwrap(), b"hello");
        assert_eq!(decode(&json!("-_8"), "base64").unwrap(), [0xfb, 0xff]);
        assert_eq!(decode(&json!("-_8="), "base64url").unwrap(), [0xfb, 0xff]);
        assert_eq!(decode(&Value::Null, "base64").unwrap(), Vec::<u8>::new());
        assert_eq!(
            decode(&json!("a"), "base64").unwrap_err(),
            "Invalid base64 payload"
        );
        assert_eq!(
            decode(&json!("a$b="), "base64").unwrap_err(),
            "Invalid base64 payload"
        );
        // base64url does not drop whitespace.
        assert!(decode(&json!("aGVs bG8"), "base64url").is_err());
    }

    #[test]
    fn hex_is_strict() {
        assert_eq!(decode(&json!("00ffA0"), "hex").unwrap(), [0, 255, 160]);
        assert_eq!(
            decode(&json!("abc"), "hex").unwrap_err(),
            "Invalid hex payload"
        );
        assert_eq!(
            decode(&json!("zz"), "hex").unwrap_err(),
            "Invalid hex payload"
        );
        assert_eq!(decode(&json!(""), "hex").unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn text_encodings() {
        assert_eq!(decode(&json!("é"), "utf8").unwrap(), "é".as_bytes());
        assert_eq!(decode(&json!("é"), "utf-8").unwrap(), [0xc3, 0xa9]);
        // One byte per UTF-16 unit, its low half.
        assert_eq!(decode(&json!("é\u{0141}"), "latin1").unwrap(), [0xe9, 0x41]);
        assert_eq!(decode(&json!(12), "utf8").unwrap(), b"12");
        assert_eq!(
            decode(&json!("x"), "ucs2").unwrap_err(),
            "Unsupported binary encoding"
        );
    }

    #[test]
    fn encodings_out() {
        let bytes = [0xfb, 0xff, 0x41];
        assert_eq!(encoded(&bytes, "hex"), json!("fbff41"));
        assert_eq!(encoded(&bytes, "base64"), json!("+/9B"));
        assert_eq!(encoded(&bytes, "anything"), json!("+/9B"));
        assert_eq!(encoded(&[0xfb, 0xff], "base64url"), json!("-_8="));
        assert_eq!(encoded(&bytes, "latin1"), json!("\u{fb}\u{ff}A"));
        assert_eq!(encoded(b"hi", "utf8"), json!("hi"));
        assert_eq!(
            encoded(&[1, 2], "bytes"),
            json!({ "__bytes": [1, 2], "__buffer": true })
        );
        assert_eq!(js_bytes(&[3]), json!({ "__bytes": [3] }));
        assert_eq!(base64(b"hello"), "aGVsbG8=");
        assert_eq!(base64url_nopad(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn round_trips() {
        let bytes: Vec<u8> = (0..=255).collect();
        for encoding in ["base64", "base64url", "hex", "latin1", "bytes"] {
            let out = encoded(&bytes, encoding);
            assert_eq!(decode(&out, encoding).unwrap(), bytes, "{encoding}");
        }
    }
}
