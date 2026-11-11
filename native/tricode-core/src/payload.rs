//! The payload envelope: a msgpack map `{"type": kind, "value": text}`, and the fallbacks used to
//! read codes that hold something else.

use serde::{Deserialize, Serialize};

/// Pack a text payload into msgpack: `{"type": "<kind>", "value": "<payload>"}`.
pub(crate) fn pack_payload(kind: &str, text: &str) -> Result<Vec<u8>, String> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        #[serde(rename = "type")]
        kind: &'a str,
        value: &'a str,
    }
    let envelope = Envelope { kind, value: text };
    rmp_serde::to_vec_named(&envelope).map_err(|e| format!("msgpack encode error: {}", e))
}

/// Unpack a msgpack blob produced by [`pack_payload`].
/// Returns `(kind, value)` where `kind` is e.g. `"text"`, `"url"`, or `"plain"`.
///
/// Fallback chain:
/// 1. Try to deserialise as the expected `{"type": ..., "value": ...}` envelope → return `(kind, value)`.
/// 2. If the bytes are valid UTF-8, return them as `("plain", string)`: codes that hold bare
///    text. This comes before generic msgpack because every ASCII byte is also a msgpack integer
///    (`"abcde"` would read as `97`), while msgpack structures are rarely valid UTF-8, as their
///    headers are bytes of `0x80` and up.
/// 3. If it *is* valid msgpack, serialise the decoded value as a JSON string and return
///    `("plain", json)`.
/// 4. Only if all three steps fail do we return `Err`.
pub(crate) fn unpack_payload(bytes: &[u8]) -> Result<(String, String), String> {
    // Step 1 — try to match the expected named-map envelope.
    if let Some(envelope) = unpack_envelope(bytes) {
        return Ok(envelope);
    }

    // Step 2 — bare UTF-8 text.
    if let Ok(s) = std::str::from_utf8(bytes) {
        return Ok(("plain".to_string(), s.to_string()));
    }

    // Step 3 — valid msgpack but wrong shape: decode to a generic JSON value and
    // return the JSON representation so the caller still gets something useful.
    let mut de = rmp_serde::Deserializer::new(bytes);
    if let Ok(json_val) = serde_json::Value::deserialize(&mut de) {
        let json_str = serde_json::to_string(&json_val).unwrap_or_else(|_| json_val.to_string());
        return Ok(("plain".to_string(), json_str));
    }

    Err(format!(
        "payload is not a recognised msgpack envelope, UTF-8 text, or valid msgpack ({} bytes)",
        bytes.len()
    ))
}

/// Step 1 of [`unpack_payload`]: the `{"type": ..., "value": ...}` envelope written by
/// [`pack_payload`], as `(kind, value)`.
fn unpack_envelope(bytes: &[u8]) -> Option<(String, String)> {
    #[derive(Deserialize)]
    struct Envelope {
        #[serde(rename = "type")]
        kind: String,
        value: String,
    }

    // `to_vec_named` writes a msgpack map with string keys; we must deserialise
    // in the same named/map mode — rmp_serde's default `from_slice` uses the
    // compact array format which is incompatible with named maps.
    let mut de = rmp_serde::Deserializer::new(bytes);
    let envelope = Envelope::deserialize(&mut de).ok()?;
    Some((envelope.kind, envelope.value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_log::test;

    #[test]
    fn unpacks_bare_utf8_before_generic_msgpack() {
        for text in ["a", "abcde", "zażółć gęślą jaźń"] {
            assert_eq!(
                unpack_payload(text.as_bytes()),
                Ok(("plain".to_string(), text.to_string()))
            );
        }
        // Not UTF-8, so still read as msgpack: `true`.
        assert_eq!(
            unpack_payload(&[0xc3]),
            Ok(("plain".to_string(), "true".to_string()))
        );
        let packed = pack_payload("url", "https://example.com").expect("pack");
        assert_eq!(
            unpack_payload(&packed),
            Ok(("url".to_string(), "https://example.com".to_string()))
        );
    }
}
