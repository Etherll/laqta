//! Checks for the `.sxie` / `ImageEffectPreset` JSON that ShareX writes. The format facts below
//! describe the `ImageEffectsLib` classes and the serializer settings that write them:
//! - The effects window serializes with `JsonHelpers.SerializeToString(preset,
//!   serializationBinder: new ImageEffectsSerializationBinder())`: indented, `StringEnumConverter`,
//!   `DefaultValueHandling.Include` (every property is written).
//! - `KnownTypesSerializationBinder.BindToName` writes `$type` as the bare class name
//!   (`assemblyName = null; typeName = serializedType.Name`), and `BindToType` resolves only
//!   `t.Name == typeName`.
//! - `System.Drawing.Color`, `Point`, `Size`, `ImageMargins` and `ImageFont` have TypeConverters, so
//!   Json.NET writes them as strings: `"Red"`, `"235, 235, 235"`, `"125, 0, 0, 0"` (A first),
//!   `"5, 5"`, `"0, 0, 0, 30"`, `"Arial, 11.25pt"`. `[Flags] ImageSides` is `"Top, Bottom, Left, Right"`.
//! - Property names and defaults are the `[DefaultValue]` attributes of each class (51 classes).
//!
//! The `audit_bug_*` tests here fail until the reader and writer match this format.
#![allow(clippy::unwrap_used)]

use std::io::{Cursor, Read, Write};

use laqta_domain::color::Rgba;
use laqta_domain::effects::{Adjust, Effect, EffectItem, Manip, Preset};
use laqta_import::{parse_sxie_with_warnings, write_sxie};
use zip::write::SimpleFileOptions;

