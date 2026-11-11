//! Effects in ShareX `ImageEffectsLib` `.sxie` packages, mapped to Laqta's effect enums. Each arm names a
//! ShareX class and its JSON properties, as Json.NET writes them (see `tests/audit_effects.rs` for the
//! format facts):
//! - `$type` is matched on the bare class name, one of the 51 classes. The namespace and assembly of an
//!   older `ShareX.ImageEffectsLib.Name, ShareX.ImageEffectsLib` form are dropped by the caller.
//! - `Color` is `"Red"`, `"R, G, B"` or `"A, R, G, B"` (alpha first). `Point` and `Size` are `"x, y"` and
//!   `"w, h"`, `ImageMargins` and `Padding` are `"left, top, right, bottom"`, `ImageSides` is a flags string
//!   such as `"Top, Bottom"`, and `ImageFont` is `"Arial, 11.25pt"` with `", style=Bold, Italic"` when styled.
//!
//! Laqta effects have fields ShareX has no property for (seeds, a rotate fill colour, ...). The writer adds
//! them as extra properties, which ShareX ignores when it loads the file, so a file Laqta writes reads back
//! unchanged. The reverse also holds: ShareX-only properties (gradients on text, dash styles other than
//! solid, ...) are not read.
//!
//! `parse` and `write` are mirror images: a name read by one is written by the other.

mod colors;

use laqta_domain::color::Rgba;
use laqta_domain::effects::draw::{BorderKind, Placement, SizeMode};
use laqta_domain::effects::manip::ForceMethod;
use laqta_domain::effects::{Adjust, Draw, Effect, Filter, Manip, TextSpec};
use serde_json::Value;

use crate::json::{self, Object};

/// The identity colour matrix in the domain layout: index `4 * input + output`.
const IDENTITY_MATRIX: [f32; 20] = [
    1.0, 0.0, 0.0, 0.0, //
    0.0, 1.0, 0.0, 0.0, //
    0.0, 0.0, 1.0, 0.0, //
    0.0, 0.0, 0.0, 1.0, //
    0.0, 0.0, 0.0, 0.0,
];

/// The 3x3 kernel that leaves the image as it is, which is ShareX's default.
const IDENTITY_KERNEL: [f32; 9] = [0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0];

const PLACEMENTS: [(&str, Placement); 9] = [
    ("TopLeft", Placement::TopLeft),
    ("TopCenter", Placement::TopCenter),
    ("TopRight", Placement::TopRight),
    ("MiddleLeft", Placement::MiddleLeft),
    ("MiddleCenter", Placement::MiddleCenter),
    ("MiddleRight", Placement::MiddleRight),
    ("BottomLeft", Placement::BottomLeft),
    ("BottomCenter", Placement::BottomCenter),
    ("BottomRight", Placement::BottomRight),
];

/// The image file an effect draws from, which the package may hold.
pub(crate) fn asset_path(effect: &Effect) -> Option<&str> {
    match effect {
        Effect::Draw(Draw::BackgroundImage { path, .. } | Draw::Image { path, .. } | Draw::Particles { path, .. }) => Some(path),
        _ => None,
    }
}

