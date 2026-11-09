//! Effects in ShareX `ImageEffectsLib` `.sxie` packages, mapped to Laqta's effect enums. Each arm names a
//! ShareX class and its JSON properties, as Json.NET writes them. `$type`, `Enabled` and the simple `Value`
//! properties follow the ShareX format. The other property names are not yet checked against a real `.sxie`
//! file, so they are kept together here to be corrected in one place.
//!
//! `parse` and `write` are mirror images: a name read by one is written by the other.

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

const DEFAULT_KERNEL: [f32; 9] = [0.0, -1.0, 0.0, -1.0, 5.0, -1.0, 0.0, -1.0, 0.0];

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
        "alpha" => Effect::Adjust(Adjust::Alpha { alpha: f.percent("Value", 100.0), add: f.percent("Add", 0.0) }),
        "blackwhite" => Effect::Adjust(Adjust::BlackWhite),
        "brightness" => Effect::Adjust(Adjust::Brightness { value: f.float("Value", 0.0) }),
        "colorize" => Effect::Adjust(Adjust::Colorize { color: f.color("Color", Rgba::rgb(255, 0, 0)), value: f.float("Value", 50.0) }),
        "contrast" => Effect::Adjust(Adjust::Contrast { value: f.float("Value", 0.0) }),
        "gamma" => Effect::Adjust(Adjust::Gamma { value: f.float("Value", 1.0) }),
        "grayscale" => Effect::Adjust(Adjust::Grayscale { value: f.float("Value", 100.0) }),
        "hue" => Effect::Adjust(Adjust::Hue { angle: f.float_first(&["Angle", "Value"], 0.0) }),
        "inverse" => Effect::Adjust(Adjust::Inverse),
        "matrixcolor" => Effect::Adjust(Adjust::MatrixColor { matrix: f.matrix("ColorMatrix") }),
        "polaroid" => Effect::Adjust(Adjust::Polaroid),
        "saturation" => Effect::Adjust(Adjust::Saturation { value: f.float("Value", 0.0) }),
        "selectivecolor" => Effect::Adjust(Adjust::SelectiveColor {
            from: f.color("Source", Rgba::rgb(255, 0, 0)),
            to: f.color("Target", Rgba::rgb(0, 0, 255)),
            tolerance: f.byte("Tolerance", 32),
        }),
        "sepia" => Effect::Adjust(Adjust::Sepia { value: f.float("Value", 100.0) }),
        "replacecolor" => Effect::Adjust(Adjust::ReplaceColor {
            source: f.color("Source", Rgba::WHITE),
            target: f.color("Target", Rgba::BLACK),
            tolerance: f.byte("Tolerance", 20),
            auto_source_from_corner: f.flag("AutoSourceFromCorner", false),
        }),

        "background" => Effect::Draw(Draw::Background { color: f.color("Color", Rgba::BLACK), gradient: f.gradient("Gradient") }),
        "backgroundimage" => {
            Effect::Draw(Draw::BackgroundImage { path: f.text("Path", ""), center: f.flag("Center", false), tile: f.flag("Tile", false) })
        }
        "border" => Effect::Draw(Draw::Border {
            kind: f.border_kind("Kind"),
            size: f.uint("Size", 10),
            color: f.color("Color", Rgba::BLACK),
            dash: f.flag("Dash", false),
        }),
        "checkerboard" => Effect::Draw(Draw::Checkerboard {
            size: f.uint("Size", 10),
            color1: f.color("Color1", Rgba::WHITE),
            color2: f.color("Color2", Rgba::rgb(0xcc, 0xcc, 0xcc)),
        }),
        "drawimage" => Effect::Draw(Draw::Image {
            path: f.text("Path", ""),
            placement: f.placement("Placement", Placement::MiddleCenter),
            offset: f.point("Offset", (0, 0)),
            size_mode: f.size_mode(),
            opacity: f.float("Opacity", 1.0),
        }),
        "particles" => Effect::Draw(Draw::Particles {
            path: f.text("Path", ""),
            count: f.uint("Count", 20),
            min_size: f.uint("MinSize", 8),
            max_size: f.uint("MaxSize", 32),
            random_angle: f.flag("RandomAngle", true),
            seed: f.seed("Seed", 1),
        }),
        "drawtext" => Effect::Draw(Draw::Text {
            text: f.text_spec(),
            placement: f.placement("Placement", Placement::BottomRight),
            offset: f.point("Offset", (0, 0)),
            background: f.opt_color("Background"),
            padding: f.uint("Padding", 0),
            corner_radius: f.uint("CornerRadius", 0),
        }),
        "drawtextex" => Effect::Draw(Draw::TextEx {
            text: f.text_spec(),
            angle: f.float("Angle", 0.0),
            placement: f.placement("Placement", Placement::MiddleCenter),
            offset: f.point("Offset", (0, 0)),
            outline: f.outline("Outline"),
            shadow: f.shadow("Shadow"),
        }),

        "blur" => Effect::Filter(Filter::Blur { radius: f.uint("Radius", 5) }),
        "colordepth" => Effect::Filter(Filter::ColorDepth { bits: f.byte("Depth", 4) }),
        "convolutionmatrix" => Effect::Filter(Filter::ConvolutionMatrix {
            kernel: f.kernel("Kernel"),
            divisor: f.float("Divisor", 1.0),
            offset: f.float("Offset", 0.0),
        }),
        "edgedetect" => Effect::Filter(Filter::EdgeDetect),
        "emboss" => Effect::Filter(Filter::Emboss),
        "gaussianblur" => Effect::Filter(Filter::GaussianBlur { sigma: f.float("Sigma", 2.0) }),
        "glow" => Effect::Filter(Filter::Glow {
            size: f.uint("Size", 10),
            strength: f.float("Strength", 2.0),
            color: f.color("Color", Rgba::WHITE),
            offset_x: f.int("OffsetX", 0),
            offset_y: f.int("OffsetY", 0),
        }),
        "meanremoval" => Effect::Filter(Filter::MeanRemoval),
        "outline" => Effect::Filter(Filter::Outline {
            size: f.uint("Size", 5),
            padding: f.uint("Padding", 10),
            color: f.color("Color", Rgba::BLACK),
            outline_only: f.flag("OutlineOnly", false),
        }),
        "pixelate" => Effect::Filter(Filter::Pixelate { size: f.uint("Size", 10) }),
        "reflection" => Effect::Filter(Filter::Reflection {
            percentage: f.byte("Percentage", 50),
            max_alpha: f.byte("MaxAlpha", 150),
            min_alpha: f.byte("MinAlpha", 0),
            offset: f.uint("Offset", 0),
            skew: f.flag("Skew", false),
            skew_size: f.uint("SkewSize", 0),
        }),
        "rgbsplit" => Effect::Filter(Filter::RgbSplit {
            offset_red: f.point("RedOffset", (-4, 0)),
            offset_green: f.point("GreenOffset", (0, 0)),
            offset_blue: f.point("BlueOffset", (4, 0)),
        }),
        "shadow" => Effect::Filter(Filter::Shadow {
            opacity: f.float("Opacity", 0.8),
            size: f.uint("Size", 10),
            darkness: f.float("Darkness", 0.0),
            color: f.color("Color", Rgba::BLACK),
            offset_x: f.int("OffsetX", 10),
            offset_y: f.int("OffsetY", 10),
            auto_resize: f.flag("AutoResize", true),
        }),
        "sharpen" => Effect::Filter(Filter::Sharpen),
        "slice" => Effect::Filter(Filter::Slice {
            min_height: f.uint("MinHeight", 2),
            max_height: f.uint("MaxHeight", 10),
            min_shift: f.uint("MinShift", 2),
            max_shift: f.uint("MaxShift", 12),
            seed: f.seed("Seed", 1),
        }),
        "smooth" => Effect::Filter(Filter::Smooth),
        "tornedge" => Effect::Filter(Filter::TornEdge {
            depth: f.uint("Depth", 8),
            range: f.uint("Range", 12),
            top: f.flag("Top", true),
            right: f.flag("Right", true),
            bottom: f.flag("Bottom", true),
            left: f.flag("Left", true),
            curved: f.flag("Curved", false),
            seed: f.seed("Seed", 1),
        }),
        "waveedge" => Effect::Filter(Filter::WaveEdge {
            depth: f.uint("Depth", 6),
            range: f.uint("Range", 16),
            top: f.flag("Top", true),
            right: f.flag("Right", false),
            bottom: f.flag("Bottom", true),
            left: f.flag("Left", false),
        }),
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

        "autocrop" => Effect::Manip(Manip::AutoCrop {
            color: f.color("Color", Rgba::TRANSPARENT),
            tolerance: f.byte("Tolerance", 0),
            margin: f.uint("Margin", 0),
        }),
        "canvas" => Effect::Manip(Manip::Canvas {
            margin: (f.int("Left", 0), f.int("Top", 0), f.int("Right", 0), f.int("Bottom", 0)),
            color: f.color("Color", Rgba::TRANSPARENT),
        }),
        "crop" => Effect::Manip(Manip::Crop { margin: (f.uint("Left", 0), f.uint("Top", 0), f.uint("Right", 0), f.uint("Bottom", 0)) }),
        "flip" => Effect::Manip(Manip::Flip { horizontal: f.flag("Horizontal", false), vertical: f.flag("Vertical", false) }),
        "forceproportions" => Effect::Manip(Manip::ForceProportions {
            width: f.uint("Width", 16),
            height: f.uint("Height", 9),
            method: f.force_method("Method"),
            color: f.color("Color", Rgba::TRANSPARENT),
        }),
        "resize" => {
            Effect::Manip(Manip::Resize { width: f.uint("Width", 0), height: f.uint("Height", 0), keep_aspect: f.flag("KeepAspect", true) })
        }
        "rotate" => Effect::Manip(Manip::Rotate {
            angle: f.float("Angle", 0.0),
            grow: f.flag("Grow", true),
            color: f.color("Color", Rgba::TRANSPARENT),
        }),
        "roundedcorners" => Effect::Manip(Manip::RoundedCorners { radius: f.uint("Radius", 10) }),
        "scale" => {
            Effect::Manip(Manip::Scale { width_pct: f.float("WidthPercentage", 100.0), height_pct: f.float("HeightPercentage", 100.0) })
        }
        "skew" => Effect::Manip(Manip::Skew { x: f.int("X", 0), y: f.int("Y", 0), grow: f.flag("Grow", true) }),
        _ => return None,
    };
    Some(effect)
}

