//! The document's data: tools, styles and annotations (M3 D6).

use laqta_domain::Bitmap;
use laqta_domain::color::Rgba;

use crate::geom::{PointF, RectF, segment_distance};

/// The 21 editor tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Select,
    Rectangle,
    Ellipse,
    Line,
    Arrow,
    Freehand,
    Text,
    SpeechBalloon,
    Step,
    Image,
    Clone,
    Emoji,
    Cursor,
    Highlight,
    SmartEraser,
    Blur,
    Pixelate,
    Magnify,
    Spotlight,
    Crop,
    CutOut,
}

impl Tool {
    pub const ALL: [Tool; 21] = [
        Tool::Select,
        Tool::Rectangle,
        Tool::Ellipse,
        Tool::Line,
        Tool::Arrow,
        Tool::Freehand,
        Tool::Text,
        Tool::SpeechBalloon,
        Tool::Step,
        Tool::Image,
        Tool::Clone,
        Tool::Emoji,
        Tool::Cursor,
        Tool::Highlight,
        Tool::SmartEraser,
        Tool::Blur,
        Tool::Pixelate,
        Tool::Magnify,
        Tool::Spotlight,
        Tool::Crop,
        Tool::CutOut,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Rectangle => "Rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Freehand => "Freehand",
            Tool::Text => "Text",
            Tool::SpeechBalloon => "Speech balloon",
            Tool::Step => "Step",
            Tool::Image => "Image",
            Tool::Clone => "Clone",
            Tool::Emoji => "Emoji",
            Tool::Cursor => "Cursor",
            Tool::Highlight => "Highlight",
            Tool::SmartEraser => "Smart eraser",
            Tool::Blur => "Blur",
            Tool::Pixelate => "Pixelate",
            Tool::Magnify => "Magnify",
            Tool::Spotlight => "Spotlight",
            Tool::Crop => "Crop",
            Tool::CutOut => "Cut out",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// Font properties for Text, SpeechBalloon and Step.
#[derive(Clone, Debug, PartialEq)]
pub struct FontStyle {
    pub family: String,
    pub size: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: Rgba,
    pub align: TextAlign,
    pub padding: f32,
}

impl Default for FontStyle {
    fn default() -> Self {
        FontStyle {
            family: "sans-serif".into(),
            size: 20.0,
            bold: false,
            italic: false,
            underline: false,
            color: Rgba::WHITE,
            align: TextAlign::Left,
            padding: 6.0,
        }
    }
}

/// The current drawing style; new annotations copy it (M3 D6).
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub border_color: Rgba,
    pub border_size: f32,
    pub fill_color: Rgba,
    pub corner_radius: f32,
    pub shadow: bool,
    pub shadow_color: Rgba,
    pub shadow_offset: (f32, f32),
    pub font: FontStyle,
    pub arrow_head_width: f32,
    pub arrow_double: bool,
    pub blur_radius: u32,
    pub pixel_size: u32,
    pub highlight_color: Rgba,
    pub magnify_zoom: f32,
    pub spotlight_dim: u8,
    pub spotlight_blur: u32,
    pub spotlight_ellipse: bool,
    pub step_increment: i32,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            border_color: Rgba::rgb(0xef, 0x44, 0x44),
            border_size: 4.0,
            fill_color: Rgba::TRANSPARENT,
            corner_radius: 0.0,
            shadow: false,
            shadow_color: Rgba { r: 0, g: 0, b: 0, a: 128 },
            shadow_offset: (3.0, 3.0),
            font: FontStyle::default(),
            arrow_head_width: 16.0,
            arrow_double: false,
            blur_radius: 15,
            pixel_size: 10,
            highlight_color: Rgba::rgb(0xff, 0xee, 0x00),
            magnify_zoom: 2.0,
            spotlight_dim: 140,
            spotlight_blur: 0,
            spotlight_ellipse: false,
            step_increment: 1,
        }
    }
}

/// A bitmap inside an annotation. Equal when it shares the same pixels; debug-prints its size only.
#[derive(Clone)]
pub struct Picture(pub Bitmap);

impl PartialEq for Picture {
    fn eq(&self, o: &Picture) -> bool {
        self.0.width() == o.0.width()
            && self.0.height() == o.0.height()
            && std::sync::Arc::ptr_eq(&self.0.shared_pixels(), &o.0.shared_pixels())
    }
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}x{}@{:p})", self.0.width(), self.0.height(), self.0.pixels().as_ptr())
    }
}

