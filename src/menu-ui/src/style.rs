//! Style struct (fixed property list) and parsers.
//!
//! N1 supports only the subset needed for "paint a colored rectangle":
//! `background-color`, `width`, `height`, `top`, `left`. Layout flags
//! (display, flex-*) and the rest of the property list per the plan
//! land in N3.

use menu_core_host::protocol::Rgba;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub background_color: Option<Rgba>,
    pub width: Option<u16>,
    pub height: Option<u16>,
    pub top: Option<u16>,
    pub left: Option<u16>,
}

/// Parse a CSS-style color string. Supports `#rgb`, `#rrggbb`,
/// `#rrggbbaa`. Returns `None` if `s` is not recognised.
///
/// Larger color forms (rgb(), rgba(), named colors) come later; we
/// only need hex for N1.
pub fn parse_color(s: &str) -> Option<Rgba> {
    let s = s.strip_prefix('#')?;
    let bytes = s.as_bytes();
    let to_hex = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    match bytes.len() {
        3 => {
            // #rgb -> expanded to #rrggbb
            let r = to_hex(bytes[0])?;
            let g = to_hex(bytes[1])?;
            let b = to_hex(bytes[2])?;
            Some(Rgba::new(r << 4 | r, g << 4 | g, b << 4 | b, 0xFF))
        }
        6 => {
            let r = (to_hex(bytes[0])? << 4) | to_hex(bytes[1])?;
            let g = (to_hex(bytes[2])? << 4) | to_hex(bytes[3])?;
            let b = (to_hex(bytes[4])? << 4) | to_hex(bytes[5])?;
            Some(Rgba::new(r, g, b, 0xFF))
        }
        8 => {
            let r = (to_hex(bytes[0])? << 4) | to_hex(bytes[1])?;
            let g = (to_hex(bytes[2])? << 4) | to_hex(bytes[3])?;
            let b = (to_hex(bytes[4])? << 4) | to_hex(bytes[5])?;
            let a = (to_hex(bytes[6])? << 4) | to_hex(bytes[7])?;
            Some(Rgba::new(r, g, b, a))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_three_digit_hex() {
        assert_eq!(parse_color("#f00"), Some(Rgba::new(0xFF, 0x00, 0x00, 0xFF)));
        assert_eq!(parse_color("#0AF"), Some(Rgba::new(0x00, 0xAA, 0xFF, 0xFF)));
    }

    #[test]
    fn parse_six_digit_hex() {
        assert_eq!(parse_color("#123456"), Some(Rgba::new(0x12, 0x34, 0x56, 0xFF)));
    }

    #[test]
    fn parse_eight_digit_hex() {
        assert_eq!(
            parse_color("#12345680"),
            Some(Rgba::new(0x12, 0x34, 0x56, 0x80))
        );
    }

    #[test]
    fn parse_rejects_non_hex() {
        assert_eq!(parse_color("red"), None);
        assert_eq!(parse_color("#xyz"), None);
        assert_eq!(parse_color(""), None);
        assert_eq!(parse_color("#1234"), None);
    }
}