/// The effect a ShareX class and its properties describe. `None` when the class has no Laqta
/// equivalent. The class name is matched without regard to case.
pub(crate) fn parse(class: &str, object: &Object) -> Option<Effect> {
    let f = Fields(object);
    let effect = match class.to_ascii_lowercase().as_str() {
        "alpha" => Effect::Adjust(Adjust::Alpha { alpha: f.float("Value", 1.0), add: f.float("Addition", 0.0) }),
        "blackwhite" => Effect::Adjust(Adjust::BlackWhite),
        // ShareX's colour amounts are fractions (`SkiaColorMatrixManager`); Laqta's are percent. A file
        // Laqta wrote carries its own value as `LaqtaValue`, which wins.
        "brightness" => Effect::Adjust(Adjust::Brightness { value: f.native(|| 100.0 * f.float("Value", 0.0)) }),
        "colorize" => Effect::Adjust(Adjust::Colorize {
            color: f.color("Color", Rgba::rgb(255, 0, 0)),
            value: f.native(|| 100.0 * f.float("Value", 0.0)),
        }),
        // ShareX multiplies R, G and B by `Value` (`SkiaColorMatrixManager.Contrast`). Laqta's own
        // `Adjust::Contrast` pivots on mid grey, so the import keeps ShareX's look as a colour matrix.
        "contrast" => match f.laqta_value() {
            Some(value) => Effect::Adjust(Adjust::Contrast { value }),
            None => Effect::Adjust(Adjust::MatrixColor {
                matrix: scaled_matrix(&[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], f.float("Value", 1.0)),
            }),
        },
        "gamma" => Effect::Adjust(Adjust::Gamma { value: f.float("Value", 1.0) }),
        // ShareX's Grayscale and Sepia scale the target matrix by `Value`; Laqta's blend towards it. They
        // agree at 1 (100 %), and any other ShareX value keeps its look as a colour matrix.
        "grayscale" => scaled_or_matrix(&f, &SHAREX_GRAY, |value| Adjust::Grayscale { value }),
        "hue" => Effect::Adjust(Adjust::Hue { angle: f.float_first(&["Angle", "Value"], 0.0) }),
        "inverse" => Effect::Adjust(Adjust::Inverse),
        "matrixcolor" => Effect::Adjust(Adjust::MatrixColor { matrix: f.matrix() }),
        "polaroid" => Effect::Adjust(Adjust::Polaroid),
        // ShareX's 1 leaves the image as it is; Laqta's 0 does.
        "saturation" => Effect::Adjust(Adjust::Saturation { value: f.native(|| 100.0 * (f.float("Value", 1.0) - 1.0)) }),
        // ShareX's `SelectiveColor` is a light/dark palette (`LightColor`, `DarkColor`, `PaletteSize`), a
        // different effect from Laqta's. These properties are Laqta's own and ShareX's are not read.
        "selectivecolor" => Effect::Adjust(Adjust::SelectiveColor {
            from: f.color("Source", Rgba::rgb(255, 0, 0)),
            to: f.color("Target", Rgba::rgb(0, 0, 255)),
            tolerance: f.byte("Tolerance", 32),
        }),
        "sepia" => scaled_or_matrix(&f, &SHAREX_SEPIA, |value| Adjust::Sepia { value }),
        "replacecolor" => Effect::Adjust(Adjust::ReplaceColor {
            source: f.color("SourceColor", Rgba::WHITE),
            target: f.color("TargetColor", Rgba::TRANSPARENT),
            tolerance: f.byte("Threshold", 0),
            auto_source_from_corner: f.flag("AutoSourceColor", false),
        }),

        "drawbackground" => Effect::Draw(Draw::Background { color: f.color("Color", Rgba::BLACK), gradient: f.gradient() }),
        "drawbackgroundimage" => Effect::Draw(Draw::BackgroundImage {
            path: f.text("ImageFilePath", ""),
            center: f.flag("Center", true),
            tile: f.flag("Tile", false),
        }),
        "drawborder" => Effect::Draw(Draw::Border {
            kind: f.border_kind("Type"),
            size: f.uint("Size", 1),
            color: f.color("Color", Rgba::BLACK),
            dash: !f.text("DashStyle", "Solid").eq_ignore_ascii_case("Solid"),
        }),
        "drawcheckerboard" => Effect::Draw(Draw::Checkerboard {
            size: f.uint("Size", 10),
            color1: f.color("Color", Rgba::rgb(211, 211, 211)),
            color2: f.color("Color2", Rgba::WHITE),
        }),
        "drawimage" => Effect::Draw(Draw::Image {
            // `Path` is the name Laqta used before it matched ShareX's.
            path: f.text_first(&["ImageLocation", "Path"], ""),
            placement: f.placement("Placement", Placement::TopLeft),
            offset: f.point("Offset", (0, 0)),
            size_mode: f.size_mode(),
            opacity: f.float("Opacity", 100.0) / 100.0,
        }),
        "drawparticles" => Effect::Draw(Draw::Particles {
            path: f.text("ImageFolder", ""),
            count: f.uint("ImageCount", 1),
            min_size: f.uint("RandomSizeMin", 64),
            max_size: f.uint("RandomSizeMax", 128),
            random_angle: f.flag("RandomAngle", false),
            seed: f.seed("Seed", 1),
        }),
        "drawtext" => Effect::Draw(Draw::Text {
            text: f.text_spec("TextFont", "TextColor", 11.25),
            placement: f.placement("Placement", Placement::BottomRight),
            offset: f.point("Offset", (5, 5)),
            background: f.flag("DrawBackground", true).then(|| f.color("BackgroundColor", Rgba::rgb(42, 47, 56))),
            padding: f.padding("Padding", 5),
            corner_radius: f.uint("CornerRadius", 4),
        }),
        "drawtextex" => Effect::Draw(Draw::TextEx {
            text: f.text_spec("Font", "Color", 36.0),
            angle: f.float("Angle", 0.0),
            placement: f.placement("Placement", Placement::TopLeft),
            offset: f.point("Offset", (0, 0)),
            outline: f.flag("Outline", false).then(|| (f.color("OutlineColor", Rgba::rgb(235, 0, 0)), f.float("OutlineSize", 5.0))),
            shadow: f.flag("Shadow", false).then(|| {
                let (x, y) = f.point("ShadowOffset", (0, 5));
                (f.color("ShadowColor", Rgba { r: 0, g: 0, b: 0, a: 125 }), x, y)
            }),
        }),

        "blur" => Effect::Filter(Filter::Blur { radius: f.uint("Radius", 15) }),
        "colordepth" => Effect::Filter(Filter::ColorDepth { bits: f.byte("BitsPerChannel", 4) }),
        "edgedetect" => Effect::Filter(Filter::EdgeDetect),
        "emboss" => Effect::Filter(Filter::Emboss),
        // ShareX's `Radius` is the standard deviation of its box-blur approximation.
        // ShareX blurs with sigma = Radius / 3 (`SkiaImageHelpers.GaussianBlur`).
        "gaussianblur" => Effect::Filter(Filter::GaussianBlur { sigma: f.native(|| f.float("Radius", 15.0) / 3.0) }),
        "glow" => {
            let (offset_x, offset_y) = f.point("Offset", (0, 0));
            Effect::Filter(Filter::Glow {
                size: f.uint("Size", 20),
                strength: f.float("Strength", 1.0),
                color: f.color("Color", Rgba::WHITE),
                offset_x,
                offset_y,
            })
        }
        "matrixconvolution" => Effect::Filter(Filter::ConvolutionMatrix {
            kernel: f.kernel(),
            divisor: f.float("Factor", 1.0),
            offset: f.float("Offset", 0.0),
        }),
        "meanremoval" => Effect::Filter(Filter::MeanRemoval),
        "outline" => Effect::Filter(Filter::Outline {
            size: f.uint("Size", 1),
            padding: f.uint("Padding", 0),
            color: f.color("Color", Rgba::BLACK),
            outline_only: f.flag("OutlineOnly", false),
        }),
        "pixelate" => Effect::Filter(Filter::Pixelate { size: f.uint("Size", 10) }),
        "reflection" => Effect::Filter(Filter::Reflection {
            percentage: f.byte("Percentage", 20),
            max_alpha: f.byte("MaxAlpha", 255),
            min_alpha: f.byte("MinAlpha", 0),
            offset: f.uint("Offset", 0),
            skew: f.flag("Skew", false),
            skew_size: f.uint("SkewSize", 25),
        }),
        "rgbsplit" => Effect::Filter(Filter::RgbSplit {
            offset_red: f.point("OffsetRed", (-5, 0)),
            offset_green: f.point("OffsetGreen", (0, 0)),
            offset_blue: f.point("OffsetBlue", (5, 0)),
        }),
        "shadow" => {
            let (offset_x, offset_y) = f.point("Offset", (0, 0));
            Effect::Filter(Filter::Shadow {
                opacity: f.float("Opacity", 0.6),
                size: f.uint("Size", 10),
                darkness: f.float("Darkness", 0.0),
                color: f.color("Color", Rgba::BLACK),
                offset_x,
                offset_y,
                auto_resize: f.flag("AutoResize", true),
            })
        }
        "sharpen" => Effect::Filter(Filter::Sharpen),
        "slice" => Effect::Filter(Filter::Slice {
            min_height: f.uint("MinSliceHeight", 10),
            max_height: f.uint("MaxSliceHeight", 100),
            min_shift: f.uint("MinSliceShift", 0),
            max_shift: f.uint("MaxSliceShift", 10),
            seed: f.seed("Seed", 1),
        }),
        "smooth" => Effect::Filter(Filter::Smooth),
        "tornedge" => {
            let sides = f.sides("Sides");
            Effect::Filter(Filter::TornEdge {
                depth: f.uint("Depth", 15),
                range: f.uint("Range", 20),
                top: sides.top,
                right: sides.right,
                bottom: sides.bottom,
                left: sides.left,
                curved: f.flag("CurvedEdges", true),
                seed: f.seed("Seed", 1),
            })
        }
        "waveedge" => {
            let sides = f.sides("Sides");
            Effect::Filter(Filter::WaveEdge {
                depth: f.uint("Depth", 15),
                range: f.uint("Range", 20),
                top: sides.top,
                right: sides.right,
                bottom: sides.bottom,
                left: sides.left,
            })
        }
        "noise" => Effect::Filter(Filter::Noise { amount: f.byte("Amount", 24), seed: f.seed("Seed", 1) }),
        "posterize" => Effect::Filter(Filter::Posterize { levels: f.byte("Levels", 4) }),
        "vignette" => Effect::Filter(Filter::Vignette {
            strength: f.float("Strength", 0.6),
            radius: f.float("Radius", 0.5),
            color: f.color("Color", Rgba::BLACK),
        }),
        "halftone" => Effect::Filter(Filter::Halftone { cell: f.uint("Cell", 6) }),
        "dither" => Effect::Filter(Filter::Dither { levels: f.byte("Levels", 2) }),
        "motionblur" => Effect::Filter(Filter::MotionBlur { distance: f.uint("Distance", 12), angle: f.float("Angle", 0.0) }),
        "zoomblur" => Effect::Filter(Filter::ZoomBlur {
            amount: f.float("Amount", 0.3),
            center_x: f.float("CenterX", 0.5),
            center_y: f.float("CenterY", 0.5),
        }),
        "median" => Effect::Filter(Filter::Median { radius: f.uint("Radius", 2) }),
        "oilpaint" => Effect::Filter(Filter::OilPaint { radius: f.uint("Radius", 3), levels: f.byte("Levels", 16) }),

        // `Color` and `Tolerance` are Laqta's own: ShareX crops transparent borders only.
        "autocrop" => Effect::Manip(Manip::AutoCrop {
            color: f.color("Color", Rgba::TRANSPARENT),
            tolerance: f.byte("Tolerance", 0),
            margin: f.uint("Padding", 0),
        }),
        "canvas" => Effect::Manip(Manip::Canvas { margin: f.margin("Margin", (0, 0, 0, 0)), color: f.color("Color", Rgba::TRANSPARENT) }),
        "crop" => {
            let (left, top, right, bottom) = f.margin("Margin", (0, 0, 0, 0));
            Effect::Manip(Manip::Crop {
                margin: (left.max(0).unsigned_abs(), top.max(0).unsigned_abs(), right.max(0).unsigned_abs(), bottom.max(0).unsigned_abs()),
            })
        }
        "flip" => Effect::Manip(Manip::Flip { horizontal: f.flag("Horizontally", true), vertical: f.flag("Vertically", false) }),
        "forceproportions" => Effect::Manip(Manip::ForceProportions {
            width: f.uint("ProportionalWidth", 1),
            height: f.uint("ProportionalHeight", 1),
            method: f.force_method("Method"),
            color: f.color("GrowFillColor", Rgba::TRANSPARENT),
        }),
        "resize" => {
            Effect::Manip(Manip::Resize { width: f.uint("Width", 0), height: f.uint("Height", 0), keep_aspect: f.flag("KeepAspect", true) })
        }
        // `Color` is Laqta's own: ShareX fills uncovered corners with transparent.
        "rotate" => Effect::Manip(Manip::Rotate {
            angle: f.float("Angle", 0.0),
            grow: f.flag("Upsize", true),
            color: f.color("Color", Rgba::TRANSPARENT),
        }),
        "roundedcorners" => Effect::Manip(Manip::RoundedCorners { radius: f.uint("CornerRadius", 20) }),
        "scale" => {
            Effect::Manip(Manip::Scale { width_pct: f.float("WidthPercentage", 100.0), height_pct: f.float("HeightPercentage", 0.0) })
        }
        // `Grow` is Laqta's own.
        "skew" => Effect::Manip(Manip::Skew { x: f.int("Horizontally", 0), y: f.int("Vertically", 0), grow: f.flag("Grow", true) }),
        _ => return None,
    };
    Some(effect)
}

