//! Unpacker for the obfuscated, Dean-Edwards-style packed JS payload that
//! `snapsave.app` returns instead of plain HTML.
//!
//! The flow is: `get_encoded_snap_app` pulls the packer's six arguments out of
//! the payload, `decode_snap_app` reconstructs the embedded script string, and
//! `get_decoded_snap_save` extracts the inner HTML (or the inline `#alert`
//! error). [`decrypt_snap_save`] composes all three.

/// The response was not in the expected packed format, or carried an inline
/// `#alert` error message (preserved as the error's text).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct DecryptError(String);

/// Alphabet used by the packer's base conversion (digits, lower, upper, `+`, `/`).
const G: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ+/";

/// Marks the start of the packer's argument list within the payload.
const MARKER: &str = "decodeURIComponent(escape(r))}(";

/// Pull the packer's six-argument list out of the packed payload: split on
/// [`MARKER`] and keep the suffix, split on `))` and keep the prefix, split on
/// `,`, then strip quotes and surrounding whitespace from each value.
fn get_encoded_snap_app(data: &str) -> Result<Vec<String>, DecryptError> {
    let after = data
        .split_once(MARKER)
        .map(|(_, after)| after)
        .ok_or_else(|| DecryptError("unexpected snapsave.app response format".to_string()))?;
    let args = after.split("))").next().unwrap_or(after);
    Ok(args
        .split(',')
        .map(|arg| arg.replace('"', "").trim().to_string())
        .collect())
}

/// Interpret the string `digits` as a base-`base` number over the [`G`]
/// alphabet and return its value. Unknown characters contribute nothing.
///
/// `G` is ASCII so this runs directly on bytes with no allocation. The packer
/// re-encodes the value as a decimal string and parses it back, which is the
/// identity for the numeric value we need, so we skip that round-trip.
/// Saturating arithmetic keeps real payloads exact while never panicking.
fn decode(digits: &str, base: usize) -> u64 {
    let alphabet = &G.as_bytes()[..base.min(G.len())];
    let mut value: u64 = 0;
    for (place, byte) in digits.bytes().rev().enumerate() {
        if let Some(digit) = alphabet.iter().position(|&symbol| symbol == byte) {
            let exponent = u32::try_from(place).unwrap_or(u32::MAX);
            let weight = (base as u64).saturating_pow(exponent);
            value = value.saturating_add((digit as u64).saturating_mul(weight));
        }
    }
    value
}

/// Reconstruct the embedded script string from the packer arguments.
///
/// Only four of the six arguments matter: the payload, the charset (whose
/// `base`-th char is the chunk delimiter), the char-code offset, and the base.
fn decode_snap_app(args: &[String]) -> String {
    let payload = args.first().map_or("", String::as_str);
    let charset = args.get(2).map_or("", String::as_str);
    let offset: i64 = args.get(3).and_then(|arg| arg.parse().ok()).unwrap_or(0);
    let base: usize = args.get(4).and_then(|arg| arg.parse().ok()).unwrap_or(0);

    let charset_chars: Vec<char> = charset.chars().collect();
    // `charset[base]`; an out-of-range index means "no delimiter" (whole payload = 1 chunk).
    let delimiter: Option<char> = charset_chars.get(base).copied();
    let payload_chars: Vec<char> = payload.chars().collect();
    let len = payload_chars.len();

    let mut bytes: Vec<u8> = Vec::new();
    let mut cursor = 0usize;
    while cursor < len {
        // Collect a chunk up to (but not including) the delimiter.
        let mut chunk = String::new();
        while cursor < len && Some(payload_chars[cursor]) != delimiter {
            chunk.push(payload_chars[cursor]);
            cursor += 1;
        }
        cursor += 1; // skip the delimiter

        // Replace each charset char with its index, IN ORDER, re-reading the
        // mutated `chunk` — do NOT collapse this into a single-pass lookup table:
        // a digit produced by one replacement may collide with a later charset
        // char, so the order-sensitive cascade is load-bearing.
        for (index, charset_char) in charset_chars.iter().enumerate() {
            chunk = chunk.replace(*charset_char, &index.to_string());
        }

        // Each output byte is `(value - offset)` reduced modulo 256; `rem_euclid(256)`
        // is always in 0..=255, so the u8 conversion never fails.
        let value = i64::try_from(decode(&chunk, base)).unwrap_or(i64::MAX);
        let byte = u8::try_from((value - offset).rem_euclid(256)).unwrap_or(0);
        bytes.push(byte);
    }

    String::from_utf8_lossy(&bytes).into_owned()
}

/// Extract the inner download-section HTML from the decoded script, or surface
/// the inline `#alert` error message as a [`DecryptError`].
fn get_decoded_snap_save(data: &str) -> Result<String, DecryptError> {
    const ALERT: &str = "document.querySelector(\"#alert\").innerHTML = \"";
    const START: &str = "getElementById(\"download-section\").innerHTML = \"";
    const END: &str = "\"; document.getElementById(\"inputData\").remove(); ";

    if let Some((_, after)) = data.split_once(ALERT) {
        let msg = after.split("\";").next().unwrap_or("").trim();
        if !msg.is_empty() {
            return Err(DecryptError(msg.to_string()));
        }
    }

    let after = data
        .split_once(START)
        .map(|(_, after)| after)
        .ok_or_else(|| DecryptError("download section not found in response".to_string()))?;
    let html = after.split(END).next().unwrap_or(after);
    // `replace(/\\(\\)?/g, "")` is equivalent to removing every backslash.
    Ok(html.replace('\\', ""))
}

