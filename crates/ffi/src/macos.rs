// SPDX-License-Identifier: MIT OR Apache-2.0
//! macOS pasteboard type identifiers shared with the native frontend.

/// Text representations Forge can read from or publish to the pasteboard.
pub const TEXT_PASTEBOARD_TYPES: [&str; 3] =
    ["public.utf8-plain-text", "public.rtf", "public.html"];

/// Classify a macOS pasteboard type supported by Forge.
pub fn text_type_rank(type_id: &str) -> Option<u8> {
    TEXT_PASTEBOARD_TYPES
        .iter()
        .position(|candidate| *candidate == type_id)
        .map(|rank| rank as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_supported_text_representations() {
        assert_eq!(text_type_rank("public.utf8-plain-text"), Some(0));
        assert_eq!(text_type_rank("public.rtf"), Some(1));
        assert_eq!(text_type_rank("public.html"), Some(2));
        assert_eq!(text_type_rank("public.png"), None);
    }
}