/// ShareX's grey target, indexed [input][output] (`SkiaColorMatrixManager.Grayscale`).
const SHAREX_GRAY: [[f32; 3]; 3] = [[0.212_671; 3], [0.715_16; 3], [0.072_169; 3]];
/// ShareX's sepia target, indexed [input][output] (`SkiaColorMatrixManager.Sepia`).
const SHAREX_SEPIA: [[f32; 3]; 3] = [[0.393, 0.349, 0.272], [0.769, 0.686, 0.534], [0.189, 0.168, 0.131]];

/// A colour matrix whose R, G and B part is `target` times `value`, alpha and the constant row
/// unchanged. Domain layout, so index `4 * input + output`.
fn scaled_matrix(target: &[[f32; 3]; 3], value: f32) -> [f32; 20] {
    let mut matrix = IDENTITY_MATRIX;
    for (input, row) in target.iter().enumerate() {
        for (output, weight) in row.iter().enumerate() {
            matrix[4 * input + output] = weight * value;
        }
    }
    matrix
}

/// ShareX's Grayscale or Sepia: Laqta's effect at 100 % when `Value` is 1 (or a Laqta file says
/// otherwise), else the scaled matrix ShareX applies.
fn scaled_or_matrix(f: &Fields<'_>, target: &[[f32; 3]; 3], effect: fn(f32) -> Adjust) -> Effect {
    if let Some(value) = f.laqta_value() {
        return Effect::Adjust(effect(value));
    }
    let value = f.float("Value", 1.0);
    if (value - 1.0).abs() < 1e-6 {
        Effect::Adjust(effect(100.0))
    } else {
        Effect::Adjust(Adjust::MatrixColor { matrix: scaled_matrix(target, value) })
    }
}