/// What an annotation is. Geometry is in image pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum AnnotationKind {
    Rectangle {
        rect: RectF,
    },
    Ellipse {
        rect: RectF,
    },
    Line {
        start: PointF,
        end: PointF,
        curve: Option<PointF>,
    },
    Arrow {
        start: PointF,
        end: PointF,
        curve: Option<PointF>,
    },
    Freehand {
        points: Vec<PointF>,
    },
    Text {
        rect: RectF,
        text: String,
    },
    SpeechBalloon {
        rect: RectF,
        tail: PointF,
        text: String,
    },
    Step {
        center: PointF,
        number: i32,
        radius: f32,
    },
    Image {
        rect: RectF,
        bitmap: Picture,
    },
    Emoji {
        rect: RectF,
        bitmap: Picture,
    },
    Cursor {
        pos: PointF,
        bitmap: Picture,
    },
    Highlight {
        rect: RectF,
    },
    SmartEraser {
        rect: RectF,
        color: Rgba,
    },
    Blur {
        rect: RectF,
    },
    Pixelate {
        rect: RectF,
    },
    Magnify {
        rect: RectF,
    },
    Spotlight {
        rect: RectF,
    },
    /// Copies the base pixels under `src` to `dst` (the Clone tool).
    CloneStamp {
        src: RectF,
        dst: PointF,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub id: u64,
    pub kind: AnnotationKind,
    pub style: Style,
}

/// A resize handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Handle {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
    Start,
    End,
    /// The curve point of a line or arrow, or a speech balloon's tail.
    Control,
}

impl AnnotationKind {
    /// The rect for rect-like kinds.
    pub fn rect(&self) -> Option<RectF> {
        match self {
            AnnotationKind::Rectangle { rect }
            | AnnotationKind::Ellipse { rect }
            | AnnotationKind::Text { rect, .. }
            | AnnotationKind::SpeechBalloon { rect, .. }
            | AnnotationKind::Image { rect, .. }
            | AnnotationKind::Emoji { rect, .. }
            | AnnotationKind::Highlight { rect }
            | AnnotationKind::SmartEraser { rect, .. }
            | AnnotationKind::Blur { rect }
            | AnnotationKind::Pixelate { rect }
            | AnnotationKind::Magnify { rect }
            | AnnotationKind::Spotlight { rect } => Some(*rect),
            _ => None,
        }
    }

    pub fn set_rect(&mut self, r: RectF) {
        match self {
            AnnotationKind::Rectangle { rect }
            | AnnotationKind::Ellipse { rect }
            | AnnotationKind::Text { rect, .. }
            | AnnotationKind::SpeechBalloon { rect, .. }
            | AnnotationKind::Image { rect, .. }
            | AnnotationKind::Emoji { rect, .. }
            | AnnotationKind::Highlight { rect }
            | AnnotationKind::SmartEraser { rect, .. }
            | AnnotationKind::Blur { rect }
            | AnnotationKind::Pixelate { rect }
            | AnnotationKind::Magnify { rect }
            | AnnotationKind::Spotlight { rect } => *rect = r,
            _ => {}
        }
    }

    /// Bounds of what the annotation covers (before border, shadow).
    pub fn bounds(&self) -> RectF {
        match self {
            AnnotationKind::Line { start, end, curve } | AnnotationKind::Arrow { start, end, curve } => {
                let mut r = RectF::from_points(*start, *end);
                if let Some(c) = curve {
                    r = r.union(RectF::new(c.x, c.y, 0.0, 0.0));
                }
                r
            }
            AnnotationKind::Freehand { points } => points
                .iter()
                .fold(None::<RectF>, |acc, p| {
                    let pr = RectF::new(p.x, p.y, 0.0, 0.0);
                    Some(acc.map_or(pr, |a| a.union(pr)))
                })
                .unwrap_or_default(),
            AnnotationKind::Step { center, radius, .. } => RectF::new(center.x - radius, center.y - radius, radius * 2.0, radius * 2.0),
            AnnotationKind::Cursor { pos, bitmap } => RectF::new(pos.x, pos.y, bitmap.0.width() as f32, bitmap.0.height() as f32),
            AnnotationKind::SpeechBalloon { rect, tail, .. } => rect.normalized().union(RectF::new(tail.x, tail.y, 0.0, 0.0)),
            AnnotationKind::CloneStamp { src, dst } => RectF::new(dst.x, dst.y, src.w, src.h),
            other => other.rect().map(RectF::normalized).unwrap_or_default(),
        }
    }