/// Every effect class in ShareX.ImageEffectsLib (Adjustments 15, Drawings 8, Filters 18,
/// Manipulations 10).
const SHAREX_CLASSES: [&str; 51] = [
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

const GRADIENT: &str =
    r#"{ "Type": "Vertical", "Colors": [ { "Color": "Black", "Location": 0.0 }, { "Color": "White", "Location": 100.0 } ] }"#;

/// All 51 classes at their C# `[DefaultValue]`s, in the shape ShareX's serializer writes.
fn sharex_defaults_config() -> String {
    let g = GRADIENT;
    format!(
        r#"{{
  "Name": "ShareX defaults",
  "Effects": [
    {{ "$type": "Alpha", "Value": 1.0, "Addition": 0.0, "Enabled": true }},
    {{ "$type": "BlackWhite", "Enabled": true }},
    {{ "$type": "Brightness", "Value": 0.0, "Enabled": true }},
    {{ "$type": "Colorize", "Color": "Red", "Value": 0.0, "Enabled": true }},
    {{ "$type": "Contrast", "Value": 1.0, "Enabled": true }},
    {{ "$type": "Gamma", "Value": 1.0, "Enabled": true }},
    {{ "$type": "Grayscale", "Value": 1.0, "Enabled": true }},
    {{ "$type": "Hue", "Angle": 0.0, "Enabled": true }},
    {{ "$type": "Inverse", "Enabled": true }},
    {{ "$type": "MatrixColor", "Rr": 1.0, "Rg": 0.0, "Rb": 0.0, "Ra": 0.0, "Ro": 0.0, "Gr": 0.0, "Gg": 1.0, "Gb": 0.0, "Ga": 0.0, "Go": 0.0,
       "Br": 0.0, "Bg": 0.0, "Bb": 1.0, "Ba": 0.0, "Bo": 0.0, "Ar": 0.0, "Ag": 0.0, "Ab": 0.0, "Aa": 1.0, "Ao": 0.0, "Enabled": true }},
    {{ "$type": "Polaroid", "Enabled": true }},
    {{ "$type": "ReplaceColor", "SourceColor": "White", "AutoSourceColor": false, "TargetColor": "Transparent", "Threshold": 0, "Enabled": true }},
    {{ "$type": "Saturation", "Value": 1.0, "Enabled": true }},
    {{ "$type": "SelectiveColor", "LightColor": "White", "DarkColor": "Black", "PaletteSize": 10, "Enabled": true }},
    {{ "$type": "Sepia", "Value": 1.0, "Enabled": true }},
    {{ "$type": "DrawBackground", "Color": "Black", "UseGradient": false, "Gradient": {g}, "Enabled": true }},
    {{ "$type": "DrawBackgroundImage", "ImageFilePath": "", "Center": true, "Tile": false, "Enabled": true }},
    {{ "$type": "DrawBorder", "Type": "Outside", "Size": 1, "DashStyle": "Solid", "Color": "Black", "UseGradient": false, "Gradient": {g}, "Enabled": true }},
    {{ "$type": "DrawCheckerboard", "Size": 10, "Color": "LightGray", "Color2": "White", "Enabled": true }},
    {{ "$type": "DrawImage", "ImageLocation": "", "Placement": "TopLeft", "Offset": "0, 0", "SizeMode": "DontResize", "Size": "0, 0",
       "RotateFlip": "None", "Tile": false, "AutoHide": false, "InterpolationMode": "HighQualityBicubic", "CompositingMode": "SourceOver",
       "Opacity": 100, "Enabled": true }},
    {{ "$type": "DrawParticles", "ImageFolder": "", "ImageCount": 1, "Background": false, "RandomSize": false, "RandomSizeMin": 64,
       "RandomSizeMax": 128, "RandomAngle": false, "RandomAngleMin": 0, "RandomAngleMax": 360, "RandomOpacity": false,
       "RandomOpacityMin": 0, "RandomOpacityMax": 100, "NoOverlap": false, "NoOverlapOffset": 0, "EdgeOverlap": false, "Enabled": true }},
    {{ "$type": "DrawText", "Text": "Text watermark", "Placement": "BottomRight", "Offset": "5, 5", "AutoHide": false,
       "TextFont": "Arial, 11.25pt", "TextRenderingMode": "SystemDefault", "TextColor": "235, 235, 235", "DrawTextShadow": true,
       "TextShadowColor": "Black", "TextShadowOffset": "-1, -1", "CornerRadius": 4, "Padding": "5, 5, 5, 5", "DrawBorder": true,
       "BorderColor": "Black", "BorderSize": 1, "DrawBackground": true, "BackgroundColor": "42, 47, 56", "UseGradient": false,
       "Gradient": {g}, "Enabled": true }},
    {{ "$type": "DrawTextEx", "Text": "Text", "Placement": "TopLeft", "Offset": "0, 0", "Angle": 0, "AutoHide": false,
       "Font": "Arial, 36pt", "Color": "235, 235, 235", "UseGradient": false, "Gradient": {g}, "Outline": false, "OutlineSize": 5,
       "OutlineColor": "235, 0, 0", "OutlineUseGradient": false, "OutlineGradient": {g}, "Shadow": false, "ShadowOffset": "0, 5",
       "ShadowColor": "125, 0, 0, 0", "ShadowUseGradient": false, "ShadowGradient": {g}, "Enabled": true }},
    {{ "$type": "Blur", "Radius": 15, "Enabled": true }},
    {{ "$type": "ColorDepth", "BitsPerChannel": 4, "Enabled": true }},
    {{ "$type": "EdgeDetect", "Enabled": true }},
    {{ "$type": "Emboss", "Enabled": true }},
    {{ "$type": "GaussianBlur", "Radius": 15, "Enabled": true }},
    {{ "$type": "Glow", "Size": 20, "Strength": 1.0, "Color": "White", "UseGradient": false, "Gradient": {g}, "Offset": "0, 0", "Enabled": true }},
    {{ "$type": "MatrixConvolution", "X0Y0": 0, "X1Y0": 0, "X2Y0": 0, "X0Y1": 0, "X1Y1": 1, "X2Y1": 0, "X0Y2": 0, "X1Y2": 0, "X2Y2": 0,
       "Factor": 1.0, "Offset": 0, "Enabled": true }},
    {{ "$type": "MeanRemoval", "Enabled": true }},
    {{ "$type": "Outline", "Size": 1, "Padding": 0, "Color": "Black", "OutlineOnly": false, "Enabled": true }},
    {{ "$type": "Pixelate", "Size": 10, "BorderSize": 0, "BorderColor": "Black", "Enabled": true }},
    {{ "$type": "Reflection", "Percentage": 20, "MaxAlpha": 255, "MinAlpha": 0, "Offset": 0, "Skew": false, "SkewSize": 25, "Enabled": true }},
    {{ "$type": "RGBSplit", "OffsetRed": "-5, 0", "OffsetGreen": "0, 0", "OffsetBlue": "5, 0", "Enabled": true }},
    {{ "$type": "Shadow", "Opacity": 0.6, "Size": 10, "Darkness": 0.0, "Color": "Black", "Offset": "0, 0", "AutoResize": true, "Enabled": true }},
    {{ "$type": "Sharpen", "Enabled": true }},
    {{ "$type": "Slice", "MinSliceHeight": 10, "MaxSliceHeight": 100, "MinSliceShift": 0, "MaxSliceShift": 10, "Enabled": true }},
    {{ "$type": "Smooth", "Enabled": true }},
    {{ "$type": "TornEdge", "Depth": 15, "Range": 20, "Sides": "Top, Bottom, Left, Right", "CurvedEdges": true, "Enabled": true }},
    {{ "$type": "WaveEdge", "Depth": 15, "Range": 20, "Sides": "Top, Bottom, Left, Right", "Enabled": true }},
    {{ "$type": "AutoCrop", "Sides": "Top, Bottom, Left, Right", "Padding": 0, "Enabled": true }},
    {{ "$type": "Canvas", "Margin": "0, 0, 0, 30", "MarginMode": "AbsoluteSize", "Color": "Transparent", "Enabled": true }},
    {{ "$type": "Crop", "Margin": "0, 0, 0, 0", "Enabled": true }},
    {{ "$type": "Flip", "Horizontally": true, "Vertically": false, "Enabled": true }},
    {{ "$type": "ForceProportions", "ProportionalWidth": 1, "ProportionalHeight": 1, "Method": "Grow", "GrowFillColor": "Transparent", "Enabled": true }},
    {{ "$type": "Resize", "Width": 250, "Height": 0, "Mode": "ResizeAll", "Enabled": true }},
    {{ "$type": "Rotate", "Angle": 0.0, "Upsize": true, "Clip": false, "Enabled": true }},
    {{ "$type": "RoundedCorners", "CornerRadius": 20, "Enabled": true }},
    {{ "$type": "Scale", "WidthPercentage": 100.0, "HeightPercentage": 0.0, "Enabled": true }},
    {{ "$type": "Skew", "Horizontally": 0, "Vertically": 0, "Enabled": true }}
  ]
}}"#
    )
}

