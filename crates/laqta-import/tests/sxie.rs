#![allow(clippy::unwrap_used)]

use std::io::{Cursor, Write};

use laqta_domain::color::Rgba;
use laqta_domain::effects::draw::{BorderKind, Placement, SizeMode};
use laqta_domain::effects::manip::ForceMethod;
use laqta_domain::effects::{Adjust, Draw, Effect, EffectItem, Filter, Manip, Preset, TextSpec};
use laqta_import::{ImportError, parse_sxie, parse_sxie_with_warnings, write_sxie};
use zip::write::SimpleFileOptions;

/// The effects that have an ImageEffectsLib class: a one-effect package writes. The Laqta-only filters and
/// the ImageEditor effects are read but never written.
fn has_sharex_class(effect: &Effect) -> bool {
    write_sxie(&Preset { name: "probe".into(), effects: vec![EffectItem { effect: effect.clone(), enabled: true }] }).is_ok()
}

fn items(effects: Vec<Effect>) -> Vec<EffectItem> {
    effects.into_iter().enumerate().map(|(index, effect)| EffectItem { effect, enabled: index % 3 != 0 }).collect()
}

fn round_trip(preset: &Preset) -> Preset {
    let bytes = write_sxie(preset).unwrap();
    let (parsed, warnings) = parse_sxie_with_warnings(&bytes).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    parsed
}

#[test]
fn every_effect_with_a_sharex_class_round_trips_at_its_defaults() {
    let mut effects = Vec::new();
    effects.extend(Adjust::defaults().into_iter().map(Effect::Adjust));
    effects.extend(Draw::defaults().into_iter().map(Effect::Draw));
    effects.extend(Filter::defaults().into_iter().map(Effect::Filter));
    effects.extend(Manip::defaults().into_iter().map(Effect::Manip));
    let effects: Vec<Effect> = effects.into_iter().filter(has_sharex_class).collect();
    assert_eq!(effects.len(), 51);

    let preset = Preset { name: "Defaults".into(), effects: items(effects) };
    assert_eq!(round_trip(&preset), preset);
}