    /// Moves the whole annotation.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        match self {
            AnnotationKind::Line { start, end, curve } | AnnotationKind::Arrow { start, end, curve } => {
                *start = start.offset(dx, dy);
                *end = end.offset(dx, dy);
                if let Some(c) = curve {
                    *c = c.offset(dx, dy);
                }
            }
            AnnotationKind::Freehand { points } => points.iter_mut().for_each(|p| *p = p.offset(dx, dy)),
            AnnotationKind::Step { center, .. } => *center = center.offset(dx, dy),
            AnnotationKind::Cursor { pos, .. } => *pos = pos.offset(dx, dy),
            AnnotationKind::SpeechBalloon { rect, tail, .. } => {
                *rect = rect.offset(dx, dy);
                *tail = tail.offset(dx, dy);
            }
            AnnotationKind::CloneStamp { dst, .. } => *dst = dst.offset(dx, dy),
            other => {
                if let Some(r) = other.rect() {
                    other.set_rect(r.offset(dx, dy));
                }
            }
        }
    }

    /// Whether `p` hits the annotation, with `tolerance` pixels of slack.
    pub fn hit(&self, p: PointF, tolerance: f32) -> bool {
        match self {
            AnnotationKind::Line { start, end, .. } | AnnotationKind::Arrow { start, end, .. } => {
                segment_distance(p, *start, *end) <= tolerance
            }
            AnnotationKind::Freehand { points } => points.windows(2).any(|w| segment_distance(p, w[0], w[1]) <= tolerance),
            AnnotationKind::Step { center, radius, .. } => center.distance(p) <= radius + tolerance,
            _ => self.bounds().contains(p, tolerance),
        }
    }

    /// Kinds whose pixels come from the base image (they use the per-annotation effect cache).
    pub fn is_effect(&self) -> bool {
        matches!(
            self,
            AnnotationKind::Highlight { .. }
                | AnnotationKind::SmartEraser { .. }
                | AnnotationKind::Blur { .. }
                | AnnotationKind::Pixelate { .. }
                | AnnotationKind::Magnify { .. }
                | AnnotationKind::CloneStamp { .. }
        )
    }

    /// The handles this kind offers.
    pub fn handles(&self) -> Vec<(Handle, PointF)> {
        match self {
            AnnotationKind::Line { start, end, curve } | AnnotationKind::Arrow { start, end, curve } => {
                let c = curve.unwrap_or(PointF::new((start.x + end.x) / 2.0, (start.y + end.y) / 2.0));
                vec![(Handle::Start, *start), (Handle::End, *end), (Handle::Control, c)]
            }
            AnnotationKind::Freehand { .. }
            | AnnotationKind::Step { .. }
            | AnnotationKind::Cursor { .. }
            | AnnotationKind::CloneStamp { .. } => Vec::new(),
            other => {
                let Some(r) = other.rect().map(RectF::normalized) else { return Vec::new() };
                let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
                let mut hs = vec![
                    (Handle::NW, PointF::new(r.x, r.y)),
                    (Handle::N, PointF::new(cx, r.y)),
                    (Handle::NE, PointF::new(r.right(), r.y)),
                    (Handle::E, PointF::new(r.right(), cy)),
                    (Handle::SE, PointF::new(r.right(), r.bottom())),
                    (Handle::S, PointF::new(cx, r.bottom())),
                    (Handle::SW, PointF::new(r.x, r.bottom())),
                    (Handle::W, PointF::new(r.x, cy)),
                ];
                if let AnnotationKind::SpeechBalloon { tail, .. } = other {
                    hs.push((Handle::Control, *tail));
                }
                hs
            }
        }
    }

    /// Moves one handle to `p`.
    pub fn drag_handle(&mut self, handle: Handle, p: PointF) {
        match (self, handle) {
            (AnnotationKind::Line { start, .. } | AnnotationKind::Arrow { start, .. }, Handle::Start) => *start = p,
            (AnnotationKind::Line { end, .. } | AnnotationKind::Arrow { end, .. }, Handle::End) => *end = p,
            (AnnotationKind::Line { curve, .. } | AnnotationKind::Arrow { curve, .. }, Handle::Control) => *curve = Some(p),
            (AnnotationKind::SpeechBalloon { tail, .. }, Handle::Control) => *tail = p,
            (kind, h) => {
                let Some(r) = kind.rect() else { return };
                let (mut x0, mut y0, mut x1, mut y1) = (r.x, r.y, r.right(), r.bottom());
                match h {
                    Handle::N => y0 = p.y,
                    Handle::S => y1 = p.y,
                    Handle::W => x0 = p.x,
                    Handle::E => x1 = p.x,
                    Handle::NW => (x0, y0) = (p.x, p.y),
                    Handle::NE => (x1, y0) = (p.x, p.y),
                    Handle::SW => (x0, y1) = (p.x, p.y),
                    Handle::SE => (x1, y1) = (p.x, p.y),
                    _ => {}
                }
                kind.set_rect(RectF::new(x0, y0, x1 - x0, y1 - y0));
            }
        }
    }

    /// Text of text-carrying kinds.
    pub fn text_mut(&mut self) -> Option<&mut String> {
        match self {
            AnnotationKind::Text { text, .. } | AnnotationKind::SpeechBalloon { text, .. } => Some(text),
            _ => None,
        }
    }

    pub fn normalize(&mut self) {
        if let Some(r) = self.rect() {
            self.set_rect(r.normalized());
        }
    }
}

impl Annotation {
    /// Bounds including border and shadow: what repaints when it changes.
    pub fn paint_bounds(&self) -> RectF {
        let mut b = self.kind.bounds().inflate(self.style.border_size + self.style.arrow_head_width.max(4.0) + 2.0);
        if self.style.shadow {
            b = b.union(b.offset(self.style.shadow_offset.0, self.style.shadow_offset.1));
        }
        b
    }
}