/// Decrypt a raw `snapsave.app/action.php` response into its inner HTML.
///
/// # Errors
/// Returns a [`DecryptError`] if `data` is not in the expected packed format,
/// or if it carries an inline `#alert` error message (the message is preserved).
pub fn decrypt_snap_save(data: &str) -> Result<String, DecryptError> {
    let args = get_encoded_snap_app(data)?;
    let decoded = decode_snap_app(&args);
    get_decoded_snap_save(&decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_reads_bases() {
        assert_eq!(decode("10", 2), 2);
        assert_eq!(decode("377", 8), 255);
        assert_eq!(decode("ff", 16), 255);
        assert_eq!(decode("z", 62), 35);
        assert_eq!(decode("", 8), 0);
        // Unknown chars contribute 0 but still occupy a place value.
        assert_eq!(decode("1x0", 2), 4);
    }

    #[test]
    fn get_encoded_snap_app_splits_args() {
        let payload = r#"x decodeURIComponent(escape(r))}("HHH","uu","abc",12,8,"rr")) trailing"#;
        let args = get_encoded_snap_app(payload).unwrap();
        assert_eq!(args, vec!["HHH", "uu", "abc", "12", "8", "rr"]);
    }

    #[test]
    fn get_encoded_snap_app_requires_marker() {
        assert!(get_encoded_snap_app("no packer marker here").is_err());
    }

    #[test]
    fn get_decoded_snap_save_extracts_and_unescapes() {
        // Raw string: the `\"` are literal backslash-quote, as they appear in the
        // packed script before backslash stripping.
        let decoded = r#"prefix getElementById("download-section").innerHTML = "<b>\"hi\"</b>"; document.getElementById("inputData").remove(); suffix"#;
        assert_eq!(get_decoded_snap_save(decoded).unwrap(), r#"<b>"hi"</b>"#);
    }

    #[test]
    fn get_decoded_snap_save_surfaces_alert() {
        let decoded = r##"document.querySelector("#alert").innerHTML = "Error: nope"; rest"##;
        match get_decoded_snap_save(decoded) {
            Err(DecryptError(msg)) => assert_eq!(msg, "Error: nope"),
            other => panic!("expected alert error, got {other:?}"),
        }
    }

    #[test]
    fn get_decoded_snap_save_missing_section_errors() {
        assert!(get_decoded_snap_save("no markers at all").is_err());
    }

    /// Inverse of the decoder: produce a packed payload that decodes to `inner`.
    fn pack(inner: &str, charset: &str, base: usize, offset: i64) -> String {
        let charset_chars: Vec<char> = charset.chars().collect();
        assert!(base < charset_chars.len(), "delimiter index must be inside the charset");
        let radix = base as u64;
        let delimiter = charset_chars[base];

        let chunks: Vec<String> = inner
            .bytes()
            .map(|byte| {
                // decoder computes (decode(chunk) - offset) mod 256 == byte
                let mut value = u64::from(byte) + offset as u64;
                let mut digits = Vec::new();
                if value == 0 {
                    digits.push(0);
                }
                while value > 0 {
                    digits.push(value % radix);
                    value /= radix;
                }
                digits.reverse(); // most-significant first
                digits.into_iter().map(|digit| charset_chars[digit as usize]).collect()
            })
            .collect();

        let payload = chunks.join(&delimiter.to_string());
        let args = format!("\"{payload}\",\"u\",\"{charset}\",{offset},{base},\"r\"");
        format!("eval(function(h,u,n,t,e,r){{}}decodeURIComponent(escape(r))}}({args}))")
    }

    fn wrap_download_section(html: &str) -> String {
        format!(
            "getElementById(\"download-section\").innerHTML = \"{html}\"; \
             document.getElementById(\"inputData\").remove(); "
        )
    }

    #[test]
    fn decrypt_round_trips_ascii_html() {
        let html = r#"<div class="download-items"><a href="https://x/v">go</a></div>"#;
        let payload = pack(&wrap_download_section(html), "abcdefghi", 8, 5);
        assert_eq!(decrypt_snap_save(&payload).unwrap(), html);
    }

    #[test]
    fn decrypt_round_trips_with_small_base() {
        let html = "<p>hello</p>";
        let payload = pack(&wrap_download_section(html), "kgPYMplsG", 2, 12);
        assert_eq!(decrypt_snap_save(&payload).unwrap(), html);
    }

    #[test]
    fn decrypt_reconstructs_utf8() {
        let html = "<p>café ✓ 日本</p>";
        let payload = pack(&wrap_download_section(html), "abcdefghi", 8, 3);
        assert_eq!(decrypt_snap_save(&payload).unwrap(), html);
    }

    #[test]
    fn decrypt_strips_backslash_escapes() {
        let escaped = r#"<a href=\"https://x/v\">go</a>"#;
        let payload = pack(&wrap_download_section(escaped), "abcdefghi", 8, 7);
        assert_eq!(decrypt_snap_save(&payload).unwrap(), r#"<a href="https://x/v">go</a>"#);
    }

    #[test]
    fn decrypt_surfaces_packed_alert() {
        let inner = r##"document.querySelector("#alert").innerHTML = "Error: Unable to connect"; x"##;
        let payload = pack(inner, "abcdefghi", 8, 9);
        match decrypt_snap_save(&payload) {
            Err(DecryptError(msg)) => assert_eq!(msg, "Error: Unable to connect"),
            other => panic!("expected alert error, got {other:?}"),
        }
    }

    #[test]
    fn decrypt_requires_packer_marker() {
        assert!(decrypt_snap_save("just some plain html").is_err());
    }
}
