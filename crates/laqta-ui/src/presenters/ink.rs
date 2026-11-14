//! Named ink colours for the pen, shapes and marker, shared by the capture toolbar and the editor.

use laqta_domain::color::Rgba;

/// A colour a swatch offers, by the name its tooltip shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ink {
    pub name: &'static str,
    pub rgba: Rgba,
}

/// Ink colours for the pen and shapes.
pub const INK: [Ink; 10] = [
    Ink { name: "Red", rgba: Rgba::rgb(0xef, 0x44, 0x44) },
    Ink { name: "Orange", rgba: Rgba::rgb(0xf9, 0x73, 0x16) },
    Ink { name: "Yellow", rgba: Rgba::rgb(0xfa, 0xcc, 0x15) },
    Ink { name: "Green", rgba: Rgba::rgb(0x22, 0xc5, 0x5e) },
    Ink { name: "Cyan", rgba: Rgba::rgb(0x06, 0xb6, 0xd4) },
    Ink { name: "Blue", rgba: Rgba::rgb(0x3b, 0x82, 0xf6) },
    Ink { name: "Purple", rgba: Rgba::rgb(0xa8, 0x55, 0xf7) },
    Ink { name: "Pink", rgba: Rgba::rgb(0xec, 0x48, 0x99) },
    Ink { name: "Black", rgba: Rgba::rgb(0x11, 0x11, 0x11) },
    Ink { name: "White", rgba: Rgba::rgb(0xff, 0xff, 0xff) },
];

/// Marker colours: bright ones, which read through the see-through marker on light and dark
/// pictures (black would hide the text).
pub const MARKER: [Ink; 5] = [
    Ink { name: "Yellow", rgba: Rgba::rgb(0xff, 0xee, 0x00) },
    Ink { name: "Green", rgba: Rgba::rgb(0x9b, 0xf0, 0x6e) },
    Ink { name: "Blue", rgba: Rgba::rgb(0x7f, 0xd3, 0xff) },
    Ink { name: "Pink", rgba: Rgba::rgb(0xff, 0x9e, 0xcb) },
    Ink { name: "Orange", rgba: Rgba::rgb(0xff, 0xb8, 0x6b) },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_inks_are_ten_named_colours_in_swatch_order() {
        let names: Vec<&str> = INK.iter().map(|i| i.name).collect();
        assert_eq!(names, ["Red", "Orange", "Yellow", "Green", "Cyan", "Blue", "Purple", "Pink", "Black", "White"]);
        assert_eq!(INK[2].rgba, Rgba::rgb(0xfa, 0xcc, 0x15), "yellow is #FACC15");
        assert_eq!(INK[8].rgba, Rgba::rgb(0x11, 0x11, 0x11), "black is #111111");
        assert_eq!(INK[9].rgba, Rgba::WHITE, "white is #FFFFFF");
    }

    #[test]
    fn the_marker_has_five_named_colours() {
        assert_eq!(MARKER.len(), 5);
        assert_eq!(MARKER[3].name, "Pink");
        assert_eq!(MARKER[2].rgba, Rgba::rgb(0x7f, 0xd3, 0xff));
    }
}
