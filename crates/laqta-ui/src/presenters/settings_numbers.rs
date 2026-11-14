//! Whole numbers typed into the settings window. A value outside its bounds is an error the page
//! shows next to the field; nothing is clamped silently. The bounds are the domain's
//! (`laqta_domain::settings::limits`, `AutoCaptureOptions::MIN_INTERVAL_MS`), passed in by the caller.

use std::fmt;

/// Why a typed number was not taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberError {
    /// The field is empty.
    Empty,
    /// Not a whole number: letters, a sign, a decimal point or a space inside.
    NotANumber,
    /// A whole number, but under the minimum.
    TooSmall { min: u32 },
    /// A whole number, but over the maximum (this includes a number too big for 32 bits).
    TooLarge { max: u32 },
}

impl fmt::Display for NumberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumberError::Empty => write!(f, "Enter a number"),
            NumberError::NotANumber => write!(f, "Enter a whole number"),
            NumberError::TooSmall { min } => write!(f, "Must be at least {min}"),
            NumberError::TooLarge { max } => write!(f, "Must be at most {max}"),
        }
    }
}

/// `text` (surrounding spaces ignored) as a number from `min` to `max`, both included.
pub fn parse_bounded(text: &str, min: u32, max: u32) -> Result<u32, NumberError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(NumberError::Empty);
    }
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(NumberError::NotANumber);
    }
    // Digits only, so the one way to fail is a value past u32::MAX, which is over any maximum.
    let value: u32 = text.parse().map_err(|_| NumberError::TooLarge { max })?;
    if value < min {
        Err(NumberError::TooSmall { min })
    } else if value > max {
        Err(NumberError::TooLarge { max })
    } else {
        Ok(value)
    }
}

/// Like [`parse_bounded`], but an empty field is `empty` (a field where blank means "off" or "default").
pub fn parse_bounded_or(text: &str, min: u32, max: u32, empty: u32) -> Result<u32, NumberError> {
    if text.trim().is_empty() { Ok(empty) } else { parse_bounded(text, min, max) }
}

#[cfg(test)]
mod tests {
    use laqta_domain::settings::{AutoCaptureOptions, limits};

    use super::*;

    #[test]
    fn a_number_inside_the_bounds_is_taken_with_its_spaces_trimmed() {
        assert_eq!(parse_bounded("50", 1, 100), Ok(50));
        assert_eq!(parse_bounded("  1 ", 1, 100), Ok(1));
        assert_eq!(parse_bounded("100", 1, 100), Ok(100));
    }

    #[test]
    fn a_number_outside_the_bounds_is_an_error_not_a_clamp() {
        assert_eq!(parse_bounded("0", 1, 100), Err(NumberError::TooSmall { min: 1 }));
        assert_eq!(parse_bounded("101", 1, 100), Err(NumberError::TooLarge { max: 100 }));
        assert_eq!(parse_bounded("101", 1, 100).expect_err("out of bounds").to_string(), "Must be at most 100");
    }

    #[test]
    fn text_that_is_not_a_whole_number_is_named() {
        assert_eq!(parse_bounded("", 0, 9), Err(NumberError::Empty));
        assert_eq!(parse_bounded("   ", 0, 9), Err(NumberError::Empty));
        for bad in ["abc", "1.5", "-1", "+1", "1 2", "1e3", "٣"] {
            assert_eq!(parse_bounded(bad, 0, 9), Err(NumberError::NotANumber), "{bad}");
        }
    }

    /// 4294968 seconds is 4294968000 ms, one past u32::MAX: converting it in the window overflowed.
    #[test]
    fn seconds_that_would_overflow_milliseconds_are_over_the_interval_maximum() {
        let (min, max) = (AutoCaptureOptions::MIN_INTERVAL_MS / 1000, AutoCaptureOptions::MAX_INTERVAL_MS / 1000);
        assert_eq!(parse_bounded("4294968", min, max), Err(NumberError::TooLarge { max: 86_400 }));
        assert_eq!(parse_bounded("99999999999999999999", min, max), Err(NumberError::TooLarge { max: 86_400 }), "past u32 itself");
        assert_eq!(parse_bounded("86400", min, max).map(|s| s * 1000), Ok(AutoCaptureOptions::MAX_INTERVAL_MS));
        assert_eq!(parse_bounded("0", min, max), Err(NumberError::TooSmall { min: 1 }));
    }

    #[test]
    fn the_domain_bounds_are_ordered() {
        for (name, (min, max)) in [
            ("jpeg", limits::JPEG_QUALITY),
            ("fps", limits::FPS),
            ("gif fps", limits::GIF_FPS),
            ("camera", limits::CAMERA_SIZE_PERCENT),
            ("toast", limits::TOAST_DURATION_MS),
            ("retries", limits::RETRIES),
            ("crf", limits::CRF),
            ("frames", limits::SCROLL_FRAMES),
            ("amount", limits::SCROLL_AMOUNT),
            ("delay", limits::SCROLL_DELAY_MS),
        ] {
            assert!(min <= max, "{name}");
        }
    }

    #[test]
    fn an_empty_field_can_mean_a_default() {
        assert_eq!(parse_bounded_or("", 0, 10, 0), Ok(0));
        assert_eq!(parse_bounded_or(" ", 0, 10, 3), Ok(3));
        assert_eq!(parse_bounded_or("11", 0, 10, 0), Err(NumberError::TooLarge { max: 10 }));
    }
}
