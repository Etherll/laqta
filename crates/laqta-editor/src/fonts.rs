//! Injected fonts (ADR 024): `main` supplies a loader; the font system is built on first use with
//! `FontSystem::new_with_locale_and_db`, so the engine never scans system fonts itself.

use std::sync::{Arc, Mutex};

use cosmic_text::{Align, Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, Style as FontSlant, SwashCache, Weight, fontdb};
use laqta_domain::color::Rgba;

use crate::model::{FontStyle, TextAlign};

type Loader = Arc<dyn Fn() -> (fontdb::Database, String) + Send + Sync>;

struct Loaded {
    system: FontSystem,
    cache: SwashCache,
    /// cosmic-text panics shaping with an empty database, so text is skipped then.
    has_fonts: bool,
}

/// A cheap, clonable handle to the lazily built font system.
#[derive(Clone)]
pub struct Fonts {
    loader: Loader,
    state: Arc<Mutex<Option<Loaded>>>,
}

impl std::fmt::Debug for Fonts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fonts").finish_non_exhaustive()
    }
}

/// One rasterized glyph pixel run: (x, y, w, h, colour) relative to the text origin.
pub(crate) type Span = (i32, i32, u32, u32, Rgba);

impl Fonts {
    pub fn new(loader: Arc<dyn Fn() -> (fontdb::Database, String) + Send + Sync>) -> Fonts {
        Fonts { loader, state: Arc::new(Mutex::new(None)) }
    }

    /// No fonts at all: text lays out to nothing (tests, or before fonts are available).
    pub fn empty() -> Fonts {
        Fonts::new(Arc::new(|| (fontdb::Database::new(), "en-US".to_string())))
    }

    fn with<T>(&self, f: impl FnOnce(&mut Loaded) -> T) -> T {
        let mut guard = match self.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let loaded = guard.get_or_insert_with(|| {
            let (db, locale) = (self.loader)();
            let has_fonts = db.faces().next().is_some();
            Loaded { system: FontSystem::new_with_locale_and_db(locale, db), cache: SwashCache::new(), has_fonts }
        });
        f(loaded)
    }

    /// Lays out `text` in a box `width` wide (None = unbounded) and returns the glyph pixels plus
    /// the laid-out size.
    pub(crate) fn layout(&self, text: &str, font: &FontStyle, width: Option<f32>) -> (Vec<Span>, f32, f32) {
        if text.is_empty() || font.size <= 0.0 {
            return (Vec::new(), 0.0, 0.0);
        }
        self.with(|l| {
            if !l.has_fonts {
                return (Vec::new(), 0.0, 0.0);
            }
            let metrics = Metrics::new(font.size, (font.size * 1.25).max(1.0));
            let mut buffer = Buffer::new(&mut l.system, metrics);
            buffer.set_size(width, None);
            let family = match font.family.as_str() {
                "" | "sans-serif" => Family::SansSerif,
                "serif" => Family::Serif,
                "monospace" => Family::Monospace,
                name => Family::Name(name),
            };
            let mut attrs = Attrs::new().family(family);
            if font.bold {
                attrs = attrs.weight(Weight::BOLD);
            }
            if font.italic {
                attrs = attrs.style(FontSlant::Italic);
            }
            let align = match font.align {
                TextAlign::Left => Align::Left,
                TextAlign::Center => Align::Center,
                TextAlign::Right => Align::Right,
            };
            buffer.set_text(text, &attrs, Shaping::Advanced, Some(align));
            buffer.shape_until_scroll(&mut l.system, false);
            let mut w: f32 = 0.0;
            let mut h: f32 = 0.0;
            for run in buffer.layout_runs() {
                w = w.max(run.line_w);
                h = h.max(run.line_top + run.line_height);
            }
            let c = font.color;
            let mut spans = Vec::new();
            buffer.draw(&mut l.system, &mut l.cache, Color::rgba(c.r, c.g, c.b, c.a), |x, y, sw, sh, color| {
                let [r, g, b, a] = color.as_rgba();
                if a > 0 {
                    spans.push((x, y, sw, sh, Rgba { r, g, b, a }));
                }
            });
            if font.underline {
                for run in buffer.layout_runs() {
                    let y = (run.line_y + font.size * 0.12).round() as i32;
                    let thickness = (font.size / 14.0).ceil().max(1.0) as u32;
                    let x0 = run.glyphs.first().map(|g| g.x).unwrap_or(0.0) as i32;
                    spans.push((x0, y, run.line_w.ceil() as u32, thickness, c));
                }
            }
            (spans, w, h)
        })
    }

    /// Renders text by itself (for the drawing effects' text callback).
    /// Renders `text` (e.g. an emoji for the Emoji tool) to a tight Rgba8 bitmap; `None` without fonts.
    pub fn render_text(&self, text: &str, font: &FontStyle) -> Option<laqta_domain::Bitmap> {
        self.render_spans_bitmap(text, font)
    }

    pub(crate) fn render_spans_bitmap(&self, text: &str, font: &FontStyle) -> Option<laqta_domain::Bitmap> {
        let (spans, w, h) = self.layout(text, font, None);
        let (w, h) = (w.ceil().max(1.0) as u32, h.ceil().max(1.0) as u32);
        let mut px = vec![0u8; (w * h * 4) as usize];
        crate::render::blend_spans(&mut px, w, h, 0, 0, &spans);
        laqta_domain::Bitmap::new(w, h, laqta_domain::PixelFormat::Rgba8, px.into()).ok()
    }
}