/// The ShareX class name and its properties for an effect. `None` for the effects that have no confirmed
/// ShareX class: they are read, but never written under a guessed name.
pub(crate) fn write(effect: &Effect) -> Option<(&'static str, Object)> {
    let mut p = Put::default();
    let class = match effect {
        Effect::Adjust(adjust) => match adjust {
            Adjust::Alpha { alpha, add } => {
                p.float("Value", *alpha);
                p.float("Addition", *add);
                "Alpha"
            }
            Adjust::BlackWhite => "BlackWhite",
            Adjust::Brightness { value } => {
                p.float("Value", *value / 100.0);
                p.float("LaqtaValue", *value);
                "Brightness"
            }
            Adjust::Colorize { color, value } => {
                p.color("Color", *color);
                p.float("Value", *value / 100.0);
                p.float("LaqtaValue", *value);
                "Colorize"
            }
            // Lossy: ShareX's `Contrast` multiplies R, G and B by `Value`, while Laqta's scales around mid
            // grey, so no `Value` gives the same picture. `1 + value / 100` is the same scale factor. ShareX
            // applies it as a plain multiply; Laqta reads its own value back from `LaqtaValue`.
            Adjust::Contrast { value } => {
                p.float("Value", 1.0 + *value / 100.0);
                p.float("LaqtaValue", *value);
                "Contrast"
            }
            Adjust::Gamma { value } => {
                p.float("Value", *value);
                "Gamma"
            }
            // Lossy away from 100 %: ShareX scales the target where Laqta blends towards it.
            Adjust::Grayscale { value } => {
                p.float("Value", *value / 100.0);
                p.float("LaqtaValue", *value);
                "Grayscale"
            }
            Adjust::Hue { angle } => {
                p.float("Angle", *angle);
                "Hue"
            }
            Adjust::Inverse => "Inverse",
            Adjust::MatrixColor { matrix } => {
                p.matrix(matrix);
                "MatrixColor"
            }
            Adjust::Polaroid => "Polaroid",
            Adjust::Saturation { value } => {
                p.float("Value", 1.0 + *value / 100.0);
                p.float("LaqtaValue", *value);
                "Saturation"
            }
            Adjust::SelectiveColor { from, to, tolerance } => {
                p.color("Source", *from);
                p.color("Target", *to);
                p.uint("Tolerance", u64::from(*tolerance));
                "SelectiveColor"
            }
            Adjust::Sepia { value } => {
                p.float("Value", *value / 100.0);
                p.float("LaqtaValue", *value);
                "Sepia"
            }
            Adjust::ReplaceColor { source, target, tolerance, auto_source_from_corner } => {
                p.color("SourceColor", *source);
                p.flag("AutoSourceColor", *auto_source_from_corner);
                p.color("TargetColor", *target);
                p.uint("Threshold", u64::from(*tolerance));
                "ReplaceColor"
            }
            Adjust::AutoContrast { .. }
            | Adjust::ChannelSwap { .. }
            | Adjust::Clarity { .. }
            | Adjust::ColorBalance { .. }
            | Adjust::CrossProcess { .. }
            | Adjust::Dehaze { .. }
            | Adjust::DuotoneGradientMap { .. }
            | Adjust::Exposure { .. }
            | Adjust::FilmEmulation { .. }
            | Adjust::Levels { .. }
            | Adjust::Lomo { .. }
            | Adjust::ShadowsHighlights { .. }
            | Adjust::Solarize { .. }
            | Adjust::SplitToning { .. }
            | Adjust::TemperatureTint { .. }
            | Adjust::Threshold { .. }
            | Adjust::Vibrance { .. } => return None,
        },
        Effect::Draw(draw) => match draw {
            Draw::Background { color, gradient } => {
                p.color("Color", *color);
                p.gradient(*gradient);
                "DrawBackground"
            }
            Draw::BackgroundImage { path, center, tile } => {
                p.text("ImageFilePath", path);
                p.flag("Center", *center);
                p.flag("Tile", *tile);
                "DrawBackgroundImage"
            }
            Draw::Border { kind, size, color, dash } => {
                p.border_kind("Type", *kind);
                p.uint("Size", u64::from(*size));
                p.text("DashStyle", if *dash { "Dash" } else { "Solid" });
                p.color("Color", *color);
                p.flag("UseGradient", false);
                "DrawBorder"
            }
            Draw::Checkerboard { size, color1, color2 } => {
                p.uint("Size", u64::from(*size));
                p.color("Color", *color1);
                p.color("Color2", *color2);
                "DrawCheckerboard"
            }
            Draw::Image { path, placement, offset, size_mode, opacity } => {
                p.text("ImageLocation", path);
                p.placement("Placement", *placement);
                p.point("Offset", *offset);
                p.size_mode(*size_mode);
                // ShareX's opacity is a whole percentage.
                p.float("Opacity", (*opacity * 100.0).round());
                "DrawImage"
            }
            Draw::Particles { path, count, min_size, max_size, random_angle, seed } => {
                p.text("ImageFolder", path);
                p.uint("ImageCount", u64::from(*count));
                p.flag("RandomSize", true);
                p.uint("RandomSizeMin", u64::from(*min_size));
                p.uint("RandomSizeMax", u64::from(*max_size));
                p.flag("RandomAngle", *random_angle);
                p.uint("Seed", *seed);
                "DrawParticles"
            }
            Draw::Text { text, placement, offset, background, padding, corner_radius } => {
                p.text("Text", &text.text);
                p.placement("Placement", *placement);
                p.point("Offset", *offset);
                p.font("TextFont", text);
                p.color("TextColor", text.color);
                p.uint("CornerRadius", u64::from(*corner_radius));
                p.padding("Padding", *padding);
                p.flag("DrawBackground", background.is_some());
                p.color("BackgroundColor", background.unwrap_or(Rgba::rgb(42, 47, 56)));
                // Laqta's text has no shadow, border or gradient, which ShareX would otherwise default to.
                p.flag("DrawTextShadow", false);
                p.flag("DrawBorder", false);
                p.flag("UseGradient", false);
                "DrawText"
            }
            Draw::TextEx { text, angle, placement, offset, outline, shadow } => {
                p.text("Text", &text.text);
                p.placement("Placement", *placement);
                p.point("Offset", *offset);
                p.float("Angle", *angle);
                p.font("Font", text);
                p.color("Color", text.color);
                p.flag("UseGradient", false);
                p.flag("Outline", outline.is_some());
                let (outline_color, outline_size) = outline.unwrap_or((Rgba::rgb(235, 0, 0), 5.0));
                p.float("OutlineSize", outline_size);
                p.color("OutlineColor", outline_color);
                p.flag("Shadow", shadow.is_some());
                let (shadow_color, shadow_x, shadow_y) = shadow.unwrap_or((Rgba { r: 0, g: 0, b: 0, a: 125 }, 0, 5));
                p.point("ShadowOffset", (shadow_x, shadow_y));
                p.color("ShadowColor", shadow_color);
                "DrawTextEx"
            }
            Draw::Confetti { .. }
            | Draw::DrawLine { .. }
            | Draw::DrawShape { .. }
            | Draw::FakeCursor { .. }
            | Draw::GradientOverlay { .. }
            | Draw::Snowflakes { .. } => return None,
        },
        Effect::Filter(filter) => match filter {
            Filter::Blur { radius } => {
                p.uint("Radius", u64::from(*radius));
                "Blur"
            }
            Filter::ColorDepth { bits } => {
                p.uint("BitsPerChannel", u64::from(*bits));
                "ColorDepth"
            }
            Filter::ConvolutionMatrix { kernel, divisor, offset } => {
                p.kernel(kernel);
                p.float("Factor", *divisor);
                p.float("Offset", *offset);
                "MatrixConvolution"
            }
            Filter::EdgeDetect => "EdgeDetect",
            Filter::Emboss => "Emboss",
            // ShareX's `Radius` is a whole number, three sigmas.
            Filter::GaussianBlur { sigma } => {
                p.uint("Radius", (f64::from(*sigma) * 3.0).round().max(0.0) as u64);
                p.float("LaqtaValue", *sigma);
                "GaussianBlur"
            }
            Filter::Glow { size, strength, color, offset_x, offset_y } => {
                p.uint("Size", u64::from(*size));
                p.float("Strength", *strength);
                p.color("Color", *color);
                p.flag("UseGradient", false);
                p.point("Offset", (*offset_x, *offset_y));
                "Glow"
            }
            Filter::MeanRemoval => "MeanRemoval",
            Filter::Outline { size, padding, color, outline_only } => {
                p.uint("Size", u64::from(*size));
                p.uint("Padding", u64::from(*padding));
                p.color("Color", *color);
                p.flag("OutlineOnly", *outline_only);
                "Outline"
            }
            Filter::Pixelate { size } => {
                p.uint("Size", u64::from(*size));
                "Pixelate"
            }
            Filter::Reflection { percentage, max_alpha, min_alpha, offset, skew, skew_size } => {
                p.uint("Percentage", u64::from(*percentage));
                p.uint("MaxAlpha", u64::from(*max_alpha));
                p.uint("MinAlpha", u64::from(*min_alpha));
                p.uint("Offset", u64::from(*offset));
                p.flag("Skew", *skew);
                p.uint("SkewSize", u64::from(*skew_size));
                "Reflection"
            }
            Filter::RgbSplit { offset_red, offset_green, offset_blue } => {
                p.point("OffsetRed", *offset_red);
                p.point("OffsetGreen", *offset_green);
                p.point("OffsetBlue", *offset_blue);
                "RGBSplit"
            }
            Filter::Shadow { opacity, size, darkness, color, offset_x, offset_y, auto_resize } => {
                p.float("Opacity", *opacity);
                p.uint("Size", u64::from(*size));
                p.float("Darkness", *darkness);
                p.color("Color", *color);
                p.point("Offset", (*offset_x, *offset_y));
                p.flag("AutoResize", *auto_resize);
                "Shadow"
            }
            Filter::Sharpen => "Sharpen",
            Filter::Slice { min_height, max_height, min_shift, max_shift, seed } => {
                p.uint("MinSliceHeight", u64::from(*min_height));
                p.uint("MaxSliceHeight", u64::from(*max_height));
                p.uint("MinSliceShift", u64::from(*min_shift));
                p.uint("MaxSliceShift", u64::from(*max_shift));
                p.uint("Seed", *seed);
                "Slice"
            }
            Filter::Smooth => "Smooth",
            Filter::TornEdge { depth, range, top, right, bottom, left, curved, seed } => {
                p.uint("Depth", u64::from(*depth));
                p.uint("Range", u64::from(*range));
                p.sides("Sides", Sides { top: *top, right: *right, bottom: *bottom, left: *left });
                p.flag("CurvedEdges", *curved);
                p.uint("Seed", *seed);
                "TornEdge"
            }
            Filter::WaveEdge { depth, range, top, right, bottom, left } => {
                p.uint("Depth", u64::from(*depth));
                p.uint("Range", u64::from(*range));
                p.sides("Sides", Sides { top: *top, right: *right, bottom: *bottom, left: *left });
                "WaveEdge"
            }
            Filter::Noise { .. }
            | Filter::Posterize { .. }
            | Filter::Vignette { .. }
            | Filter::Halftone { .. }
            | Filter::Dither { .. }
            | Filter::MotionBlur { .. }
            | Filter::ZoomBlur { .. }
            | Filter::Median { .. }
            | Filter::OilPaint { .. } => return None,
            // Laqta-only filters have no ImageEffectsLib class: add `Filter::Name { .. } => return None,` arms.
            Filter::Anaglyph3D { .. } => return None,
            Filter::AnamorphicLensFlare { .. } => return None,
            Filter::AnimeSpeedLines { .. } => return None,
            Filter::ArtDecoBorder { .. } => return None,
            Filter::ASCIIArt { .. } => return None,
            Filter::Bevel { .. } => return None,
            Filter::BlockGlitch { .. } => return None,
            Filter::BloodSplash { .. } => return None,
            Filter::Bloom { .. } => return None,
            Filter::BlueprintDrawing { .. } => return None,
            Filter::BrickWallBorder { .. } => return None,
            Filter::CandyCaneBorder { .. } => return None,
            Filter::CartoonStickerCutout { .. } => return None,
            Filter::CelticKnotBorder { .. } => return None,
            Filter::ChainLinkBorder { .. } => return None,
            Filter::ChalkDrawing { .. } => return None,
            Filter::ChromaticAberration { .. } => return None,
            Filter::ChromeMetallicBorder { .. } => return None,
            Filter::CircuitBoardBorder { .. } => return None,
            Filter::ClaymationTexture { .. } => return None,
            Filter::Clouds { .. } => return None,
            Filter::ColorHalftone { .. } => return None,
            Filter::ColorSplash { .. } => return None,
            Filter::CrimsonBorder { .. } => return None,
            Filter::Crosshatch { .. } => return None,
            Filter::CRT { .. } => return None,
            Filter::CrystalizeShards { .. } => return None,
            Filter::CrystalPrism { .. } => return None,
            Filter::Cyanotype { .. } => return None,
            Filter::DatamoshSmear { .. } => return None,
            Filter::DiamondPixelate { .. } => return None,
            Filter::DisposableCamera { .. } => return None,
            Filter::DotMatrix { .. } => return None,
            Filter::DoubleExposure { .. } => return None,
            Filter::DreamGlow { .. } => return None,
            Filter::EdgeFeather { .. } => return None,
            Filter::Embroidery { .. } => return None,
            Filter::Engraving { .. } => return None,
            Filter::EtchedGlass { .. } => return None,
            Filter::FakeHair { .. } => return None,
            Filter::FilmGrain { .. } => return None,
            Filter::FilmStripFrame { .. } => return None,
            Filter::FrostedGlass { .. } => return None,
            Filter::FrostedGlassIceEdges { .. } => return None,
            Filter::FrostIceBorder { .. } => return None,
            Filter::GhostTrail { .. } => return None,
            Filter::Glare { .. } => return None,
            Filter::GlitchShift { .. } => return None,
            Filter::GoldenBorder { .. } => return None,
            Filter::GradientFadeBorder { .. } => return None,
            Filter::Halation { .. } => return None,
            Filter::HeatHazeRefraction { .. } => return None,
            Filter::HeatMap => return None,
            Filter::HexagonalPixelate { .. } => return None,
            Filter::HologramScan { .. } => return None,
            Filter::HolographicFoilShimmer { .. } => return None,
            Filter::InfraredFalseColor { .. } => return None,
            Filter::InkSplatterDrips { .. } => return None,
            Filter::InnerShadow { .. } => return None,
            Filter::JpegArtifact { .. } => return None,
            Filter::LavaMoltenBorder { .. } => return None,
            Filter::LensBlur { .. } => return None,
            Filter::LightLeak { .. } => return None,
            Filter::LiquidGlass { .. } => return None,
            Filter::LiquidMercury { .. } => return None,
            Filter::LuminanceContourLines { .. } => return None,
            Filter::MacOSWindow { .. } => return None,
            Filter::MarbleBorder { .. } => return None,
            Filter::MatrixDigitalRain { .. } => return None,
            Filter::MinecraftGrass { .. } => return None,
            Filter::Miniature { .. } => return None,
            Filter::MosaicPolygon { .. } => return None,
            Filter::MosaicTileBorder { .. } => return None,
            Filter::NebulaStarfield { .. } => return None,
            Filter::NeonEdgeGlow { .. } => return None,
            Filter::NeonGlowBorder { .. } => return None,
            Filter::NightVision { .. } => return None,
            Filter::OilSlickInterference { .. } => return None,
            Filter::OldCameraFlashBurn { .. } => return None,
            Filter::OrnateScrollBorder { .. } => return None,
            Filter::PaletteMap { .. } => return None,
            Filter::PaperStencilMask { .. } => return None,
            Filter::PencilSketch { .. } => return None,
            Filter::PerlinNoise { .. } => return None,
            Filter::PixelSorting { .. } => return None,
            Filter::PlasmaEnergyArcs { .. } => return None,
            Filter::Pointillism { .. } => return None,
            Filter::PolaroidFrame { .. } => return None,
            Filter::PopArtGrid { .. } => return None,
            Filter::RadialRainbow { .. } => return None,
            Filter::RainbowBorder { .. } => return None,
            Filter::RainyWindow { .. } => return None,
            Filter::RetroSunset { .. } => return None,
            Filter::RisoPrint { .. } => return None,
            Filter::RopeBorder { .. } => return None,
            Filter::RustCorrosion { .. } => return None,
            Filter::Scanline { .. } => return None,
            Filter::SmokeOverlay { .. } => return None,
            Filter::SnowfallDepthFog { .. } => return None,
            Filter::SobelEdge { .. } => return None,
            Filter::SoftDiffusion { .. } => return None,
            Filter::SpinBlur { .. } => return None,
            Filter::StainedGlass { .. } => return None,
            Filter::StainedGlassBorder { .. } => return None,
            Filter::StarfieldBorder { .. } => return None,
            Filter::StarFilter { .. } => return None,
            Filter::StripeOverlay { .. } => return None,
            Filter::SurfaceBlur { .. } => return None,
            Filter::ThermalVision { .. } => return None,
            Filter::TiltShift { .. } => return None,
            Filter::TrianglePixelate { .. } => return None,
            Filter::TVStatic { .. } => return None,
            Filter::UnsharpMask { .. } => return None,
            Filter::VhsTapeDamage { .. } => return None,
            Filter::VintagePrintDamage { .. } => return None,
            Filter::VoronoiMosaic { .. } => return None,
            Filter::WatercolorBorder { .. } => return None,
            Filter::WatercolorKuwahara { .. } => return None,
            Filter::WetPlateCollodion { .. } => return None,
            Filter::Windows11 { .. } => return None,
            Filter::WindStreaks { .. } => return None,
            Filter::WoodcutPrint { .. } => return None,
            Filter::WoodenFrame { .. } => return None,
            Filter::XRayScan { .. } => return None,
            Filter::ZigZagBorder { .. } => return None,
        },
        Effect::Manip(manip) => match manip {
            Manip::AutoCrop { color, tolerance, margin } => {
                p.sides("Sides", Sides { top: true, right: true, bottom: true, left: true });
                p.uint("Padding", u64::from(*margin));
                p.color("Color", *color);
                p.uint("Tolerance", u64::from(*tolerance));
                "AutoCrop"
            }
            Manip::Canvas { margin, color } => {
                p.margin("Margin", *margin);
                p.text("MarginMode", "AbsoluteSize");
                p.color("Color", *color);
                "Canvas"
            }
            Manip::Crop { margin } => {
                let signed = |value: u32| i32::try_from(value).unwrap_or(i32::MAX);
                p.margin("Margin", (signed(margin.0), signed(margin.1), signed(margin.2), signed(margin.3)));
                "Crop"
            }
            Manip::Flip { horizontal, vertical } => {
                p.flag("Horizontally", *horizontal);
                p.flag("Vertically", *vertical);
                "Flip"
            }
            Manip::ForceProportions { width, height, method, color } => {
                p.uint("ProportionalWidth", u64::from(*width));
                p.uint("ProportionalHeight", u64::from(*height));
                p.force_method("Method", *method);
                p.color("GrowFillColor", *color);
                "ForceProportions"
            }
            Manip::Resize { width, height, keep_aspect } => {
                p.uint("Width", u64::from(*width));
                p.uint("Height", u64::from(*height));
                p.flag("KeepAspect", *keep_aspect);
                "Resize"
            }
            Manip::Rotate { angle, grow, color } => {
                p.float("Angle", *angle);
                p.flag("Upsize", *grow);
                p.flag("Clip", false);
                p.color("Color", *color);
                "Rotate"
            }
            Manip::RoundedCorners { radius } => {
                p.uint("CornerRadius", u64::from(*radius));
                "RoundedCorners"
            }
            Manip::Scale { width_pct, height_pct } => {
                p.float("WidthPercentage", *width_pct);
                p.float("HeightPercentage", *height_pct);
                "Scale"
            }
            Manip::Skew { x, y, grow } => {
                p.int("Horizontally", *x);
                p.int("Vertically", *y);
                p.flag("Grow", *grow);
                "Skew"
            }
            Manip::CylinderWrap { .. }
            | Manip::DisplacementMap { .. }
            | Manip::FisheyeLens { .. }
            | Manip::FoldCreaseWarp { .. }
            | Manip::GlassSphere { .. }
            | Manip::Kaleidoscope { .. }
            | Manip::LiquifyPushSmudge { .. }
            | Manip::MirrorTiles { .. }
            | Manip::PageCurl { .. }
            | Manip::PerspectiveWarp { .. }
            | Manip::PinchBulge { .. }
            | Manip::PolarWarp { .. }
            | Manip::RippleRefraction { .. }
            | Manip::Rotate3D { .. }
            | Manip::Rotate3DBox { .. }
            | Manip::Symmetry { .. }
            | Manip::TileRepeat { .. }
            | Manip::TileShuffle { .. }
            | Manip::Twirl { .. }
            | Manip::WaveDistortion { .. } => return None,
        },
    };
    Some((class, p.into_object()))
}

