//! Colours as data.

/// Straight-alpha 8-bit RGBA.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba { r: 0, g: 0, b: 0, a: 255 };
    pub const WHITE: Rgba = Rgba { r: 255, g: 255, b: 255, a: 255 };
    pub const TRANSPARENT: Rgba = Rgba { r: 0, g: 0, b: 0, a: 0 };

    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r, g, b, a: 255 }
    }

    /// `#rrggbb` or `#rrggbbaa`.
    pub fn parse_hex(s: &str) -> Option<Rgba> {
        let h = s.strip_prefix('#').unwrap_or(s);
        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
        match h.len() {
            6 => Some(Rgba { r: byte(0)?, g: byte(2)?, b: byte(4)?, a: 255 }),
            8 => Some(Rgba { r: byte(0)?, g: byte(2)?, b: byte(4)?, a: byte(6)? }),
            _ => None,
        }
    }

    pub fn to_hex(self) -> String {
        if self.a == 255 { format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b) } else { format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trip() {
        assert_eq!(Rgba::parse_hex("#336699"), Some(Rgba::rgb(0x33, 0x66, 0x99)));
        assert_eq!(Rgba::parse_hex("33669980").map(|c| c.a), Some(0x80));
        assert_eq!(Rgba::rgb(1, 2, 3).to_hex(), "#010203");
        assert_eq!(Rgba::parse_hex("#12"), None);
    }
}
