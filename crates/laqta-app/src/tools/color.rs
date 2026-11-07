//! Colour formats, colour text parsing and the picker's history (M5 colour picker). Pure values.

use std::collections::VecDeque;

use laqta_domain::color::Rgba;

/// A colour in every text form the picker offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColorFormats {
    /// `#336699`, or `#33669980` when the colour is not opaque.
    pub hex: String,
    /// `rgb(51, 102, 153)`.
    pub rgb: String,
    /// `rgba(51, 102, 153, 0.5)`, with the alpha as 0..=1.
    pub rgba: String,
    /// `hsl(210, 50%, 40%)`.
    pub hsl: String,
    /// `hsv(210, 67%, 60%)`.
    pub hsv: String,
    /// `cmyk(67%, 33%, 0%, 40%)`.
    pub cmyk: String,
    /// The colour as one number: `0xRRGGBB` read in decimal.
    pub decimal: String,
    /// Linear-light RGB, 0..=1 with four decimals: `linear(0.0331, 0.1329, 0.3185)`.
    pub linear: String,
}

impl ColorFormats {
    /// The formats of `color`. HSL, HSV and CMYK are rounded to whole numbers.
    pub fn from_rgba(color: Rgba) -> ColorFormats {
        let (r, g, b) = (channel(color.r), channel(color.g), channel(color.b));
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        let hue = (hue_degrees(r, g, b, max, delta).round() as u32) % 360;
        let light = (max + min) / 2.0;
        let sat_hsl = if delta == 0.0 { 0.0 } else { delta / (1.0 - (2.0 * light - 1.0).abs()) };
        let sat_hsv = if max == 0.0 { 0.0 } else { delta / max };
        let key = 1.0 - max;
        let (c, m, y) = if key >= 1.0 {
            (0.0, 0.0, 0.0)
        } else {
            ((1.0 - r - key) / (1.0 - key), (1.0 - g - key) / (1.0 - key), (1.0 - b - key) / (1.0 - key))
        };
        let alpha = (f64::from(color.a) / 255.0 * 1000.0).round() / 1000.0;
        ColorFormats {
            hex: color.to_hex(),
            rgb: format!("rgb({}, {}, {})", color.r, color.g, color.b),
            rgba: format!("rgba({}, {}, {}, {alpha})", color.r, color.g, color.b),
            hsl: format!("hsl({hue}, {}%, {}%)", percent(sat_hsl), percent(light)),
            hsv: format!("hsv({hue}, {}%, {}%)", percent(sat_hsv), percent(max)),
            cmyk: format!("cmyk({}%, {}%, {}%, {}%)", percent(c), percent(m), percent(y), percent(key)),
            decimal: ((u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b)).to_string(),
            linear: format!("linear({:.4}, {:.4}, {:.4})", linear(r), linear(g), linear(b)),
        }
    }
}

/// A channel as 0..=1.
fn channel(value: u8) -> f64 {
    f64::from(value) / 255.0
}

/// A 0..=1 fraction as a whole percentage.
fn percent(fraction: f64) -> u32 {
    (fraction * 100.0).round() as u32
}

/// The hue in degrees, 0 for a grey.
fn hue_degrees(r: f64, g: f64, b: f64, max: f64, delta: f64) -> f64 {
    if delta == 0.0 {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    }
}

/// sRGB transfer function, inverted: the linear-light value of a 0..=1 sRGB channel.
fn linear(c: f64) -> f64 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Reads a colour from text: `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` (the six and eight digit forms also without the
/// `#`), `rgb(…)` or `rgba(…)`, `hsl(…)` or `hsla(…)`, or one of the 16 basic CSS colour names. Anything else is `None`.
pub fn parse_color(text: &str) -> Option<Rgba> {
    let text = text.trim().to_ascii_lowercase();
    if let Some(digits) = text.strip_prefix('#') {
        return parse_hex_digits(digits);
    }
    if matches!(text.len(), 6 | 8) && text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Rgba::parse_hex(&text);
    }
    if let Some(args) = function_args(&text, "rgba").or_else(|| function_args(&text, "rgb")) {
        return parse_rgb(&args);
    }
    if let Some(args) = function_args(&text, "hsla").or_else(|| function_args(&text, "hsl")) {
        return parse_hsl(&args);
    }
    named(&text)
}

fn parse_hex_digits(digits: &str) -> Option<Rgba> {
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match digits.len() {
        3 | 4 => {
            // Each digit repeats: `f` is `ff`.
            let values: Vec<u8> = digits.chars().map(|c| c.to_digit(16).map(|d| (d * 17) as u8)).collect::<Option<_>>()?;
            let [r, g, b, rest @ ..] = values.as_slice() else { return None };
            Some(Rgba { r: *r, g: *g, b: *b, a: rest.first().copied().unwrap_or(255) })
        }
        6 | 8 => Rgba::parse_hex(digits),
        _ => None,
    }
}