/// Non-default values in every field, so swapped or dropped fields show up.
fn distinct_effects() -> Vec<Effect> {
    vec![
        Effect::Adjust(Adjust::Alpha { alpha: 0.25, add: 0.125 }),
        Effect::Adjust(Adjust::Brightness { value: -37.25 }),
        Effect::Adjust(Adjust::Colorize { color: Rgba { r: 12, g: 200, b: 99, a: 180 }, value: 66.5 }),
        Effect::Adjust(Adjust::Gamma { value: 2.5 }),
        Effect::Adjust(Adjust::Hue { angle: 45.5 }),
        Effect::Adjust(Adjust::MatrixColor {
            matrix: [0.5, 0.0, 0.25, 0.0, 0.0, 1.5, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.1, 0.2, 0.3, 0.4],
        }),
        Effect::Adjust(Adjust::SelectiveColor { from: Rgba::rgb(10, 20, 30), to: Rgba::rgb(40, 50, 60), tolerance: 77 }),
        Effect::Adjust(Adjust::ReplaceColor {
            source: Rgba::rgb(1, 2, 3),
            target: Rgba::rgb(4, 5, 6),
            tolerance: 9,
            auto_source_from_corner: true,
        }),
        Effect::Draw(Draw::Background { color: Rgba::rgb(7, 8, 9), gradient: Some((Rgba::rgb(1, 1, 1), Rgba::rgb(2, 2, 2), 33.5)) }),
        Effect::Draw(Draw::Border { kind: BorderKind::Inside, size: 4, color: Rgba::rgb(9, 9, 9), dash: true }),
        Effect::Draw(Draw::Checkerboard { size: 3, color1: Rgba::rgb(5, 5, 5), color2: Rgba::rgb(6, 6, 6) }),
        Effect::Draw(Draw::Image {
            path: "logo.png".into(),
            placement: Placement::TopRight,
            offset: (-12, 30),
            size_mode: SizeMode::Absolute(120, 40),
            opacity: 0.75,
        }),
        Effect::Draw(Draw::Image {
            path: "logo.png".into(),
            placement: Placement::BottomLeft,
            offset: (1, 2),
            size_mode: SizeMode::Percentage(50.5),
            opacity: 1.0,
        }),
        Effect::Draw(Draw::Particles {
            path: "dot.png".into(),
            count: 7,
            min_size: 3,
            max_size: 9,
            random_angle: false,
            seed: (1 << 60) | 3,
        }),
        Effect::Draw(Draw::Text {
            text: TextSpec {
                text: "Hello".into(),
                font_family: "Serif".into(),
                size: 22.5,
                bold: true,
                italic: true,
                color: Rgba::rgb(250, 1, 2),
            },
            placement: Placement::TopCenter,
            offset: (-5, 6),
            background: Some(Rgba::rgb(0, 0, 0)),
            padding: 4,
            corner_radius: 8,
        }),
        Effect::Draw(Draw::TextEx {
            text: TextSpec {
                text: "Outlined".into(),
                font_family: "Mono".into(),
                size: 12.0,
                bold: false,
                italic: false,
                color: Rgba::BLACK,
            },
            angle: -15.5,
            placement: Placement::MiddleLeft,
            offset: (3, -3),
            outline: Some((Rgba::rgb(255, 255, 0), 2.5)),
            shadow: Some((Rgba::rgb(20, 20, 20), 4, -4)),
        }),
        Effect::Filter(Filter::ConvolutionMatrix { kernel: [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0], divisor: 2.0, offset: 0.5 }),
        Effect::Filter(Filter::Glow { size: 14, strength: 3.5, color: Rgba::rgb(9, 99, 199), offset_x: -2, offset_y: 5 }),
        Effect::Filter(Filter::Outline { size: 6, padding: 12, color: Rgba::rgb(3, 3, 3), outline_only: true }),
        Effect::Filter(Filter::Reflection { percentage: 30, max_alpha: 200, min_alpha: 10, offset: 4, skew: true, skew_size: 7 }),
        Effect::Filter(Filter::RgbSplit { offset_red: (-7, 1), offset_green: (2, -2), offset_blue: (8, 0) }),
        Effect::Filter(Filter::Shadow {
            opacity: 0.4,
            size: 18,
            darkness: 0.3,
            color: Rgba::rgb(1, 2, 3),
            offset_x: -6,
            offset_y: 9,
            auto_resize: false,
        }),
        Effect::Filter(Filter::Slice { min_height: 3, max_height: 11, min_shift: 1, max_shift: 19, seed: u64::MAX - 5 }),
        Effect::Filter(Filter::TornEdge { depth: 5, range: 9, top: false, right: true, bottom: false, left: true, curved: true, seed: 42 }),
        Effect::Filter(Filter::WaveEdge { depth: 4, range: 20, top: true, right: true, bottom: false, left: false }),
        Effect::Filter(Filter::GaussianBlur { sigma: 1.75 }),
        Effect::Manip(Manip::AutoCrop { color: Rgba::rgb(0, 255, 0), tolerance: 12, margin: 3 }),
        Effect::Manip(Manip::Canvas { margin: (-4, 10, 20, -8), color: Rgba::rgb(128, 128, 128) }),
        Effect::Manip(Manip::Crop { margin: (1, 2, 3, 4) }),
        Effect::Manip(Manip::Flip { horizontal: true, vertical: false }),
        Effect::Manip(Manip::ForceProportions { width: 4, height: 3, method: ForceMethod::Crop, color: Rgba::rgb(9, 9, 1) }),
        Effect::Manip(Manip::Resize { width: 640, height: 0, keep_aspect: false }),
        Effect::Manip(Manip::Rotate { angle: 271.5, grow: false, color: Rgba::rgb(4, 4, 4) }),
        Effect::Manip(Manip::RoundedCorners { radius: 25 }),
        Effect::Manip(Manip::Scale { width_pct: 150.0, height_pct: 66.5 }),
        Effect::Manip(Manip::Skew { x: -20, y: 15, grow: false }),
    ]
}