/// Which edges of an `ImageSides` flags value are set.
#[derive(Clone, Copy)]
struct Sides {
    top: bool,
    right: bool,
    bottom: bool,
    left: bool,
}

/// The channels of a ShareX `MatrixColor` property name: `Rg` is how much input G adds to output R
/// (ShareX builds row g, column R of its `ImageColorMatrix` from it). `Ro`, `Go`, `Bo` and `Ao` are
/// the offsets of outputs R, G, B and A, the constant row of the domain layout.
const MATRIX_CHANNELS: [char; 4] = ['R', 'G', 'B', 'A'];

/// Typed reads from one ShareX effect object. Every read falls back to the default when the property is
/// absent or not of the right type.
struct Fields<'a>(&'a Object);

impl Fields<'_> {
    fn float(&self, key: &str, default: f32) -> f32 {
        json::number(self.0, key).map_or(default, |value| value as f32)
    }

    /// The value a file Laqta wrote keeps in its own units, which ShareX ignores.
    fn laqta_value(&self) -> Option<f32> {
        json::number(self.0, "LaqtaValue").map(|value| value as f32)
    }

    /// `LaqtaValue` when there is one, else the ShareX value converted by `sharex`.
    fn native(&self, sharex: impl FnOnce() -> f32) -> f32 {
        self.laqta_value().unwrap_or_else(sharex)
    }

    fn float_first(&self, keys: &[&str], default: f32) -> f32 {
        keys.iter().find_map(|key| json::number(self.0, key)).map_or(default, |value| value as f32)
    }

    fn uint(&self, key: &str, default: u32) -> u32 {
        json::number(self.0, key).map_or(default, |value| value.round().clamp(0.0, f64::from(u32::MAX)) as u32)
    }

    fn int(&self, key: &str, default: i32) -> i32 {
        json::number(self.0, key).map_or(default, |value| value.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32)
    }

    fn byte(&self, key: &str, default: u8) -> u8 {
        json::number(self.0, key).map_or(default, |value| value.round().clamp(0.0, 255.0) as u8)
    }

    /// A 64-bit seed. Integers are read exactly, so a seed above 2^53 survives.
    fn seed(&self, key: &str, default: u64) -> u64 {
        match self.0.get(key) {
            Some(value) => value.as_u64().or_else(|| json::number(self.0, key).map(|number| number as u64)).unwrap_or(default),
            None => default,
        }
    }

    fn flag(&self, key: &str, default: bool) -> bool {
        json::flag(self.0, key).unwrap_or(default)
    }

    fn text(&self, key: &str, default: &str) -> String {
        self.text_first(&[key], default)
    }

    /// The text under the first of `keys` that holds a string.
    fn text_first(&self, keys: &[&str], default: &str) -> String {
        keys.iter().find_map(|key| self.0.get(*key).and_then(Value::as_str)).unwrap_or(default).to_owned()
    }

    fn color(&self, key: &str, default: Rgba) -> Rgba {
        self.opt_color(key).unwrap_or(default)
    }

    fn opt_color(&self, key: &str) -> Option<Rgba> {
        self.0.get(key).and_then(color_of)
    }

    fn point(&self, key: &str, default: (i32, i32)) -> (i32, i32) {
        self.0.get(key).and_then(point_of).unwrap_or(default)
    }

    /// `ImageMargins`, written as `"left, top, right, bottom"`.
    fn margin(&self, key: &str, default: (i32, i32, i32, i32)) -> (i32, i32, i32, i32) {
        match self.0.get(key).and_then(Value::as_str).and_then(ints).as_deref() {
            Some([left, top, right, bottom]) => (*left, *top, *right, *bottom),
            _ => default,
        }
    }

    /// A uniform `Padding`, written as `"left, top, right, bottom"`. The left value is used.
    fn padding(&self, key: &str, default: u32) -> u32 {
        match self.0.get(key) {
            Some(Value::String(text)) => {
                ints(text).and_then(|values| values.first().copied()).map_or(default, |value| value.max(0).unsigned_abs())
            }
            Some(_) => self.uint(key, default),
            None => default,
        }
    }

    /// A `Size` written as `"width, height"`.
    fn size(&self, key: &str) -> (u32, u32) {
        let (width, height) = self.point(key, (0, 0));
        (width.max(0).unsigned_abs(), height.max(0).unsigned_abs())
    }

    /// `[Flags] ImageSides`, written as `"Top, Bottom, Left, Right"`, or `"0"` for none. Every side when absent.
    fn sides(&self, key: &str) -> Sides {
        match self.0.get(key).and_then(Value::as_str) {
            Some(text) => {
                let has = |name: &str| text.split(',').any(|part| part.trim().eq_ignore_ascii_case(name));
                Sides { top: has("Top"), right: has("Right"), bottom: has("Bottom"), left: has("Left") }
            }
            None => Sides { top: true, right: true, bottom: true, left: true },
        }
    }

    fn placement(&self, key: &str, default: Placement) -> Placement {
        self.0.get(key).and_then(Value::as_str).and_then(placement_of).unwrap_or(default)
    }

    fn border_kind(&self, key: &str) -> BorderKind {
        match self.0.get(key).and_then(Value::as_str) {
            Some(name) if name.eq_ignore_ascii_case("Inside") => BorderKind::Inside,
            _ => BorderKind::Outside,
        }
    }

    fn force_method(&self, key: &str) -> ForceMethod {
        match self.0.get(key).and_then(Value::as_str) {
            Some(name) if name.eq_ignore_ascii_case("Crop") => ForceMethod::Crop,
            _ => ForceMethod::Grow,
        }
    }

    /// `SizeMode` and `Size`. `PercentageOfCanvas` has no Laqta equivalent and keeps the overlay's size.
    fn size_mode(&self) -> SizeMode {
        let (width, height) = self.size("Size");
        match self.0.get("SizeMode").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
            Some("absolutesize" | "absolute") => SizeMode::Absolute(width, height),
            // `Percentage` holds the exact value Laqta wrote. ShareX has whole percentages in `Size`.
            Some("percentageofwatermark" | "percentage") => SizeMode::Percentage(self.float("Percentage", width as f32)),
            _ => SizeMode::DontResize,
        }
    }

    /// A font string and a colour. `default_size` is in points.
    fn text_spec(&self, font_key: &str, color_key: &str, default_size: f32) -> TextSpec {
        let font = self.0.get(font_key).and_then(Value::as_str).and_then(parse_font);
        let (font_family, size, bold, italic) = match font {
            Some(font) => (font.family, font.size.unwrap_or(default_size), font.bold, font.italic),
            None => ("Arial".to_owned(), default_size, false, false),
        };
        TextSpec { text: self.text("Text", ""), font_family, size, bold, italic, color: self.color(color_key, Rgba::rgb(235, 235, 235)) }
    }

    /// `UseGradient` and the `GradientInfo` under `Gradient`: its first and last stop, and the angle of its
    /// `Type`, or the exact `Angle` Laqta wrote.
    fn gradient(&self) -> Option<(Rgba, Rgba, f32)> {
        if !self.flag("UseGradient", false) {
            return None;
        }
        let empty = Object::new();
        let object = json::sub(self.0, "Gradient").unwrap_or(&empty);
        let mut stops: Vec<(f32, Rgba)> = json::list(object, "Colors")
            .iter()
            .filter_map(Value::as_object)
            .filter_map(|stop| {
                let stop = Fields(stop);
                Some((stop.float("Location", 0.0), stop.opt_color("Color")?))
            })
            .collect();
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        let start = stops.first().map_or(Rgba::BLACK, |stop| stop.1);
        let end = stops.last().map_or(Rgba::WHITE, |stop| stop.1);
        let fields = Fields(object);
        let type_angle = match fields.text("Type", "Vertical").to_ascii_lowercase().as_str() {
            "horizontal" => 0.0,
            "forwarddiagonal" => 45.0,
            "backwarddiagonal" => 135.0,
            _ => 90.0,
        };
        Some((start, end, fields.float("Angle", type_angle)))
    }

    /// The 20 `Rr`..`Ao` properties as the domain matrix. Missing ones are the identity.
    fn matrix(&self) -> [f32; 20] {
        let mut matrix = IDENTITY_MATRIX;
        for (output, out_name) in MATRIX_CHANNELS.iter().enumerate() {
            for (input, in_name) in MATRIX_CHANNELS.iter().enumerate() {
                let index = 4 * input + output;
                matrix[index] = self.float(&format!("{out_name}{}", in_name.to_ascii_lowercase()), matrix[index]);
            }
            matrix[16 + output] = self.float(&format!("{out_name}o"), 0.0);
        }
        matrix
    }

    /// The `X0Y0`..`X2Y2` properties, row by row from the top left.
    fn kernel(&self) -> [f32; 9] {
        let mut kernel = IDENTITY_KERNEL;
        for (index, cell) in kernel.iter_mut().enumerate() {
            *cell = self.float(&format!("X{}Y{}", index % 3, index / 3), *cell);
        }
        kernel
    }
}