/// The comma-separated arguments of `name(…)`, trimmed, when `text` is that function call.
fn function_args<'a>(text: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let inner = text.strip_prefix(name)?.trim_start().strip_prefix('(')?.strip_suffix(')')?;
    Some(inner.split(',').map(str::trim).collect())
}

fn parse_rgb(args: &[&str]) -> Option<Rgba> {
    let (r, g, b) = match args {
        [r, g, b] | [r, g, b, _] => (r, g, b),
        _ => return None,
    };
    let a = match args.get(3) {
        Some(alpha) => alpha_byte(alpha)?,
        None => 255,
    };
    Some(Rgba { r: channel_byte(r)?, g: channel_byte(g)?, b: channel_byte(b)?, a })
}

fn parse_hsl(args: &[&str]) -> Option<Rgba> {
    let (h, s, l) = match args {
        [h, s, l] | [h, s, l, _] => (h, s, l),
        _ => return None,
    };
    let a = match args.get(3) {
        Some(alpha) => alpha_byte(alpha)?,
        None => 255,
    };
    let hue = h.strip_suffix("deg").unwrap_or(h).parse::<f64>().ok()?.rem_euclid(360.0);
    let (r, g, b) = hsl_to_rgb(hue, percent_value(s)? / 100.0, percent_value(l)? / 100.0);
    Some(Rgba { r, g, b, a })
}

/// A channel: 0..=255, or a percentage of it.
fn channel_byte(text: &str) -> Option<u8> {
    let value = match text.strip_suffix('%') {
        Some(percent) => percent.parse::<f64>().ok().filter(|p| (0.0..=100.0).contains(p))? * 2.55,
        None => text.parse::<f64>().ok().filter(|v| (0.0..=255.0).contains(v))?,
    };
    Some(value.round() as u8)
}

/// An alpha: 0..=1, or a percentage.
fn alpha_byte(text: &str) -> Option<u8> {
    let fraction = match text.strip_suffix('%') {
        Some(percent) => percent_value(percent)? / 100.0,
        None => text.parse::<f64>().ok().filter(|v| (0.0..=1.0).contains(v))?,
    };
    Some((fraction * 255.0).round() as u8)
}

/// A percentage's number, 0..=100, with or without its `%` sign.
fn percent_value(text: &str) -> Option<f64> {
    text.strip_suffix('%').unwrap_or(text).parse::<f64>().ok().filter(|v| (0.0..=100.0).contains(v))
}

/// HSL to RGB bytes. `hue` is in degrees (0..360), `sat` and `light` are 0..=1.
fn hsl_to_rgb(hue: f64, sat: f64, light: f64) -> (u8, u8, u8) {
    let chroma = (1.0 - (2.0 * light - 1.0).abs()) * sat;
    let sector = hue / 60.0;
    let x = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
    let (r, g, b) = match sector as u32 {
        0 => (chroma, x, 0.0),
        1 => (x, chroma, 0.0),
        2 => (0.0, chroma, x),
        3 => (0.0, x, chroma),
        4 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let m = light - chroma / 2.0;
    let byte = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (byte(r), byte(g), byte(b))
}

const NAMED: [(&str, [u8; 3]); 16] = [
    ("black", [0, 0, 0]),
    ("silver", [192, 192, 192]),
    ("gray", [128, 128, 128]),
    ("white", [255, 255, 255]),
    ("maroon", [128, 0, 0]),
    ("red", [255, 0, 0]),
    ("purple", [128, 0, 128]),
    ("fuchsia", [255, 0, 255]),
    ("green", [0, 128, 0]),
    ("lime", [0, 255, 0]),
    ("olive", [128, 128, 0]),
    ("yellow", [255, 255, 0]),
    ("navy", [0, 0, 128]),
    ("blue", [0, 0, 255]),
    ("teal", [0, 128, 128]),
    ("aqua", [0, 255, 255]),
];

fn named(name: &str) -> Option<Rgba> {
    NAMED.iter().find(|(known, _)| *known == name).map(|(_, [r, g, b])| Rgba::rgb(*r, *g, *b))
}

/// The colours the picker used, newest first. Holds at most `ColorHistory::CAPACITY` colours, each one once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ColorHistory {
    colors: VecDeque<Rgba>,
}

impl ColorHistory {
    /// How many colours are kept.
    pub const CAPACITY: usize = 32;

    /// Makes `color` the newest entry. An older entry of the same colour is removed first.
    pub fn push(&mut self, color: Rgba) {
        self.colors.retain(|known| *known != color);
        self.colors.push_front(color);
        self.colors.truncate(Self::CAPACITY);
    }

    /// The colours, newest first.
    pub fn iter(&self) -> impl Iterator<Item = Rgba> + '_ {
        self.colors.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.colors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.colors.is_empty()
    }
}