/// The ShareX class name and its properties for an effect. `None` for the effects that have no confirmed
/// ShareX class: they are read, but never written under a guessed name.
pub(crate) fn write(effect: &Effect) -> Option<(&'static str, Object)> {
    let mut p = Put::default();
    let class = match effect {
        Effect::Adjust(adjust) => match adjust {
            Adjust::Alpha { alpha, add } => {
                p.percent("Value", *alpha);
                p.percent("Add", *add);
                "Alpha"
            }
            Adjust::BlackWhite => "BlackWhite",
            Adjust::Brightness { value } => {
                p.float("Value", *value);
                "Brightness"
            }
            Adjust::Colorize { color, value } => {
                p.color("Color", *color);
                p.float("Value", *value);
                "Colorize"
            }
            Adjust::Contrast { value } => {
                p.float("Value", *value);
                "Contrast"
            }
            Adjust::Gamma { value } => {
                p.float("Value", *value);
                "Gamma"
            }
            Adjust::Grayscale { value } => {
                p.float("Value", *value);
                "Grayscale"
            }
            Adjust::Hue { angle } => {
                p.float("Angle", *angle);
                "Hue"
            }
            Adjust::Inverse => "Inverse",
            Adjust::MatrixColor { matrix } => {
                p.matrix("ColorMatrix", matrix);
                "MatrixColor"
            }
            Adjust::Polaroid => "Polaroid",
            Adjust::Saturation { value } => {
                p.float("Value", *value);
                "Saturation"
            }
            Adjust::SelectiveColor { from, to, tolerance } => {
                p.color("Source", *from);
                p.color("Target", *to);
                p.uint("Tolerance", u64::from(*tolerance));
                "SelectiveColor"
            }
            Adjust::Sepia { value } => {
                p.float("Value", *value);
                "Sepia"
            }
            Adjust::ReplaceColor { source, target, tolerance, auto_source_from_corner } => {
                p.color("Source", *source);
                p.color("Target", *target);
                p.uint("Tolerance", u64::from(*tolerance));
                p.flag("AutoSourceFromCorner", *auto_source_from_corner);
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
                p.gradient("Gradient", *gradient);
                "Background"
            }
            Draw::BackgroundImage { path, center, tile } => {
                p.text("Path", path);
                p.flag("Center", *center);
                p.flag("Tile", *tile);
                "BackgroundImage"
            }
            Draw::Border { kind, size, color, dash } => {
                p.border_kind("Kind", *kind);
                p.uint("Size", u64::from(*size));
                p.color("Color", *color);
                p.flag("Dash", *dash);
                "Border"
            }
            Draw::Checkerboard { size, color1, color2 } => {
                p.uint("Size", u64::from(*size));
                p.color("Color1", *color1);
                p.color("Color2", *color2);
                "Checkerboard"
            }
            Draw::Image { path, placement, offset, size_mode, opacity } => {
                p.text("Path", path);
                p.placement("Placement", *placement);
                p.point("Offset", *offset);
                p.size_mode(*size_mode);
                p.float("Opacity", *opacity);
                "DrawImage"
            }
            Draw::Particles { path, count, min_size, max_size, random_angle, seed } => {
                p.text("Path", path);
                p.uint("Count", u64::from(*count));
                p.uint("MinSize", u64::from(*min_size));
                p.uint("MaxSize", u64::from(*max_size));
                p.flag("RandomAngle", *random_angle);
                p.uint("Seed", *seed);
                "Particles"
            }
            Draw::Text { text, placement, offset, background, padding, corner_radius } => {
                p.text_spec(text);
                p.placement("Placement", *placement);
                p.point("Offset", *offset);
                p.opt_color("Background", *background);
                p.uint("Padding", u64::from(*padding));
                p.uint("CornerRadius", u64::from(*corner_radius));
                "DrawText"
            }
            Draw::TextEx { text, angle, placement, offset, outline, shadow } => {
                p.text_spec(text);
                p.float("Angle", *angle);
                p.placement("Placement", *placement);
                p.point("Offset", *offset);
                p.outline("Outline", *outline);
                p.shadow("Shadow", *shadow);
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
                p.uint("Depth", u64::from(*bits));
                "ColorDepth"
            }
            Filter::ConvolutionMatrix { kernel, divisor, offset } => {
                p.kernel("Kernel", kernel);
                p.float("Divisor", *divisor);
                p.float("Offset", *offset);
                "ConvolutionMatrix"
            }
            Filter::EdgeDetect => "EdgeDetect",
            Filter::Emboss => "Emboss",
            Filter::GaussianBlur { sigma } => {
                p.float("Sigma", *sigma);
                "GaussianBlur"
            }
            Filter::Glow { size, strength, color, offset_x, offset_y } => {
                p.uint("Size", u64::from(*size));
                p.float("Strength", *strength);
                p.color("Color", *color);
                p.int("OffsetX", *offset_x);
                p.int("OffsetY", *offset_y);
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
                p.point("RedOffset", *offset_red);
                p.point("GreenOffset", *offset_green);
                p.point("BlueOffset", *offset_blue);
                "RGBSplit"
            }
            Filter::Shadow { opacity, size, darkness, color, offset_x, offset_y, auto_resize } => {
                p.float("Opacity", *opacity);
                p.uint("Size", u64::from(*size));
                p.float("Darkness", *darkness);
                p.color("Color", *color);
                p.int("OffsetX", *offset_x);
                p.int("OffsetY", *offset_y);
                p.flag("AutoResize", *auto_resize);
                "Shadow"
            }
            Filter::Sharpen => "Sharpen",
            Filter::Slice { min_height, max_height, min_shift, max_shift, seed } => {
                p.uint("MinHeight", u64::from(*min_height));
                p.uint("MaxHeight", u64::from(*max_height));
                p.uint("MinShift", u64::from(*min_shift));
                p.uint("MaxShift", u64::from(*max_shift));
                p.uint("Seed", *seed);
                "Slice"
            }
            Filter::Smooth => "Smooth",
            Filter::TornEdge { depth, range, top, right, bottom, left, curved, seed } => {
                p.uint("Depth", u64::from(*depth));
                p.uint("Range", u64::from(*range));
                p.flag("Top", *top);
                p.flag("Right", *right);
                p.flag("Bottom", *bottom);
                p.flag("Left", *left);
                p.flag("Curved", *curved);
                p.uint("Seed", *seed);
                "TornEdge"
            }
            Filter::WaveEdge { depth, range, top, right, bottom, left } => {
                p.uint("Depth", u64::from(*depth));
                p.uint("Range", u64::from(*range));
                p.flag("Top", *top);
                p.flag("Right", *right);
                p.flag("Bottom", *bottom);
                p.flag("Left", *left);
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
                p.color("Color", *color);
                p.uint("Tolerance", u64::from(*tolerance));
                p.uint("Margin", u64::from(*margin));
                "AutoCrop"
            }
            Manip::Canvas { margin, color } => {
                p.int("Left", margin.0);
                p.int("Top", margin.1);
                p.int("Right", margin.2);
                p.int("Bottom", margin.3);
                p.color("Color", *color);
                "Canvas"
            }
            Manip::Crop { margin } => {
                p.uint("Left", u64::from(margin.0));
                p.uint("Top", u64::from(margin.1));
                p.uint("Right", u64::from(margin.2));
                p.uint("Bottom", u64::from(margin.3));
                "Crop"
            }
            Manip::Flip { horizontal, vertical } => {
                p.flag("Horizontal", *horizontal);
                p.flag("Vertical", *vertical);
                "Flip"
            }
            Manip::ForceProportions { width, height, method, color } => {
                p.uint("Width", u64::from(*width));
                p.uint("Height", u64::from(*height));
                p.force_method("Method", *method);
                p.color("Color", *color);
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
                p.flag("Grow", *grow);
                p.color("Color", *color);
                "Rotate"
            }
            Manip::RoundedCorners { radius } => {
                p.uint("Radius", u64::from(*radius));
                "RoundedCorners"
            }
            Manip::Scale { width_pct, height_pct } => {
                p.float("WidthPercentage", *width_pct);
                p.float("HeightPercentage", *height_pct);
                "Scale"
            }
            Manip::Skew { x, y, grow } => {
                p.int("X", *x);
                p.int("Y", *y);
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

/// Typed reads from one ShareX effect object. Every read falls back to the default when the property is
/// absent or not of the right type.
struct Fields<'a>(&'a Object);

impl Fields<'_> {
    fn float(&self, key: &str, default: f32) -> f32 {
        json::number(self.0, key).map_or(default, |value| value as f32)
    }

    /// A percentage, read as a fraction of one.
    fn percent(&self, key: &str, default_percent: f64) -> f32 {
        (json::number(self.0, key).unwrap_or(default_percent) / 100.0) as f32
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
        self.0.get(key).and_then(Value::as_str).unwrap_or(default).to_owned()
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

    fn size_mode(&self) -> SizeMode {
        match self.0.get("SizeMode").and_then(Value::as_str) {
            Some(name) if name.eq_ignore_ascii_case("Absolute") => SizeMode::Absolute(self.uint("Width", 0), self.uint("Height", 0)),
            Some(name) if name.eq_ignore_ascii_case("Percentage") => SizeMode::Percentage(self.float("Percentage", 100.0)),
            _ => SizeMode::DontResize,
        }
    }

    fn text_spec(&self) -> TextSpec {
        TextSpec {
            text: self.text("Text", ""),
            font_family: self.text("FontFamily", "sans-serif"),
            size: self.float("FontSize", 18.0),
            bold: self.flag("Bold", false),
            italic: self.flag("Italic", false),
            color: self.color("TextColor", Rgba::WHITE),
        }
    }

    fn gradient(&self, key: &str) -> Option<(Rgba, Rgba, f32)> {
        json::sub(self.0, key).map(|object| {
            let object = Fields(object);
            (object.color("Start", Rgba::BLACK), object.color("End", Rgba::WHITE), object.float("Angle", 0.0))
        })
    }

    fn outline(&self, key: &str) -> Option<(Rgba, f32)> {
        json::sub(self.0, key).map(|object| {
            let object = Fields(object);
            (object.color("Color", Rgba::BLACK), object.float("Width", 1.0))
        })
    }

    fn shadow(&self, key: &str) -> Option<(Rgba, i32, i32)> {
        json::sub(self.0, key).map(|object| {
            let object = Fields(object);
            (object.color("Color", Rgba::BLACK), object.int("X", 0), object.int("Y", 0))
        })
    }

    /// Every number under `key`, in document order, however the arrays are nested.
    fn numbers(&self, key: &str) -> Vec<f32> {
        let mut out = Vec::new();
        if let Some(value) = self.0.get(key) {
            flatten(value, &mut out);
        }
        out
    }

    /// Rows of five (the input channels and the constant, then four outputs and an unused column), or the
    /// same 25 numbers flat. Anything else is the identity.
    fn matrix(&self, key: &str) -> [f32; 20] {
        let flat = self.numbers(key);
        if flat.len() != 25 {
            return IDENTITY_MATRIX;
        }
        let mut matrix = [0.0; 20];
        let (rows, _) = flat.as_chunks::<5>();
        for (row_index, row) in rows.iter().enumerate() {
            for (column, value) in row.iter().take(4).enumerate() {
                matrix[4 * row_index + column] = *value;
            }
        }
        matrix
    }

    fn kernel(&self, key: &str) -> [f32; 9] {
        let flat = self.numbers(key);
        <[f32; 9]>::try_from(flat.as_slice()).unwrap_or(DEFAULT_KERNEL)
    }
}

fn flatten(value: &Value, out: &mut Vec<f32>) {
    match value {
        Value::Number(number) => out.extend(number.as_f64().map(|number| number as f32)),
        Value::Array(items) => {
            for item in items {
                flatten(item, out);
            }
        }
        _ => {}
    }
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

    fn float(&mut self, key: &str, value: f32) {
        self.set(key, number(f64::from(value)));
    }

    /// A fraction written as a percentage.
    fn percent(&mut self, key: &str, value: f32) {
        self.set(key, number(f64::from(value) * 100.0));
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
        self.set(key, color_value(value));
    }

    fn opt_color(&mut self, key: &str, value: Option<Rgba>) {
        self.set(key, value.map_or(Value::Null, color_value));
    }

    fn point(&mut self, key: &str, value: (i32, i32)) {
        self.set(key, point_value(value));
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
        match value {
            SizeMode::DontResize => self.text("SizeMode", "DontResize"),
            SizeMode::Absolute(width, height) => {
                self.text("SizeMode", "Absolute");
                self.uint("Width", u64::from(width));
                self.uint("Height", u64::from(height));
            }
            SizeMode::Percentage(percent) => {
                self.text("SizeMode", "Percentage");
                self.float("Percentage", percent);
            }
        }
    }

    fn text_spec(&mut self, spec: &TextSpec) {
        self.text("Text", &spec.text);
        self.text("FontFamily", &spec.font_family);
        self.float("FontSize", spec.size);
        self.flag("Bold", spec.bold);
        self.flag("Italic", spec.italic);
        self.color("TextColor", spec.color);
    }

    fn gradient(&mut self, key: &str, value: Option<(Rgba, Rgba, f32)>) {
        let value = value.map_or(Value::Null, |(start, end, angle)| {
            let mut object = Put::default();
            object.color("Start", start);
            object.color("End", end);
            object.float("Angle", angle);
            Value::Object(object.into_object())
        });
        self.set(key, value);
    }

    fn outline(&mut self, key: &str, value: Option<(Rgba, f32)>) {
        let value = value.map_or(Value::Null, |(color, width)| {
            let mut object = Put::default();
            object.color("Color", color);
            object.float("Width", width);
            Value::Object(object.into_object())
        });
        self.set(key, value);
    }

    fn shadow(&mut self, key: &str, value: Option<(Rgba, i32, i32)>) {
        let value = value.map_or(Value::Null, |(color, x, y)| {
            let mut object = Put::default();
            object.color("Color", color);
            object.int("X", x);
            object.int("Y", y);
            Value::Object(object.into_object())
        });
        self.set(key, value);
    }

    /// Five rows of five numbers. The fifth output column is not part of the domain layout, so it is
    /// written as the identity's value for the constant row and zero elsewhere.
    fn matrix(&mut self, key: &str, matrix: &[f32; 20]) {
        let rows: Vec<Value> = (0..5)
            .map(|row| {
                Value::Array(
                    (0..5)
                        .map(|column| {
                            let value = match column {
                                0..=3 => matrix[4 * row + column],
                                _ if row == 4 => 1.0,
                                _ => 0.0,
                            };
                            number(f64::from(value))
                        })
                        .collect(),
                )
            })
            .collect();
        self.set(key, Value::Array(rows));
    }

    fn kernel(&mut self, key: &str, kernel: &[f32; 9]) {
        self.set(key, Value::Array(kernel.iter().map(|value| number(f64::from(*value))).collect()));
    }
}

/// A finite JSON number. Non-finite values cannot be written, so they become null.
fn number(value: f64) -> Value {
    serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number)
}

fn color_value(color: Rgba) -> Value {
    let mut object = Object::new();
    object.insert("A".to_owned(), Value::from(u64::from(color.a)));
    object.insert("R".to_owned(), Value::from(u64::from(color.r)));
    object.insert("G".to_owned(), Value::from(u64::from(color.g)));
    object.insert("B".to_owned(), Value::from(u64::from(color.b)));
    Value::Object(object)
}

fn point_value((x, y): (i32, i32)) -> Value {
    let mut object = Object::new();
    object.insert("X".to_owned(), Value::from(x));
    object.insert("Y".to_owned(), Value::from(y));
    Value::Object(object)
}

/// A colour as an object with A, R, G and B, or as text: `#rrggbb`, `#aarrggbb` or `r, g, b[, a]`.
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

fn parse_color(text: &str) -> Option<Rgba> {
    let text = text.trim();
    if text.starts_with('#') {
        return Rgba::parse_hex(text);
    }
    let channels: Option<Vec<u8>> = text.split(',').map(|part| part.trim().parse().ok()).collect();
    match channels?.as_slice() {
        [r, g, b] => Some(Rgba { r: *r, g: *g, b: *b, a: 255 }),
        [r, g, b, a] => Some(Rgba { r: *r, g: *g, b: *b, a: *a }),
        _ => None,
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
        Value::String(text) => {
            let parts: Option<Vec<i32>> = text.split(',').map(|part| part.trim().parse().ok()).collect();
            match parts?.as_slice() {
                [x, y] => Some((*x, *y)),
                _ => None,
            }
        }
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
        assert_eq!(parse_color("Red"), None);
    }
}