#[test]
fn non_default_values_round_trip_field_by_field() {
    let preset = Preset { name: "Distinct & \"quoted\"".into(), effects: items(distinct_effects()) };
    assert_eq!(round_trip(&preset), preset);
}

#[test]
fn unknown_effect_classes_are_skipped_with_a_warning() {
    let config = r#"{
        "Name": "Mixed",
        "Effects": [
            { "$type": "ShareX.ImageEffectsLib.Brightness, ShareX.ImageEffectsLib", "Value": 20, "Enabled": true },
            { "$type": "ShareX.ImageEffectsLib.Frobnicate, ShareX.ImageEffectsLib", "Value": 1, "Enabled": true }
        ]
    }"#;
    let bytes = package(&[("Config.json", config)]);
    let (preset, warnings) = parse_sxie_with_warnings(&bytes).unwrap();
    assert_eq!(preset.name, "Mixed");
    assert_eq!(preset.effects.len(), 1);
    assert_eq!(preset.effects[0].effect, Effect::Adjust(Adjust::Brightness { value: 20.0 }));
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("Frobnicate"));
    assert!(parse_sxie(&bytes).is_ok());
}

#[test]
fn values_written_as_text_and_missing_flags_are_read() {
    let config = r#"{
        "Name": "Text numbers",
        "Effects": [
            { "$type": "ShareX.ImageEffectsLib.Brightness, ShareX.ImageEffectsLib", "Value": "20" },
            { "$type": "ShareX.ImageEffectsLib.Pixelate, ShareX.ImageEffectsLib", "Size": 6, "Enabled": "false" }
        ]
    }"#;
    let preset = parse_sxie(&package(&[("Config.json", config)])).unwrap();
    assert_eq!(preset.effects[0], EffectItem { effect: Effect::Adjust(Adjust::Brightness { value: 20.0 }), enabled: true });
    assert_eq!(preset.effects[1], EffectItem { effect: Effect::Filter(Filter::Pixelate { size: 6 }), enabled: false });
}

#[test]
fn image_files_inside_the_package_are_reported_not_extracted() {
    let config = r#"{ "Name": "Logo", "Effects": [
        { "$type": "ShareX.ImageEffectsLib.DrawImage, ShareX.ImageEffectsLib", "Path": "assets/logo.png", "Enabled": true }
    ] }"#;
    let bytes = package(&[("Config.json", config), ("assets/logo.png", "not really a png")]);
    let (preset, warnings) = parse_sxie_with_warnings(&bytes).unwrap();
    assert_eq!(preset.effects.len(), 1);
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("logo.png"));
}

#[test]
fn a_package_without_config_is_an_error() {
    let bytes = package(&[("readme.txt", "nothing here")]);
    assert!(matches!(parse_sxie(&bytes), Err(ImportError::Invalid(_))));
}

#[test]
fn bytes_that_are_not_a_zip_are_an_error() {
    assert!(matches!(parse_sxie(b"not a zip"), Err(ImportError::Zip { .. })));
}

#[test]
fn writing_an_effect_without_a_sharex_class_is_refused() {
    let preset = Preset {
        name: "Noisy".into(),
        effects: items(vec![Effect::Adjust(Adjust::Inverse), Effect::Filter(Filter::Noise { amount: 10, seed: 1 })]),
    };
    let error = write_sxie(&preset).unwrap_err();
    assert!(matches!(error, ImportError::Unsupported(_)));
    assert!(error.to_string().contains("Noise"), "{error}");
}

#[test]
fn written_packages_hold_a_config_json_with_sharex_type_names() {
    let preset = Preset { name: "One".into(), effects: items(vec![Effect::Adjust(Adjust::Sepia { value: 50.0 })]) };
    let bytes = write_sxie(&preset).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut text = String::new();
    std::io::Read::read_to_string(&mut archive.by_name("Config.json").unwrap(), &mut text).unwrap();
    assert!(text.contains("ShareX.ImageEffectsLib.Sepia, ShareX.ImageEffectsLib"), "{text}");
}

/// A zip archive holding the given entries.
fn package(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, text) in entries {
        writer.start_file(*name, SimpleFileOptions::default()).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
