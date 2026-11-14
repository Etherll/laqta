//! The colour picker as data: the formats it lists, the tints and shades it offers, and which
//! text colour reads on a swatch.

use laqta_app::ColorFormats;
use laqta_domain::color::Rgba;

/// One copyable format row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatRow {
    pub label: &'static str,
    pub value: String,
}

/// The formats, most used first.
pub fn rows(color: Rgba) -> Vec<FormatRow> {
    let f = ColorFormats::from_rgba(color);
    vec![
        FormatRow { label: "HEX", value: f.hex },
        FormatRow { label: "RGB", value: f.rgb },
        FormatRow { label: "HSL", value: f.hsl },
        FormatRow { label: "HSV", value: f.hsv },
        FormatRow { label: "CMYK", value: f.cmyk },
        FormatRow { label: "Decimal", value: f.decimal },
    ]
}

/// How far each step moves toward white (tints) or black (shades).
const STEPS: [f32; 4] = [0.8, 0.6, 0.4, 0.2];

/// Four tints, the colour itself, then four shades: lightest to darkest. Alpha is kept.
pub fn shades(color: Rgba) -> Vec<Rgba> {
    let mix = |c: u8, to: u8, t: f32| (f32::from(c) + (f32::from(to) - f32::from(c)) * t).round() as u8;
    let toward = |to: u8, t: f32| Rgba { r: mix(color.r, to, t), g: mix(color.g, to, t), b: mix(color.b, to, t), a: color.a };
    let tints = STEPS.iter().map(|&t| toward(255, t));
    let darks = STEPS.iter().rev().map(|&t| toward(0, t));
    tints.chain(std::iter::once(color)).chain(darks).collect()
}

/// True when dark text reads better than white on `color` (relative luminance above ~0.179).
pub fn dark_ink(color: Rgba) -> bool {
    // The contrast against black beats the contrast against white above l ≈ 0.179.
    relative_luminance(color.r, color.g, color.b) > 0.179
}

/// The text colour for a swatch: near-black or white.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ink {
    /// #111111
    Dark,
    /// #FFFFFF
    Light,
}

/// The ink with the higher WCAG contrast on `(r, g, b)`, against #111111 or #FFFFFF.
pub fn readable_ink(r: u8, g: u8, b: u8) -> Ink {
    let l = relative_luminance(r, g, b);
    let on_dark = (l + 0.05) / (relative_luminance(0x11, 0x11, 0x11) + 0.05);
    let on_light = 1.05 / (l + 0.05);
    if on_dark > on_light { Ink::Dark } else { Ink::Light }
}

/// WCAG relative luminance of an sRGB colour: 0 for black, 1 for white.
fn relative_luminance(r: u8, g: u8, b: u8) -> f32 {
    let lin = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_list_hex_first_and_every_format() {
        let r = rows(Rgba::rgb(0x33, 0x66, 0x99));
        assert_eq!(r[0], FormatRow { label: "HEX", value: "#336699".into() });
        assert_eq!(r.iter().map(|r| r.label).collect::<Vec<_>>(), ["HEX", "RGB", "HSL", "HSV", "CMYK", "Decimal"]);
    }

    #[test]
    fn shades_run_light_to_dark_around_the_colour() {
        let c = Rgba::rgb(100, 150, 200);
        let s = shades(c);
        assert_eq!(s.len(), 9);
        assert_eq!(s[4], c);
        let sum = |c: &Rgba| u32::from(c.r) + u32::from(c.g) + u32::from(c.b);
        assert!(s.windows(2).all(|w| sum(&w[0]) >= sum(&w[1])), "{s:?}");
        assert_eq!(shades(Rgba::rgb(0, 0, 0))[0], Rgba::rgb(204, 204, 204));
    }

    #[test]
    fn ink_follows_luminance() {
        assert!(dark_ink(Rgba::rgb(255, 255, 255)));
        assert!(dark_ink(Rgba::rgb(0xea, 0xb3, 0x08)), "yellow takes dark text");
        assert!(!dark_ink(Rgba::rgb(0, 0, 0)));
        assert!(!dark_ink(Rgba::rgb(0x33, 0x66, 0x99)));
    }

    #[test]
    fn readable_ink_picks_the_higher_contrast() {
        assert_eq!(readable_ink(255, 255, 255), Ink::Dark, "white takes dark text");
        assert_eq!(readable_ink(0xfa, 0xcc, 0x15), Ink::Dark, "yellow takes dark text");
        assert_eq!(readable_ink(0, 0, 0), Ink::Light, "black takes white text");
        assert_eq!(readable_ink(0x11, 0x11, 0x11), Ink::Light, "the dark ink itself takes white");
        assert_eq!(readable_ink(0x76, 0x76, 0x76), Ink::Light, "#767676 reads better in white (4.5:1 against 4.2:1)");
        assert_eq!(readable_ink(0x3b, 0x82, 0xf6), Ink::Dark, "blue reads better in #111111 (5.1:1 against 3.7:1)");
    }
}
