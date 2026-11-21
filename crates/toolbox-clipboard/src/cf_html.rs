//! Windows CF_HTML header parsing and generation.
//!
//! The header offsets are byte offsets into the UTF-8 payload, so they must be
//! computed after encoding Chinese text rather than over a `char` count.

const PREFIX: &str = "<html><body><!--StartFragment-->";
const SUFFIX: &str = "<!--EndFragment--></body></html>";

/// Extract the fragment from a CF_HTML payload. If the payload has no valid
/// header, the whole value is treated as an HTML fragment.
pub fn parse(payload: &[u8]) -> Option<String> {
    let payload = payload.strip_suffix(&[0]).unwrap_or(payload);
    let text = std::str::from_utf8(payload).ok()?;
    let start = header_value(text, "StartFragment");
    let end = header_value(text, "EndFragment");
    match (start, end) {
        (Some(start), Some(end)) if start <= end && end <= payload.len() => {
            let fragment = payload.get(start..end)?;
            Some(String::from_utf8_lossy(fragment).into_owned())
        }
        _ => Some(text.to_owned()),
    }
}

/// Build a CF_HTML payload for a fragment. The fragment may contain any UTF-8,
/// including Chinese; all offsets are byte offsets.
pub fn build(fragment: &str) -> Vec<u8> {
    let fragment = fragment.as_bytes();
    // The header width is fixed by the ten-digit fields; measure a zero-filled
    // header so the offsets are exact for every value width.
    let header_len = format_header(0, 0, 0, 0).len();
    let start_html = header_len;
    let start_fragment = start_html + PREFIX.len();
    let end_fragment = start_fragment + fragment.len();
    let end_html = end_fragment + SUFFIX.len();
    let header = format_header(start_html, end_html, start_fragment, end_fragment);
    debug_assert_eq!(header.len(), header_len);
    let mut out = Vec::with_capacity(end_html + 1);
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(PREFIX.as_bytes());
    out.extend_from_slice(fragment);
    out.extend_from_slice(SUFFIX.as_bytes());
    out.push(0);
    out
}

fn format_header(
    start_html: usize,
    end_html: usize,
    start_fragment: usize,
    end_fragment: usize,
) -> String {
    format!(
        "Version:0.9\nStartHTML:{start_html:010}\nEndHTML:{end_html:010}\nStartFragment:{start_fragment:010}\nEndFragment:{end_fragment:010}\nStartSelection:{start_fragment:010}\nEndSelection:{end_fragment:010}\nSourceURL:about:blank\n"
    )
}

fn header_value(text: &str, key: &str) -> Option<usize> {
    let prefix = format!("{key}:");
    text.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .and_then(|value| value.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_keeps_chinese_and_offsets() {
        let fragment = "<p>中文 <b>bold</b></p>";
        let payload = build(fragment);
        assert_eq!(parse(&payload).unwrap(), fragment);
        let text = String::from_utf8(payload).unwrap();
        let start: usize = header_value(&text, "StartFragment").unwrap();
        let end: usize = header_value(&text, "EndFragment").unwrap();
        assert_eq!(&text[start..end], fragment);
        assert!(start < end);
    }

    #[test]
    fn payload_without_header_is_treated_as_fragment() {
        assert_eq!(parse(b"<b>x</b>").unwrap(), "<b>x</b>");
    }

    #[test]
    fn invalid_offsets_are_rejected_by_slicing_rules() {
        let payload = b"Version:0.9\nStartFragment:999\nEndFragment:1\n<html></html>";
        // Out-of-range offsets fall back to the whole payload, never panic.
        assert!(parse(payload).is_some());
    }
}
