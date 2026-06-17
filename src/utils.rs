//! Pure helpers: the default `User-Agent`, host inspection, and thumbnail/
//! percent decoding. The compiled URL patterns live on
//! [`SnapSave`](crate::SnapSave); these helpers have no shared state.

/// Default `User-Agent` sent with every request.
pub const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/138.0.0.0 Safari/537.36";

/// Scheme present, host not already starting with `www.`, first host char alphanumeric.
pub(crate) fn needs_www(url: &str) -> bool {
    let host = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"));
    match host {
        Some(host) => {
            if host.get(..4).is_some_and(|prefix| prefix.eq_ignore_ascii_case("www.")) {
                return false;
            }
            host.chars().next().is_some_and(|first| first.is_ascii_alphanumeric())
        }
        None => false,
    }
}

/// Unwrap a `snapinsta.app` photo-proxy thumbnail URL back to its original,
/// percent-decoding the wrapped URL. Other URLs are returned unchanged.
#[must_use]
pub fn fix_thumbnail(url: &str) -> String {
    const TO_REPLACE: &str = "https://snapinsta.app/photo.php?photo=";
    if url.contains(TO_REPLACE) {
        percent_decode(&url.replacen(TO_REPLACE, "", 1))
    } else {
        url.to_string()
    }
}

/// Minimal `decodeURIComponent`-style percent decoder (UTF-8, `+` kept literal).
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut pos = 0;
    while pos < bytes.len() {
        if bytes[pos] == b'%'
            && pos + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex_val(bytes[pos + 1]), hex_val(bytes[pos + 2]))
        {
            out.push(high * 16 + low);
            pos += 3;
            continue;
        }
        out.push(bytes[pos]);
        pos += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_www_logic() {
        assert!(needs_www("https://tiktok.com"));
        assert!(needs_www("http://example.org/path"));
        assert!(!needs_www("https://www.tiktok.com"));
        assert!(!needs_www("https://WWW.tiktok.com")); // case-insensitive
        assert!(!needs_www("ftp://tiktok.com")); // no http(s) scheme
        assert!(!needs_www("https://-dash.com")); // first char not alphanumeric
    }

    #[test]
    fn fix_thumbnail_unwraps_snapinsta_proxy() {
        assert_eq!(
            fix_thumbnail("https://snapinsta.app/photo.php?photo=https%3A%2F%2Fexample.com%2Fa.jpg"),
            "https://example.com/a.jpg"
        );
    }

    #[test]
    fn fix_thumbnail_passes_through_other_urls() {
        let url = "https://d.rapidcdn.app/thumb?token=abc";
        assert_eq!(fix_thumbnail(url), url);
    }

    #[test]
    fn percent_decode_handles_sequences_and_utf8() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("%2F%2f"), "//");
        assert_eq!(percent_decode("plain+text"), "plain+text"); // '+' stays literal
        assert_eq!(percent_decode("%E2%9C%93"), "✓");
        assert_eq!(percent_decode("trailing%"), "trailing%"); // malformed tail kept
        assert_eq!(percent_decode("%zz"), "%zz"); // non-hex kept
    }
}
