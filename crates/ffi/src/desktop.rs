// SPDX-License-Identifier: MIT OR Apache-2.0
//! Platform-neutral external-open requests used by graphical frontends.

/// A file and optional zero-based editor row from a `forge://open` URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    pub file: String,
    pub line: Option<u32>,
}

/// Parse `forge://open?file=PATH&line=NUMBER`. Invalid or unrelated URLs
/// are ignored by callers so the operating system can handle them normally.
pub fn parse_open_url(value: &str) -> Option<OpenRequest> {
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "forge" || url.host_str() != Some("open") {
        return None;
    }
    let mut file = None;
    let mut line = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "file" if file.is_none() => file = Some(value.into_owned()),
            "line" if line.is_none() => {
                line = Some(value.parse::<u32>().ok()?.checked_sub(1)?);
            }
            _ => {}
        }
    }
    let file = file.filter(|path| !path.is_empty())?;
    Some(OpenRequest { file, line })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_encoded_paths_and_converts_lines_to_zero_based_positions() {
        assert_eq!(
            parse_open_url("forge://open?file=%2Ftmp%2Fhello%20world.rs&line=12"),
            Some(OpenRequest {
                file: "/tmp/hello world.rs".into(),
                line: Some(11),
            })
        );
    }

    #[test]
    fn ignores_invalid_or_unrelated_urls() {
        assert_eq!(parse_open_url("https://example.test/"), None);
        assert_eq!(parse_open_url("forge://open?line=2"), None);
        assert_eq!(parse_open_url("forge://open?file=%2Ftmp%2Fa&line=0"), None);
    }
}