/// An `ImageFont` or `Font` string: `"Arial, 11.25pt"`, and `"Arial, 11.25pt, style=Bold, Italic"` when styled.
struct ParsedFont {
    family: String,
    size: Option<f32>,
    bold: bool,
    italic: bool,
}

fn parse_font(text: &str) -> Option<ParsedFont> {
    let mut parts = text.split(',').map(str::trim);
    let family = parts.next().filter(|family| !family.is_empty())?.to_owned();
    let size = parts.next().and_then(|size| size.trim_end_matches(|c: char| c.is_ascii_alphabetic()).trim().parse().ok());
    let styles: Vec<&str> = parts.filter_map(|part| part.rsplit('=').next()).collect();
    let has = |name: &str| styles.iter().any(|style| style.eq_ignore_ascii_case(name));
    Some(ParsedFont { family, size, bold: has("Bold"), italic: has("Italic") })
}

/// The integers in `"a, b, c"`, or `None` when a part is not an integer.
fn ints(text: &str) -> Option<Vec<i32>> {
    text.split(',').map(|part| part.trim().parse().ok()).collect()
}

/// Writes effect properties. Mirrors `Fields`.
#[derive(Default)]
struct Put(Object);

impl Put {
    fn into_object(self) -> Object {
        self.0
    }

