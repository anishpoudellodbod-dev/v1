use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

/// Extracts a non-empty trimmed string from a JSON String or integer value.
pub fn extract_string_or_int(val: Option<&Value>) -> Option<String> {
    let val = val?;
    val.as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .or_else(|| val.as_i64().map(|n| n.to_string()))
        .or_else(|| val.as_u64().map(|n| n.to_string()))
}

/// Converts Devanagari digits `[०-९]` to ASCII digits `[0-9]`.
pub fn normalize_ascii_digits(raw: &str) -> String {
    if raw.is_ascii() {
        return raw.to_string();
    }
    raw.chars()
        .map(|c| match c {
            '०'..='९' => char::from(b'0' + (c as u32 - '०' as u32) as u8),
            _ => c,
        })
        .collect()
}

#[inline]
fn is_zero_width(c: char) -> bool {
    matches!(c, '\u{200B}' | '\u{200C}' | '\u{200D}' | '\u{FEFF}' | '\u{00AD}')
}

/// Canonical report key normalization per `reports-backend-architecture-guide.md`:
/// Devanagari digits -> ASCII digits, Unicode NFKC, strip zero-width chars,
/// lowercase, and collapse whitespace.
pub fn normalize_key(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_space = false;

    for c in raw.nfkc() {
        if is_zero_width(c) {
            continue;
        }
        let mapped = match c {
            '०'..='९' => char::from(b'0' + (c as u32 - '०' as u32) as u8),
            _ => c,
        };
        if mapped.is_whitespace() {
            if !out.is_empty() {
                pending_space = true;
            }
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        for lc in mapped.to_lowercase() {
            out.push(lc);
        }
    }

    out
}

/// Strips all non-alphanumeric Unicode characters (and normalizes Devanagari digits + NFKC)
/// and lowercases for tolerant candidate search (`normalizeSearchValue`).
pub fn normalize_vehicle_search(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.nfkc() {
        if is_zero_width(c) {
            continue;
        }
        let mapped = match c {
            '०'..='९' => char::from(b'0' + (c as u32 - '०' as u32) as u8),
            _ => c,
        };
        if mapped.is_alphanumeric() {
            for lc in mapped.to_lowercase() {
                out.push(lc);
            }
        }
    }
    out
}

/// Existing canonical vehicle plate key normalization: strips whitespace & ASCII punctuation
/// and uppercases (preserving exact legacy vehicle/dashboard matching behavior).
pub fn normalize_vehicle_number(raw: &str) -> Option<String> {
    let normalized: String = if raw.is_ascii() {
        let mut out = String::with_capacity(raw.len());
        for &b in raw.as_bytes() {
            if !b.is_ascii_whitespace() && !b.is_ascii_punctuation() {
                out.push(b.to_ascii_uppercase() as char);
            }
        }
        out
    } else {
        raw.chars()
            .filter(|c| !c.is_whitespace() && !c.is_ascii_punctuation())
            .flat_map(char::to_uppercase)
            .collect()
    };

    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_key_unicode_and_devanagari() {
        assert_eq!(normalize_key("  २०८३\u{200B} /  ८४  "), "2083 / 84");
        assert_eq!(normalize_key("  Main   Branch "), "main branch");
        assert_eq!(normalize_vehicle_search("BA ०१ KHA १२३४"), "ba01kha1234");
        assert_eq!(
            normalize_vehicle_search("Province-4-15-23-KHA-5555"),
            "province41523kha5555"
        );
    }

    #[test]
    fn test_normalize_vehicle_number() {
        assert_eq!(
            normalize_vehicle_number("lu-548-ga-6236").as_deref(),
            Some("LU548GA6236")
        );
        assert_eq!(normalize_vehicle_number(" -  "), None);
    }
}