fn package(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, text) in entries {
        writer.start_file(*name, SimpleFileOptions::default()).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn read(config: &str) -> (Preset, Vec<String>) {
    parse_sxie_with_warnings(&package(&[("Config.json", config)])).unwrap()
}

fn one(json: &str) -> Effect {
    let (preset, warnings) = read(&format!(r#"{{ "Name": "p", "Effects": [ {json} ] }}"#));
    assert_eq!(preset.effects.len(), 1, "{json} was skipped: {warnings:?}");
    preset.effects[0].effect.clone()
}

/// Sanity: the fixture is valid JSON with 51 effects, one per ShareX class.
#[test]
fn audit_fixture_is_well_formed() {
    let value: serde_json::Value = serde_json::from_str(&sharex_defaults_config()).unwrap();
    let types: Vec<&str> = value["Effects"].as_array().unwrap().iter().map(|e| e["$type"].as_str().unwrap()).collect();
    assert_eq!(types, SHAREX_CLASSES.to_vec());
}

/// A ShareX-written package with every class: all 51 load. The reader once matched invented names for
/// DrawBackground, DrawBackgroundImage, DrawBorder, DrawCheckerboard, DrawParticles and MatrixConvolution.
#[test]
fn audit_bug_sharex_classes_are_skipped_on_import() {
    let (preset, warnings) = read(&sharex_defaults_config());
    assert!(warnings.is_empty(), "skipped: {warnings:#?}");
    assert_eq!(preset.effects.len(), 51);
}

/// `ImageEffectPreset.GetDefaultPreset()` adds `Canvas { Margin = new ImageMargins(0, 0, 0, 30) }`.
/// ShareX writes the margin as one `"0, 0, 0, 30"` string, in the order left, top, right, bottom.
#[test]
fn audit_bug_canvas_margin_is_lost() {
    let effect =
        one(r#"{ "$type": "Canvas", "Margin": "0, 0, 0, 30", "MarginMode": "AbsoluteSize", "Color": "Transparent", "Enabled": true }"#);
    assert_eq!(effect, Effect::Manip(Manip::Canvas { margin: (0, 0, 0, 30), color: Rgba::TRANSPARENT }));
}

/// Alpha.Value is a multiplier (`[DefaultValue(1f)]`, "Example 0.5 will decrease alpha of pixel 50%"),
/// and the addend is `Addition`.
#[test]
fn audit_bug_alpha_scale_and_name() {
    let effect = one(r#"{ "$type": "Alpha", "Value": 0.5, "Addition": 0.25, "Enabled": true }"#);
    assert_eq!(effect, Effect::Adjust(Adjust::Alpha { alpha: 0.5, add: 0.25 }));
}

/// ShareX's `Contrast` is a plain multiply (`SkiaColorMatrixManager.Contrast`): R, G and B times `Value`,
/// alpha and the constant row unchanged. Laqta's own `Adjust::Contrast` pivots on mid grey, so the import
/// keeps ShareX's look as the equivalent `MatrixColor`.
#[test]
fn sharex_contrast_imports_as_the_plain_multiply_matrix() {
    let effect = one(r#"{ "$type": "Contrast", "Value": 1.5, "Enabled": true }"#);
    #[rustfmt::skip]
    let matrix = [
        1.5, 0.0, 0.0, 0.0,
        0.0, 1.5, 0.0, 0.0,
        0.0, 0.0, 1.5, 0.0,
        0.0, 0.0, 0.0, 1.0,
        0.0, 0.0, 0.0, 0.0,
    ];
    assert_eq!(effect, Effect::Adjust(Adjust::MatrixColor { matrix }));
}

/// ColorConverter writes non-opaque colours alpha first ("125, 0, 0, 0" is DrawTextEx.ShadowColor's
/// default) and known colours by name.
#[test]
fn audit_bug_colour_strings_misread() {
    let argb = one(r#"{ "$type": "Colorize", "Color": "128, 10, 20, 30", "Value": 0.5, "Enabled": true }"#);
    let named = one(r#"{ "$type": "Colorize", "Color": "Lime", "Value": 0.5, "Enabled": true }"#);
    let colour = |e: &Effect| match e {
        Effect::Adjust(Adjust::Colorize { color, .. }) => *color,
        other => panic!("{other:?}"),
    };
    let mut wrong = Vec::new();
    if colour(&argb) != (Rgba { r: 10, g: 20, b: 30, a: 128 }) {
        wrong.push(format!("\"128, 10, 20, 30\" read as {:?}", colour(&argb)));
    }
    if colour(&named) != Rgba::rgb(0, 255, 0) {
        wrong.push(format!("\"Lime\" read as {:?}", colour(&named)));
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Real ShareX property names carrying non-default values. Each pair is (ShareX JSON, the Debug text
/// the value must appear as in the parsed effect). Every mismatch is listed.
#[test]
fn audit_bug_sharex_property_names_are_not_read() {
    let cases: &[(&str, &str)] = &[
        (r#"{ "$type": "Flip", "Horizontally": true, "Vertically": true }"#, "horizontal: true, vertical: true"),
        (r#"{ "$type": "RGBSplit", "OffsetRed": "-9, 2", "OffsetGreen": "0, 0", "OffsetBlue": "9, -2" }"#, "offset_red: (-9, 2)"),
        (r#"{ "$type": "RoundedCorners", "CornerRadius": 33 }"#, "radius: 33"),
        (r#"{ "$type": "ColorDepth", "BitsPerChannel": 3 }"#, "bits: 3"),
        (r#"{ "$type": "Crop", "Margin": "1, 2, 3, 4" }"#, "margin: (1, 2, 3, 4)"),
        (
            r#"{ "$type": "ForceProportions", "ProportionalWidth": 4, "ProportionalHeight": 3, "Method": "Grow", "GrowFillColor": "Transparent" }"#,
            "width: 4, height: 3",
        ),
        (r#"{ "$type": "Skew", "Horizontally": 7, "Vertically": 8 }"#, "x: 7, y: 8"),
        (r#"{ "$type": "Rotate", "Angle": 10.0, "Upsize": false, "Clip": false }"#, "grow: false"),
        (
            r#"{ "$type": "Slice", "MinSliceHeight": 11, "MaxSliceHeight": 99, "MinSliceShift": 1, "MaxSliceShift": 9 }"#,
            "min_height: 11, max_height: 99",
        ),
        (
            r#"{ "$type": "Shadow", "Opacity": 0.6, "Size": 10, "Darkness": 0.0, "Color": "Black", "Offset": "3, 4", "AutoResize": true }"#,
            "offset_x: 3, offset_y: 4",
        ),
        (r#"{ "$type": "Glow", "Size": 20, "Strength": 1.0, "Color": "White", "Offset": "3, 4" }"#, "offset_x: 3, offset_y: 4"),
        (
            r#"{ "$type": "ReplaceColor", "SourceColor": "0, 0, 255", "AutoSourceColor": true, "TargetColor": "Transparent", "Threshold": 40 }"#,
            "tolerance: 40",
        ),
        (r#"{ "$type": "TornEdge", "Depth": 15, "Range": 20, "Sides": "Top, Bottom", "CurvedEdges": true }"#, "right: false"),
        (
            r#"{ "$type": "DrawImage", "ImageLocation": "%ShareXImageEffects%\\p\\logo.png", "Placement": "TopLeft", "Offset": "0, 0", "SizeMode": "DontResize", "Size": "0, 0", "Opacity": 100 }"#,
            "logo.png",
        ),
    ];
    let mut wrong = Vec::new();
    for (json, expect) in cases {
        let shown = format!("{:?}", one(json));
        if !shown.contains(expect) {
            wrong.push(format!("{json}\n    expected `{expect}` in {shown}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} not read:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}

/// What `write_sxie` emits must load in ShareX: Json.NET splits `$type` at the first comma, and the
/// binder resolves only a bare class name equal to one of the 51 classes. `effects::write` returns the
/// bare names, and `sxie.rs` (`preset_to_value`) writes them as they are.
#[test]
fn audit_bug_exported_type_names_do_not_resolve_in_sharex() {
    let mut effects = Vec::new();
    effects.extend(Adjust::defaults().into_iter().map(Effect::Adjust));
    effects.extend(laqta_domain::effects::Draw::defaults().into_iter().map(Effect::Draw));
    effects.extend(laqta_domain::effects::Filter::defaults().into_iter().map(Effect::Filter));
    effects.extend(Manip::defaults().into_iter().map(Effect::Manip));
    let mut unresolved = Vec::new();
    for effect in effects {
        let preset = Preset { name: "p".into(), effects: vec![EffectItem { effect, enabled: true }] };
        let Ok(bytes) = write_sxie(&preset) else { continue };
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut text = String::new();
        archive.by_name("Config.json").unwrap().read_to_string(&mut text).unwrap();
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        let type_name = value["Effects"][0]["$type"].as_str().unwrap().to_owned();
        let bound = type_name.split(',').next().unwrap().trim().to_owned();
        if !SHAREX_CLASSES.contains(&bound.as_str()) {
            unresolved.push(type_name);
        }
    }
    assert!(unresolved.is_empty(), "{} exported $type values ShareX's binder cannot resolve: {unresolved:#?}", unresolved.len());
}