    fn set(&mut self, key: &str, value: Value) {
        self.0.insert(key.to_owned(), value);
    }

    /// A whole value is written as an integer, so it also loads into an `int` property of ShareX.
    fn float(&mut self, key: &str, value: f32) {
        self.set(key, number(f64::from(value)));
    }

    fn uint(&mut self, key: &str, value: u64) {
        self.set(key, Value::from(value));
    }

    fn int(&mut self, key: &str, value: i32) {
        self.set(key, Value::from(value));
    }

    fn flag(&mut self, key: &str, value: bool) {
        self.set(key, Value::Bool(value));
    }

    fn text(&mut self, key: &str, value: &str) {
        self.set(key, Value::from(value));
    }

    fn color(&mut self, key: &str, value: Rgba) {
        self.text(key, &color_text(value));
    }

    fn point(&mut self, key: &str, (x, y): (i32, i32)) {
        self.text(key, &format!("{x}, {y}"));
    }

    fn margin(&mut self, key: &str, (left, top, right, bottom): (i32, i32, i32, i32)) {
        self.text(key, &format!("{left}, {top}, {right}, {bottom}"));
    }

    /// A uniform `Padding`.
    fn padding(&mut self, key: &str, value: u32) {
        self.text(key, &format!("{value}, {value}, {value}, {value}"));
    }

    fn sides(&mut self, key: &str, sides: Sides) {
        let names: Vec<&str> = [(sides.top, "Top"), (sides.bottom, "Bottom"), (sides.left, "Left"), (sides.right, "Right")]
            .into_iter()
            .filter_map(|(set, name)| set.then_some(name))
            .collect();
        self.text(key, &if names.is_empty() { "0".to_owned() } else { names.join(", ") });
    }

    fn placement(&mut self, key: &str, value: Placement) {
        self.text(key, placement_name(value));
    }

    fn border_kind(&mut self, key: &str, value: BorderKind) {
        self.text(
            key,
            match value {
                BorderKind::Outside => "Outside",
                BorderKind::Inside => "Inside",
            },
        );
    }

    fn force_method(&mut self, key: &str, value: ForceMethod) {
        self.text(
            key,
            match value {
                ForceMethod::Grow => "Grow",
                ForceMethod::Crop => "Crop",
            },
        );
    }

    fn size_mode(&mut self, value: SizeMode) {
        let signed = |side: u32| i32::try_from(side).unwrap_or(i32::MAX);
        match value {
            SizeMode::DontResize => {
                self.text("SizeMode", "DontResize");
                self.point("Size", (0, 0));
            }
            SizeMode::Absolute(width, height) => {
                self.text("SizeMode", "AbsoluteSize");
                self.point("Size", (signed(width), signed(height)));
            }
            SizeMode::Percentage(percent) => {
                self.text("SizeMode", "PercentageOfWatermark");
                let whole = percent.round().clamp(0.0, i32::MAX as f32) as i32;
                self.point("Size", (whole, whole));
                self.float("Percentage", percent);
            }
        }
    }

    /// `"Arial, 11.25pt"`, with `", style=Bold, Italic"` when styled.
    fn font(&mut self, key: &str, spec: &TextSpec) {
        let styles: Vec<&str> =
            [(spec.bold, "Bold"), (spec.italic, "Italic")].into_iter().filter_map(|(set, name)| set.then_some(name)).collect();
        let style = if styles.is_empty() { String::new() } else { format!(", style={}", styles.join(", ")) };
        self.text(key, &format!("{}, {}pt{style}", spec.font_family, spec.size));
    }

    /// `UseGradient` and a `GradientInfo` with the two colours at 0 and 100. `Type` is the nearest
    /// `LinearGradientMode`, and `Angle` is Laqta's own, exact.
    fn gradient(&mut self, value: Option<(Rgba, Rgba, f32)>) {
        let (start, end, angle) = value.unwrap_or((Rgba::BLACK, Rgba::WHITE, 90.0));
        self.flag("UseGradient", value.is_some());
        let stops = [(start, 0.0), (end, 100.0)].map(|(color, location)| {
            let mut stop = Put::default();
            stop.color("Color", color);
            stop.set("Location", number(location));
            Value::Object(stop.into_object())
        });
        let mut gradient = Put::default();
        gradient.text("Type", gradient_type(angle));
        gradient.set("Colors", Value::Array(stops.into()));
        gradient.float("Angle", angle);
        self.set("Gradient", Value::Object(gradient.into_object()));
    }

    /// The 20 `Rr`..`Ao` properties.
    fn matrix(&mut self, matrix: &[f32; 20]) {
        for (output, out_name) in MATRIX_CHANNELS.iter().enumerate() {
            for (input, in_name) in MATRIX_CHANNELS.iter().enumerate() {
                self.float(&format!("{out_name}{}", in_name.to_ascii_lowercase()), matrix[4 * input + output]);
            }
            self.float(&format!("{out_name}o"), matrix[16 + output]);
        }
    }

    /// The `X0Y0`..`X2Y2` properties.
    fn kernel(&mut self, kernel: &[f32; 9]) {
        for (index, cell) in kernel.iter().enumerate() {
            self.float(&format!("X{}Y{}", index % 3, index / 3), *cell);
        }
    }
}

/// The `LinearGradientMode` closest to an angle in degrees (0 runs left to right, 90 top to bottom).
fn gradient_type(angle: f32) -> &'static str {
    let angle = angle.rem_euclid(360.0);
    let distance = |target: f32| {
        let apart = (angle - target).abs();
        apart.min(360.0 - apart)
    };
    [("Horizontal", 0.0), ("ForwardDiagonal", 45.0), ("Vertical", 90.0), ("BackwardDiagonal", 135.0)]
        .into_iter()
        .min_by(|a, b| distance(a.1).total_cmp(&distance(b.1)))
        .map_or("Vertical", |(name, _)| name)
}

/// A finite JSON number, an integer when the value is whole. Non-finite values cannot be written, so they
/// become null.
fn number(value: f64) -> Value {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        return Value::from(value as i64);
    }
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

/// How `System.Drawing.ColorConverter` writes a colour that has no name: `"R, G, B"`, or `"A, R, G, B"`
/// when it is not opaque.
fn color_text(color: Rgba) -> String {
    if color.a == 255 {
        format!("{}, {}, {}", color.r, color.g, color.b)
    } else {
        format!("{}, {}, {}, {}", color.a, color.r, color.g, color.b)
    }
}

/// A colour as text (see `parse_color`), or as an object with A, R, G and B.
fn color_of(value: &Value) -> Option<Rgba> {
    match value {
        Value::Object(object) => {
            let channel = |key: &str| json::number(object, key).map(|value| value.round().clamp(0.0, 255.0) as u8);
            Some(Rgba { r: channel("R")?, g: channel("G")?, b: channel("B")?, a: channel("A").unwrap_or(255) })
        }
        Value::String(text) => parse_color(text),
        _ => None,
    }
}

/// A .NET colour string: a known name (`"Lime"`), `"R, G, B"`, `"A, R, G, B"` (alpha first), or `#rrggbb`
/// and `#aarrggbb`.
fn parse_color(text: &str) -> Option<Rgba> {
    let text = text.trim();
    if text.starts_with('#') {
        return Rgba::parse_hex(text);
    }
    let channels: Option<Vec<u8>> = text.split(',').map(|part| part.trim().parse().ok()).collect();
    match channels {
        Some(channels) => match channels.as_slice() {
            [r, g, b] => Some(Rgba { r: *r, g: *g, b: *b, a: 255 }),
            [a, r, g, b] => Some(Rgba { r: *r, g: *g, b: *b, a: *a }),
            _ => None,
        },
        None => colors::by_name(text),
    }
}

/// A point as an object with X and Y, or as text `x, y`.
fn point_of(value: &Value) -> Option<(i32, i32)> {
    match value {
        Value::Object(object) => {
            let axis =
                |key: &str| json::number(object, key).map(|value| value.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32);
            Some((axis("X")?, axis("Y")?))
        }
        Value::String(text) => match ints(text)?.as_slice() {
            [x, y] => Some((*x, *y)),
            _ => None,
        },
        _ => None,
    }
}

fn placement_name(placement: Placement) -> &'static str {
    PLACEMENTS.iter().find(|(_, known)| *known == placement).map_or("MiddleCenter", |(name, _)| *name)
}

fn placement_of(name: &str) -> Option<Placement> {
    PLACEMENTS.iter().find(|(known, _)| known.eq_ignore_ascii_case(name)).map(|(_, placement)| *placement)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn object(json: &str) -> Object {
        serde_json::from_str::<Value>(json).unwrap().as_object().unwrap().clone()
    }

    #[test]
    fn placements_round_trip_by_name() {
        for (name, placement) in PLACEMENTS {
            assert_eq!(placement_name(placement), name);
            assert_eq!(placement_of(name), Some(placement));
        }
    }

    #[test]
    fn colour_text_forms() {
        assert_eq!(parse_color("#336699"), Some(Rgba::rgb(0x33, 0x66, 0x99)));
        assert_eq!(parse_color("255, 0, 10"), Some(Rgba::rgb(255, 0, 10)));
        assert_eq!(parse_color("125, 0, 0, 0"), Some(Rgba { r: 0, g: 0, b: 0, a: 125 }));
        assert_eq!(parse_color("Red"), Some(Rgba::rgb(255, 0, 0)));
        assert_eq!(parse_color("not a colour"), None);
        assert_eq!(parse_color("1, 2"), None);
    }

    #[test]
    fn colours_are_written_alpha_first_and_read_back() {
        for color in [Rgba::rgb(235, 235, 235), Rgba { r: 1, g: 2, b: 3, a: 4 }, Rgba::TRANSPARENT, Rgba::WHITE] {
            assert_eq!(parse_color(&color_text(color)), Some(color));
        }
        assert_eq!(color_text(Rgba { r: 10, g: 20, b: 30, a: 128 }), "128, 10, 20, 30");
    }

    #[test]
    fn contrast_is_a_plain_multiply_in_and_a_scale_factor_out() {
        let Some(Effect::Adjust(Adjust::MatrixColor { matrix })) = parse("Contrast", &object(r#"{ "Value": 1.5 }"#)) else {
            panic!("Contrast should import as a colour matrix");
        };
        assert_eq!(matrix, [1.5, 0.0, 0.0, 0.0, 0.0, 1.5, 0.0, 0.0, 0.0, 0.0, 1.5, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0]);
        let (class, written) = write(&Effect::Adjust(Adjust::Contrast { value: 50.0 })).unwrap();
        assert_eq!(class, "Contrast");
        assert_eq!(written.get("Value").and_then(Value::as_f64), Some(1.5));
    }

    #[test]
    fn fonts_and_margins_use_the_dotnet_text_forms() {
        let spec = TextSpec { text: "t".into(), font_family: "Arial".into(), size: 11.25, bold: true, italic: true, color: Rgba::WHITE };
        let mut put = Put::default();
        put.font("Font", &spec);
        put.margin("Margin", (0, 0, 0, 30));
        let written = put.into_object();
        assert_eq!(written["Font"], "Arial, 11.25pt, style=Bold, Italic");
        assert_eq!(written["Margin"], "0, 0, 0, 30");
        let font = parse_font("Segoe UI, 9pt").unwrap();
        assert_eq!((font.family.as_str(), font.size, font.bold, font.italic), ("Segoe UI", Some(9.0), false, false));
    }

    /// All effects that have a ShareX class, at their defaults.
    fn effects_with_a_class() -> Vec<Effect> {
        let mut effects = Vec::new();
        effects.extend(Adjust::defaults().into_iter().map(Effect::Adjust));
        effects.extend(Draw::defaults().into_iter().map(Effect::Draw));
        effects.extend(Filter::defaults().into_iter().map(Effect::Filter));
        effects.extend(Manip::defaults().into_iter().map(Effect::Manip));
        effects.retain(|effect| write(effect).is_some());
        effects
    }

    #[test]
    fn every_written_class_is_one_of_the_51_sharex_classes() {
        const SHAREX: [&str; 51] = [
            "Alpha",
            "BlackWhite",
            "Brightness",
            "Colorize",
            "Contrast",
            "Gamma",
            "Grayscale",
            "Hue",
            "Inverse",
            "MatrixColor",
            "Polaroid",
            "ReplaceColor",
            "Saturation",
            "SelectiveColor",
            "Sepia",
            "DrawBackground",
            "DrawBackgroundImage",
            "DrawBorder",
            "DrawCheckerboard",
            "DrawImage",
            "DrawParticles",
            "DrawText",
            "DrawTextEx",
            "Blur",
            "ColorDepth",
            "EdgeDetect",
            "Emboss",
            "GaussianBlur",
            "Glow",
            "MatrixConvolution",
            "MeanRemoval",
            "Outline",
            "Pixelate",
            "Reflection",
            "RGBSplit",
            "Shadow",
            "Sharpen",
            "Slice",
            "Smooth",
            "TornEdge",
            "WaveEdge",
            "AutoCrop",
            "Canvas",
            "Crop",
            "Flip",
            "ForceProportions",
            "Resize",
            "Rotate",
            "RoundedCorners",
            "Scale",
            "Skew",
        ];
        let written: Vec<&str> = effects_with_a_class().iter().filter_map(|effect| write(effect).map(|(class, _)| class)).collect();
        let mut written = written;
        written.sort_unstable();
        let mut expected = SHAREX;
        expected.sort_unstable();
        assert_eq!(written, expected);
    }

    #[test]
    fn every_effect_with_a_class_reads_back_as_written() {
        for effect in effects_with_a_class() {
            let (class, object) = write(&effect).unwrap();
            assert_eq!(parse(class, &object), Some(effect), "{class}: {object:?}");
        }
    }

    /// ShareX's colour amounts (`SkiaColorMatrixManager`) are fractions, and its Saturation is 1 at
    /// no change; Laqta's are percent with 0 at no change.
    /// ShareX `MatrixColor.Apply`: row g, column R of the matrix is `Rg`, so `Rg` feeds input G into
    /// output R.
    #[test]
    fn matrix_color_names_are_output_then_input() {
        let Some(Effect::Adjust(Adjust::MatrixColor { matrix })) = parse("MatrixColor", &object(r#"{ "Rg": 0.5, "Bo": 0.25 }"#)) else {
            panic!("MatrixColor should import");
        };
        assert_eq!(matrix[4], 0.5, "input G (row 1) to output R (column 0): {matrix:?}");
        assert_eq!(matrix[1], 0.0);
        assert_eq!(matrix[18], 0.25, "the offset of output B");
    }

    #[test]
    fn sharex_colour_amounts_are_converted_to_percent() {
        let read = |class: &str, json: &str| parse(class, &object(json)).unwrap();
        assert_eq!(read("Brightness", r#"{ "Value": 0.5 }"#), Effect::Adjust(Adjust::Brightness { value: 50.0 }));
        assert_eq!(read("Saturation", r#"{ "Value": 1.5 }"#), Effect::Adjust(Adjust::Saturation { value: 50.0 }));
        assert_eq!(read("Saturation", "{}"), Effect::Adjust(Adjust::Saturation { value: 0.0 }));
        assert!(matches!(read("Colorize", r#"{ "Value": 0.25 }"#), Effect::Adjust(Adjust::Colorize { value, .. }) if value == 25.0));
        assert_eq!(read("Grayscale", r#"{ "Value": 1 }"#), Effect::Adjust(Adjust::Grayscale { value: 100.0 }));
        assert_eq!(read("Sepia", "{}"), Effect::Adjust(Adjust::Sepia { value: 100.0 }));
        assert_eq!(read("GaussianBlur", r#"{ "Radius": 15 }"#), Effect::Filter(Filter::GaussianBlur { sigma: 5.0 }));
        let Effect::Adjust(Adjust::MatrixColor { matrix }) = read("Grayscale", r#"{ "Value": 0.5 }"#) else {
            panic!("a scaled ShareX Grayscale keeps its look as a matrix");
        };
        assert!((matrix[0] - 0.106_335_5).abs() < 1e-6 && (matrix[4] - 0.357_58).abs() < 1e-6 && matrix[15] == 1.0, "{matrix:?}");
        let (_, written) = write(&Effect::Adjust(Adjust::Saturation { value: -100.0 })).unwrap();
        assert_eq!(json::number(&written, "Value"), Some(0.0), "Laqta's -100 % is ShareX's 0");
    }
}
